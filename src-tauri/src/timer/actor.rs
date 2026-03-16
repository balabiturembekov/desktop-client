use chrono::{DateTime, Datelike, Local, Utc};
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::Receiver;
use tokio::time::{interval, Duration};

use crate::timer::models::TimerCommand;
use crate::tracker::models::ActivityState;

const IDLE_TIMEOUT_SECS: i64 = 300;

#[derive(Serialize, Clone)]
pub struct TimerPayload {
    pub total_secs: u64,
    pub is_running: bool,
}

#[allow(dead_code)]
#[derive(Serialize, Clone)]
pub struct IdlePayload {
    pub idle_secs: u64,
}

async fn get_today_secs(pool: &SqlitePool) -> i64 {
    sqlx::query_as::<_, (Option<i64>,)>(
        "SELECT SUM(duration_secs) FROM time_slots WHERE date(started_at) = date('now')",
    )
    .fetch_one(pool)
    .await
    .ok()
    .and_then(|(v,)| v)
    .unwrap_or(0)
}

async fn save_slot(
    pool: &SqlitePool,
    project_id: &Option<String>,
    started_at: DateTime<Utc>,
    duration_secs: i64,
    activity: &ActivityState,
) {
    if let Some(project_id) = project_id {
        let ended_at = Utc::now().to_rfc3339();
        let started_at_str = started_at.to_rfc3339();
        let active = activity.active_seconds.load(Ordering::Relaxed);
        let total = activity.total_seconds.load(Ordering::Relaxed);
        let percent = if total > 0 {
            ((active * 100) / total) as i64
        } else {
            0
        };

        let _ = sqlx::query(
            r#"INSERT INTO time_slots
            (project_id, started_at, ended_at, duration_secs, activity_percent, synced)
            VALUES (?, ?, ?, ?, ?, 0)"#,
        )
        .bind(project_id)
        .bind(&started_at_str)
        .bind(&ended_at)
        .bind(duration_secs)
        .bind(percent)
        .execute(pool)
        .await;
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
            // Делаем главное окно неактивным
            if let Some(main) = app.get_webview_window("main") {
                let _ = main.set_enabled(false);
            }
            println!("[timer] idle window opened");
        }
        Err(e) => eprintln!("[timer] failed to open idle window: {}", e),
    }
}

pub async fn time_actor(
    mut receiver: Receiver<TimerCommand>,
    app: AppHandle,
    pool: SqlitePool,
    is_running_flag: Arc<AtomicBool>,
    activity: ActivityState,
) {
    let mut running = false;
    let mut session_started_at: Option<DateTime<Utc>> = None;
    let mut current_project_id: Option<String> = None;
    let mut last_activity_at: Option<DateTime<Utc>> = None;
    let mut tick = interval(Duration::from_secs(1));

    let mut today_secs_cache = get_today_secs(&pool).await;
    let mut current_day = Local::now().ordinal();

    println!(
        "[timer] initialized with {} secs from today",
        today_secs_cache
    );

    loop {
        tokio::select! {
            cmd = receiver.recv() => {
                match cmd {
                    Some(TimerCommand::Start { project_id }) => {
                        if !running {
                            today_secs_cache = get_today_secs(&pool).await;
                            current_day = Local::now().ordinal();
                            running = true;
                            is_running_flag.store(true, Ordering::Relaxed);
                            session_started_at = Some(Utc::now());
                            last_activity_at = Some(Utc::now());
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
                            if let Some(started_at) = session_started_at.take() {
                                let duration = (Utc::now() - started_at).num_seconds().max(0);
                                save_slot(&pool, &current_project_id, started_at, duration, &activity).await;
                            }
                            current_project_id = None;
                            last_activity_at = None;
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
                        session_started_at = None;
                        current_project_id = None;
                        last_activity_at = None;
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
                let now_day = Local::now().ordinal();
                if now_day != current_day {
                    println!("[timer] day rollover!");
                    current_day = now_day;
                    if let Some(started_at) = session_started_at.take() {
                        let duration = (Utc::now() - started_at).num_seconds().max(0);
                        save_slot(&pool, &current_project_id, started_at, duration, &activity).await;
                    }
                    today_secs_cache = 0;
                    session_started_at = Some(Utc::now());
                    activity.active_seconds.store(0, Ordering::Relaxed);
                    activity.total_seconds.store(0, Ordering::Relaxed);
                    let _ = app.emit("day-rollover", ());
                    let _ = app.emit("timer-tick", TimerPayload {
                        total_secs: 0,
                        is_running: true,
                    });
                    continue;
                }

                let was_active = activity.activity_flag.swap(false, Ordering::Relaxed);
                if was_active {
                    last_activity_at = Some(Utc::now());
                }

                if let Some(last_active) = last_activity_at {
                    let idle_secs = (Utc::now() - last_active).num_seconds();
                    if idle_secs >= IDLE_TIMEOUT_SECS {
                        println!("[timer] idle: {} secs", idle_secs);
                        running = false;
                        is_running_flag.store(false, Ordering::Relaxed);

                        if let Some(started_at) = session_started_at.take() {
                            let active_until = last_active;
                            let duration = (active_until - started_at).num_seconds().max(0);
                            save_slot(&pool, &current_project_id, started_at, duration, &activity).await;
                        }
                        last_activity_at = None;

                        today_secs_cache = get_today_secs(&pool).await;
                        let _ = app.emit("timer-tick", TimerPayload {
                            total_secs: today_secs_cache as u64,
                            is_running: false,
                        });

                        open_idle_window(&app, idle_secs as u64);
                        continue;
                    }
                }

                let session_secs = session_started_at
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
