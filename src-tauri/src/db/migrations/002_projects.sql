CREATE TABLE IF NOT EXISTS projects (
    id          INTEGER PRIMARY KEY,
    remote_id   TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    is_active   INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL
);
