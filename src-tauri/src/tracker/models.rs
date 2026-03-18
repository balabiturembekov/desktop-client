use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Arc;

/// Shared state between the listener thread and the async actors.
#[derive(Clone)]
pub struct ActivityState {
    /// Set to true by the listener on each input event.
    /// Consumed (swap → false) only by activity_actor for counting.
    pub activity_flag: Arc<AtomicBool>,
    /// Active-input seconds in the current 10-minute interval.
    pub active_seconds: Arc<AtomicU32>,
    /// Total elapsed seconds in the current 10-minute interval (max 600).
    pub total_seconds: Arc<AtomicU32>,
    /// Unix timestamp (seconds) of the most recent input event.
    /// Written by the listener; read by time_actor for idle detection.
    /// Separate from activity_flag so the two actors don't race on the same flag.
    pub last_activity_secs: Arc<AtomicU64>,
    /// Set to true by time_actor immediately after sleep/wake detection,
    /// consumed (swap → false) by activity_actor to skip the post-sleep
    /// burst of backlogged ticks and avoid inflating total_seconds.
    pub is_waking: Arc<AtomicBool>,
    /// Set to true by time_actor on Start, false on Stop/Idle/Sleep/Reset.
    /// Read by the listener thread to throttle polling frequency:
    /// 100 ms while the timer is running, 1 000 ms otherwise.
    pub timer_active: Arc<AtomicBool>,
}

impl ActivityState {
    pub fn new() -> Self {
        Self {
            activity_flag: Arc::new(AtomicBool::new(false)),
            active_seconds: Arc::new(AtomicU32::new(0)),
            total_seconds: Arc::new(AtomicU32::new(0)),
            last_activity_secs: Arc::new(AtomicU64::new(0)),
            is_waking: Arc::new(AtomicBool::new(false)),
            timer_active: Arc::new(AtomicBool::new(false)),
        }
    }
}
