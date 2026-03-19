-- Migration 010: track how many times a row has failed to sync.
-- Rows that exceed MAX_SYNC_ATTEMPTS (5) are permanently excluded from sync
-- queries so they stop burning network on every cycle.
ALTER TABLE time_slots  ADD COLUMN sync_attempts INTEGER DEFAULT 0;
ALTER TABLE screenshots ADD COLUMN sync_attempts INTEGER DEFAULT 0;
