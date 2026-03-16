CREATE TABLE IF NOT EXISTS users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    remote_id     TEXT NOT NULL UNIQUE,
    email         TEXT NOT NULL,
    name          TEXT NOT NULL,
    avatar        TEXT,
    role          TEXT NOT NULL DEFAULT 'USER',
    access_token  TEXT NOT NULL,
    refresh_token TEXT NOT NULL,
    created_at    TEXT NOT NULL
);
