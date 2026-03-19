use crate::api::models::auth::{
    LoginRequest, LoginResponse, MemberSettingsEntry, RefreshRequest, RefreshResponse,
    RemoteOrganization, RemoteProject, RemoteTrackingSettings, TrackingSettings,
};
use reqwest::Client;
use std::time::Duration;

const BASE_URL: &str = "https://api.hubnity.io/api/v1";

fn build_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_default()
}

pub async fn login(email: &str, password: &str) -> Result<LoginResponse, String> {
    let client = build_client();
    let res = client
        .post(format!("{}/auth/login", BASE_URL))
        .json(&LoginRequest {
            email: email.to_string(),
            password: password.to_string(),
        })
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Login failed: {}", res.status()));
    }

    res.json::<LoginResponse>().await.map_err(|e| e.to_string())
}

pub async fn refresh_token(refresh_token: &str) -> Result<RefreshResponse, String> {
    let client = build_client();
    let res = client
        .post(format!("{}/auth/refresh", BASE_URL))
        .json(&RefreshRequest {
            refresh_token: refresh_token.to_string(),
        })
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Refresh failed: {}", res.status()));
    }

    res.json::<RefreshResponse>()
        .await
        .map_err(|e| e.to_string())
}

/// Fetches the list of organizations the current user belongs to.
/// `org_id` is taken from `[0].id` — users typically belong to exactly one org.
pub async fn fetch_organizations(token: &str) -> Result<Vec<RemoteOrganization>, String> {
    let client = build_client();
    let res = client
        .get(format!("{}/organizations", BASE_URL))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Failed to fetch organizations: {}", res.status()));
    }

    res.json::<Vec<RemoteOrganization>>()
        .await
        .map_err(|e| e.to_string())
}

pub async fn fetch_projects(token: &str) -> Result<Vec<RemoteProject>, String> {
    let client = build_client();
    let res = client
        .get(format!("{}/projects", BASE_URL))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!("Failed to fetch projects: {}", res.status()));
    }

    res.json::<Vec<RemoteProject>>()
        .await
        .map_err(|e| e.to_string())
}

/// Fetches organization-level tracking settings (the org-wide defaults).
pub async fn fetch_org_tracking_settings(
    client: &Client,
    token: &str,
    org_id: &str,
) -> Result<TrackingSettings, String> {
    let res = client
        .get(format!(
            "{}/organizations/{}/tracking-settings",
            BASE_URL, org_id
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!(
            "Failed to fetch org tracking settings: {}",
            res.status()
        ));
    }

    let remote: RemoteTrackingSettings = res.json().await.map_err(|e| e.to_string())?;
    Ok(TrackingSettings::from(remote))
}

/// Fetches per-member tracking settings for all members of the org.
/// Returns `Some(settings)` for the current user's effective (merged) settings,
/// or `None` if the user is not found in the list.
pub async fn fetch_member_tracking_settings(
    client: &Client,
    token: &str,
    org_id: &str,
    user_id: &str,
) -> Result<Option<TrackingSettings>, String> {
    let res = client
        .get(format!(
            "{}/organizations/{}/members/tracking-settings",
            BASE_URL, org_id
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !res.status().is_success() {
        return Err(format!(
            "Failed to fetch member tracking settings: {}",
            res.status()
        ));
    }

    let entries: Vec<MemberSettingsEntry> = res.json().await.map_err(|e| e.to_string())?;
    let found = entries.into_iter().find(|e| e.user_id == user_id);
    Ok(found.map(|e| TrackingSettings::from(e.effective)))
}

/// Returns the effective tracking settings for a user:
/// 1. Tries the per-member endpoint (most specific — org defaults + member overrides).
/// 2. Falls back to org-level defaults.
/// 3. Falls back to hardcoded defaults if both API calls fail.
pub async fn get_effective_settings(
    token: &str,
    org_id: &str,
    user_id: &str,
) -> TrackingSettings {
    let client = build_client();

    // Try member settings first (org defaults merged with per-member overrides).
    match fetch_member_tracking_settings(&client, token, org_id, user_id).await {
        Ok(Some(s)) => {
            log::info!("[settings] loaded member-specific effective settings");
            return s;
        }
        Ok(None) => log::info!(
            "[settings] user not found in member settings list — falling back to org defaults"
        ),
        Err(e) => log::warn!(
            "[settings] member settings fetch failed: {} — falling back to org defaults",
            e
        ),
    }

    // Fall back to org-level defaults.
    match fetch_org_tracking_settings(&client, token, org_id).await {
        Ok(s) => {
            log::info!("[settings] loaded org-level tracking settings");
            s
        }
        Err(e) => {
            log::warn!(
                "[settings] org settings fetch failed: {} — using hardcoded defaults",
                e
            );
            TrackingSettings::default()
        }
    }
}
