use std::{env, str::FromStr};
#[derive(Clone)]
pub struct Config {
    pub bot_token: String,
    pub admin_group_id: i64,
    pub bot_owner_id: i64,
    pub database_url: String,
    pub rate_limit_max_msg: u32,
    pub rate_limit_window_sec: u64,
    pub captcha_enabled: bool,
    pub work_hours_enabled: bool,
    pub work_hours_start: String,
    pub work_hours_end: String,
    pub out_of_hours_text: String,
}
impl Config {
    pub fn from_env() -> Result<Self, String> {
        dotenvy::dotenv().ok();
        Ok(Self {
            bot_token: required("BOT_TOKEN")?,
            admin_group_id: parse("ADMIN_GROUP_ID", 0i64)?,
            bot_owner_id: parse("BOT_OWNER_ID", 8883130864i64)?,
            database_url: get("DATABASE_URL", "sqlite://data/bot.db?mode=rwc"),
            rate_limit_max_msg: parse("RATE_LIMIT_MAX_MSG", 5u32)?,
            rate_limit_window_sec: parse("RATE_LIMIT_WINDOW_SEC", 10u64)?,
            captcha_enabled: parse("CAPTCHA_ENABLED", true)?,
            work_hours_enabled: parse("WORK_HOURS_ENABLED", false)?,
            work_hours_start: get("WORK_HOURS_START", "09:00"),
            work_hours_end: get("WORK_HOURS_END", "18:00"),
            out_of_hours_text: get(
                "OUT_OF_HOURS_TEXT",
                "Сейчас мы вне рабочего времени. Мы ответим вам в ближайшее рабочее время.",
            ),
        })
    }
}
fn required(k: &str) -> Result<String, String> {
    env::var(k).map_err(|_| format!("{k} is required"))
}
fn get(k: &str, d: &str) -> String {
    env::var(k).unwrap_or_else(|_| d.to_owned())
}
fn parse<T: FromStr>(k: &str, d: T) -> Result<T, String> {
    match env::var(k) {
        Ok(value) if !value.trim().is_empty() => value.parse().map_err(|_| format!("invalid {k}")),
        _ => Ok(d),
    }
}
