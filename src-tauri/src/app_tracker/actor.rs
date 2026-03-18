use std::collections::HashMap;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

use chrono::Utc;
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::app_tracker::models::AppKey;

const POLL_SECS: u64 = 5;
const FLUSH_EVERY_N_TICKS: u64 = 12; // flush every 60s
const LOG_ALIVE_EVERY_N_TICKS: u64 = 6; // log "alive" every 30s
/// After this many consecutive get_active_window errors, suppress per-tick logs
/// and only emit one log every LOG_ALIVE_EVERY_N_TICKS ticks to avoid log spam
/// (e.g. when Screen Recording permission is permanently denied).
const ERROR_LOG_SUPPRESS_AFTER: u64 = 3;

struct Accum {
    duration_secs: i64,
    started_at: String,
}

pub async fn app_tracker_actor(
    pool: SqlitePool,
    is_running: Arc<std::sync::atomic::AtomicBool>,
    current_slot_id: Arc<Mutex<Option<i64>>>,
) {
    log::info!("[app_tracker] actor started");

    let mut accum: HashMap<AppKey, Accum> = HashMap::new();
    let mut last_slot_id: Option<i64> = None;
    let mut tick_count: u64 = 0;
    let mut alive_tick: u64 = 0;
    // Counts consecutive get_active_window failures to suppress log spam.
    let mut consecutive_errors: u64 = 0;
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_SECS));

    loop {
        interval.tick().await;
        alive_tick += 1;

        let running = is_running.load(Ordering::Relaxed);
        let slot_id = *current_slot_id.lock().await;

        // Periodic heartbeat
        if alive_tick.is_multiple_of(LOG_ALIVE_EVERY_N_TICKS) {
            log::info!(
                "[app_tracker] alive — is_running={} slot_id={:?}",
                running,
                slot_id
            );
        }

        // Slot changed — flush old accumulation into the old slot.
        if slot_id != last_slot_id {
            log::info!(
                "[app_tracker] slot changed: {:?} → {:?}",
                last_slot_id,
                slot_id
            );
            if let Some(old_slot) = last_slot_id {
                flush(&pool, old_slot, &mut accum).await;
            }
            last_slot_id = slot_id;
            tick_count = 0;
        }

        // Only track when timer is running and we have an active slot.
        let current_slot = match slot_id {
            Some(id) if running => id,
            _ => continue,
        };

        // Sample active window on a blocking OS thread.
        let result = tokio::task::spawn_blocking(active_win_pos_rs::get_active_window).await;

        let window = match result {
            Ok(Ok(w)) => {
                consecutive_errors = 0;
                w
            }
            Ok(Err(e)) => {
                consecutive_errors += 1;
                // Log every error for the first few, then throttle to avoid spam
                // when e.g. Screen Recording permission is permanently denied.
                if consecutive_errors <= ERROR_LOG_SUPPRESS_AFTER
                    || consecutive_errors.is_multiple_of(LOG_ALIVE_EVERY_N_TICKS)
                {
                    log::warn!(
                        "[app_tracker] get_active_window error (#{consecutive_errors}): {:?}",
                        e
                    );
                }
                continue;
            }
            Err(join_err) => {
                consecutive_errors += 1;
                log::error!(
                    "[app_tracker] spawn_blocking panicked (#{consecutive_errors}): {}",
                    join_err
                );
                continue;
            }
        };

        // Window titles can contain sensitive content (email subjects, document
        // names, banking pages). Log only the app name at INFO so these stay
        // out of production log files; full title goes to DEBUG only (BUG-A01).
        log::info!("[app_tracker] tick {} — app='{}'", tick_count, window.app_name);
        log::debug!("[app_tracker] tick {} — title='{}'", tick_count, window.title);

        // Skip our own app.
        if window.app_name.to_lowercase().contains("hubnity") {
            log::info!("[app_tracker] skipping own app window");
            continue;
        }

        // Aggregate time by (app_name, window_title).
        // Note: window_title changes frequently (browser tabs, IDE files), so
        // each unique title becomes its own entry. This is intentional — it lets
        // the backend see which specific context the user was in.
        let key = AppKey {
            app_name: window.app_name,
            window_title: window.title,
            url: None,
        };

        let entry = accum.entry(key).or_insert_with(|| Accum {
            duration_secs: 0,
            started_at: Utc::now().to_rfc3339(),
        });
        entry.duration_secs += POLL_SECS as i64;

        tick_count += 1;
        if tick_count >= FLUSH_EVERY_N_TICKS {
            flush(&pool, current_slot, &mut accum).await;
            tick_count = 0;
        }
    }
}

/// Flushes accumulated app-usage entries to the DB for `slot_id`.
///
/// Entries that fail to insert are put back into `accum` so they are
/// retried on the next flush cycle instead of being silently dropped.
/// This prevents data loss when SQLite is temporarily busy or locked.
async fn flush(pool: &SqlitePool, slot_id: i64, accum: &mut HashMap<AppKey, Accum>) {
    if accum.is_empty() {
        return;
    }

    log::info!(
        "[app_tracker] flushing {} entries for slot {}",
        accum.len(),
        slot_id
    );

    // Drain into a Vec so we can put failures back without borrow conflicts.
    let entries: Vec<(AppKey, Accum)> = accum.drain().collect();
    let mut failed: Vec<(AppKey, Accum)> = Vec::new();

    for (key, entry) in entries {
        match sqlx::query(
            r#"INSERT INTO app_usage (time_slot_id, app_name, window_title, url, duration_secs, started_at)
               VALUES (?, ?, ?, ?, ?, ?)"#,
        )
        .bind(slot_id)
        .bind(&key.app_name)
        .bind(&key.window_title)
        .bind(key.url.as_deref())
        .bind(entry.duration_secs)
        .bind(&entry.started_at)
        .execute(pool)
        .await
        {
            Ok(_) => log::info!(
                "[app_tracker] inserted '{}' {}s",
                key.app_name,
                entry.duration_secs
            ),
            Err(e) => {
                log::warn!(
                    "[app_tracker] insert failed for '{}', will retry: {}",
                    key.app_name,
                    e
                );
                failed.push((key, entry));
            }
        }
    }

    if !failed.is_empty() {
        log::warn!(
            "[app_tracker] {} entries failed to insert, keeping for next flush",
            failed.len()
        );
        for (key, entry) in failed {
            // Merge back: if the key re-appeared since drain, add durations.
            accum
                .entry(key)
                .and_modify(|e| e.duration_secs += entry.duration_secs)
                .or_insert(entry);
        }
    }

    log::info!("[app_tracker] flush done for slot {}", slot_id);
}
