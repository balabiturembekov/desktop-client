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

/// Saves a screenshot to disk, records it in the DB, and emits "screenshot-taken"
/// so the frontend can show a system notification.
///
/// AppHandle is cloned before entering the spawn so the owned clone is moved
/// into the task — no borrowed references cross the spawn boundary.
/// The emit fires only after a successful DB insert.
async fn take_screenshot(
    app: &AppHandle,
    pool: &SqlitePool,
    screenshots_dir: &std::path::Path,
    slot_id: i64,
    label: &str,
) {
    match capture_screenshot(screenshots_dir) {
        Ok(path) => {
            let taken_at = Utc::now().to_rfc3339();
            let path_str = path.to_string_lossy().to_string();
            let pool = pool.clone();
            let label = label.to_string();
            let app = app.clone(); // owned clone — safe to move into spawn

            tokio::spawn(async move {
                match sqlx::query(
                    "INSERT INTO screenshots (time_slot_id, file_path, taken_at, synced) \
                     VALUES (?, ?, ?, 0)",
                )
                .bind(slot_id)
                .bind(&path_str)
                .bind(&taken_at)
                .execute(&pool)
                .await
                {
                    Ok(_) => {
                        log::info!(
                            "[screenshot] saved ({label}): {path_str} (slot {slot_id})"
                        );
                        let _ = app.emit("screenshot-taken", &path_str);
                    }
                    Err(e) => log::error!(
                        "[screenshot] db insert failed ({label}): {e} (slot {slot_id})"
                    ),
                }
            });
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

            take_screenshot(&app, &pool, &screenshots_dir, slot_id, &format!("offset {offset}s")).await;
            shot_taken_this_chunk = true;
        }
    }
}
