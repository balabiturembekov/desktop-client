use chrono::Utc;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::time::{interval, Duration, MissedTickBehavior};

use crate::api::auth::refresh_token;
use crate::api::sync::{sync_time_entries, upload_screenshot, SyncAppUsage, SyncTimeEntry};
use crate::db::models::user::User;

/// Max slots to sync per cycle — prevents loading thousands of rows into memory.
const SYNC_BATCH_LIMIT: i64 = 50;

pub async fn sync_actor(pool: SqlitePool, app: AppHandle) {
    // Build the HTTP client once so all sync cycles share the same connection
    // pool and DNS cache — no new TLS handshake every 30 seconds (BUG-A02).
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client");

    // Emit initial connectivity state immediately so the UI doesn't show a
    // false "online" indicator for the first 30 s (M-01 from audit #3).
    let initial_online = is_online(&client).await;
    let _ = app.emit("connectivity-changed", initial_online);

    // Run sync and cleanup as independent loops so a slow sync cycle never
    // delays the hourly cleanup (and vice-versa).
    let pool_sync = pool.clone();
    let client_sync = client.clone(); // cheap clone — Client is Arc-backed internally
    let app_sync = app.clone();
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(30));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        // Initialise from the startup ping so the first sync tick doesn't
        // immediately re-emit connectivity-changed with the same value.
        let mut was_online = initial_online;
        loop {
            tick.tick().await;
            sync_pending(&pool_sync, &client_sync, &app_sync, &mut was_online).await;
        }
    });

    let mut cleanup_tick = interval(Duration::from_secs(3600));
    cleanup_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        cleanup_tick.tick().await;
        cleanup_old_data(&pool).await;
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
            if let Err(e) = sqlx::query(
                "UPDATE users SET access_token = ?, refresh_token = ? WHERE remote_id = ?",
            )
            .bind(&res.access_token)
            .bind(&res.refresh_token)
            .bind(&user.remote_id)
            .execute(pool)
            .await
            {
                log::error!("[sync] failed to persist refreshed token: {}", e);
                return None;
            }
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
async fn build_entries(pool: &SqlitePool) -> Result<(Vec<SyncTimeEntry>, Vec<i64>), sqlx::Error> {
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
        let app_usages = match sqlx::query_as::<_, (String, String, Option<String>, i64, String)>(
            "SELECT app_name, window_title, url, duration_secs, started_at \
             FROM app_usage WHERE time_slot_id = ? AND synced = 0 AND trim(app_name) != ''",
        )
        .bind(slot_id)
        .fetch_all(pool)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                log::error!(
                    "[sync] failed to fetch app_usage for slot {}: {} — skipping slot",
                    slot_id,
                    e
                );
                continue;
            }
        };

        entries.push(SyncTimeEntry {
            project_id: project_id.clone(),
            started_at: started_at.clone(),
            ended_at: ended_at.clone(),
            duration_seconds: *duration_secs,
            activity_percent: *activity_percent,
            app_usage: app_usages
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
                .collect(),
        });
        slot_ids.push(*slot_id);
    }

    Ok((entries, slot_ids))
}

async fn sync_pending(
    pool: &SqlitePool,
    client: &reqwest::Client,
    app: &AppHandle,
    was_online: &mut bool,
) {
    // Check for a logged-in user first — if nobody is signed in there is
    // nothing to sync and we avoid an unauthenticated ping to the API server
    // on every 30-second cycle (BUG-A06 / privacy).
    let user = match User::get_current(pool).await {
        Ok(Some(u)) => u,
        _ => return,
    };

    // Track connectivity changes and emit an event when the state flips.
    // This lets the frontend show an offline indicator without polling.
    let online = is_online(client).await;
    if online != *was_online {
        let _ = app.emit("connectivity-changed", online);
        *was_online = online;
    }
    if !online {
        log::info!("[sync] offline — skipping");
        return;
    }

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
        let (response, final_slot_ids) = match sync_time_entries(client, &token, entries).await {
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
                        match sync_time_entries(client, &token, entries2).await {
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

            let _ = sqlx::query("UPDATE time_slots SET synced = 1, remote_id = ? WHERE id = ?")
                .bind(remote_id)
                .bind(slot_id)
                .execute(pool)
                .await;

            let _ = sqlx::query("UPDATE app_usage SET synced = 1 WHERE time_slot_id = ?")
                .bind(slot_id)
                .execute(pool)
                .await;

            upload_slot_screenshots(pool, client, &token, slot_id, remote_id).await;

            log::info!("[sync] slot {} → remote {} ✓", slot_id, remote_id);
        }
    }

    // ── Orphan cleanup (always runs, even when no new slots were synced) ──
    // This catches app_usage/screenshots that were written after their slot
    // was already synced — which previously got stuck forever because this
    // block was only reachable when slot_ids was non-empty.

    // Invalid app_usage: rows with empty app_name are skipped during sync but
    // never marked synced, blocking all future cycles. Mark them done now.
    let _ = sqlx::query("UPDATE app_usage SET synced = 1 WHERE synced = 0 AND trim(app_name) = ''")
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
            match upload_screenshot(client, &token, &remote_id, &file_path, activity_percent).await
            {
                Ok(_) => {
                    log::info!("[sync] orphan screenshot uploaded: {}", file_path);
                    let _ = sqlx::query("UPDATE screenshots SET synced = 1 WHERE id = ?")
                        .bind(screenshot_id)
                        .execute(pool)
                        .await;
                    match tokio::fs::remove_file(&file_path).await {
                        Ok(_) => log::info!("[sync] orphan screenshot file deleted: {}", file_path),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => log::warn!("[sync] failed to remove orphan screenshot file {}: {}", file_path, e),
                    }
                }
                Err(e) => log::warn!("[sync] orphan screenshot failed: {}", e),
            }
        }
    }

    // Notify the frontend that a full sync cycle completed so it can
    // refresh "Last updated at" in the footer.
    let _ = app.emit("sync-completed", Utc::now().to_rfc3339());
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
                // Delete the local file immediately — no reason to keep it on
                // disk once the server has it. Cleanup happens here, not in the
                // hourly cleanup cycle, so disk space is freed right away.
                match tokio::fs::remove_file(&file_path).await {
                    Ok(_) => log::info!("[sync] screenshot file deleted after upload: {}", file_path),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => log::warn!("[sync] failed to remove screenshot file {}: {}", file_path, e),
                }
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
    // Screenshot files are deleted immediately after a successful upload, so
    // there is no filesystem work to do here — only DB record cleanup.

    // Remove time_slots that were synced more than 24 hours ago.
    // Rows are kept for 24 h so that today_secs and recent history remain
    // queryable by the frontend even after the data is on the server.
    let slots_result = sqlx::query(
        r#"DELETE FROM time_slots
           WHERE synced = 1
           AND datetime(ended_at) < datetime('now', '-24 hours')"#,
    )
    .execute(pool)
    .await;

    match slots_result {
        Ok(r) if r.rows_affected() > 0 => {
            log::info!("[cleanup] deleted {} old time_slots", r.rows_affected());
        }
        Err(e) => log::warn!("[cleanup] failed to delete old time_slots: {}", e),
        _ => {}
    }

    // Remove app_usage rows whose parent time_slot was just deleted (or was
    // already missing from a previous cycle).
    let dangling_usage = sqlx::query(
        "DELETE FROM app_usage WHERE time_slot_id NOT IN (SELECT id FROM time_slots)",
    )
    .execute(pool)
    .await;

    match dangling_usage {
        Ok(r) if r.rows_affected() > 0 => {
            log::info!(
                "[cleanup] deleted {} orphaned app_usage rows",
                r.rows_affected()
            );
        }
        Err(e) => log::warn!("[cleanup] orphaned app_usage cleanup failed: {}", e),
        _ => {}
    }

    // Remove screenshot records whose parent time_slot no longer exists.
    // Files are already gone (deleted after upload); this clears any lingering
    // DB rows so the orphan-retry query never re-attempts a missing file.
    let orphan_ss = sqlx::query(
        "DELETE FROM screenshots WHERE time_slot_id NOT IN (SELECT id FROM time_slots)",
    )
    .execute(pool)
    .await;

    match orphan_ss {
        Ok(r) if r.rows_affected() > 0 => {
            log::info!(
                "[cleanup] deleted {} orphaned screenshot records",
                r.rows_affected()
            );
        }
        Err(e) => log::warn!("[cleanup] orphaned screenshot records cleanup failed: {}", e),
        _ => {}
    }
}
