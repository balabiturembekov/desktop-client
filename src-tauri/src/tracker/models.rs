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
}

impl ActivityState {
    pub fn new() -> Self {
        Self {
            activity_flag: Arc::new(AtomicBool::new(false)),
            active_seconds: Arc::new(AtomicU32::new(0)),
            total_seconds: Arc::new(AtomicU32::new(0)),
            last_activity_secs: Arc::new(AtomicU64::new(0)),
        }
    }
}
