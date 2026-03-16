use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Arc;

/// Расшариваемое состояние между rdev thread и timer actor
#[derive(Clone)]
pub struct ActivityState {
    /// Была ли активность в текущую секунду
    pub activity_flag: Arc<AtomicBool>,
    /// Счётчик активных секунд в текущем 10-минутном интервале
    pub active_seconds: Arc<AtomicU32>,
    /// Счётчик всего секунд прошло в интервале (макс 600)
    pub total_seconds: Arc<AtomicU32>,
}

impl ActivityState {
    pub fn new() -> Self {
        Self {
            activity_flag: Arc::new(AtomicBool::new(false)),
            active_seconds: Arc::new(AtomicU32::new(0)),
            total_seconds: Arc::new(AtomicU32::new(0)),
        }
    }
}
