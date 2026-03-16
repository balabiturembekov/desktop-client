CREATE TABLE IF NOT EXISTS app_usage (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    time_slot_id INTEGER NOT NULL,
    app_name TEXT NOT NULL,
    window_title TEXT NOT NULL,
    url TEXT,
    duration_secs INTEGER NOT NULL DEFAULT 0,
    started_at TEXT NOT NULL,
    synced INTEGER NOT NULL DEFAULT 0
);
