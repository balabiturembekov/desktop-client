use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginResponse {
    pub user: RemoteUser,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoteUser {
    pub id: String,
    pub name: String,
    pub email: String,
    pub avatar: Option<String>,
    pub role: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

// ── Organizations ──────────────────────────────────────────────────────────────

/// Response item from `GET /api/v1/organizations`.
#[derive(Debug, Deserialize)]
pub struct RemoteOrganization {
    pub id: String,
    #[allow(dead_code)]
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct RefreshRequest {
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshResponse {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoteProject {
    pub id: String,
    pub name: String,
}

// ── Tracking Settings ──────────────────────────────────────────────────────────

/// Resolved tracking settings used by the local actors.
/// Stored in `Arc<Mutex<TrackingSettings>>` and refreshed on login and every 30 minutes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingSettings {
    pub screenshot_interval_minutes: u64,
    pub idle_timeout_seconds: u64,
    pub app_tracking_enabled: bool,
    pub screenshots_enabled: bool,
    pub idle_detection_enabled: bool,
}

impl Default for TrackingSettings {
    fn default() -> Self {
        Self {
            screenshot_interval_minutes: 3,
            idle_timeout_seconds: 300,
            app_tracking_enabled: true,
            screenshots_enabled: true,
            idle_detection_enabled: true,
        }
    }
}

// ── API deserialization helpers ────────────────────────────────────────────────

fn default_screenshot_interval() -> u64 {
    3
}
fn default_idle_timeout() -> u64 {
    300
}
fn default_true() -> bool {
    true
}

/// Raw response shape from the server (camelCase JSON → snake_case Rust).
/// Converted to `TrackingSettings` via `From<RemoteTrackingSettings>`.
#[derive(Debug, Deserialize)]
pub struct RemoteTrackingSettings {
    #[serde(rename = "screenshotIntervalMinutes", default = "default_screenshot_interval")]
    pub screenshot_interval_minutes: u64,
    #[serde(rename = "idleTimeoutSeconds", default = "default_idle_timeout")]
    pub idle_timeout_seconds: u64,
    #[serde(rename = "appTrackingEnabled", default = "default_true")]
    pub app_tracking_enabled: bool,
    #[serde(rename = "screenshotsEnabled", default = "default_true")]
    pub screenshots_enabled: bool,
    #[serde(rename = "idleDetectionEnabled", default = "default_true")]
    pub idle_detection_enabled: bool,
}

impl From<RemoteTrackingSettings> for TrackingSettings {
    fn from(r: RemoteTrackingSettings) -> Self {
        Self {
            // Clamp to safe minimums to prevent runaway behaviour.
            screenshot_interval_minutes: r.screenshot_interval_minutes.max(1),
            idle_timeout_seconds: r.idle_timeout_seconds.max(60),
            app_tracking_enabled: r.app_tracking_enabled,
            screenshots_enabled: r.screenshots_enabled,
            idle_detection_enabled: r.idle_detection_enabled,
        }
    }
}

/// One entry in the `GET /organizations/{id}/members/tracking-settings` response array.
#[derive(Debug, Deserialize)]
pub struct MemberSettingsEntry {
    #[serde(rename = "userId")]
    pub user_id: String,
    pub effective: RemoteTrackingSettings,
}
