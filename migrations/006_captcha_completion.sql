CREATE TABLE captcha_completions (
 user_id INTEGER PRIMARY KEY,
 completed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT OR IGNORE INTO captcha_completions(user_id)
 SELECT user_id FROM users WHERE captcha_passed=1;
