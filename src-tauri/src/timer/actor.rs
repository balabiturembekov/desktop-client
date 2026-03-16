use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use chrono::{DateTime, Datelike, Local, Utc};
use serde::Serialize;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager};
use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;
use tokio::sync::mpsc::Receiver;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

use crate::timer::models::TimerCommand;
use crate::tracker::models::ActivityState;

const IDLE_TIMEOUT_SECS: i64 = 300;
const CHUNK_SECS: i64 = 600; // 10 минут

#[derive(Serialize, Clone)]
pub struct TimerPayload {
    pub total_secs: u64,
    pub is_running: bool,
}

#[derive(Serialize, Clone)]
pub struct IdlePayload {
    pub idle_secs: u64,
}

async fn get_today_secs(pool: &SqlitePool) -> i64 {
    sqlx::query_as::<_, (Option<i64>,)>(
        "SELECT SUM(duration_secs) FROM time_slots WHERE date(started_at) = date('now')"
    )
    .fetch_one(pool)
    .await
    .ok()
    .and_then(|(v,)| v)
    .unwrap_or(0)
}

/// Сохраняет чанк и сразу помечает для sync. Возвращает id новой записи.
async fn save_chunk(
    pool: &SqlitePool,
    project_id: &str,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    activity: &ActivityState,
) -> Option<i64> {
    let duration_secs = (ended_at - started_at).num_seconds().max(0);
    let started_at_str = started_at.to_rfc3339();
    let ended_at_str = ended_at.to_rfc3339();
    let active = activity.active_seconds.load(Ordering::Relaxed);
    let total = activity.total_seconds.load(Ordering::Relaxed);
    let percent = if total > 0 { ((active * 100) / total) as i64 } else { 0 };

    match sqlx::query(
        r#"INSERT INTO time_slots
        (project_id, started_at, ended_at, duration_secs, activity_percent, synced)
        VALUES (?, ?, ?, ?, ?, 0)"#,
    )
    .bind(project_id)
    .bind(&started_at_str)
    .bind(&ended_at_str)
    .bind(duration_secs)
    .bind(percent)
    .execute(pool)
    .await
    {
        Ok(result) => {
            let slot_id = result.last_insert_rowid();
            log::info!("[timer] chunk saved: {}s, {}% activity, slot_id={}", duration_secs, percent, slot_id);
            Some(slot_id)
        }
        Err(e) => {
            log::error!("[timer] failed to save chunk: {}", e);
            None
        }
    }
}

fn open_idle_window(app: &AppHandle, idle_secs: u64) {
    let idle_mins = idle_secs / 60;
    let url = format!("/idle?idle_mins={}", idle_mins);

    if let Some(w) = app.get_webview_window("idle") {
        let _ = w.close();
    }

    match WebviewWindowBuilder::new(app, "idle", WebviewUrl::App(url.into()))
        .title("Hubnity")
        .inner_size(320.0, 260.0)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .center()
        .build()
    {
        Ok(_) => {
            if let Some(main) = app.get_webview_window("main") {
                let _ = main.set_enabled(false);
            }
        }
        Err(e) => log::error!("[timer] failed to open idle window: {}", e),
    }
}

pub async fn time_actor(
    mut receiver: Receiver<TimerCommand>,
    app: AppHandle,
    pool: SqlitePool,
    is_running_flag: Arc<AtomicBool>,
    activity: ActivityState,
    current_slot_id: Arc<Mutex<Option<i64>>>,
) {
    let mut running = false;
    let mut chunk_started_at: Option<DateTime<Utc>> = None;
    let mut current_project_id: Option<String> = None;
    let mut last_activity_at: Option<DateTime<Utc>> = None;
    let mut chunk_elapsed_secs: i64 = 0; // сколько секунд прошло в текущем чанке
    let mut idle_window_opened = false;
    let mut tick = interval(Duration::from_secs(1));

    let mut today_secs_cache = get_today_secs(&pool).await;
    let mut current_day = Local::now().ordinal();

    log::info!("[timer] initialized with {} secs from today", today_secs_cache);

    loop {
        tokio::select! {
            cmd = receiver.recv() => {
                match cmd {
                    Some(TimerCommand::Start { project_id }) => {
                        if !running {
                            today_secs_cache = get_today_secs(&pool).await;
                            current_day = Local::now().ordinal();
                            running = true;
                            idle_window_opened = false;
                            is_running_flag.store(true, Ordering::Relaxed);
                            chunk_started_at = Some(Utc::now());
                            last_activity_at = Some(Utc::now());
                            chunk_elapsed_secs = 0;
                            current_project_id = Some(project_id);
                            activity.active_seconds.store(0, Ordering::Relaxed);
                            activity.total_seconds.store(0, Ordering::Relaxed);
                            tick.reset();
                            let _ = app.emit("timer-tick", TimerPayload {
                                total_secs: today_secs_cache as u64,
                                is_running: true,
                            });
                        }
                    }
                    Some(TimerCommand::Stop) => {
                        if running {
                            running = false;
                            is_running_flag.store(false, Ordering::Relaxed);

                            // Сохраняем последний чанк
                            if let (Some(started_at), Some(project_id)) = (chunk_started_at.take(), current_project_id.take()) {
                                if let Some(slot_id) = save_chunk(&pool, &project_id, started_at, Utc::now(), &activity).await {
                                    *current_slot_id.lock().await = Some(slot_id);
                                }
                            }

                            last_activity_at = None;
                            chunk_elapsed_secs = 0;
                            activity.active_seconds.store(0, Ordering::Relaxed);
                            activity.total_seconds.store(0, Ordering::Relaxed);

                            today_secs_cache = get_today_secs(&pool).await;
                            current_day = Local::now().ordinal();
                            let _ = app.emit("timer-tick", TimerPayload {
                                total_secs: today_secs_cache as u64,
                                is_running: false,
                            });
                        }
                    }
                    Some(TimerCommand::Reset) => {
                        running = false;
                        is_running_flag.store(false, Ordering::Relaxed);
                        chunk_started_at = None;
                        current_project_id = None;
                        last_activity_at = None;
                        chunk_elapsed_secs = 0;
                        activity.active_seconds.store(0, Ordering::Relaxed);
                        activity.total_seconds.store(0, Ordering::Relaxed);
                        today_secs_cache = get_today_secs(&pool).await;
                        current_day = Local::now().ordinal();
                        let _ = app.emit("timer-tick", TimerPayload {
                            total_secs: today_secs_cache as u64,
                            is_running: false,
                        });
                    }
                    None => break,
                }
            }
            _ = tick.tick(), if running => {
                // Day rollover
                let now_day = Local::now().ordinal();
                if now_day != current_day {
                    log::info!("[timer] day rollover!");
                    current_day = now_day;

                    if let (Some(started_at), Some(ref project_id)) = (chunk_started_at.take(), &current_project_id) {
                        if let Some(slot_id) = save_chunk(&pool, project_id, started_at, Utc::now(), &activity).await {
                            *current_slot_id.lock().await = Some(slot_id);
                        }
                    }

                    today_secs_cache = 0;
                    chunk_started_at = Some(Utc::now());
                    chunk_elapsed_secs = 0;
                    activity.active_seconds.store(0, Ordering::Relaxed);
                    activity.total_seconds.store(0, Ordering::Relaxed);

                    let _ = app.emit("day-rollover", ());
                    let _ = app.emit("timer-tick", TimerPayload {
                        total_secs: 0,
                        is_running: true,
                    });
                    continue;
                }

                // Activity + Idle detection
                let was_active = activity.activity_flag.swap(false, Ordering::Relaxed);
                if was_active {
                    last_activity_at = Some(Utc::now());
                }

                if let Some(last_active) = last_activity_at {
                    let idle_secs = (Utc::now() - last_active).num_seconds();
                    if idle_secs >= IDLE_TIMEOUT_SECS && !idle_window_opened {
                        log::info!("[timer] idle: {} secs", idle_secs);
                        running = false;
                        idle_window_opened = true;
                        is_running_flag.store(false, Ordering::Relaxed);

                        if let (Some(started_at), Some(ref project_id)) = (chunk_started_at.take(), &current_project_id) {
                            let active_until = last_active;
                            let duration = (active_until - started_at).num_seconds().max(0);
                            if duration > 0 {
                                if let Some(slot_id) = save_chunk(&pool, project_id, started_at, active_until, &activity).await {
                                    *current_slot_id.lock().await = Some(slot_id);
                                }
                            }
                        }

                        current_project_id = None;
                        last_activity_at = None;
                        chunk_elapsed_secs = 0;
                        activity.active_seconds.store(0, Ordering::Relaxed);
                        activity.total_seconds.store(0, Ordering::Relaxed);

                        today_secs_cache = get_today_secs(&pool).await;
                        let _ = app.emit("timer-idle", IdlePayload { idle_secs: idle_secs as u64 });
                        let _ = app.emit("timer-tick", TimerPayload {
                            total_secs: today_secs_cache as u64,
                            is_running: false,
                        });

                        open_idle_window(&app, idle_secs as u64);
                        continue;
                    }
                }

                // 10-минутный чанк
                chunk_elapsed_secs += 1;
                if chunk_elapsed_secs >= CHUNK_SECS {
                    log::info!("[timer] chunk complete — saving");

                    if let (Some(started_at), Some(ref project_id)) = (chunk_started_at.take(), &current_project_id) {
                        let ended_at = Utc::now();
                        if let Some(slot_id) = save_chunk(&pool, project_id, started_at, ended_at, &activity).await {
                            *current_slot_id.lock().await = Some(slot_id);
                        }

                        // Начинаем новый чанк
                        chunk_started_at = Some(ended_at);
                        chunk_elapsed_secs = 0;
                        activity.active_seconds.store(0, Ordering::Relaxed);
                        activity.total_seconds.store(0, Ordering::Relaxed);

                        // Обновляем кеш
                        today_secs_cache = get_today_secs(&pool).await;
                    }
                }

                // Обычный тик
                let session_secs = chunk_started_at
                    .map(|s| (Utc::now() - s).num_seconds().max(0) as u64)
                    .unwrap_or(0);

                let _ = app.emit("timer-tick", TimerPayload {
                    total_secs: today_secs_cache as u64 + session_secs,
                    is_running: true,
                });
            }
        }
    }
}
