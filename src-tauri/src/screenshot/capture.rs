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

    let monitor = monitors.into_iter().next().ok_or_else(|| {
        sentry::capture_message("No monitor found", sentry::Level::Warning);
        "No monitor found".to_string()
    })?;

    let image = monitor.capture_image().map_err(|e| {
        let err = e.to_string();
        sentry::capture_message(
            &format!("xcap capture_image() failed: {}", err),
            sentry::Level::Error,
        );
        err
    })?;

    let filename = format!(
        "screenshot_{}.png",
        chrono::Utc::now().format("%Y%m%d_%H%M%S")
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
