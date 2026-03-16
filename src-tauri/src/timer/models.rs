use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum TimerCommand {
    Start { project_id: String },
    Stop,
    Reset,
}

pub struct TimerState {
    pub sender: mpsc::Sender<TimerCommand>,
    #[allow(dead_code)]
    pub is_running: Arc<AtomicBool>,
}
