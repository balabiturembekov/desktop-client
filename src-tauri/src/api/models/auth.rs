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
