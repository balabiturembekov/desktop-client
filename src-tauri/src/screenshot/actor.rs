use chrono::Utc;
use rand::Rng;
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

use crate::screenshot::capture::capture_screenshot;

/// Returns true if the process has Screen Recording permission on macOS.
/// Uses CGPreflightScreenCaptureAccess (available macOS 10.15+).
#[cfg(target_os = "macos")]
fn is_screen_recording_trusted() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
}

#[cfg(not(target_os = "macos"))]
fn is_screen_recording_trusted() -> bool {
    true
}

/// Length of one tracking chunk in seconds.
const CHUNK_SECS: u64 = 600;
/// Poll interval — how often we check elapsed time and timer state.
const POLL_SECS: u64 = 1;
/// Minimum elapsed seconds before a stop-triggered screenshot is taken.
const STOP_SHOT_MIN_SECS: u64 = 30;
/// Number of equal windows the chunk is divided into (one shot per window).
const WINDOWS: u64 = 3;

/// Plans one screenshot offset per window, distributed uniformly across the chunk.
///
/// The chunk is split into WINDOWS equal slices ([0,200), [200,400), [400,600)).
/// One random second is chosen inside each slice, so screenshots are spread
/// across the whole chunk rather than clustering at one end.
fn plan_chunk() -> Vec<u64> {
    let mut rng = rand::thread_rng();
    let window_secs = CHUNK_SECS / WINDOWS; // 200s each
    let offsets: Vec<u64> = (0..WINDOWS)
        .map(|w| {
            let lo = w * window_secs;
            let hi = lo + window_secs; // exclusive upper bound
            rng.gen_range(lo..hi)
        })
        .collect();
    // offsets are already in ascending order (one per window, windows non-overlapping)
    log::info!("[screenshot] plan_chunk: {:?}", offsets);
    offsets
}

/// Minimum file size (bytes) for a captured PNG to be considered a real frame.
/// When Screen Recording permission is denied, some OS versions silently return
/// a black/blank image that still saves successfully — its compressed PNG is
/// much smaller than a genuine screenshot. Discard anything below this threshold.
const MIN_SCREENSHOT_BYTES: u64 = 1024; // 1 KB

/// Saves a screenshot to disk, records it in the DB, and emits "screenshot-taken"
/// so the frontend can show a system notification.
///
/// Guards (applied before capture and after):
/// 1. Screen Recording permission — skip entirely if not granted (primary).
/// 2. File size — discard files < 1 KB that are likely silent blank captures (secondary).
///
/// The DB insert is awaited inline (no tokio::spawn) so that a failed insert
/// can immediately remove the captured file — preventing orphaned PNG files
/// on disk that no cleanup pipeline can ever find (BUG-A03).
async fn take_screenshot(
    app: &AppHandle,
    pool: &SqlitePool,
    screenshots_dir: &std::path::Path,
    slot_id: i64,
    label: &str,
) {
    // Primary guard: bail out before touching the filesystem if the OS will
    // only give us a black frame anyway. This also prevents the notification
    // from firing when the user has not yet granted permission.
    if !is_screen_recording_trusted() {
        log::warn!("[screenshot] Screen Recording permission not granted — skipping ({label})");
        return;
    }

    match capture_screenshot(screenshots_dir) {
        Ok(path) => {
            // Secondary guard: if the saved PNG is suspiciously small it is
            // likely a blank/black frame produced when the OS silently denies
            // the capture without returning an error. Discard and delete.
            let file_size = tokio::fs::metadata(&path)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            if file_size < MIN_SCREENSHOT_BYTES {
                log::warn!(
                    "[screenshot] captured file too small ({file_size} bytes) — \
                     likely blank frame, discarding ({label})"
                );
                let _ = tokio::fs::remove_file(&path).await;
                return;
            }

            let taken_at = Utc::now().to_rfc3339();
            let path_str = path.to_string_lossy().to_string();

            match sqlx::query(
                "INSERT INTO screenshots (time_slot_id, file_path, taken_at, synced) \
                 VALUES (?, ?, ?, 0)",
            )
            .bind(slot_id)
            .bind(&path_str)
            .bind(&taken_at)
            .execute(pool)
            .await
            {
                Ok(_) => {
                    log::info!("[screenshot] saved ({label}): {path_str} (slot {slot_id})");
                    // emit_to("main") instead of emit() to avoid broadcasting to
                    // the "idle" window — both load App.tsx and would each fire
                    // a separate system notification, producing duplicate alerts.
                    let _ = app.emit_to("main", "screenshot-taken", &path_str);
                }
                Err(e) => {
                    log::error!("[screenshot] db insert failed ({label}): {e} (slot {slot_id})");
                    // Remove the file so it doesn't linger on disk without a DB record.
                    // Without this the file can never be found by the sync/cleanup pipeline.
                    if let Err(del_err) = tokio::fs::remove_file(&path).await {
                        log::warn!(
                            "[screenshot] failed to remove orphaned file {path_str}: {del_err}"
                        );
                    }
                }
            }
        }
        Err(e) => log::error!("[screenshot] capture error ({label}): {e}"),
    }
}

pub async fn screenshot_actor(
    app: AppHandle,
    pool: SqlitePool,
    screenshots_dir: PathBuf,
    is_running: Arc<AtomicBool>,
    current_slot_id: Arc<Mutex<Option<i64>>>,
) {
    let mut tick = interval(Duration::from_secs(POLL_SECS));

    // Elapsed seconds since the start of the current chunk.
    let mut chunk_elapsed: u64 = 0;
    // Planned screenshot offsets for the current chunk (one per window, ascending).
    let mut scheduled: Vec<u64> = vec![];
    // Index into `scheduled` — points to the next shot not yet taken.
    let mut next_shot: usize = 0;
    // Whether the timer was running on the previous poll.
    let mut was_running = false;
    // Whether at least one screenshot was taken in the current session chunk.
    let mut shot_taken_this_chunk = false;

    log::info!("[screenshot] actor started");

    // Check Screen Recording permission at startup.
    if !is_screen_recording_trusted() {
        log::warn!("[screenshot] Screen Recording permission not granted — emitting permissions-required");
        let _ = app.emit("permissions-required", ());
    }

    loop {
        tick.tick().await;

        let running = is_running.load(Ordering::Relaxed);

        // ── State transitions ─────────────────────────────────────────────
        if running && !was_running {
            // Timer started / resumed → plan a fresh chunk.
            chunk_elapsed = 0;
            scheduled = plan_chunk();
            next_shot = 0;
            shot_taken_this_chunk = false;
            log::info!(
                "[screenshot] planned {} screenshot(s) at {:?}s",
                scheduled.len(),
                scheduled
            );
        } else if !running && was_running {
            // Timer stopped or went idle.
            // Take a stop-shot if at least STOP_SHOT_MIN_SECS have elapsed
            // and no screenshot was taken yet in this chunk.
            if chunk_elapsed >= STOP_SHOT_MIN_SECS && !shot_taken_this_chunk {
                let slot_id = *current_slot_id.lock().await;
                if let Some(slot_id) = slot_id {
                    log::info!(
                        "[screenshot] stop-shot after {}s with no prior screenshot",
                        chunk_elapsed
                    );
                    take_screenshot(&app, &pool, &screenshots_dir, slot_id, "stop").await;
                }
            }

            scheduled.clear();
            next_shot = 0;
            chunk_elapsed = 0;
            shot_taken_this_chunk = false;
            log::info!("[screenshot] timer stopped — scheduled shots cleared");
        }
        was_running = running;

        if !running {
            continue;
        }

        chunk_elapsed += POLL_SECS;

        // ── Chunk rollover ────────────────────────────────────────────────
        if chunk_elapsed >= CHUNK_SECS {
            chunk_elapsed = 0;
            scheduled = plan_chunk();
            next_shot = 0;
            shot_taken_this_chunk = false;
            log::info!(
                "[screenshot] chunk rollover — planned {} screenshot(s) at {:?}s",
                scheduled.len(),
                scheduled
            );
        }

        // ── Fire scheduled shots ──────────────────────────────────────────
        while next_shot < scheduled.len() && chunk_elapsed >= scheduled[next_shot] {
            let offset = scheduled[next_shot];
            next_shot += 1;

            let slot_id = *current_slot_id.lock().await;
            let Some(slot_id) = slot_id else {
                log::warn!(
                    "[screenshot] skipping — no active slot_id at offset {}s",
                    offset
                );
                continue;
            };

            take_screenshot(
                &app,
                &pool,
                &screenshots_dir,
                slot_id,
                &format!("offset {offset}s"),
            )
            .await;
            shot_taken_this_chunk = true;
        }
    }
}
