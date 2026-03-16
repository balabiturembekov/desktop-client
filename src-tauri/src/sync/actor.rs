use reqwest::Client;
use sqlx::SqlitePool;
use std::time::Duration;
use tokio::time::interval;

use crate::api::auth::refresh_token;
use crate::api::sync::{create_time_entry, upload_app_usage, upload_screenshot, TimeEntryRequest};
use crate::app_tracker::models::AppUsagePayload;
use crate::db::models::user::User;

pub async fn sync_actor(pool: SqlitePool) {
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build http client");

    let mut sync_tick = interval(Duration::from_secs(30));
    let mut cleanup_tick = interval(Duration::from_secs(3600));

    loop {
        tokio::select! {
            _ = sync_tick.tick() => {
                sync_pending(&pool, &client).await;
            }
            _ = cleanup_tick.tick() => {
                cleanup_old_data(&pool).await;
            }
        }
    }
}

async fn is_online(client: &Client) -> bool {
    client
        .get("https://api.hubnity.io/api/v1")
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .is_ok()
}

async fn try_refresh(pool: &SqlitePool) -> Option<String> {
    let user = User::get_current(pool).await.ok()??;
    match refresh_token(&user.refresh_token).await {
        Ok(res) => {
            let _ = sqlx::query(
                "UPDATE users SET access_token = ?, refresh_token = ? WHERE remote_id = ?",
            )
            .bind(&res.access_token)
            .bind(&res.refresh_token)
            .bind(&user.remote_id)
            .execute(pool)
            .await;
            log::info!("[sync] token refreshed");
            Some(res.access_token)
        }
        Err(e) => {
            sentry::capture_message(
                &format!("Token refresh failed: {}", e),
                sentry::Level::Error,
            );
            log::error!("[sync] token refresh failed: {}", e);
            None
        }
    }
}

async fn sync_pending(pool: &SqlitePool, client: &Client) {
    if !is_online(client).await {
        return;
    }

    let user = match User::get_current(pool).await {
        Ok(Some(u)) => u,
        _ => return,
    };

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
            sentry::capture_message(
                &format!("Failed to fetch unsynced slots: {}", e),
                sentry::Level::Error,
            );
            return;
        }
    };

    if slots.is_empty() {
        return;
    }

    log::info!("[sync] syncing {} slots", slots.len());
    let mut token = user.access_token.clone();

    for (slot_id, project_id, started_at, ended_at, duration_secs, activity_percent) in slots {
        let entry = TimeEntryRequest {
            project_id,
            started_at,
            ended_at,
            duration_secs,
            activity_percent,
        };

        let remote_id = match create_time_entry(client, &token, &entry).await {
            Ok(res) => res.id,
            Err(e) if e.contains("401") => match try_refresh(pool).await {
                Some(new_token) => {
                    token = new_token;
                    match create_time_entry(client, &token, &entry).await {
                        Ok(res) => res.id,
                        Err(e) => {
                            sentry::capture_message(
                                &format!("Sync retry failed for slot {}: {}", slot_id, e),
                                sentry::Level::Error,
                            );
                            continue;
                        }
                    }
                }
                None => continue,
            },
            Err(e) => {
                // 404 — endpoint ещё не готов, не шлём в Sentry
                if !e.contains("404") {
                    sentry::capture_message(
                        &format!("Sync failed for slot {}: {}", slot_id, e),
                        sentry::Level::Error,
                    );
                }
                log::error!("[sync] failed slot {}: {}", slot_id, e);
                continue;
            }
        };

        let screenshots = sqlx::query_as::<_, (i64, String)>(
            "SELECT id, file_path FROM screenshots WHERE time_slot_id = ? AND synced = 0",
        )
        .bind(slot_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        for (screenshot_id, file_path) in screenshots {
            match upload_screenshot(client, &token, &remote_id, &file_path, activity_percent).await {
                Ok(_) => {
                    log::info!("[sync] screenshot uploaded: {}", file_path);
                    let _ = sqlx::query("UPDATE screenshots SET synced = 1 WHERE id = ?")
                        .bind(screenshot_id)
                        .execute(pool)
                        .await;
                }
                Err(e) => {
                    sentry::capture_message(
                        &format!("Screenshot upload failed {}: {}", file_path, e),
                        sentry::Level::Warning,
                    );
                    log::warn!("[sync] screenshot failed: {}", e);
                }
            }
        }

        // Sync app usage for this slot
        let app_usages = sqlx::query_as::<_, (i64, String, String, Option<String>, i64)>(
            "SELECT id, app_name, window_title, url, duration_secs FROM app_usage WHERE time_slot_id = ? AND synced = 0",
        )
        .bind(slot_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        if !app_usages.is_empty() {
            let payloads: Vec<AppUsagePayload> = app_usages
                .iter()
                .map(|(_, app, title, url, dur)| AppUsagePayload {
                    app_name: app.clone(),
                    window_title: title.clone(),
                    url: url.clone(),
                    duration_secs: *dur,
                })
                .collect();

            match upload_app_usage(client, &token, &remote_id, &payloads).await {
                Ok(_) => {
                    log::info!("[sync] app usage uploaded for slot {}", slot_id);
                    for (usage_id, ..) in &app_usages {
                        let _ = sqlx::query("UPDATE app_usage SET synced = 1 WHERE id = ?")
                            .bind(usage_id)
                            .execute(pool)
                            .await;
                    }
                }
                Err(e) => {
                    if !e.contains("404") {
                        sentry::capture_message(
                            &format!("App usage upload failed for slot {}: {}", slot_id, e),
                            sentry::Level::Warning,
                        );
                    }
                    log::warn!("[sync] app usage upload failed: {}", e);
                }
            }
        }

        let _ = sqlx::query("UPDATE time_slots SET synced = 1 WHERE id = ?")
            .bind(slot_id)
            .execute(pool)
            .await;

        log::info!("[sync] slot {} synced ✓", slot_id);
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

    let mut deleted_files = 0;
    let mut deleted_records = 0;

    for (id, file_path) in old_screenshots {
        match tokio::fs::remove_file(&file_path).await {
            Ok(_) => deleted_files += 1,
            Err(e) => log::warn!("[cleanup] file error {}: {}", file_path, e),
        }
        if sqlx::query("DELETE FROM screenshots WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await
            .is_ok()
        {
            deleted_records += 1;
        }
    }

    if deleted_records > 0 {
        log::info!(
            "[cleanup] {} files, {} records removed",
            deleted_files, deleted_records
        );
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
            log::info!("[cleanup] deleted {} old slots", r.rows_affected());
        }
    }
}
