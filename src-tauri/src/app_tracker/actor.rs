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
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_SECS));

    loop {
        interval.tick().await;
        alive_tick += 1;

        let running = is_running.load(Ordering::Relaxed);
        let slot_id = *current_slot_id.lock().await;

        // Periodic heartbeat so we can see the actor is alive in logs
        if alive_tick.is_multiple_of(LOG_ALIVE_EVERY_N_TICKS) {
            log::info!(
                "[app_tracker] alive — is_running={} slot_id={:?}",
                running,
                slot_id
            );
        }

        // Slot changed — flush old accumulation to DB
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

        // Only track when timer is running and we have an active slot
        let current_slot = match slot_id {
            Some(id) if running => id,
            _ => continue,
        };

        // Sample active window on a blocking OS thread
        let result =
            tokio::task::spawn_blocking(active_win_pos_rs::get_active_window).await;

        let window = match result {
            Ok(Ok(w)) => w,
            Ok(Err(e)) => {
                // Happens when Screen Recording permission is denied on macOS,
                // or when no window is focused. Log at warn level so it's visible.
                log::warn!("[app_tracker] get_active_window error: {:?}", e);
                continue;
            }
            Err(join_err) => {
                log::error!("[app_tracker] spawn_blocking panicked: {}", join_err);
                continue;
            }
        };

        log::info!(
            "[app_tracker] tick {} — app='{}' title='{}'",
            tick_count,
            window.app_name,
            window.title
        );

        // Skip our own app
        if window.app_name.to_lowercase().contains("hubnity") {
            log::info!("[app_tracker] skipping own app window");
            continue;
        }

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

async fn flush(pool: &SqlitePool, slot_id: i64, accum: &mut HashMap<AppKey, Accum>) {
    if accum.is_empty() {
        log::info!("[app_tracker] flush called but accum is empty for slot {}", slot_id);
        return;
    }

    log::info!(
        "[app_tracker] flushing {} entries for slot {}",
        accum.len(),
        slot_id
    );

    for (key, entry) in accum.drain() {
        let result = sqlx::query(
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
        .await;

        match result {
            Ok(_) => log::info!(
                "[app_tracker] inserted '{}' {}s",
                key.app_name,
                entry.duration_secs
            ),
            Err(e) => log::warn!("[app_tracker] insert failed for '{}': {}", key.app_name, e),
        }
    }

    log::info!("[app_tracker] flush done for slot {}", slot_id);
}
