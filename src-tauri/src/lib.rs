use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tauri::Manager;
use tokio::sync::mpsc;

mod api;
mod app;
mod db;
mod screenshot;
mod sync;
mod timer;
mod tracker;

use app::commands::{
    cmd_get_current_user, cmd_get_projects, cmd_get_today_secs,
    cmd_login, cmd_resume_after_idle, cmd_stop_after_idle,
};
use db::init_db;
use screenshot::actor::screenshot_actor;
use sync::actor::sync_actor;
use timer::{
    actor::time_actor,
    commands::{reset_worker_timer, start_worker_timer, stop_worker_timer},
    models::TimerState,
};
use tracker::{
    actor::activity_actor,
    listener::start_listener,
    models::ActivityState,
};

pub fn run() {
    // Инициализируем Sentry — перехватывает все паники и ошибки
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
        .setup(|app| {
            let handle = app.handle().clone();

            let pool = tauri::async_runtime::block_on(init_db(&handle))
                .expect("failed to init database");

            let screenshots_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir")
                .join("screenshots");
            std::fs::create_dir_all(&screenshots_dir)
                .expect("failed to create screenshots dir");

            let is_running = Arc::new(AtomicBool::new(false));
            let activity_state = ActivityState::new();

            let (tx, rx) = mpsc::channel(8);
            tauri::async_runtime::spawn(time_actor(
                rx,
                handle.clone(),
                pool.clone(),
                is_running.clone(),
                activity_state.clone(),
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
