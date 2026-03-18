use rand::{distributions::Alphanumeric, Rng};
use std::path::Path;
use xcap::Monitor;

pub fn capture_screenshot(save_dir: &Path) -> Result<std::path::PathBuf, String> {
    let monitors = Monitor::all().map_err(|e| {
        let err = e.to_string();
        sentry::capture_message(
            &format!("xcap Monitor::all() failed: {}", err),
            sentry::Level::Error,
        );
        err
    })?;

    // Pick the monitor with the largest visible area (width × height).
    // On single-monitor setups this is the only one; on multi-monitor setups
    // it selects the highest-resolution screen where active work most likely
    // happens, instead of always capturing monitors[0] which may be a small
    // secondary display (BUG-A05).
    let monitor = monitors
        .into_iter()
        .max_by_key(|m| m.width().unwrap_or(0) * m.height().unwrap_or(0))
        .ok_or_else(|| {
            sentry::capture_message("No usable monitor found", sentry::Level::Warning);
            "No usable monitor found".to_string()
        })?;

    let image = monitor.capture_image().map_err(|e| {
        let err = e.to_string();
        sentry::capture_message(
            &format!("xcap capture_image() failed: {}", err),
            sentry::Level::Error,
        );
        err
    })?;

    // Append a 4-character random suffix so two screenshots taken within the
    // same UTC second (possible at chunk boundaries) never overwrite each
    // other on disk. Without this, the second file would silently replace the
    // first and the DB would hold two records for one file (BUG-A09).
    let suffix: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(4)
        .map(char::from)
        .collect::<String>()
        .to_lowercase();

    let filename = format!(
        "screenshot_{}_{}.png",
        chrono::Utc::now().format("%Y%m%d_%H%M%S"),
        suffix
    );
    let path = save_dir.join(&filename);

    image.save(&path).map_err(|e| {
        let err = e.to_string();
        sentry::capture_message(
            &format!("Failed to save screenshot to {:?}: {}", path, err),
            sentry::Level::Error,
        );
        err
    })?;

    Ok(path)
}
