use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Project {
    pub id: i64,
    pub remote_id: String,
    pub name: String,
    pub is_active: i64,
    pub created_at: String,
}

impl Project {
    pub async fn save_many(pool: &SqlitePool, projects: &[Project]) -> Result<(), sqlx::Error> {
        for p in projects {
            sqlx::query(
                r#"
                INSERT INTO projects (remote_id, name, is_active, created_at)
                VALUES (?, ?, ?, ?)
                ON CONFLICT(remote_id) DO UPDATE SET
                    name = excluded.name,
                    is_active = excluded.is_active
                "#,
            )
            .bind(&p.remote_id)
            .bind(&p.name)
            .bind(p.is_active)
            .bind(&p.created_at)
            .execute(pool)
            .await?;
        }
        Ok(())
    }

    pub async fn get_active(pool: &SqlitePool) -> Result<Vec<Project>, sqlx::Error> {
        sqlx::query_as::<_, Project>("SELECT * FROM projects WHERE is_active = 1")
            .fetch_all(pool)
            .await
    }
}
