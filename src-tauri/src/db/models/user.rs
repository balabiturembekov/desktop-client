use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct User {
    pub id: i64,
    pub remote_id: String,
    pub email: String,
    pub name: String,
    pub avatar: Option<String>,
    pub role: String,
    pub access_token: String,
    pub refresh_token: String,
    pub created_at: String,
}

impl User {
    pub async fn save(pool: &SqlitePool, user: &User) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO users (remote_id, email, name, avatar, role, access_token, refresh_token, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(remote_id) DO UPDATE SET
                access_token  = excluded.access_token,
                refresh_token = excluded.refresh_token,
                email         = excluded.email,
                name          = excluded.name,
                avatar        = excluded.avatar
            "#,
        )
        .bind(&user.remote_id)
        .bind(&user.email)
        .bind(&user.name)
        .bind(&user.avatar)
        .bind(&user.role)
        .bind(&user.access_token)
        .bind(&user.refresh_token)
        .bind(&user.created_at)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn get_current(pool: &SqlitePool) -> Result<Option<User>, sqlx::Error> {
        sqlx::query_as::<_, User>("SELECT * FROM users LIMIT 1")
            .fetch_optional(pool)
            .await
    }
}
