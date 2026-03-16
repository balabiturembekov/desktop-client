use serde::Serialize;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct AppKey {
    pub app_name: String,
    pub window_title: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppUsagePayload {
    pub app_name: String,
    pub window_title: String,
    pub url: Option<String>,
    pub duration_secs: i64,
}
