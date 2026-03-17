use sqlx::SqlitePool;
use tokio::time::{interval, Duration};

use crate::api::auth::refresh_token;
use crate::api::sync::{sync_time_entries, upload_screenshot, SyncAppUsage, SyncTimeEntry};
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

async fn is_online(client: &reqwest::Client) -> bool {
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

async fn sync_pending(pool: &SqlitePool) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    if !is_online(&client).await {
        log::info!("[sync] offline — skipping");
        return;
    }

    let user = match User::get_current(pool).await {
        Ok(Some(u)) => u,
        _ => return,
    };

    // Берём все несинхронизированные завершённые слоты
    let slots = match sqlx::query_as::<_, (i64, String, String, String, i64, i64)>(
        r#"SELECT id, project_id, started_at, ended_at, duration_secs, activity_percent
           FROM time_slots
           WHERE synced = 0 AND ended_at IS NOT NULL AND duration_secs > 0"#,
    )
    .fetch_all(pool)
    .await
    {
        Ok(s) => s,
        Err(e) => {
            log::error!("[sync] failed to fetch slots: {}", e);
            return;
        }
    };

    if slots.is_empty() {
        return;
    }

    log::info!("[sync] syncing {} slots", slots.len());
    let mut token = user.access_token.clone();

    // Собираем все слоты в один bulk запрос
    let mut entries: Vec<SyncTimeEntry> = vec![];
    let mut slot_ids: Vec<i64> = vec![];

    for (slot_id, project_id, started_at, ended_at, duration_secs, activity_percent) in &slots {
        // Загружаем app_usage для слота
        let app_usages = sqlx::query_as::<_, (String, String, Option<String>, i64, String)>(
            "SELECT app_name, window_title, url, duration_secs, started_at FROM app_usage WHERE time_slot_id = ? AND synced = 0",
        )
        .bind(slot_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        let app_usage: Vec<SyncAppUsage> = app_usages
            .into_iter()
            .map(
                |(app_name, window_title, url, duration_seconds, started_at)| SyncAppUsage {
                    app_name,
                    window_title,
                    url,
                    duration_seconds,
                    started_at,
                },
            )
            .collect();

        entries.push(SyncTimeEntry {
            project_id: project_id.clone(),
            started_at: started_at.clone(),
            ended_at: ended_at.clone(),
            duration_seconds: *duration_secs,
            activity_percent: *activity_percent,
            app_usage,
        });
        slot_ids.push(*slot_id);
    }

    // Отправляем bulk запрос
    let response = match sync_time_entries(&client, &token, entries).await {
        Ok(res) => res,
        Err(e) if e.contains("401") => {
            log::info!("[sync] 401 — refreshing token");
            match try_refresh(pool).await {
                Some(new_token) => {
                    token = new_token.clone();
                    // Пересобираем entries после refresh
                    let slots2 = match sqlx::query_as::<_, (i64, String, String, String, i64, i64)>(
                        r#"SELECT id, project_id, started_at, ended_at, duration_secs, activity_percent
                           FROM time_slots
                           WHERE synced = 0 AND ended_at IS NOT NULL AND duration_secs > 0"#,
                    )
                    .fetch_all(pool)
                    .await {
                        Ok(s) => s,
                        Err(_) => return,
                    };

                    let mut entries2: Vec<SyncTimeEntry> = vec![];
                    for (
                        slot_id,
                        project_id,
                        started_at,
                        ended_at,
                        duration_secs,
                        activity_percent,
                    ) in &slots2
                    {
                        let app_usages = sqlx::query_as::<_, (String, String, Option<String>, i64, String)>(
                            "SELECT app_name, window_title, url, duration_secs, started_at FROM app_usage WHERE time_slot_id = ? AND synced = 0",
                        )
                        .bind(slot_id)
                        .fetch_all(pool)
                        .await
                        .unwrap_or_default();

                        entries2.push(SyncTimeEntry {
                            project_id: project_id.clone(),
                            started_at: started_at.clone(),
                            ended_at: ended_at.clone(),
                            duration_seconds: *duration_secs,
                            activity_percent: *activity_percent,
                            app_usage: app_usages
                                .into_iter()
                                .map(
                                    |(
                                        app_name,
                                        window_title,
                                        url,
                                        duration_seconds,
                                        started_at,
                                    )| SyncAppUsage {
                                        app_name,
                                        window_title,
                                        url,
                                        duration_seconds,
                                        started_at,
                                    },
                                )
                                .collect(),
                        });
                    }

                    match sync_time_entries(&client, &new_token, entries2).await {
                        Ok(res) => res,
                        Err(e) => {
                            log::error!("[sync] retry failed: {}", e);
                            return;
                        }
                    }
                }
                None => return,
            }
        }
        Err(e) => {
            if !e.contains("404") {
                sentry::capture_message(&format!("Sync failed: {}", e), sentry::Level::Error);
            }
            log::error!("[sync] failed: {}", e);
            return;
        }
    };

    log::info!(
        "[sync] synced={} failed={}",
        response.synced,
        response.failed
    );

    // Обновляем статус синхронизированных слотов
    for (i, result) in response.entries.iter().enumerate() {
        if result.synced && i < slot_ids.len() {
            let slot_id = slot_ids[i];
            let remote_id = &result.id;

            // Помечаем слот как синхронизированный
            let _ = sqlx::query("UPDATE time_slots SET synced = 1 WHERE id = ?")
                .bind(slot_id)
                .execute(pool)
                .await;

            // Помечаем app_usage как синхронизированный
            let _ = sqlx::query("UPDATE app_usage SET synced = 1 WHERE time_slot_id = ?")
                .bind(slot_id)
                .execute(pool)
                .await;

            // Загружаем скриншоты для этого слота
            let screenshots = sqlx::query_as::<_, (i64, String)>(
                "SELECT id, file_path FROM screenshots WHERE time_slot_id = ? AND synced = 0",
            )
            .bind(slot_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();

            for (screenshot_id, file_path) in screenshots {
                let activity_percent = slots
                    .iter()
                    .find(|(id, ..)| *id == slot_id)
                    .map(|(_, _, _, _, _, ap)| *ap)
                    .unwrap_or(0);

                match upload_screenshot(&client, &token, remote_id, &file_path, activity_percent)
                    .await
                {
                    Ok(_) => {
                        log::info!("[sync] screenshot uploaded: {}", file_path);
                        let _ = sqlx::query("UPDATE screenshots SET synced = 1 WHERE id = ?")
                            .bind(screenshot_id)
                            .execute(pool)
                            .await;
                    }
                    Err(e) => {
                        sentry::capture_message(
                            &format!("Screenshot upload failed: {}", e),
                            sentry::Level::Warning,
                        );
                        log::warn!("[sync] screenshot failed: {}", e);
                    }
                }
            }

            log::info!("[sync] slot {} → remote {} ✓", slot_id, remote_id);
        }
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
            deleted_files,
            deleted_records
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
