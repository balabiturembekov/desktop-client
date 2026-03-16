use sqlx::SqlitePool;
use tokio::time::{interval, Duration};

use crate::api::auth::refresh_token;
use crate::api::sync::{create_time_entry, upload_screenshot, TimeEntryRequest};
use crate::db::models::user::User;

pub async fn sync_actor(pool: SqlitePool) {
    let mut sync_tick = interval(Duration::from_secs(30));
    let mut cleanup_tick = interval(Duration::from_secs(3600));

    loop {
        tokio::select! {
            _ = sync_tick.tick() => {
                sync_pending(&pool).await;
            }
            _ = cleanup_tick.tick() => {
                cleanup_old_data(&pool).await;
            }
        }
    }
}

/// Проверяет сеть — простой ping к API
async fn is_online() -> bool {
    reqwest::Client::new()
        .get("https://api.hubnity.io/api/v1")
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .is_ok()
}

/// Обновляет токен и сохраняет в БД
async fn try_refresh(pool: &SqlitePool) -> Option<String> {
    let user = User::get_current(pool).await.ok()??;

    match refresh_token(&user.refresh_token).await {
        Ok(res) => {
            // Сохраняем новые токены
            let _ = sqlx::query(
                "UPDATE users SET access_token = ?, refresh_token = ? WHERE remote_id = ?",
            )
            .bind(&res.access_token)
            .bind(&res.refresh_token)
            .bind(&user.remote_id)
            .execute(pool)
            .await;

            println!("[sync] token refreshed");
            Some(res.access_token)
        }
        Err(e) => {
            eprintln!("[sync] token refresh failed: {}", e);
            None
        }
    }
}

async fn sync_pending(pool: &SqlitePool) {
    // Проверяем сеть
    if !is_online().await {
        println!("[sync] offline — skipping sync");
        return;
    }

    // Получаем токен
    let user = match User::get_current(pool).await {
        Ok(Some(u)) => u,
        _ => return,
    };

    // Получаем несинхронизированные слоты
    let slots = match sqlx::query_as::<_, (i64, String, String, String, i64, i64)>(
        r#"SELECT id, project_id, started_at, ended_at, duration_secs, activity_percent
           FROM time_slots
           WHERE synced = 0 AND ended_at IS NOT NULL"#,
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[sync] failed to fetch slots: {}", e);
            return;
        }
    };

    if slots.is_empty() {
        return;
    }

    println!("[sync] syncing {} slots", slots.len());

    // Используем текущий токен, при 401 — обновляем
    let mut token = user.access_token.clone();

    for (slot_id, project_id, started_at, ended_at, duration_secs, activity_percent) in slots {
        let entry = TimeEntryRequest {
            project_id,
            started_at,
            ended_at,
            duration_secs,
            activity_percent,
        };

        // Пробуем отправить — если 401 обновляем токен и повторяем
        let remote_id = match create_time_entry(&token, &entry).await {
            Ok(res) => res.id,
            Err(e) if e.contains("401") => {
                println!("[sync] 401 — refreshing token");
                match try_refresh(pool).await {
                    Some(new_token) => {
                        token = new_token;
                        match create_time_entry(&token, &entry).await {
                            Ok(res) => res.id,
                            Err(e) => {
                                eprintln!("[sync] retry failed: {}", e);
                                continue;
                            }
                        }
                    }
                    None => {
                        eprintln!("[sync] refresh failed — skipping slot {}", slot_id);
                        continue;
                    }
                }
            }
            Err(e) => {
                eprintln!("[sync] failed slot {}: {}", slot_id, e);
                continue; // offline queue — попробуем в следующий раз
            }
        };

        println!("[sync] time entry created: {}", remote_id);

        // Загружаем скриншоты
        let screenshots = sqlx::query_as::<_, (i64, String)>(
            "SELECT id, file_path FROM screenshots WHERE time_slot_id = ? AND synced = 0",
        )
        .bind(slot_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        for (screenshot_id, file_path) in screenshots {
            match upload_screenshot(&token, &remote_id, &file_path, activity_percent).await {
                Ok(_) => {
                    println!("[sync] screenshot uploaded: {}", file_path);
                    let _ = sqlx::query("UPDATE screenshots SET synced = 1 WHERE id = ?")
                        .bind(screenshot_id)
                        .execute(pool)
                        .await;
                }
                Err(e) => eprintln!("[sync] screenshot failed: {}", e),
            }
        }

        // Помечаем слот синхронизированным
        let _ = sqlx::query("UPDATE time_slots SET synced = 1 WHERE id = ?")
            .bind(slot_id)
            .execute(pool)
            .await;

        println!("[sync] slot {} synced ✓", slot_id);
    }
}

async fn cleanup_old_data(pool: &SqlitePool) {
    let old_screenshots = sqlx::query_as::<_, (i64, String)>(
        r#"SELECT id, file_path FROM screenshots
           WHERE synced = 1
           AND datetime(taken_at) < datetime('now', '-24 hours')"#,
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for (id, file_path) in old_screenshots {
        if let Err(e) = tokio::fs::remove_file(&file_path).await {
            eprintln!("[cleanup] failed to delete file {}: {}", file_path, e);
        }
        let _ = sqlx::query("DELETE FROM screenshots WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await;
    }

    let result = sqlx::query(
        r#"DELETE FROM time_slots
           WHERE synced = 1
           AND datetime(ended_at) < datetime('now', '-24 hours')"#,
    )
    .execute(pool)
    .await;

    if let Ok(r) = result {
        if r.rows_affected() > 0 {
            println!("[cleanup] deleted {} old slots", r.rows_affected());
        }
    }
}
