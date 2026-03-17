use sqlx::SqlitePool;
use tokio::time::{interval, Duration};

use crate::api::auth::refresh_token;
use crate::api::sync::{sync_time_entries, upload_screenshot, SyncAppUsage, SyncTimeEntry};
use crate::db::models::user::User;

/// Max slots to sync per cycle — prevents loading thousands of rows into memory.
const SYNC_BATCH_LIMIT: i64 = 50;

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

/// Fetches pending slots from DB and builds entries for bulk sync.
/// ORDER BY id ensures stable ordering so response indices match slot_ids.
async fn build_entries(
    pool: &SqlitePool,
) -> Result<(Vec<SyncTimeEntry>, Vec<i64>), sqlx::Error> {
    let slots = sqlx::query_as::<_, (i64, String, String, String, i64, i64)>(
        r#"SELECT id, project_id, started_at, ended_at, duration_secs, activity_percent
           FROM time_slots
           WHERE synced = 0 AND ended_at IS NOT NULL AND duration_secs > 0
           ORDER BY id
           LIMIT ?"#,
    )
    .bind(SYNC_BATCH_LIMIT)
    .fetch_all(pool)
    .await?;

    let mut entries = Vec::with_capacity(slots.len());
    let mut slot_ids = Vec::with_capacity(slots.len());

    for (slot_id, project_id, started_at, ended_at, duration_secs, activity_percent) in &slots {
        let app_usages = sqlx::query_as::<_, (String, String, Option<String>, i64, String)>(
            "SELECT app_name, window_title, url, duration_secs, started_at \
             FROM app_usage WHERE time_slot_id = ? AND synced = 0 AND trim(app_name) != ''",
        )
        .bind(slot_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        entries.push(SyncTimeEntry {
            project_id: project_id.clone(),
            started_at: started_at.clone(),
            ended_at: ended_at.clone(),
            duration_seconds: *duration_secs,
            activity_percent: *activity_percent,
            app_usage: app_usages
                .into_iter()
                .map(|(app_name, window_title, url, duration_seconds, started_at)| {
                    SyncAppUsage {
                        app_name,
                        window_title,
                        url,
                        duration_seconds,
                        started_at,
                    }
                })
                .collect(),
        });
        slot_ids.push(*slot_id);
    }

    Ok((entries, slot_ids))
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

    let mut token = user.access_token.clone();

    // ── Sync pending slots ────────────────────────────────────────────────
    // Only attempt network sync when there are unsynced slots; orphan cleanup
    // below always runs regardless so stale rows are never permanently stuck.
    let (entries, slot_ids) = match build_entries(pool).await {
        Ok(pair) => pair,
        Err(e) => {
            log::error!("[sync] failed to fetch slots: {}", e);
            return;
        }
    };

    if !slot_ids.is_empty() {
        log::info!("[sync] syncing {} slots", slot_ids.len());

        // On 401: refresh token and re-fetch entries.
        // Re-fetching is safe because ORDER BY id + same LIMIT returns the same rows.
        // Critically, we also get a fresh slot_ids so indices stay consistent with
        // the entries we send on retry.
        let (response, final_slot_ids) = match sync_time_entries(&client, &token, entries).await {
            Ok(res) => (res, slot_ids),
            Err(e) if e.contains("401") => {
                log::info!("[sync] 401 — refreshing token");
                match try_refresh(pool).await {
                    Some(new_token) => {
                        token = new_token;
                        let (entries2, slot_ids2) = match build_entries(pool).await {
                            Ok(pair) => pair,
                            Err(e) => {
                                log::error!("[sync] rebuild after 401 failed: {}", e);
                                return;
                            }
                        };
                        match sync_time_entries(&client, &token, entries2).await {
                            Ok(res) => (res, slot_ids2),
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

        // Mark synced slots, store remote_id, upload screenshots.
        // response.entries is ordered the same as our request (backend preserves order).
        for (i, result) in response.entries.iter().enumerate() {
            if !result.synced || i >= final_slot_ids.len() {
                continue;
            }
            let slot_id = final_slot_ids[i];
            let remote_id = &result.id;

            let _ = sqlx::query(
                "UPDATE time_slots SET synced = 1, remote_id = ? WHERE id = ?",
            )
            .bind(remote_id)
            .bind(slot_id)
            .execute(pool)
            .await;

            let _ = sqlx::query("UPDATE app_usage SET synced = 1 WHERE time_slot_id = ?")
                .bind(slot_id)
                .execute(pool)
                .await;

            upload_slot_screenshots(pool, &client, &token, slot_id, remote_id).await;

            log::info!("[sync] slot {} → remote {} ✓", slot_id, remote_id);
        }
    }

    // ── Orphan cleanup (always runs, even when no new slots were synced) ──
    // This catches app_usage/screenshots that were written after their slot
    // was already synced — which previously got stuck forever because this
    // block was only reachable when slot_ids was non-empty.

    // Invalid app_usage: rows with empty app_name are skipped during sync but
    // never marked synced, blocking all future cycles. Mark them done now.
    let _ = sqlx::query(
        "UPDATE app_usage SET synced = 1 WHERE synced = 0 AND trim(app_name) = ''",
    )
    .execute(pool)
    .await;

    // Orphan app_usage: mark synced so they don't block future sync cycles.
    let orphan = sqlx::query(
        "UPDATE app_usage SET synced = 1 \
         WHERE synced = 0 \
         AND time_slot_id IN (SELECT id FROM time_slots WHERE synced = 1)",
    )
    .execute(pool)
    .await;

    match orphan {
        Ok(r) if r.rows_affected() > 0 => {
            log::info!(
                "[sync] marked {} orphan app_usage rows as synced",
                r.rows_affected()
            );
        }
        Err(e) => log::warn!("[sync] orphan app_usage cleanup failed: {}", e),
        _ => {}
    }

    // Orphan screenshots: retry upload for screenshots whose slot is already synced.
    // remote_id is stored in time_slots so we can retry without re-syncing the entry.
    let orphan_screenshots = sqlx::query_as::<_, (i64, String, String, i64)>(
        r#"SELECT s.id, s.file_path, t.remote_id, t.activity_percent
           FROM screenshots s
           JOIN time_slots t ON t.id = s.time_slot_id
           WHERE s.synced = 0 AND t.synced = 1 AND t.remote_id IS NOT NULL"#,
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    if !orphan_screenshots.is_empty() {
        log::info!(
            "[sync] retrying {} orphan screenshots",
            orphan_screenshots.len()
        );
        for (screenshot_id, file_path, remote_id, activity_percent) in orphan_screenshots {
            match upload_screenshot(&client, &token, &remote_id, &file_path, activity_percent)
                .await
            {
                Ok(_) => {
                    log::info!("[sync] orphan screenshot uploaded: {}", file_path);
                    let _ = sqlx::query("UPDATE screenshots SET synced = 1 WHERE id = ?")
                        .bind(screenshot_id)
                        .execute(pool)
                        .await;
                }
                Err(e) => log::warn!("[sync] orphan screenshot failed: {}", e),
            }
        }
    }
}

async fn upload_slot_screenshots(
    pool: &SqlitePool,
    client: &reqwest::Client,
    token: &str,
    slot_id: i64,
    remote_id: &str,
) {
    let screenshots = sqlx::query_as::<_, (i64, String, i64)>(
        r#"SELECT s.id, s.file_path, t.activity_percent
           FROM screenshots s
           JOIN time_slots t ON t.id = s.time_slot_id
           WHERE s.time_slot_id = ? AND s.synced = 0"#,
    )
    .bind(slot_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for (screenshot_id, file_path, activity_percent) in screenshots {
        match upload_screenshot(client, token, remote_id, &file_path, activity_percent).await {
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

    // Delete dangling app_usage rows that reference deleted time_slots.
    // This can happen if cleanup_old_data deleted time_slots rows before the
    // corresponding app_usage rows were cleaned up (or if they were written after
    // the slot was already deleted).
    let dangling = sqlx::query(
        "DELETE FROM app_usage WHERE time_slot_id NOT IN (SELECT id FROM time_slots)",
    )
    .execute(pool)
    .await;

    match dangling {
        Ok(r) if r.rows_affected() > 0 => {
            log::info!("[cleanup] deleted {} dangling app_usage rows", r.rows_affected());
        }
        Err(e) => log::warn!("[cleanup] dangling app_usage cleanup failed: {}", e),
        _ => {}
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
