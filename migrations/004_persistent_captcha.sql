ALTER TABLE users ADD COLUMN captcha_question TEXT;
ALTER TABLE users ADD COLUMN captcha_answer INTEGER;
ALTER TABLE users ADD COLUMN captcha_options TEXT;
ALTER TABLE users ADD COLUMN captcha_id TEXT;
ALTER TABLE users ADD COLUMN captcha_expires_at DATETIME;
