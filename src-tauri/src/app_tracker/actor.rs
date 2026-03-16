use std::collections::HashMap;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

use chrono::Utc;
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::app_tracker::models::AppKey;

const POLL_SECS: u64 = 5;
const FLUSH_EVERY_N_TICKS: u64 = 12; // flush every 60s

struct Accum {
    duration_secs: i64,
    started_at: String,
}

pub async fn app_tracker_actor(
    pool: SqlitePool,
    is_running: Arc<std::sync::atomic::AtomicBool>,
    current_slot_id: Arc<Mutex<Option<i64>>>,
) {
    let mut accum: HashMap<AppKey, Accum> = HashMap::new();
    let mut last_slot_id: Option<i64> = None;
    let mut tick_count: u64 = 0;
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_SECS));

    loop {
        interval.tick().await;

        let slot_id = *current_slot_id.lock().await;

        // Slot changed — flush old accumulation to DB
        if slot_id != last_slot_id {
            if let Some(old_slot) = last_slot_id {
                flush(&pool, old_slot, &mut accum).await;
            }
            last_slot_id = slot_id;
            tick_count = 0;
        }

        let current_slot = match slot_id {
            Some(id) if is_running.load(Ordering::Relaxed) => id,
            _ => continue,
        };

        // Sample active window on the OS thread to avoid blocking the async runtime
        let window = match tokio::task::spawn_blocking(active_win_pos_rs::get_active_window).await {
            Ok(Ok(w)) => w,
            _ => continue,
        };

        // Skip our own app
        if window.app_name.to_lowercase().contains("hubnity") {
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
        return;
    }

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

        if let Err(e) = result {
            log::warn!("[app_tracker] insert failed: {}", e);
        }
    }

    log::info!("[app_tracker] flushed usage for slot {}", slot_id);
}
