use chrono::Utc;
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use crate::screenshot::capture::capture_screenshot;

const INTERVAL_SECS: u64 = 600;

pub async fn screenshot_actor(
    pool: SqlitePool,
    screenshots_dir: PathBuf,
    is_running: Arc<AtomicBool>,
) {
    loop {
        if !is_running.load(Ordering::Relaxed) {
            sleep(Duration::from_secs(2)).await;
            continue;
        }

        let random_delay = {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos();
            (nanos % (INTERVAL_SECS as u32 * 1000)) as u64 / 1000
        };

        sleep(Duration::from_secs(random_delay.max(5))).await;

        if !is_running.load(Ordering::Relaxed) {
            continue;
        }

        match capture_screenshot(&screenshots_dir) {
            Ok(path) => {
                let taken_at = Utc::now().to_rfc3339();
                let path_str = path.to_string_lossy().to_string();
                let pool = pool.clone();

                tokio::spawn(async move {
                    let result = sqlx::query_as::<_, (i64,)>(
                        "SELECT id FROM time_slots ORDER BY id DESC LIMIT 1",
                    )
                    .fetch_optional(&pool)
                    .await;

                    if let Ok(Some((slot_id,))) = result {
                        let _ = sqlx::query(
                            r#"INSERT INTO screenshots (time_slot_id, file_path, taken_at, synced)
                            VALUES (?, ?, ?, 0)"#,
                        )
                        .bind(slot_id)
                        .bind(&path_str)
                        .bind(&taken_at)
                        .execute(&pool)
                        .await;
                        println!("[screenshot] saved: {}", path_str);
                    }
                });
            }
            Err(e) => eprintln!("[screenshot] capture error: {}", e),
        }

        let remaining = INTERVAL_SECS.saturating_sub(random_delay.max(5));
        sleep(Duration::from_secs(remaining.max(1))).await;
    }
}
