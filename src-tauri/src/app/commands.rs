use chrono::Utc;
use sqlx::SqlitePool;
use tauri::{Manager, State};

use crate::api::auth::{fetch_projects, login};
use crate::db::models::{project::Project, user::User};
use crate::timer::models::{TimerCommand, TimerState};

#[tauri::command]
pub async fn cmd_login(
    email: String,
    password: String,
    pool: State<'_, SqlitePool>,
) -> Result<User, String> {
    let res = login(&email, &password).await?;
    let user = User {
        id: 0,
        remote_id: res.user.id,
        email: res.user.email,
        name: res.user.name,
        avatar: res.user.avatar,
        role: res.user.role,
        access_token: res.access_token.clone(),
        refresh_token: res.refresh_token.clone(),
        created_at: res.user.created_at,
    };
    User::save(&pool, &user).await.map_err(|e| e.to_string())?;

    let remote_projects = fetch_projects(&res.access_token).await?;
    let projects: Vec<Project> = remote_projects
        .into_iter()
        .map(|p| Project {
            id: 0,
            remote_id: p.id,
            name: p.name,
            is_active: 1,
            created_at: Utc::now().to_rfc3339(),
        })
        .collect();
    Project::save_many(&pool, &projects).await.map_err(|e| e.to_string())?;
    Ok(user)
}

#[tauri::command]
pub async fn cmd_get_current_user(
    pool: State<'_, SqlitePool>,
) -> Result<Option<User>, String> {
    User::get_current(&pool).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_get_projects(
    pool: State<'_, SqlitePool>,
) -> Result<Vec<Project>, String> {
    Project::get_active(&pool).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_get_today_secs(
    pool: State<'_, SqlitePool>,
) -> Result<u64, String> {
    let row = sqlx::query_as::<_, (Option<i64>,)>(
        "SELECT SUM(duration_secs) FROM time_slots WHERE date(started_at) = date('now')"
    )
    .fetch_one(&*pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(row.0.unwrap_or(0) as u64)
}

fn close_idle_and_enable_main(app: &tauri::AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_enabled(true);
        let _ = main.set_focus();
    }
    if let Some(idle) = app.get_webview_window("idle") {
        let _ = idle.close();
    }
}

#[tauri::command]
pub async fn cmd_resume_after_idle(
    app: tauri::AppHandle,
    state: State<'_, TimerState>,
    pool: State<'_, SqlitePool>,
) -> Result<(), String> {
    let last_project = sqlx::query_as::<_, (String,)>(
        "SELECT project_id FROM time_slots ORDER BY id DESC LIMIT 1"
    )
    .fetch_optional(&*pool)
    .await
    .map_err(|e| e.to_string())?;

    close_idle_and_enable_main(&app);

    if let Some((project_id,)) = last_project {
        state.sender
            .send(TimerCommand::Start { project_id })
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn cmd_stop_after_idle(
    app: tauri::AppHandle,
    state: State<'_, TimerState>,
) -> Result<(), String> {
    close_idle_and_enable_main(&app);
    state.sender
        .send(TimerCommand::Stop)
        .await
        .map_err(|e| e.to_string())
}
