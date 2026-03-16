CREATE TABLE IF NOT EXISTS screenshots (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    time_slot_id INTEGER NOT NULL,
    file_path    TEXT NOT NULL,
    taken_at     TEXT NOT NULL,
    synced       INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (time_slot_id) REFERENCES time_slots(id)
);
