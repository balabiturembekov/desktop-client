use chrono::{DateTime, Datelike, Local, NaiveTime, TimeZone, Utc};
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::Receiver;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration, MissedTickBehavior};

use crate::api::models::auth::TrackingSettings;
use crate::timer::models::TimerCommand;
use crate::tracker::models::ActivityState;
const CHUNK_SECS: i64 = 600; // 10 минут
const PROGRESS_SAVE_EVERY_SECS: i64 = 60; // обновлять stub каждую минуту
/// Gap between consecutive 1-second ticks that indicates a system suspend/resume.
/// tokio::time::interval uses a monotonic clock (advances during sleep on macOS),
/// so on wake the tick fires immediately — but Utc::now() also advanced.
/// Comparing wall-clock elapsed since the last tick lets us detect suspend cycles.
/// Lowered from 60 → 10 so that brief suspend/resume cycles (lid close then
/// open within a minute) are also detected instead of silently counting as
/// active time.
// Lowered from 60 → 10 in audit #2 for brief suspend cycles; raised back to 30
// to reduce false positives from large NTP clock adjustments (which can jump
// up to ~15 s on some networks). 30 s is a safe compromise: any real sleep
// cycle is detectable within half a minute, while NTP jitter stays below the
// threshold (BUG-H05 from audit #3).
const SLEEP_DETECT_SECS: i64 = 30;

/// Возвращает ключ текущего дня как (year, ordinal).
/// Использование пары year+ordinal вместо одного ordinal корректно
/// обрабатывает смену года: Dec 31 (2024, 366) ≠ Jan 1 (2025, 1).
fn day_key(dt: &DateTime<Local>) -> (i32, u32) {
    (dt.year(), dt.ordinal())
}

/// UTC-момент начала (полночь 00:00:00) того локального дня, которому принадлежит `dt`.
/// Используется для точного разделения chunk при day rollover.
/// При DST-переходе, который убирает полночь, возвращает `dt.to_utc()` как fallback.
fn midnight_utc_of(dt: &DateTime<Local>) -> DateTime<Utc> {
    let naive_midnight = dt
        .date_naive()
        .and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
    Local
        .from_local_datetime(&naive_midnight)
        .single()
        .map(|ldt| ldt.to_utc())
        .unwrap_or_else(|| dt.to_utc())
}

#[derive(Serialize, Clone)]
pub struct TimerPayload {
    pub total_secs: u64,
    pub is_running: bool,
    pub project_id: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct IdlePayload {
    pub idle_secs: u64,
}

async fn get_today_secs(pool: &SqlitePool) -> i64 {
    sqlx::query_as::<_, (Option<i64>,)>(
        "SELECT SUM(duration_secs) FROM time_slots WHERE date(started_at, 'localtime') = date('now', 'localtime')"
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
    let percent = if total > 0 {
        ((active * 100) / total) as i64
    } else {
        0
    };

    match sqlx::query(
        r#"INSERT INTO time_slots
        (project_id, started_at, ended_at, duration_secs, activity_percent, synced)
        VALUES (?, ?, ?, ?, ?, ?)"#,
    )
    .bind(project_id)
    .bind(&started_at_str)
    .bind(&ended_at_str)
    .bind(duration_secs)
    .bind(percent)
    .bind(if duration_secs == 0 { 2 } else { 0 })
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
            log::info!(
                "[timer] chunk saved: {}s, {}% activity, slot_id={}",
                duration_secs,
                percent,
                slot_id
            );
            Some(slot_id)
        }
        Err(e) => {
            log::error!("[timer] failed to save chunk: {}", e);
            None
        }
    }
}

/// Finalizes a time slot, choosing between UPDATE (stub exists) and INSERT (fallback).
///
/// The stub slot is created at Start so app_tracker has a valid slot_id from second 1.
/// If that INSERT failed (rare DB error), stub_id is None — we fall back to a direct
/// INSERT here so the session data is never silently lost.
/// Skips if duration == 0 (e.g. idle fired at the same second as start).
async fn finalize_slot(
    pool: &SqlitePool,
    project_id: &str,
    stub_id: Option<i64>,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    activity: &ActivityState,
) {
    let duration_secs = (ended_at - started_at).num_seconds();
    if duration_secs <= 0 {
        // Delete zero-duration stub so it doesn't accumulate as an unsynced row.
        if let Some(sid) = stub_id {
            let _ = sqlx::query("DELETE FROM time_slots WHERE id = ? AND duration_secs = 0")
                .bind(sid)
                .execute(pool)
                .await;
            log::info!("[timer] zero-duration stub {} deleted", sid);
        }
        return;
    }
    if let Some(sid) = stub_id {
        update_slot(pool, sid, started_at, ended_at, activity).await;
        // Mark as ready for sync
        let _ = sqlx::query("UPDATE time_slots SET synced = 0 WHERE id = ?")
            .bind(sid)
            .execute(pool)
            .await;
    } else {
        log::warn!("[timer] stub slot missing — falling back to direct INSERT");
        save_chunk(pool, project_id, started_at, ended_at, activity).await;
    }
}

/// Обновляет существующий stub-слот актуальными значениями (UPDATE, не INSERT).
/// Используется для прогресс-сейвов и финализации вместо save_chunk,
/// чтобы исключить двойной подсчёт в get_today_secs.
async fn update_slot(
    pool: &SqlitePool,
    slot_id: i64,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    activity: &ActivityState,
) {
    let duration_secs = (ended_at - started_at).num_seconds().max(0);
    let active = activity.active_seconds.load(Ordering::Relaxed);
    let total = activity.total_seconds.load(Ordering::Relaxed);
    let percent = if total > 0 {
        ((active * 100) / total) as i64
    } else {
        0
    };

    match sqlx::query(
        "UPDATE time_slots SET ended_at = ?, duration_secs = ?, activity_percent = ? WHERE id = ?",
    )
    .bind(ended_at.to_rfc3339())
    .bind(duration_secs)
    .bind(percent)
    .bind(slot_id)
    .execute(pool)
    .await
    {
        Ok(_) => log::info!(
            "[timer] slot {} updated: {}s, {}% activity",
            slot_id,
            duration_secs,
            percent
        ),
        Err(e) => log::warn!("[timer] slot update failed: {}", e),
    }
}

fn open_idle_window(app: &AppHandle, idle_secs: u64) {
    let url = format!("/idle?idle_secs={}", idle_secs);

    if let Some(w) = app.get_webview_window("idle") {
        let _ = w.close();
    }

    match WebviewWindowBuilder::new(app, "idle", WebviewUrl::App(url.into()))
        .title("Hubnity")
        .inner_size(320.0, 300.0)
        .resizable(false)
        .decorations(false)
        .always_on_top(false) // UX-14: do not force idle window above all other apps
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
    settings: Arc<Mutex<TrackingSettings>>,
) {
    let mut running = false;
    let mut chunk_started_at: Option<DateTime<Utc>> = None;
    let mut current_project_id: Option<String> = None;
    let mut last_activity_at: Option<DateTime<Utc>> = None;
    // Tracks the last `last_activity_secs` value we have seen.
    // Updated on Start so that pre-session activity doesn't affect idle detection.
    let mut last_seen_activity_ts: u64 = 0;
    let mut chunk_elapsed_secs: i64 = 0; // сколько секунд прошло в текущем чанке
    let mut stub_slot_id: Option<i64> = None; // ID текущего stub-слота (обновляется in-place)
    let mut last_progress_save: i64 = 0; // chunk_elapsed_secs на момент последнего прогресс-сейва
    let mut idle_window_opened = false;
    // Wall-clock time of last activity when the idle window was opened.
    // Used to recompute the total idle duration (including sleep) on wake.
    let mut idle_last_active_at: Option<DateTime<Utc>> = None;
    let mut tick = interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // Wall-clock time of the previous tick — used to detect system sleep/wake.
    // Updated at Start (reset) and on every tick.
    let mut last_tick_at: DateTime<Utc> = Utc::now();

    let mut today_secs_cache = get_today_secs(&pool).await;
    let mut current_day = day_key(&Local::now());

    log::info!(
        "[timer] initialized with {} secs from today",
        today_secs_cache
    );

    loop {
        tokio::select! {
            cmd = receiver.recv() => {
                match cmd {
                    Some(TimerCommand::Start { project_id }) => {
                        if !running {
                            // If the idle window is still open (e.g. user started timer from
                            // the tray menu), close it and re-enable main. The Destroyed handler
                            // in lib.rs also re-enables main, so this is a belt-and-suspenders guard.
                            if let Some(idle) = app.get_webview_window("idle") {
                                let _ = idle.close();
                            }
                            if let Some(main) = app.get_webview_window("main") {
                                let _ = main.set_enabled(true);
                            }

                            today_secs_cache = get_today_secs(&pool).await;
                            current_day = day_key(&Local::now());
                            running = true;
                            idle_window_opened = false;
                            idle_last_active_at = None;
                            is_running_flag.store(true, Ordering::Relaxed);
                            let start_time = Utc::now();
                            chunk_started_at = Some(start_time);
                            last_activity_at = Some(start_time);
                            // Snapshot current activity timestamp so pre-session activity
                            // does not count toward idle detection for the new session.
                            last_seen_activity_ts = activity.last_activity_secs.load(Ordering::Relaxed);
                            chunk_elapsed_secs = 0;
                            last_progress_save = 0;
                            activity.active_seconds.store(0, Ordering::Relaxed);
                            activity.total_seconds.store(0, Ordering::Relaxed);

                            // Создаём stub-слот сразу: app_tracker получает slot_id с первой секунды.
                            // Stub обновляется in-place каждые 60с и финализируется на Stop —
                            // никогда не дублируется новым INSERT.
                            // Set synced = 2 to mark as stub slot, preventing premature sync
                            if let Some(sid) = save_chunk(&pool, &project_id, start_time, start_time, &activity).await {
                                stub_slot_id = Some(sid);
                                *current_slot_id.lock().await = Some(sid);
                                log::info!("[timer] stub slot created: slot_id={}", sid);
                            }

                            current_project_id = Some(project_id);
                            // Signal listener to switch to high-frequency (100 ms) polling.
                            activity.timer_active.store(true, Ordering::Relaxed);
                            tick.reset();
                            // Reset last_tick_at so the first post-start tick doesn't
                            // misidentify the idle period before Start as a sleep gap.
                            last_tick_at = Utc::now();
                            let _ = app.emit("timer-tick", TimerPayload {
                                total_secs: today_secs_cache as u64,
                                is_running: true,
                                project_id: current_project_id.clone(),
                            });
                        }
                    }
                    Some(TimerCommand::Stop) => {
                        // Always clear idle state regardless of the running flag.
                        // cmd_stop_after_idle sends Stop when running=false (idle window
                        // already open), so we must reset outside the guard to stop the
                        // tick from firing forever (BUG-H01 from audit #3).
                        idle_window_opened = false;
                        idle_last_active_at = None;
                        if running {
                            running = false;
                            is_running_flag.store(false, Ordering::Relaxed);
                            activity.timer_active.store(false, Ordering::Relaxed);
                            // Ensure idle window is gone and main is re-enabled
                            // (covers tray "Stop" while idle dialog is showing).
                            if let Some(idle) = app.get_webview_window("idle") {
                                let _ = idle.close();
                            }
                            if let Some(main) = app.get_webview_window("main") {
                                let _ = main.set_enabled(true);
                            }

                            let started_at_opt = chunk_started_at.take();
                            let sid = stub_slot_id.take();
                            if let (Some(started_at), Some(ref project_id)) =
                                (started_at_opt, &current_project_id)
                            {
                                finalize_slot(&pool, project_id, sid, started_at, Utc::now(), &activity).await;
                            }
                            current_project_id = None;
                            *current_slot_id.lock().await = None;

                            last_activity_at = None;
                            last_seen_activity_ts = 0;
                            chunk_elapsed_secs = 0;
                            last_progress_save = 0;
                            activity.active_seconds.store(0, Ordering::Relaxed);
                            activity.total_seconds.store(0, Ordering::Relaxed);

                            today_secs_cache = get_today_secs(&pool).await;
                            current_day = day_key(&Local::now());
                            let _ = app.emit("timer-tick", TimerPayload {
                                total_secs: today_secs_cache as u64,
                                is_running: false,
                                project_id: None,
                            });
                        }
                    }
                    Some(TimerCommand::Reset) => {
                        // Clear idle state unconditionally so the tick stops firing
                        // if Reset is called while the idle window is open (BUG-H02).
                        idle_window_opened = false;
                        idle_last_active_at = None;
                        running = false;
                        is_running_flag.store(false, Ordering::Relaxed);
                        activity.timer_active.store(false, Ordering::Relaxed);

                        // Clear current_slot_id BEFORE deleting the stub row.
                        // app_tracker_actor reads current_slot_id every 5s; if we delete
                        // the row first, it can still read the old id and attempt an INSERT
                        // into app_usage referencing a now-deleted time_slot, causing a
                        // foreign-key error or silent data corruption.
                        *current_slot_id.lock().await = None;

                        // Удаляем stub — прогресс сбрасывается
                        if let Some(sid) = stub_slot_id.take() {
                            let _ = sqlx::query("DELETE FROM time_slots WHERE id = ? AND synced = 0")
                                .bind(sid)
                                .execute(&pool)
                                .await;
                            log::info!("[timer] stub slot {} deleted on reset", sid);
                        }

                        chunk_started_at = None;
                        current_project_id = None;
                        last_activity_at = None;
                        last_seen_activity_ts = 0;
                        chunk_elapsed_secs = 0;
                        last_progress_save = 0;
                        activity.active_seconds.store(0, Ordering::Relaxed);
                        activity.total_seconds.store(0, Ordering::Relaxed);
                        today_secs_cache = get_today_secs(&pool).await;
                        current_day = day_key(&Local::now());
                        let _ = app.emit("timer-tick", TimerPayload {
                            total_secs: today_secs_cache as u64,
                            is_running: false,
                            project_id: None,
                        });
                    }
                    None => break,
                }
            }
            // Tick fires when running OR when idle window is open (to detect
            // sleep/wake while the user is looking at the idle dialog).
            _ = tick.tick(), if running || idle_window_opened => {
                // ── Read current tracking settings (cheap clone) ──────────────
                let s = settings.lock().await.clone();

                // ── Sleep/wake detection ──────────────────────────────────────
                // Compute wall-clock time since the previous tick. On macOS,
                // tokio's monotonic interval advances during system sleep, so the
                // interval fires immediately on wake — but Utc::now() shows the
                // full wall-clock gap including sleep time. A gap > SLEEP_DETECT_SECS
                // means the machine was suspended.
                let now_utc = Utc::now();
                let real_elapsed = (now_utc - last_tick_at).num_seconds();
                let prev_tick_at = last_tick_at;
                last_tick_at = now_utc;

                if real_elapsed > SLEEP_DETECT_SECS {
                    if !s.idle_detection_enabled {
                        // Idle detection disabled — treat sleep time as active.
                        // Do not open the idle window; just let the timer continue.
                        continue;
                    }
                    if !running && idle_window_opened {
                        // Machine slept while the idle window was already open.
                        // Recompute total idle time from the original last-activity
                        // moment (includes both pre-sleep idle + sleep duration).
                        let new_idle_secs = idle_last_active_at
                            .map(|t| (now_utc - t).num_seconds().max(0) as u64)
                            .unwrap_or(real_elapsed as u64);
                        log::info!(
                            "[timer] sleep/wake with idle window open: updating idle_secs={}",
                            new_idle_secs
                        );
                        let _ = app.emit("timer-idle-update", IdlePayload { idle_secs: new_idle_secs });
                        continue;
                    }

                    // Timer was running when the machine went to sleep.
                    log::warn!(
                        "[timer] sleep/wake detected: {}s gap — finalizing chunk at pre-sleep time",
                        real_elapsed
                    );
                    running = false;
                    idle_window_opened = true;
                    is_running_flag.store(false, Ordering::Relaxed);
                    // Signal activity_actor to skip its next tick (the one that fires
                    // immediately on wake). Must be set before resetting counters so
                    // activity_actor's SeqCst swap sees it before our Relaxed stores.
                    activity.is_waking.store(true, Ordering::SeqCst);
                    activity.timer_active.store(false, Ordering::Relaxed);

                    let started_at_opt = chunk_started_at.take();
                    let sid = stub_slot_id.take();
                    if let (Some(started_at), Some(ref project_id)) =
                        (started_at_opt, &current_project_id)
                    {
                        finalize_slot(&pool, project_id, sid, started_at, prev_tick_at, &activity)
                            .await;
                    }

                    // Record where idle started so subsequent sleeps can compute
                    // the total idle duration correctly. Use the last known activity
                    // timestamp if available — more accurate than prev_tick_at when
                    // the user was already idle before closing the lid (BUG-H03).
                    idle_last_active_at = Some(last_activity_at.unwrap_or(prev_tick_at));
                    current_project_id = None;
                    last_activity_at = None;
                    last_seen_activity_ts = 0;
                    chunk_elapsed_secs = 0;
                    last_progress_save = 0;
                    activity.active_seconds.store(0, Ordering::Relaxed);
                    activity.total_seconds.store(0, Ordering::Relaxed);
                    *current_slot_id.lock().await = None;

                    today_secs_cache = get_today_secs(&pool).await;
                    let _ = app.emit("timer-tick", TimerPayload {
                        total_secs: today_secs_cache as u64,
                        is_running: false,
                        project_id: None,
                    });

                    open_idle_window(&app, real_elapsed as u64);
                    continue;
                }

                // Idle window is open but no sleep detected — nothing else to do.
                if !running {
                    continue;
                }

                // Day rollover
                let now_local = Local::now();
                let now_day = day_key(&now_local);
                if now_day != current_day {
                    log::info!(
                        "[timer] day rollover: ({}, {}) → ({}, {})",
                        current_day.0, current_day.1, now_day.0, now_day.1
                    );
                    current_day = now_day;

                    // Полночь начала нового дня в UTC — граница разделения chunk
                    let midnight = midnight_utc_of(&now_local);

                    // Финализируем stub старого дня до полуночи.
                    // finalize_slot falls back to INSERT if stub creation had failed.
                    let started_at_opt = chunk_started_at.take();
                    let sid = stub_slot_id.take();
                    if let (Some(started_at), Some(ref project_id)) =
                        (started_at_opt, &current_project_id)
                    {
                        finalize_slot(&pool, project_id, sid, started_at, midnight, &activity).await;
                    }

                    // Новый stub для нового дня, начиная с полуночи
                    if let Some(ref project_id) = current_project_id {
                        if let Some(sid) = save_chunk(&pool, project_id, midnight, midnight, &activity).await {
                            stub_slot_id = Some(sid);
                            *current_slot_id.lock().await = Some(sid);
                        }
                    }

                    chunk_started_at = Some(midnight);
                    today_secs_cache = 0;
                    chunk_elapsed_secs = 0;
                    last_progress_save = 0;
                    activity.active_seconds.store(0, Ordering::Relaxed);
                    activity.total_seconds.store(0, Ordering::Relaxed);

                    let _ = app.emit("day-rollover", ());
                    let _ = app.emit("timer-tick", TimerPayload {
                        total_secs: 0,
                        is_running: true,
                        project_id: current_project_id.clone(),
                    });
                    continue;
                }

                // Activity + Idle detection.
                // Read last_activity_secs written by the listener thread.
                // We do NOT swap activity_flag here — that's activity_actor's job.
                // Using a separate timestamp field prevents the two actors from
                // racing on the same atomic and each missing ~50% of events.
                let activity_ts = activity.last_activity_secs.load(Ordering::Relaxed);
                if activity_ts > last_seen_activity_ts {
                    last_seen_activity_ts = activity_ts;
                    // Use the actual event timestamp stored by the listener rather than
                    // Utc::now() (which is the processing time, up to 1s later).
                    // Utc.timestamp_opt is the non-deprecated chrono API.
                    last_activity_at = Utc.timestamp_opt(activity_ts as i64, 0)
                        .single()
                        .or(Some(Utc::now()));
                }

                if s.idle_detection_enabled {
                if let Some(last_active) = last_activity_at {
                    // .max(0): if the system clock is adjusted backward last_active
                    // can land in the future, making the difference negative. Without
                    // the clamp, idle_secs < 0 means the idle check never fires and
                    // the timer runs forever without auto-saving (BUG-A04).
                    let idle_secs = (Utc::now() - last_active).num_seconds().max(0);
                    if idle_secs >= s.idle_timeout_seconds as i64 && !idle_window_opened {
                        log::info!("[timer] idle: {} secs", idle_secs);
                        running = false;
                        idle_window_opened = true;
                        is_running_flag.store(false, Ordering::Relaxed);
                        activity.timer_active.store(false, Ordering::Relaxed);

                        // Финализируем stub до момента последней активности.
                        // finalize_slot handles the fallback if stub creation had failed.
                        let started_at_opt = chunk_started_at.take();
                        let sid = stub_slot_id.take();
                        if let (Some(started_at), Some(ref project_id)) =
                            (started_at_opt, &current_project_id)
                        {
                            finalize_slot(&pool, project_id, sid, started_at, last_active, &activity).await;
                        }

                        // Record where idle started so that if the machine subsequently
                        // sleeps, we can compute the total idle duration on wake.
                        idle_last_active_at = Some(last_active);
                        current_project_id = None;
                        last_activity_at = None;
                        last_seen_activity_ts = 0;
                        chunk_elapsed_secs = 0;
                        last_progress_save = 0;
                        activity.active_seconds.store(0, Ordering::Relaxed);
                        activity.total_seconds.store(0, Ordering::Relaxed);
                        *current_slot_id.lock().await = None;

                        today_secs_cache = get_today_secs(&pool).await;
                        let _ = app.emit("timer-tick", TimerPayload {
                            total_secs: today_secs_cache as u64,
                            is_running: false,
                            project_id: None,
                        });

                        open_idle_window(&app, idle_secs as u64);
                        continue;
                    }
                }
                } // end if s.idle_detection_enabled

                // Прогресс-сейв каждые 60 секунд: UPDATE stub in-place
                chunk_elapsed_secs += 1;
                if chunk_elapsed_secs - last_progress_save >= PROGRESS_SAVE_EVERY_SECS {
                    last_progress_save = chunk_elapsed_secs;
                    if let (Some(sid), Some(started_at)) = (stub_slot_id, chunk_started_at) {
                        update_slot(&pool, sid, started_at, Utc::now(), &activity).await;
                    }
                }

                // 10-минутный чанк: финализируем текущий stub, создаём новый
                if chunk_elapsed_secs >= CHUNK_SECS {
                    log::info!("[timer] chunk complete");

                    if let (Some(started_at), Some(ref project_id)) = (chunk_started_at.take(), &current_project_id) {
                        let ended_at = Utc::now();

                        // Финализируем текущий stub (fallback to INSERT if stub was missing).
                        finalize_slot(&pool, project_id, stub_slot_id.take(), started_at, ended_at, &activity).await;

                        // Создаём новый stub для следующего чанка
                        if let Some(new_sid) = save_chunk(&pool, project_id, ended_at, ended_at, &activity).await {
                            stub_slot_id = Some(new_sid);
                            *current_slot_id.lock().await = Some(new_sid);
                        }

                        chunk_started_at = Some(ended_at);
                        chunk_elapsed_secs = 0;
                        last_progress_save = 0;
                        activity.active_seconds.store(0, Ordering::Relaxed);
                        activity.total_seconds.store(0, Ordering::Relaxed);

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
                    project_id: current_project_id.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{day_key, midnight_utc_of};
    use chrono::{Datelike, Local, TimeZone, Timelike};

    // ── day_key ──────────────────────────────────────────────────────────────

    /// Dec 31 → Jan 1: ordinal прыгает 366 → 1, но пара (year, ordinal) различается.
    #[test]
    fn test_day_key_new_year_rollover() {
        // 2024 — високосный, поэтому Dec 31 = ordinal 366
        let dec31 = Local.with_ymd_and_hms(2024, 12, 31, 23, 59, 59).unwrap();
        let jan01 = Local.with_ymd_and_hms(2025, 1, 1, 0, 0, 1).unwrap();

        assert_ne!(
            day_key(&dec31),
            day_key(&jan01),
            "year boundary must be detected"
        );
        assert_eq!(day_key(&dec31), (2024, 366));
        assert_eq!(day_key(&jan01), (2025, 1));
    }

    /// Два момента одного и того же дня должны давать одинаковый ключ.
    #[test]
    fn test_day_key_same_day() {
        let morning = Local.with_ymd_and_hms(2025, 6, 15, 0, 0, 0).unwrap();
        let night = Local.with_ymd_and_hms(2025, 6, 15, 23, 59, 59).unwrap();
        assert_eq!(day_key(&morning), day_key(&night));
    }

    /// 23:59:59 и 00:00:00 следующего дня должны давать разные ключи.
    #[test]
    fn test_day_key_midnight_boundary() {
        let before = Local.with_ymd_and_hms(2025, 6, 15, 23, 59, 59).unwrap();
        let after = Local.with_ymd_and_hms(2025, 6, 16, 0, 0, 0).unwrap();
        assert_ne!(day_key(&before), day_key(&after));
    }

    /// Один и тот же ordinal в разных годах — не должны быть равны.
    #[test]
    fn test_day_key_same_ordinal_different_year() {
        let y2024 = Local.with_ymd_and_hms(2024, 4, 9, 12, 0, 0).unwrap(); // ordinal 100
        let y2025 = Local.with_ymd_and_hms(2025, 4, 10, 12, 0, 0).unwrap(); // ordinal 100
                                                                            // Оба ordinal == 100, но годы разные → ключи должны быть разные
        assert_eq!(
            y2024.ordinal(),
            y2025.ordinal(),
            "setup: both should be day 100"
        );
        assert_ne!(
            day_key(&y2024),
            day_key(&y2025),
            "different years must not be equal"
        );
    }

    // ── midnight_utc_of ───────────────────────────────────────────────────────

    /// Конвертация обратно в локальное время должна давать 00:00:00 того же дня.
    #[test]
    fn test_midnight_utc_is_local_midnight() {
        let dt = Local.with_ymd_and_hms(2025, 6, 15, 22, 30, 45).unwrap();
        let midnight = midnight_utc_of(&dt);
        let midnight_local = midnight.with_timezone(&Local);

        assert_eq!(midnight_local.hour(), 0);
        assert_eq!(midnight_local.minute(), 0);
        assert_eq!(midnight_local.second(), 0);
        assert_eq!(midnight_local.date_naive(), dt.date_naive());
    }

    /// Полночь, вычисленная из разных моментов одного дня, должна быть одинаковой.
    #[test]
    fn test_midnight_utc_same_for_same_day() {
        let t1 = Local.with_ymd_and_hms(2025, 3, 17, 8, 0, 0).unwrap();
        let t2 = Local.with_ymd_and_hms(2025, 3, 17, 23, 59, 59).unwrap();
        assert_eq!(midnight_utc_of(&t1), midnight_utc_of(&t2));
    }

    // ── chunk split logic ─────────────────────────────────────────────────────

    /// Chunk начался в 23:55:00, rollover-тик пришёл в 00:00:01.
    /// Часть до полуночи = 5 мин = 300 сек. Новый chunk начинается с полуночи.
    #[test]
    fn test_chunk_split_at_midnight() {
        let chunk_start = Local
            .with_ymd_and_hms(2025, 6, 15, 23, 55, 0)
            .unwrap()
            .to_utc();

        // Тик после полуночи
        let tick_local = Local.with_ymd_and_hms(2025, 6, 16, 0, 0, 1).unwrap();
        let midnight = midnight_utc_of(&tick_local);

        let secs_yesterday = (midnight - chunk_start).num_seconds().max(0);
        assert_eq!(secs_yesterday, 300, "5 minutes before midnight = 300 secs");

        // Новый chunk стартует ровно с полуночи
        let new_start_local = midnight.with_timezone(&Local);
        assert_eq!(new_start_local.hour(), 0);
        assert_eq!(new_start_local.minute(), 0);
        assert_eq!(new_start_local.second(), 0);
        assert_eq!(new_start_local.date_naive(), tick_local.date_naive());
    }

    /// Если chunk начался до полуночи, но rollover был обнаружен через 3 тика
    /// (до 00:00:03), вчерашняя часть всё равно должна рассчитываться корректно.
    #[test]
    fn test_chunk_split_late_detection() {
        let chunk_start = Local
            .with_ymd_and_hms(2025, 12, 31, 23, 50, 0)
            .unwrap()
            .to_utc();

        let tick_local = Local.with_ymd_and_hms(2026, 1, 1, 0, 0, 3).unwrap();
        let midnight = midnight_utc_of(&tick_local);

        let secs_yesterday = (midnight - chunk_start).num_seconds().max(0);
        // 23:50:00 → 00:00:00 = 10 мин = 600 сек
        assert_eq!(secs_yesterday, 600);

        // Проверяем, что midnight принадлежит новому году
        let midnight_local = midnight.with_timezone(&Local);
        assert_eq!(midnight_local.year(), 2026);
        assert_eq!(midnight_local.month(), 1);
        assert_eq!(midnight_local.day(), 1);
    }

    /// Chunk начался уже после полуночи — secs_yesterday должно быть 0
    /// (не сохранять пустой слот для вчерашнего дня).
    #[test]
    fn test_chunk_no_yesterday_if_started_after_midnight() {
        // Гипотетический случай: rollover определён поздно, но chunk начался
        // уже после полуночи (например, таймер запустили в 00:05)
        let chunk_start = Local
            .with_ymd_and_hms(2025, 6, 16, 0, 5, 0)
            .unwrap()
            .to_utc();

        let tick_local = Local.with_ymd_and_hms(2025, 6, 16, 0, 5, 1).unwrap();
        let midnight = midnight_utc_of(&tick_local);

        let secs_yesterday = (midnight - chunk_start).num_seconds().max(0);
        assert_eq!(
            secs_yesterday, 0,
            "chunk started after midnight — no yesterday portion"
        );
    }
}
