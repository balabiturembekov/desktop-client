use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use device_query::{DeviceQuery, DeviceState};
use crate::tracker::models::ActivityState;

/// Minimum mouse displacement (squared) to count as intentional movement.
/// Filters out hardware jitter / resting-hand micro-movements.
/// 5 px threshold → 5² = 25.
const MOUSE_THRESHOLD_SQ: i32 = 25;

pub fn start_listener(state: ActivityState) {
    // device_query panics if Accessibility permissions are not granted on macOS.
    let result = std::panic::catch_unwind(DeviceState::new);

    let device_state = match result {
        Ok(ds) => ds,
        Err(_) => {
            sentry::capture_message(
                "device_query failed: Accessibility permissions not granted. Activity tracking disabled.",
                sentry::Level::Warning,
            );
            eprintln!("[tracker] Accessibility permissions not granted — activity tracking disabled");
            return;
        }
    };

    let mut last_mouse_pos = device_state.get_mouse().coords;
    let mut last_keys: Vec<device_query::Keycode> = vec![];

    loop {
        let mouse = device_state.get_mouse();
        let keys = device_state.get_keys();

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

        std::thread::sleep(Duration::from_millis(100));
    }
}
