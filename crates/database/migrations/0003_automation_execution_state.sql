ALTER TABLE automations ADD COLUMN prompt TEXT NOT NULL DEFAULT '';
ALTER TABLE automations ADD COLUMN interval_minutes INTEGER NOT NULL DEFAULT 60;
ALTER TABLE automations ADD COLUMN last_status TEXT NOT NULL DEFAULT 'idle';
ALTER TABLE automations ADD COLUMN last_error TEXT;
ALTER TABLE automations ADD COLUMN last_conversation_id TEXT;
