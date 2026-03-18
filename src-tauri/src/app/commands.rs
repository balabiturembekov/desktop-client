use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use tauri::{Manager, State};
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;
use tokio::time::Duration;

use crate::api::auth::{fetch_projects, login};
use crate::db::models::{project::Project, user::User};
use crate::timer::models::{TimerCommand, TimerState};

/// Managed state holding the tray menu item that toggles Start/Stop.
/// `last_tooltip` caches the last value written to the tray so we can skip
/// redundant set_text / set_tooltip calls (avoids a macOS/tao race condition).
pub struct TrayState {
    pub timer_item: tauri::menu::MenuItem<tauri::Wry>,
    pub last_tooltip: std::sync::Mutex<Option<(bool, String)>>,
}

/// Payload, отправляемый на фронт при попытке закрытия окна
#[derive(Serialize, Clone)]
pub struct CloseRequestedPayload {
    pub unsynced_count: i64,
    pub timer_running: bool,
}

/// Прогресс скачивания обновления
#[derive(Serialize, Clone)]
pub struct UpdateProgressPayload {
    pub downloaded: u64,
    pub total: u64,
}

#[tauri::command]
pub async fn cmd_login(
    app: tauri::AppHandle,
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
    Project::save_many(&pool, &projects)
        .await
        .map_err(|e| e.to_string())?;

    // Enable autostart on first login so the app launches automatically
    // with the system without requiring any manual configuration.
    if let Err(e) = app.autolaunch().enable() {
        log::warn!("[autostart] failed to enable at login: {}", e);
    } else {
        log::info!("[autostart] enabled at login");
    }

    Ok(user)
}

#[tauri::command]
pub async fn cmd_get_current_user(pool: State<'_, SqlitePool>) -> Result<Option<User>, String> {
    User::get_current(&pool).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_get_projects(pool: State<'_, SqlitePool>) -> Result<Vec<Project>, String> {
    Project::get_active(&pool).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_get_today_secs(pool: State<'_, SqlitePool>) -> Result<u64, String> {
    let row = sqlx::query_as::<_, (Option<i64>,)>(
        "SELECT SUM(duration_secs) FROM time_slots WHERE date(started_at, 'localtime') = date('now', 'localtime')",
    )
    .fetch_one(&*pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(row.0.unwrap_or(0) as u64)
}

/// Opens the macOS Accessibility privacy pane in System Settings / System Preferences.
/// Uses the `open` CLI instead of a URL scheme so it works across all macOS versions
/// (the x-apple.systempreferences: deep-link URL changed in macOS 13 Ventura).
#[tauri::command]
pub async fn cmd_open_accessibility_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Try the new System Settings deep link (macOS 13+) first, then fall back
        // to the legacy System Preferences URL (macOS 12 and earlier).
        let new_url =
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility";
        let legacy_url =
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

        let ok = std::process::Command::new("open")
            .arg(new_url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !ok {
            std::process::Command::new("open")
                .arg(legacy_url)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Немедленно завершает приложение без каких-либо проверок
#[tauri::command]
pub async fn cmd_force_quit(app: tauri::AppHandle) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

/// Останавливает таймер (сохраняет текущий чанк) и завершает приложение
#[tauri::command]
pub async fn cmd_stop_and_quit(
    app: tauri::AppHandle,
    state: State<'_, TimerState>,
) -> Result<(), String> {
    // Only wait for the actor if the timer is actually running; otherwise
    // there is no chunk to flush and the 600 ms delay is pure waste.
    if state
        .is_running
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        let _ = state.sender.send(TimerCommand::Stop).await;
        // Give the actor time to write the final chunk to SQLite.
        tokio::time::sleep(Duration::from_millis(600)).await;
    }
    app.exit(0);
    Ok(())
}

/// Updates the tray tooltip and the Start/Stop menu item label.
/// Called from the frontend at most once per state-change or every 10 s.
/// Skips redundant writes (same is_running + same tooltip) to avoid the
/// macOS/tao race condition that occurs when set_text/set_tooltip are called
/// too frequently from a background thread.
#[tauri::command]
pub async fn cmd_update_tray_status(
    app: tauri::AppHandle,
    tray_state: State<'_, TrayState>,
    is_running: bool,
    time_str: String,
) -> Result<(), String> {
    let tooltip = if is_running {
        format!("Hubnity — {}", time_str)
    } else {
        "Hubnity".to_string()
    };

    // De-duplicate: skip if nothing changed since last call.
    {
        let mut cache = tray_state.last_tooltip.lock().map_err(|e| e.to_string())?;
        if cache.as_ref() == Some(&(is_running, tooltip.clone())) {
            return Ok(());
        }
        *cache = Some((is_running, tooltip.clone()));
    }

    // Update the menu item label
    tray_state
        .timer_item
        .set_text(if is_running {
            "Stop Timer"
        } else {
            "Start Timer"
        })
        .map_err(|e| e.to_string())?;

    // Update tray tooltip
    if let Some(tray) = app.tray_by_id("main") {
        tray.set_tooltip(Some(&tooltip))
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Скачивает и устанавливает доступное обновление.
/// Прогресс передаётся через emit "update-progress".
/// После установки перезапускает приложение.
#[tauri::command]
pub async fn cmd_download_and_install(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Emitter;
    use tauri_plugin_updater::UpdaterExt;

    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(());
    };

    log::info!("[updater] downloading update {}", update.version);

    let app_for_progress = app.clone();
    let mut downloaded_total: u64 = 0;
    update
        .download_and_install(
            move |chunk_size, total| {
                downloaded_total += chunk_size as u64;
                let _ = app_for_progress.emit(
                    "update-progress",
                    UpdateProgressPayload {
                        downloaded: downloaded_total,
                        total: total.unwrap_or(0),
                    },
                );
            },
            || log::info!("[updater] download complete, installing"),
        )
        .await
        .map_err(|e| {
            sentry::capture_message(
                &format!("Update install failed: {}", e),
                sentry::Level::Error,
            );
            e.to_string()
        })?;

    log::info!("[updater] update installed — restarting");
    app.restart()
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
        "SELECT project_id FROM time_slots ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&*pool)
    .await
    .map_err(|e| e.to_string())?;

    close_idle_and_enable_main(&app);

    if let Some((project_id,)) = last_project {
        state
            .sender
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
    state
        .sender
        .send(TimerCommand::Stop)
        .await
        .map_err(|e| e.to_string())
}

/// Stops the timer (idempotent) and clears all user/project rows so the
/// frontend can return to the login screen with a clean slate.
#[tauri::command]
pub async fn cmd_logout(
    state: State<'_, TimerState>,
    pool: State<'_, SqlitePool>,
) -> Result<(), String> {
    // Stop unconditionally — TimerCommand::Stop is a no-op if not running.
    let _ = state.sender.send(TimerCommand::Stop).await;
    // Give the actor time to finalize the current chunk before clearing the DB.
    tokio::time::sleep(Duration::from_millis(300)).await;
    sqlx::query("DELETE FROM users")
        .execute(&*pool)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM projects")
        .execute(&*pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn cmd_autostart_enable(app: tauri::AppHandle) -> Result<(), String> {
    app.autolaunch().enable().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_autostart_disable(app: tauri::AppHandle) -> Result<(), String> {
    app.autolaunch()
        .disable()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_autostart_is_enabled(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|e| e.to_string())
}
