use std::panic::AssertUnwindSafe;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use device_query::{DeviceQuery, DeviceState};
use tauri::{AppHandle, Emitter};

use crate::tracker::models::ActivityState;

/// Minimum mouse displacement (squared) to count as intentional movement.
/// Filters out hardware jitter / resting-hand micro-movements.
/// 5 px threshold → 5² = 25.
const MOUSE_THRESHOLD_SQ: i32 = 25;
const RETRY_SECS: u64 = 30;

/// Returns true if the process has Accessibility permission on macOS.
/// Calls `AXIsProcessTrusted()` from the ApplicationServices framework directly
/// so we can check the status **before** calling DeviceState::new — which panics
/// if the permission is not granted. Checking first means:
/// - No panic → no unwanted Sentry report (`handled: false`)
/// - Clean retry every RETRY_SECS without spamming the panic hook
#[cfg(target_os = "macos")]
fn is_accessibility_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

#[cfg(not(target_os = "macos"))]
fn is_accessibility_trusted() -> bool {
    true
}

pub fn start_listener(state: ActivityState, app: AppHandle) {
    log::info!("[tracker] listener thread started");

    loop {
        if !is_accessibility_trusted() {
            log::warn!(
                "[tracker] Accessibility permission not granted — retrying in {}s",
                RETRY_SECS
            );
            let _ = app.emit("accessibility-denied", ());
            std::thread::sleep(Duration::from_secs(RETRY_SECS));
            continue;
        }

        // Accessibility is granted — create DeviceState.
        // Still wrapped in catch_unwind as a safety net for unexpected device_query internals.
        let device_state = match std::panic::catch_unwind(DeviceState::new) {
            Ok(ds) => ds,
            Err(_) => {
                log::warn!(
                    "[tracker] DeviceState::new failed unexpectedly — retrying in {}s",
                    RETRY_SECS
                );
                std::thread::sleep(Duration::from_secs(RETRY_SECS));
                continue;
            }
        };

        log::info!("[tracker] activity listener running");
        let _ = app.emit("accessibility-granted", ());

        let mut last_mouse_pos = device_state.get_mouse().coords;
        let mut last_keys: Vec<device_query::Keycode> = vec![];

        loop {
            // Wrap each poll in catch_unwind — device_query 2.x can panic in
            // get_keys()/get_mouse() if accessibility is revoked at runtime.
            let poll = std::panic::catch_unwind(AssertUnwindSafe(|| {
                let mouse = device_state.get_mouse();
                let keys = device_state.get_keys();
                (mouse, keys)
            }));

            match poll {
                Ok((mouse, keys)) => {
                    let dx = mouse.coords.0 - last_mouse_pos.0;
                    let dy = mouse.coords.1 - last_mouse_pos.1;
                    let mouse_moved = dx * dx + dy * dy > MOUSE_THRESHOLD_SQ;
                    let new_key = keys.iter().any(|k| !last_keys.contains(k));
                    let mouse_clicked = mouse.button_pressed.iter().any(|&b| b);

                    if mouse_moved || new_key || mouse_clicked {
                        let now_secs = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        state.activity_flag.store(true, Ordering::Relaxed);
                        state.last_activity_secs.store(now_secs, Ordering::Relaxed);
                    }

                    last_mouse_pos = mouse.coords;
                    last_keys = keys;
                }
                Err(_) => {
                    log::warn!(
                        "[tracker] poll panicked (accessibility revoked?) — restarting in {}s",
                        RETRY_SECS
                    );
                    let _ = app.emit("accessibility-denied", ());
                    std::thread::sleep(Duration::from_secs(RETRY_SECS));
                    break; // break inner loop → outer loop re-checks permission
                }
            }

            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
