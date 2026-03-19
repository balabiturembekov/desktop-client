-- Migration 011: add ON DELETE CASCADE to app_usage and screenshots foreign keys.
--
-- SQLite does not support ALTER TABLE ... MODIFY CONSTRAINT, so the tables must
-- be recreated using the recommended 12-step procedure.
--
-- After this migration, deleting a time_slots row automatically removes all
-- related app_usage and screenshots rows via the DB engine — no manual orphan
-- cleanup queries are needed in application code.
--
-- Note: PRAGMA foreign_keys cannot be changed inside a transaction.  However,
-- dropping a child table is safe with enforcement enabled (no constraint is
-- violated), so no PRAGMA gymnastics are required here.

-- ── app_usage ──────────────────────────────────────────────────────────────────
CREATE TABLE app_usage_new (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    time_slot_id  INTEGER NOT NULL,
    app_name      TEXT    NOT NULL,
    window_title  TEXT    NOT NULL,
    url           TEXT,
    duration_secs INTEGER NOT NULL DEFAULT 0,
    started_at    TEXT    NOT NULL,
    synced        INTEGER NOT NULL DEFAULT 0,
    sync_attempts INTEGER          DEFAULT 0,
    FOREIGN KEY (time_slot_id) REFERENCES time_slots(id) ON DELETE CASCADE
);
-- app_usage never received sync_attempts via ALTER TABLE (migration 010 only
-- touched time_slots and screenshots), so we list columns explicitly and
-- supply 0 as the default for sync_attempts on all existing rows.
INSERT INTO app_usage_new (id, time_slot_id, app_name, window_title, url, duration_secs, started_at, synced, sync_attempts)
SELECT                     id, time_slot_id, app_name, window_title, url, duration_secs, started_at, synced, 0
FROM app_usage;
DROP TABLE app_usage;
ALTER TABLE app_usage_new RENAME TO app_usage;

-- Recreate indexes dropped with the old table.
CREATE INDEX idx_app_usage_synced  ON app_usage(synced);
CREATE INDEX idx_app_usage_slot_id ON app_usage(time_slot_id);

-- ── screenshots ────────────────────────────────────────────────────────────────
CREATE TABLE screenshots_new (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    time_slot_id  INTEGER NOT NULL,
    file_path     TEXT    NOT NULL,
    taken_at      TEXT    NOT NULL,
    synced        INTEGER NOT NULL DEFAULT 0,
    sync_attempts INTEGER          DEFAULT 0,
    FOREIGN KEY (time_slot_id) REFERENCES time_slots(id) ON DELETE CASCADE
);
INSERT INTO screenshots_new SELECT * FROM screenshots;
DROP TABLE screenshots;
ALTER TABLE screenshots_new RENAME TO screenshots;

-- Recreate indexes dropped with the old table.
CREATE INDEX idx_screenshots_synced  ON screenshots(synced);
CREATE INDEX idx_screenshots_slot_id ON screenshots(time_slot_id);
