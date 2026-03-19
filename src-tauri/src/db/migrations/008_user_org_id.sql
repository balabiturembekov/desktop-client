-- Migration 008: add org_id to users so tracking-settings endpoints can be called.
-- org_id is nullable: existing rows keep NULL until the user re-authenticates.
ALTER TABLE users ADD COLUMN org_id TEXT;
