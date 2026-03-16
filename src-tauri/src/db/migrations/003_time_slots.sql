CREATE TABLE IF NOT EXISTS time_slots (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  TEXT NOT NULL,
    started_at  TEXT NOT NULL,
    ended_at    TEXT,
    duration_secs INTEGER NOT NULL DEFAULT 0,
    activity_percent INTEGER NOT NULL DEFAULT 0,
    synced      INTEGER NOT NULL DEFAULT 0
);
