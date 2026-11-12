ALTER TABLE users ADD COLUMN user_status_msg_id INTEGER;
ALTER TABLE users ADD COLUMN user_status TEXT NOT NULL DEFAULT 'waiting';
ALTER TABLE users ADD COLUMN user_status_pinned BOOLEAN NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN status_updated_at DATETIME;
