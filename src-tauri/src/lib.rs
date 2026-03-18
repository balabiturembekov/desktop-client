use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::Emitter;
use tauri::Manager;
use tokio::sync::{mpsc, Mutex};
mod api;
mod app;
mod app_tracker;
mod db;
mod screenshot;
mod sync;
mod timer;
mod tracker;

use app::commands::{
    cmd_autostart_disable, cmd_autostart_enable, cmd_autostart_is_enabled,
    cmd_download_and_install, cmd_force_quit, cmd_get_current_user, cmd_get_projects,
    cmd_get_today_secs, cmd_login, cmd_logout, cmd_open_accessibility_settings,
    cmd_resume_after_idle, cmd_stop_after_idle, cmd_stop_and_quit, cmd_update_tray_status,
    CloseRequestedPayload, TrayState,
};
use app_tracker::actor::app_tracker_actor;
use db::init_db;
use screenshot::actor::screenshot_actor;
use sync::actor::sync_actor;
use timer::{
    actor::time_actor,
    commands::{reset_worker_timer, start_worker_timer, stop_worker_timer},
    models::{TimerCommand, TimerState},
};
use tracker::{
    actor::activity_actor,
    listener::{is_accessibility_trusted, start_listener},
    models::ActivityState,
};

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
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();

            // Проверяем обновления при старте и затем каждые 4 часа
            let update_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(4 * 3600));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tick.tick().await; // первый тик — немедленно
                    check_for_updates(update_handle.clone()).await;
                }
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
            let listener_handle = handle.clone();
            let listener_join = std::thread::Builder::new()
                .name("activity-listener".to_string())
                .spawn(move || start_listener(listener_state, listener_handle))
                .expect("failed to spawn activity listener thread");

            // Watchdog: checks every 60s that the listener thread is still alive.
            // If it has exited unexpectedly, logs an error and emits "permissions-required"
            // so the frontend can guide the user to re-grant Accessibility permission.
            let watchdog_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                tick.tick().await; // skip the immediate first tick
                loop {
                    tick.tick().await;
                    if listener_join.is_finished() {
                        log::error!("[tracker] activity-listener thread has exited unexpectedly");
                        // Use a dedicated event so the frontend can show a targeted
                        // message instead of the generic permissions screen (H-04 / L-03
                        // from audit #3). Break immediately — is_finished() stays true
                        // forever, so looping would spam the event every 60 s (BUG-G02).
                        let _ = watchdog_handle.emit("listener-died", ());
                        break;
                    }
                }
            });

            tauri::async_runtime::spawn(activity_actor(
                activity_state,
                handle.clone(),
                is_running.clone(),
            ));

            tauri::async_runtime::spawn(screenshot_actor(
                handle.clone(),
                pool.clone(),
                screenshots_dir,
                is_running.clone(),
                current_slot_id.clone(),
            ));

            tauri::async_runtime::spawn(app_tracker_actor(
                pool.clone(),
                is_running.clone(),
                current_slot_id,
            ));

            tauri::async_runtime::spawn(sync_actor(pool.clone(), handle.clone()));

            app.manage(pool);

            // Emit "permissions-required" at startup if Accessibility is not granted.
            // Deferred by 500ms so the webview's event listeners are ready to receive it.
            let perm_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if !is_accessibility_trusted() {
                    log::warn!("[permissions] Accessibility not granted at startup");
                    let _ = perm_handle.emit("permissions-required", ());
                }
            });

            // ── System Tray ──────────────────────────────────────────────
            let show_item = MenuItemBuilder::with_id("show", "Show Hubnity").build(app)?;
            let timer_item = MenuItemBuilder::with_id("timer_toggle", "Start Timer").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;

            let menu = MenuBuilder::new(app)
                .item(&show_item)
                .item(&sep1)
                .item(&timer_item)
                .item(&sep2)
                .item(&quit_item)
                .build()?;

            let icon = app.default_window_icon().expect("no default icon").clone();

            TrayIconBuilder::with_id("main")
                .icon(icon)
                .menu(&menu)
                .tooltip("Hubnity")
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "timer_toggle" => {
                        let Some(timer_state) = app.try_state::<TimerState>() else {
                            return;
                        };
                        let is_running = timer_state.is_running.load(Ordering::Relaxed);
                        let sender = timer_state.sender.clone();
                        if is_running {
                            tauri::async_runtime::spawn(async move {
                                let _ = sender.send(TimerCommand::Stop).await;
                            });
                        } else {
                            let pool = match app.try_state::<sqlx::SqlitePool>() {
                                Some(p) => p.inner().clone(),
                                None => return,
                            };
                            tauri::async_runtime::spawn(async move {
                                let last = sqlx::query_as::<_, (String,)>(
                                    "SELECT project_id FROM time_slots ORDER BY id DESC LIMIT 1",
                                )
                                .fetch_optional(&pool)
                                .await
                                .ok()
                                .flatten();
                                if let Some((project_id,)) = last {
                                    let _ = sender.send(TimerCommand::Start { project_id }).await;
                                }
                            });
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            app.manage(TrayState {
                timer_item,
                last_tooltip: std::sync::Mutex::new(None),
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            // If the idle window is destroyed by any means (cmd+W, OS, or programmatically)
            // without going through cmd_resume_after_idle / cmd_stop_after_idle,
            // ensure the main window is re-enabled so it doesn't stay frozen forever.
            if window.label() == "idle" {
                if let tauri::WindowEvent::Destroyed = event {
                    if let Some(main) = window.app_handle().get_webview_window("main") {
                        let _ = main.set_enabled(true);
                    }
                }
                return;
            }

            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    check_close_requested(app).await;
                });
            }
        })
        .invoke_handler(tauri::generate_handler![
            cmd_login,
            cmd_logout,
            cmd_get_current_user,
            cmd_get_projects,
            cmd_get_today_secs,
            cmd_resume_after_idle,
            cmd_stop_after_idle,
            start_worker_timer,
            stop_worker_timer,
            reset_worker_timer,
            cmd_force_quit,
            cmd_open_accessibility_settings,
            cmd_stop_and_quit,
            cmd_update_tray_status,
            cmd_download_and_install,
            cmd_autostart_enable,
            cmd_autostart_disable,
            cmd_autostart_is_enabled,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Проверяет наличие несинхронизированных данных и запущенного таймера.
/// Если всё чисто — завершает приложение. Иначе — отправляет событие на фронт.
async fn check_close_requested(app: tauri::AppHandle) {
    let (unsynced_count, timer_running) = {
        let pool = match app.try_state::<sqlx::SqlitePool>() {
            Some(p) => p.inner().clone(),
            None => {
                app.exit(0);
                return;
            }
        };

        let timer_running = app
            .try_state::<timer::models::TimerState>()
            .map(|s| s.is_running.load(Ordering::Relaxed))
            .unwrap_or(false);

        let count = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM time_slots WHERE synced = 0")
            .fetch_one(&pool)
            .await
            .map(|(c,)| c)
            .unwrap_or(0);

        (count, timer_running)
    };

    if unsynced_count > 0 || timer_running {
        let _ = app.emit(
            "close-requested-with-unsynced",
            CloseRequestedPayload {
                unsynced_count,
                timer_running,
            },
        );
    } else {
        // Hide to tray instead of exiting; only "Quit" in tray menu exits fully
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.hide();
        }
    }
}

/// Проверяет наличие обновления и уведомляет фронт.
/// Не скачивает автоматически — пользователь сам инициирует через cmd_download_and_install.
async fn check_for_updates(app: tauri::AppHandle) {
    use tauri_plugin_updater::UpdaterExt;

    match app.updater() {
        Ok(updater) => match updater.check().await {
            Ok(Some(update)) => {
                log::info!("[updater] new version available: {}", update.version);
                let _ = app.emit("update-available", update.version.clone());
            }
            Ok(None) => log::info!("[updater] app is up to date"),
            Err(e) => log::warn!("[updater] check error: {}", e),
        },
        Err(e) => log::warn!("[updater] init error: {}", e),
    }
}
