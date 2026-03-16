use std::sync::atomic::Ordering;
use std::time::Duration;
use device_query::{DeviceQuery, DeviceState};
use crate::tracker::models::ActivityState;

pub fn start_listener(state: ActivityState) {
    let device_state = DeviceState::new();
    let mut last_mouse_pos = device_state.get_mouse().coords;
    let mut last_keys: Vec<device_query::Keycode> = vec![];

    loop {
        let mouse = device_state.get_mouse();
        let keys = device_state.get_keys();

        let mouse_moved = mouse.coords != last_mouse_pos;
        let new_key = keys.iter().any(|k| !last_keys.contains(k));
        // button_pressed — Vec<bool>, true означает нажата
        let mouse_clicked = mouse.button_pressed.iter().any(|&b| b);

        if mouse_moved || new_key || mouse_clicked {
            state.activity_flag.store(true, Ordering::Relaxed);
        }

        last_mouse_pos = mouse.coords;
        last_keys = keys;

        std::thread::sleep(Duration::from_millis(100));
    }
}
