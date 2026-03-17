use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::time::{interval, Duration};
use serde::Serialize;

use crate::tracker::models::ActivityState;

#[derive(Serialize, Clone)]
pub struct ActivityPayload {
    pub active_seconds: u32,
    pub total_seconds: u32,
    pub percent: u32,
}

/// Runs every second while the timer is active.
///
/// Responsibilities:
/// - Swap `activity_flag` (only this actor consumes it — time_actor uses `last_activity_secs`)
/// - Increment `active_seconds` / `total_seconds`
/// - Emit `activity-tick` to the frontend
///
/// Does NOT persist to DB — timer_actor owns all DB writes via update_slot / save_chunk
/// and resets the counters at every chunk boundary.
pub async fn activity_actor(
    state: ActivityState,
    app: AppHandle,
    timer_running: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut tick = interval(Duration::from_secs(1));

    loop {
        tick.tick().await;

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

        let _ = app.emit_to(
            "main",
            "activity-tick",
            ActivityPayload {
                active_seconds: active,
                total_seconds: total,
                percent,
            },
        );
    }
}
