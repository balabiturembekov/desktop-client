use crate::timer::models::{TimerCommand, TimerState};
use tauri::State;
use tokio::time::timeout;

/// If the time_actor is blocked on a DB operation and the mpsc channel is full,
/// `send().await` would hang indefinitely. A 5-second timeout surfaces the
/// problem as a Tauri command error instead of silently freezing the UI (BUG-A11).
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[tauri::command]
pub async fn start_worker_timer(
    project_id: String,
    state: State<'_, TimerState>,
) -> Result<(), String> {
    timeout(
        SEND_TIMEOUT,
        state.sender.send(TimerCommand::Start { project_id }),
    )
    .await
    .map_err(|_| "Timer command timed out — backend may be busy".to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stop_worker_timer(state: State<'_, TimerState>) -> Result<(), String> {
    timeout(SEND_TIMEOUT, state.sender.send(TimerCommand::Stop))
        .await
        .map_err(|_| "Timer command timed out — backend may be busy".to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reset_worker_timer(state: State<'_, TimerState>) -> Result<(), String> {
    timeout(SEND_TIMEOUT, state.sender.send(TimerCommand::Reset))
        .await
        .map_err(|_| "Timer command timed out — backend may be busy".to_string())?
        .map_err(|e| e.to_string())
}
