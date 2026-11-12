CREATE TABLE IF NOT EXISTS users (
 user_id INTEGER PRIMARY KEY, username TEXT, first_name TEXT NOT NULL, language_code TEXT,
 is_premium BOOLEAN NOT NULL DEFAULT 0, thread_id INTEGER UNIQUE, dossier_msg_id INTEGER,
 is_banned BOOLEAN NOT NULL DEFAULT 0, captcha_passed BOOLEAN NOT NULL DEFAULT 0,
 admin_note TEXT DEFAULT '', created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
 last_active_at DATETIME DEFAULT CURRENT_TIMESTAMP, last_out_of_hours_at DATETIME
);
CREATE TABLE IF NOT EXISTS message_map (
 id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL, user_msg_id INTEGER NOT NULL,
 admin_msg_id INTEGER NOT NULL, direction TEXT NOT NULL, created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
 FOREIGN KEY (user_id) REFERENCES users(user_id)
);
CREATE INDEX IF NOT EXISTS idx_message_map_user_in ON message_map(user_id, user_msg_id, direction);
CREATE INDEX IF NOT EXISTS idx_message_map_admin ON message_map(admin_msg_id, direction);
CREATE TABLE IF NOT EXISTS templates (slug TEXT PRIMARY KEY, content TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
