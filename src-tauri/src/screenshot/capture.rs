use std::path::PathBuf;
use xcap::Monitor;

/// Делает скриншот основного монитора, сохраняет в app data dir
/// Возвращает путь к файлу
pub fn capture_screenshot(save_dir: &std::path::Path) -> Result<PathBuf, String> {
    let monitors = Monitor::all().map_err(|e| e.to_string())?;
    let monitor = monitors.into_iter().next().ok_or("No monitor found")?;

    let image = monitor.capture_image().map_err(|e| e.to_string())?;

    let filename = format!(
        "screenshot_{}.png",
        chrono::Utc::now().format("%Y%m%d_%H%M%S")
    );
    let path = save_dir.join(&filename);

    image.save(&path).map_err(|e| e.to_string())?;

    Ok(path)
}
