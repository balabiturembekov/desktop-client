use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::Emitter;
use tauri::Manager;
use tokio::sync::{mpsc, Mutex};
mod api;
mod app;
mod db;
mod screenshot;
mod sync;
mod timer;
mod tracker;

use app::commands::{
    cmd_get_current_user, cmd_get_projects, cmd_get_today_secs, cmd_login, cmd_resume_after_idle,
    cmd_stop_after_idle,
};
use db::init_db;
use screenshot::actor::screenshot_actor;
use sync::actor::sync_actor;
use timer::{
    actor::time_actor,
    commands::{reset_worker_timer, start_worker_timer, stop_worker_timer},
    models::TimerState,
};
use tracker::{actor::activity_actor, listener::start_listener, models::ActivityState};

pub fn run() {
    let _sentry = sentry::init((
        option_env!("SENTRY_DSN").unwrap_or(""),
        sentry::ClientOptions {
            release: sentry::release_name!(),
            environment: Some(if cfg!(debug_assertions) {
                "development".into()
            } else {
                "production".into()
            }),
            ..Default::default()
        },
    ));


    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();

            // Проверяем обновления при старте — в фоне
            let update_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                check_for_updates(update_handle).await;
            });

            let pool =
                tauri::async_runtime::block_on(init_db(&handle)).expect("failed to init database");

            let screenshots_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir")
                .join("screenshots");
            std::fs::create_dir_all(&screenshots_dir).expect("failed to create screenshots dir");

            let is_running = Arc::new(AtomicBool::new(false));
            let activity_state = ActivityState::new();
            let current_slot_id: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(None));

            let (tx, rx) = mpsc::channel(8);
            tauri::async_runtime::spawn(time_actor(
                rx,
                handle.clone(),
                pool.clone(),
                is_running.clone(),
                activity_state.clone(),
                current_slot_id.clone(),
            ));
            app.manage(TimerState {
                sender: tx,
                is_running: is_running.clone(),
            });

            let listener_state = activity_state.clone();
            std::thread::Builder::new()
                .name("activity-listener".to_string())
                .spawn(move || start_listener(listener_state))
                .expect("failed to spawn activity listener thread");

            tauri::async_runtime::spawn(activity_actor(
                activity_state,
                handle.clone(),
                pool.clone(),
                is_running.clone(),
            ));

            tauri::async_runtime::spawn(screenshot_actor(
                pool.clone(),
                screenshots_dir,
                is_running.clone(),
                current_slot_id,
            ));

            tauri::async_runtime::spawn(sync_actor(pool.clone()));

            app.manage(pool);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cmd_login,
            cmd_get_current_user,
            cmd_get_projects,
            cmd_get_today_secs,
            cmd_resume_after_idle,
            cmd_stop_after_idle,
            start_worker_timer,
            stop_worker_timer,
            reset_worker_timer,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

async fn check_for_updates(app: tauri::AppHandle) {
    use tauri_plugin_updater::UpdaterExt;

    match app.updater() {
        Ok(updater) => {
            match updater.check().await {
                Ok(Some(update)) => {
                    log::info!("[updater] new version available: {}", update.version);
                    // Уведомляем фронт
                    let _ = app.emit("update-available", update.version.clone());

                    // Скачиваем и устанавливаем
                    match update.download_and_install(|_, _| {}, || {}).await {
                        Ok(_) => {
                            log::info!("[updater] update installed — restarting");
                            app.restart();
                        }
                        Err(e) => {
                            sentry::capture_message(
                                &format!("Update install failed: {}", e),
                                sentry::Level::Error,
                            );
                            log::error!("[updater] install error: {}", e);
                        }
                    }
                }
                Ok(None) => log::info!("[updater] app is up to date"),
                Err(e) => log::warn!("[updater] check error: {}", e),
            }
        }
        Err(e) => log::warn!("[updater] init error: {}", e),
    }
}
