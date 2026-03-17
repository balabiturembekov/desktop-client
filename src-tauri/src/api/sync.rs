use reqwest::Client;
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://api.hubnity.io/api/v1";

#[derive(Debug, Serialize)]
pub struct SyncAppUsage {
    #[serde(rename = "appName")]
    pub app_name: String,
    #[serde(rename = "windowTitle")]
    pub window_title: String,
    #[serde(rename = "url")]
    pub url: Option<String>,
    #[serde(rename = "durationSeconds")]
    pub duration_seconds: i64,
    #[serde(rename = "startedAt")]
    pub started_at: String,
}

#[derive(Debug, Serialize)]
pub struct SyncTimeEntry {
    #[serde(rename = "projectId")]
    pub project_id: String,
    #[serde(rename = "startedAt")]
    pub started_at: String,
    #[serde(rename = "endedAt")]
    pub ended_at: String,
    #[serde(rename = "durationSeconds")]
    pub duration_seconds: i64,
    #[serde(rename = "activityPercent")]
    pub activity_percent: i64,
    #[serde(rename = "appUsage")]
    pub app_usage: Vec<SyncAppUsage>,
}

#[derive(Debug, Serialize)]
pub struct SyncTimeEntriesRequest {
    pub entries: Vec<SyncTimeEntry>,
}

#[derive(Debug, Deserialize)]
pub struct SyncEntryResult {
    pub id: String,
    pub synced: bool,
}

#[derive(Debug, Deserialize)]
pub struct SyncTimeEntriesResponse {
    pub synced: i64,
    pub failed: i64,
    pub entries: Vec<SyncEntryResult>,
}

pub async fn sync_time_entries(
    client: &Client,
    token: &str,
    entries: Vec<SyncTimeEntry>,
) -> Result<SyncTimeEntriesResponse, String> {
    let body = SyncTimeEntriesRequest { entries };

    let res = client
        .post(format!("{}/time-entries/sync", BASE_URL))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Sync failed: {}", res.status()));
    }

    res.json::<SyncTimeEntriesResponse>()
        .await
        .map_err(|e| e.to_string())
}

pub async fn upload_screenshot(
    client: &Client,
    token: &str,
    time_entry_id: &str,
    file_path: &str,
    activity_percent: i64,
) -> Result<(), String> {
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
        .post(format!(
            "{}/time-entries/{}/screenshots",
            BASE_URL, time_entry_id
        ))
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
