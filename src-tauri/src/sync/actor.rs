use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tokio::time::{interval, Duration, MissedTickBehavior};

use crate::api::auth::refresh_token;
use crate::api::sync::{
    sync_time_entries, upload_screenshot, SyncAppUsage, SyncEntryResult, SyncTimeEntry,
};
use crate::db::models::user::User;

/// Max slots to sync per cycle — prevents loading thousands of rows into memory.
const SYNC_BATCH_LIMIT: i64 = 50;
/// Max app_usage rows sent per slot — prevents oversized request payloads.
const MAX_APP_USAGE_PER_SLOT: usize = 100;
/// After this many failed attempts a slot/screenshot is permanently skipped.
const MAX_SYNC_ATTEMPTS: i64 = 5;

pub async fn sync_actor(pool: SqlitePool, app: AppHandle) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client");

    // Emit initial connectivity state so the UI doesn't show a false "online"
    // indicator for the first 30 s (M-01 from audit #3).
    let initial_online = is_online(&client).await;
    let _ = app.emit("connectivity-changed", initial_online);

    // Shared mutex prevents concurrent token refreshes: if two sync cycles
    // overlap and both receive 401, only one calls the refresh endpoint — the
    // second waits for the first result instead of double-rotating the token.
    let refresh_lock: Arc<Mutex<()>> = Arc::new(Mutex::new(()));

    // Sync loop with exponential back-off on errors.
    // Runs sync immediately on startup, then sleeps 30 s (or back-off) before
    // each subsequent cycle — same first-run behaviour as interval-based tick.
    let pool_sync = pool.clone();
    let client_sync = client.clone();
    let app_sync = app.clone();
    let refresh_lock_sync = refresh_lock.clone();
    tokio::spawn(async move {
        let mut was_online = initial_online;
        let mut consecutive_failures: u32 = 0;
        loop {
            let had_error = sync_pending(
                &pool_sync,
                &client_sync,
                &app_sync,
                &mut was_online,
                &refresh_lock_sync,
            )
            .await;

            let delay_secs = if had_error {
                consecutive_failures += 1;
                // 30 * 2^n capped at 300 s (5 min).
                let d = (30u64 * 2u64.pow(consecutive_failures.min(9))).min(300);
                log::info!(
                    "[sync] back-off {}s (consecutive_failures={})",
                    d,
                    consecutive_failures
                );
                d
            } else {
                consecutive_failures = 0;
                30
            };
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;
        }
    });

    // Cleanup loop runs independently every hour so a slow sync cycle never
    // delays cleanup (and vice-versa).
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

/// Fetches pending slots from DB.
///
/// Returns `(entries, slot_meta)` where `slot_meta[i] = (slot_id, started_at, has_usage)`.
/// `started_at` is used for result matching instead of positional indexing.
/// `has_usage` is `true` when the slot had at least one app_usage row, letting
/// the caller skip the `UPDATE app_usage SET synced=1` no-op for empty slots.
///
/// Filters:
/// - `synced = 0 AND ended_at IS NOT NULL AND duration_secs > 0` — ready to sync
/// - `sync_attempts < MAX_SYNC_ATTEMPTS` — skip permanently-failed slots
///
/// `app_usage` is capped at `MAX_APP_USAGE_PER_SLOT` rows to avoid oversized payloads.
async fn build_entries(
    pool: &SqlitePool,
) -> Result<(Vec<SyncTimeEntry>, Vec<(i64, String, bool)>), sqlx::Error> {
    let slots = sqlx::query_as::<_, (i64, String, String, String, i64, i64)>(
        r#"SELECT id, project_id, started_at, ended_at, duration_secs, activity_percent
           FROM time_slots
           WHERE synced = 0
             AND ended_at IS NOT NULL
             AND duration_secs > 0
             AND sync_attempts < ?
           ORDER BY id
           LIMIT ?"#,
    )
    .bind(MAX_SYNC_ATTEMPTS)
    .bind(SYNC_BATCH_LIMIT)
    .fetch_all(pool)
    .await?;

    let mut entries = Vec::with_capacity(slots.len());
    let mut slot_meta: Vec<(i64, String, bool)> = Vec::with_capacity(slots.len());

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

        let has_usage = !app_usages.is_empty();
        entries.push(SyncTimeEntry {
            project_id: project_id.clone(),
            started_at: started_at.clone(),
            ended_at: ended_at.clone(),
            duration_seconds: *duration_secs,
            activity_percent: *activity_percent,
            app_usage: app_usages
                .into_iter()
                .take(MAX_APP_USAGE_PER_SLOT)
                .map(
                    |(app_name, window_title, url, duration_seconds, au_started_at)| {
                        SyncAppUsage {
                            app_name,
                            window_title,
                            url,
                            duration_seconds,
                            started_at: au_started_at,
                        }
                    },
                )
                .collect(),
        });
        // has_usage is stored so the caller can skip the app_usage UPDATE
        // for slots that had no rows — avoids a guaranteed no-op round-trip.
        slot_meta.push((*slot_id, started_at.clone(), has_usage));
    }

    Ok((entries, slot_meta))
}

/// Returns `true` when a retryable network/server error occurred (triggers
/// exponential back-off in the caller).
/// Returns `false` for expected non-error states: offline, nothing to sync,
/// or a successful cycle.
async fn sync_pending(
    pool: &SqlitePool,
    client: &reqwest::Client,
    app: &AppHandle,
    was_online: &mut bool,
    refresh_lock: &Arc<Mutex<()>>,
) -> bool {
    let user = match User::get_current(pool).await {
        Ok(Some(u)) => u,
        _ => return false,
    };

    let online = is_online(client).await;
    if online != *was_online {
        let _ = app.emit("connectivity-changed", online);
        *was_online = online;
    }
    if !online {
        log::info!("[sync] offline — skipping");
        return false;
    }

    let mut token = user.access_token.clone();

    // ── Sync pending slots ────────────────────────────────────────────────────
    let (entries, slot_meta) = match build_entries(pool).await {
        Ok(pair) => pair,
        Err(e) => {
            log::error!("[sync] failed to fetch slots: {}", e);
            return true;
        }
    };

    if !slot_meta.is_empty() {
        log::info!("[sync] syncing {} slots", slot_meta.len());

        let (response, final_meta) = match sync_time_entries(client, &token, entries).await {
            Ok(res) => (res, slot_meta),
            Err(e) if e.contains("401") => {
                log::info!("[sync] 401 — refreshing token");
                // Hold the lock only for the refresh call so a second concurrent
                // cycle waits here instead of rotating the token a second time
                // with an already-invalid refresh_token.
                let new_token = {
                    let _guard = refresh_lock.lock().await;
                    try_refresh(pool).await
                };
                match new_token {
                    Some(t) => {
                        token = t;
                        // Re-fetch entries after token rotation (ORDER BY id +
                        // same LIMIT returns the same rows; slot_meta is rebuilt
                        // so indices stay consistent).
                        let (entries2, meta2) = match build_entries(pool).await {
                            Ok(pair) => pair,
                            Err(e) => {
                                log::error!("[sync] rebuild after 401 failed: {}", e);
                                return true;
                            }
                        };
                        match sync_time_entries(client, &token, entries2).await {
                            Ok(res) => (res, meta2),
                            Err(e) => {
                                log::error!("[sync] retry failed: {}", e);
                                return true;
                            }
                        }
                    }
                    None => return true,
                }
            }
            Err(e) => {
                if !e.contains("404") {
                    sentry::capture_message(&format!("Sync failed: {}", e), sentry::Level::Error);
                }
                log::error!("[sync] failed: {}", e);
                return true;
            }
        };

        log::info!(
            "[sync] synced={} failed={}",
            response.synced,
            response.failed
        );

        // ── Result matching ───────────────────────────────────────────────────
        // Prefer matching by `started_at` to avoid writing the wrong remote_id
        // if the server ever reorders response entries.
        // Fall back to index-based matching when the server returns an older
        // response format that doesn't include `startedAt` (detected by all
        // results having an empty started_at field).
        let all_have_started_at = response.entries.iter().all(|r| !r.started_at.is_empty());

        if !all_have_started_at && !response.entries.is_empty() {
            log::warn!("[sync] server did not return startedAt — falling back to index-based matching");
        }

        let result_map: Option<HashMap<&str, &SyncEntryResult>> = if all_have_started_at {
            Some(
                response
                    .entries
                    .iter()
                    .map(|r| (r.started_at.as_str(), r))
                    .collect(),
            )
        } else {
            None
        };

        for (i, (slot_id, started_at, has_usage)) in final_meta.iter().enumerate() {
            let result: &SyncEntryResult = if let Some(ref map) = result_map {
                // Safe path: match by started_at
                match map.get(started_at.as_str()) {
                    Some(r) => r,
                    None => {
                        log::warn!(
                            "[sync] no result for slot {} (started_at={}), incrementing attempts",
                            slot_id,
                            started_at
                        );
                        let _ = sqlx::query(
                            "UPDATE time_slots SET sync_attempts = sync_attempts + 1 WHERE id = ?",
                        )
                        .bind(slot_id)
                        .execute(pool)
                        .await;
                        continue;
                    }
                }
            } else {
                // Legacy fallback: positional index
                if i >= response.entries.len() {
                    continue;
                }
                &response.entries[i]
            };

            if !result.synced {
                log::warn!("[sync] server rejected slot {} — incrementing attempts", slot_id);
                let _ = sqlx::query(
                    "UPDATE time_slots SET sync_attempts = sync_attempts + 1 WHERE id = ?",
                )
                .bind(slot_id)
                .execute(pool)
                .await;
                continue;
            }

            let _ =
                sqlx::query("UPDATE time_slots SET synced = 1, remote_id = ? WHERE id = ?")
                    .bind(&result.id)
                    .bind(slot_id)
                    .execute(pool)
                    .await;

            // Skip the UPDATE when the slot had no app_usage rows — avoids a
            // guaranteed no-op round-trip to the DB.
            if *has_usage {
                let _ = sqlx::query("UPDATE app_usage SET synced = 1 WHERE time_slot_id = ?")
                    .bind(slot_id)
                    .execute(pool)
                    .await;
            }

            upload_slot_screenshots(pool, client, &token, *slot_id, &result.id).await;

            log::info!("[sync] slot {} → remote {} ✓", slot_id, result.id);
        }
    }

    // ── Orphan cleanup (always runs, even when no new slots were synced) ──────

    // Invalid app_usage with empty app_name: skipped during sync but never
    // marked synced — mark them done so they stop blocking sync cycles.
    let _ =
        sqlx::query("UPDATE app_usage SET synced = 1 WHERE synced = 0 AND trim(app_name) = ''")
            .execute(pool)
            .await;

    // app_usage rows whose parent slot is already synced: mark done.
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

    // Retry orphan screenshots whose parent slot is already synced but the
    // screenshot upload was deferred or failed. Excludes rows that have already
    // exceeded MAX_SYNC_ATTEMPTS.
    let orphan_screenshots = sqlx::query_as::<_, (i64, String, String, i64)>(
        r#"SELECT s.id, s.file_path, t.remote_id, t.activity_percent
           FROM screenshots s
           JOIN time_slots t ON t.id = s.time_slot_id
           WHERE s.synced = 0
             AND s.sync_attempts < ?
             AND t.synced = 1
             AND t.remote_id IS NOT NULL"#,
    )
    .bind(MAX_SYNC_ATTEMPTS)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    if !orphan_screenshots.is_empty() {
        log::info!(
            "[sync] retrying {} orphan screenshots",
            orphan_screenshots.len()
        );
        for (screenshot_id, file_path, remote_id, activity_percent) in orphan_screenshots {
            handle_screenshot_upload(
                pool,
                client,
                &token,
                screenshot_id,
                &file_path,
                &remote_id,
                activity_percent,
            )
            .await;
        }
    }

    let _ = app.emit("sync-completed", Utc::now().to_rfc3339());
    false
}

/// Returns `true` if the error string indicates the local file is missing.
fn is_file_not_found(e: &str) -> bool {
    e.contains("No such file") || e.contains("os error 2") || e.contains("NotFound")
}

/// Uploads a single screenshot, handling all error cases:
/// - File missing: marks `synced = 1` immediately — nothing to upload.
/// - Other errors: increments `sync_attempts`; permanently abandons
///   (marks `synced = 1`) after `MAX_SYNC_ATTEMPTS` failures.
/// - Success: marks `synced = 1` and deletes the local file.
async fn handle_screenshot_upload(
    pool: &SqlitePool,
    client: &reqwest::Client,
    token: &str,
    screenshot_id: i64,
    file_path: &str,
    remote_id: &str,
    activity_percent: i64,
) {
    match upload_screenshot(client, token, remote_id, file_path, activity_percent).await {
        Ok(_) => {
            log::info!("[sync] screenshot uploaded: {}", file_path);
            let _ = sqlx::query("UPDATE screenshots SET synced = 1 WHERE id = ?")
                .bind(screenshot_id)
                .execute(pool)
                .await;
            match tokio::fs::remove_file(file_path).await {
                Ok(_) => log::info!("[sync] screenshot file deleted: {}", file_path),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => log::warn!(
                    "[sync] failed to remove screenshot file {}: {}",
                    file_path,
                    e
                ),
            }
        }
        Err(ref e) if is_file_not_found(e) => {
            // File was deleted externally — mark done so we stop retrying.
            log::warn!(
                "[sync] screenshot file missing, marking done: {}",
                file_path
            );
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

            // Increment attempts and permanently abandon if limit reached.
            let _ = sqlx::query(
                "UPDATE screenshots SET sync_attempts = sync_attempts + 1 WHERE id = ?",
            )
            .bind(screenshot_id)
            .execute(pool)
            .await;

            let attempts = sqlx::query_as::<_, (i64,)>(
                "SELECT sync_attempts FROM screenshots WHERE id = ?",
            )
            .bind(screenshot_id)
            .fetch_one(pool)
            .await
            .map(|(a,)| a)
            .unwrap_or(0);

            if attempts >= MAX_SYNC_ATTEMPTS {
                log::warn!(
                    "[sync] screenshot permanently abandoned after {} attempts: {}",
                    attempts,
                    file_path
                );
                let _ = sqlx::query("UPDATE screenshots SET synced = 1 WHERE id = ?")
                    .bind(screenshot_id)
                    .execute(pool)
                    .await;
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
        handle_screenshot_upload(
            pool,
            client,
            token,
            screenshot_id,
            &file_path,
            remote_id,
            activity_percent,
        )
        .await;
    }
}

async fn cleanup_old_data(pool: &SqlitePool) {
    // Single query covers both expiry cases:
    //   1. Synced slots older than 24 h with no pending screenshots.
    //      Guard against deleting a slot whose screenshots haven't uploaded yet —
    //      that would sever the JOIN in the orphan-retry query (data loss).
    //   2. Permanently-failed slots (sync_attempts ≥ MAX) older than 7 days.
    //      These never reach synced=1, so the normal 24 h path never touches them.
    //
    // NOTE: DISTINCT is intentionally omitted from the subquery — NOT IN only
    // checks membership, so duplicates are harmless and DISTINCT adds wasted work.
    let slots_result = sqlx::query(
        r#"DELETE FROM time_slots
           WHERE (synced = 1
                  AND datetime(ended_at) < datetime('now', '-24 hours')
                  AND id NOT IN (SELECT time_slot_id FROM screenshots WHERE synced = 0))
              OR (synced = 0
                  AND sync_attempts >= ?
                  AND datetime(ended_at) < datetime('now', '-7 days'))"#,
    )
    .bind(MAX_SYNC_ATTEMPTS)
    .execute(pool)
    .await;

    match slots_result {
        Ok(r) if r.rows_affected() > 0 => {
            log::info!("[cleanup] deleted {} time_slots", r.rows_affected());
        }
        Err(e) => log::warn!("[cleanup] failed to delete time_slots: {}", e),
        _ => {}
    }

    // app_usage and screenshots are cleaned up automatically via ON DELETE CASCADE
    // (migration 011) — no manual orphan queries needed.
}
