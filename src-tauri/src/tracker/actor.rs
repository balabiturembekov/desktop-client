use std::sync::atomic::Ordering;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::time::{interval, Duration};
use serde::Serialize;

use crate::tracker::models::ActivityState;

const INTERVAL_SECS: u32 = 600; // 10 минут

#[derive(Serialize, Clone)]
pub struct ActivityPayload {
    pub active_seconds: u32,
    pub total_seconds: u32,
    pub percent: u32,
}

/// Запускается как tokio task
/// Каждую секунду читает флаг активности
/// Каждые 10 минут считает % и сохраняет в последний time_slot
pub async fn activity_actor(
    state: ActivityState,
    app: AppHandle,
    pool: SqlitePool,
    timer_running: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let mut tick = interval(Duration::from_secs(1));

    loop {
        tick.tick().await;

        // Считаем только когда таймер запущен
        if !timer_running.load(Ordering::Relaxed) {
            continue;
        }

        let was_active = state.activity_flag.swap(false, Ordering::Relaxed);
        let total = state.total_seconds.fetch_add(1, Ordering::Relaxed) + 1;

        if was_active {
            state.active_seconds.fetch_add(1, Ordering::Relaxed);
        }

        let active = state.active_seconds.load(Ordering::Relaxed);
        let percent = if total > 0 { (active * 100) / total } else { 0 };

        // Emit на фронт каждую секунду
        let _ = app.emit("activity-tick", ActivityPayload {
            active_seconds: active,
            total_seconds: total,
            percent,
        });

        // Каждые 10 минут — сохраняем в последний time_slot и сбрасываем
        if total >= INTERVAL_SECS {
            let final_percent = percent as i64;
            let pool = pool.clone();

            tokio::spawn(async move {
                let _ = sqlx::query(
                    "UPDATE time_slots SET activity_percent = ? WHERE id = (SELECT MAX(id) FROM time_slots)"
                )
                .bind(final_percent)
                .execute(&pool)
                .await;
            });

            // Сброс счётчиков
            state.active_seconds.store(0, Ordering::Relaxed);
            state.total_seconds.store(0, Ordering::Relaxed);
        }
    }
}
