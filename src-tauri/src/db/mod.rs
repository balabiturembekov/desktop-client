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

    // WAL mode: concurrent readers don't block the writer (and vice-versa),
    // which is critical because 5 actors write to the DB simultaneously.
    sqlx::query("PRAGMA journal_mode = WAL;")
        .execute(&pool)
        .await?;
    // NORMAL durability is safe with WAL and avoids an fsync on every commit.
    sqlx::query("PRAGMA synchronous = NORMAL;")
        .execute(&pool)
        .await?;
    // Enforce FK constraints declared in the schema (OFF by default in SQLite).
    sqlx::query("PRAGMA foreign_keys = ON;")
        .execute(&pool)
        .await?;

    Ok(pool)
}
