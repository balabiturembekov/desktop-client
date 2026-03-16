use reqwest::Client;
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://api.hubnity.io/api/v1";

#[derive(Debug, Serialize)]
pub struct TimeEntryRequest {
    pub project_id: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_secs: i64,
    pub activity_percent: i64,
}

#[derive(Debug, Deserialize)]
pub struct TimeEntryResponse {
    pub id: String,
}

pub async fn create_time_entry(
    token: &str,
    entry: &TimeEntryRequest,
) -> Result<TimeEntryResponse, String> {
    let client = Client::new();

    let res = client
        .post(format!("{}/time-entries", BASE_URL))
        .bearer_auth(token)
        .json(entry)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Failed to create time entry: {}", res.status()));
    }

    res.json::<TimeEntryResponse>()
        .await
        .map_err(|e| e.to_string())
}

pub async fn upload_screenshot(
    token: &str,
    time_entry_id: &str,
    file_path: &str,
    activity_percent: i64,
) -> Result<(), String> {
    let client = Client::new();

    let file_bytes = tokio::fs::read(file_path)
        .await
        .map_err(|e| e.to_string())?;

    let filename = std::path::Path::new(file_path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let part = reqwest::multipart::Part::bytes(file_bytes)
        .file_name(filename)
        .mime_str("image/png")
        .map_err(|e| e.to_string())?;

    let activity_data = serde_json::json!({
        "activity_percent": activity_percent
    });

    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("isBlurred", "false")
        .text("activityData", activity_data.to_string());

    let res = client
        .post(format!("{}/time-entries/{}/screenshots", BASE_URL, time_entry_id))
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Failed to upload screenshot: {}", res.status()));
    }

    Ok(())
}
