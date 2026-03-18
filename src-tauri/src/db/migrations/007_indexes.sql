CREATE INDEX IF NOT EXISTS idx_time_slots_synced   ON time_slots(synced);
CREATE INDEX IF NOT EXISTS idx_time_slots_ended_at ON time_slots(ended_at);
CREATE INDEX IF NOT EXISTS idx_app_usage_synced    ON app_usage(synced);
CREATE INDEX IF NOT EXISTS idx_app_usage_slot_id   ON app_usage(time_slot_id);
CREATE INDEX IF NOT EXISTS idx_screenshots_synced  ON screenshots(synced);
CREATE INDEX IF NOT EXISTS idx_screenshots_slot_id ON screenshots(time_slot_id);
