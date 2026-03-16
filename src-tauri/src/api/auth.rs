use crate::api::models::auth::{
    LoginRequest, LoginResponse, RefreshRequest, RefreshResponse, RemoteProject,
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
