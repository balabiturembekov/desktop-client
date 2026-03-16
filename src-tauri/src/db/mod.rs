use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::fs;
use tauri::AppHandle;
use tauri::Manager;

pub mod models;

pub async fn init_db(app: &AppHandle) -> Result<SqlitePool, sqlx::Error> {
    let app_dir = app
        .path()
        .app_data_dir()
        .expect("failed to get app data dir");

    fs::create_dir_all(&app_dir).expect("failed to create app data dir");

    let db_path = app_dir.join("hubnity.db");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.to_string_lossy());

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await?;

    sqlx::migrate!("./src/db/migrations").run(&pool).await?;

    Ok(pool)
}
