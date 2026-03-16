use tauri::State;
use crate::timer::models::{TimerCommand, TimerState};

#[tauri::command]
pub async fn start_worker_timer(
    project_id: String,
    state: State<'_, TimerState>,
) -> Result<(), String> {
    state
        .sender
        .send(TimerCommand::Start { project_id })
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stop_worker_timer(state: State<'_, TimerState>) -> Result<(), String> {
    state.sender.send(TimerCommand::Stop).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reset_worker_timer(state: State<'_, TimerState>) -> Result<(), String> {
    state.sender.send(TimerCommand::Reset).await.map_err(|e| e.to_string())
}
