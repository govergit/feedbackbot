mod config;
mod db;
mod handlers;
use config::Config;
use moka::future::Cache;
use sqlx::SqlitePool;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc,
};
use std::{collections::HashMap, sync::Mutex};
use teloxide::{prelude::*, types::MessageId};
#[derive(Clone)]
struct App {
    cfg: Arc<Config>,
    db: SqlitePool,
    limits: Cache<i64, Vec<i64>>,
    strikes: Cache<i64, u8>,
    albums: Arc<Mutex<HashMap<String, Vec<Message>>>>,
    topic_lock: Arc<tokio::sync::Mutex<()>>,
    rate_album_seen: Cache<String, bool>,
    admin_group_id: Arc<AtomicI64>,
}
impl App {
    fn admin_group_id(&self) -> i64 {
        self.admin_group_id.load(Ordering::Relaxed)
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cfg = Arc::new(Config::from_env()?);
    let db = db::connect(&cfg.database_url).await?;
    let stored_group: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key='admin_group_id'")
            .fetch_optional(&db)
            .await?;
    let admin_group_id = if cfg.admin_group_id != 0 {
        cfg.admin_group_id
    } else {
        stored_group
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    };
    let app = App {
        cfg: cfg.clone(),
        db,
        limits: Cache::new(10000),
        strikes: Cache::new(10000),
        albums: Arc::new(Mutex::new(HashMap::new())),
        topic_lock: Arc::new(tokio::sync::Mutex::new(())),
        rate_album_seen: Cache::builder()
            .max_capacity(20_000)
            .time_to_live(std::time::Duration::from_secs(30))
            .build(),
        admin_group_id: Arc::new(AtomicI64::new(admin_group_id)),
    };
    let bot = Bot::new(&cfg.bot_token);
    if app.admin_group_id() != 0 {
        if let Err(error) = handlers::ensure_help_topic(&bot, &app.db, app.admin_group_id()).await {
            tracing::error!(%error, "could not create Help topic at startup");
        }
    }
    handlers::initialize_user_statuses(&bot, &app).await;
    Dispatcher::builder(
        bot,
        dptree::entry()
            .branch(Update::filter_message().endpoint(on_message))
            .branch(Update::filter_callback_query().endpoint(on_callback))
            .branch(Update::filter_edited_message().endpoint(on_edited)),
    )
    .dependencies(dptree::deps![app])
    .build()
    .dispatch()
    .await;
    Ok(())
}
async fn on_message(bot: Bot, msg: Message, app: App) -> ResponseResult<()> {
    if let Some(user) = msg.from.as_ref() {
        if msg.chat.is_private() {
            handlers::private(&bot, &msg, user, &app).await?;
            return Ok(());
        }
    }
    handlers::admin(&bot, &msg, &app).await?;
    Ok(())
}

async fn on_callback(bot: Bot, q: CallbackQuery, app: App) -> ResponseResult<()> {
    let Some(rest) = q.data.as_deref().and_then(|data| data.strip_prefix("cap:")) else {
        return Ok(());
    };
    let fields = rest.split(':').collect::<Vec<_>>();
    let (challenge_id, value) = match fields.as_slice() {
        [challenge_id, value] => (*challenge_id, value.parse::<i32>().ok()),
        [_legacy_uid, challenge_id, value] => (*challenge_id, value.parse::<i32>().ok()),
        _ => ("", None),
    };
    let uid = q.from.id.0 as i64;
    let Some(value) = value.filter(|_| !challenge_id.is_empty()) else {
        bot.answer_callback_query(q.id)
            .text("Не удалось прочитать ответ. Отправьте сообщение боту, чтобы получить новое задание.")
            .show_alert(true)
            .await?;
        return Ok(());
    };
    let answer = sqlx::query_scalar::<_, i32>(
        "SELECT captcha_answer FROM users WHERE user_id=? AND captcha_id=? AND captcha_expires_at>datetime('now') AND captcha_passed=0",
    )
    .bind(uid)
    .bind(challenge_id)
    .fetch_optional(&app.db)
    .await
    .unwrap_or(None);
    match answer {
        None => match handlers::current_captcha(&app, uid).await {
            Ok(Some((question, options, challenge_id))) => {
                let keyboard = teloxide::types::InlineKeyboardMarkup::new(vec![options
                    .split(',')
                    .filter_map(|item| item.parse::<i32>().ok())
                    .map(|number| {
                        teloxide::types::InlineKeyboardButton::callback(
                            number.to_string(),
                            format!("cap:{challenge_id}:{number}"),
                        )
                    })
                    .collect::<Vec<_>>()]);
                bot.answer_callback_query(q.id)
                    .text("Задание обновлено. Нажмите ответ в новом сообщении.")
                    .await?;
                let previous_id: Option<i32> =
                    sqlx::query_scalar("SELECT captcha_message_id FROM users WHERE user_id=?")
                        .bind(uid)
                        .fetch_optional(&app.db)
                        .await
                        .unwrap_or(None);
                if let Some(message) = q.message {
                    let sent = bot
                        .send_message(message.chat().id, format!("Решите пример: {question}"))
                        .reply_markup(keyboard)
                        .await?;
                    sqlx::query("UPDATE users SET captcha_message_id=? WHERE user_id=?")
                        .bind(sent.id.0)
                        .bind(uid)
                        .execute(&app.db)
                        .await
                        .ok();
                    if let Some(old_id) = previous_id.filter(|id| *id != sent.id.0) {
                        let _ = bot.delete_message(ChatId(uid), MessageId(old_id)).await;
                    }
                    if message.id() != sent.id {
                        let _ = bot.delete_message(message.chat().id, message.id()).await;
                    }
                }
            }
            Ok(None) => {
                bot.answer_callback_query(q.id)
                    .text("Проверка уже пройдена. Отправьте сообщение боту.")
                    .await?;
            }
            Err(error) => {
                tracing::error!(user_id = uid, %error, "could not refresh captcha challenge");
                bot.answer_callback_query(q.id)
                    .text("Не удалось обновить задание. Отправьте сообщение боту ещё раз.")
                    .show_alert(true)
                    .await?;
            }
        },
        Some(answer) if answer == value => {
            let prompt_id: Option<i32> =
                sqlx::query_scalar("SELECT captcha_message_id FROM users WHERE user_id=?")
                    .bind(uid)
                    .fetch_optional(&app.db)
                    .await
                    .unwrap_or(None);
            let saved = async {
                let mut tx = app.db.begin().await?;
                sqlx::query("INSERT OR IGNORE INTO captcha_completions(user_id) VALUES(?)")
                    .bind(uid)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE users SET captcha_passed=1,captcha_question=NULL,captcha_answer=NULL,captcha_options=NULL,captcha_id=NULL,captcha_expires_at=NULL,captcha_message_id=NULL WHERE user_id=?")
                    .bind(uid)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await
            }
            .await;
            if let Err(error) = saved {
                tracing::error!(user_id = uid, %error, "could not save captcha completion");
                bot.answer_callback_query(q.id)
                    .text("Не удалось сохранить проверку. Нажмите ответ ещё раз.")
                    .show_alert(true)
                    .await?;
                return Ok(());
            }
            bot.answer_callback_query(q.id)
                .text("Проверка пройдена")
                .await?;
            if let Some(id) = prompt_id {
                if let Err(error) = bot.delete_message(ChatId(uid), MessageId(id)).await {
                    tracing::warn!(user_id = uid, message_id = id, %error, "could not delete solved captcha message");
                }
            }
            if let Some(message) = q.message {
                if Some(message.id().0) != prompt_id {
                    let _ = bot.delete_message(message.chat().id, message.id()).await;
                }
            }
        }
        Some(_) => {
            bot.answer_callback_query(q.id)
                .text("Ответ не совпал с решением. Проверьте пример и попробуйте ещё раз.")
                .show_alert(true)
                .await?;
        }
    }
    Ok(())
}

async fn on_edited(bot: Bot, msg: Message, app: App) -> ResponseResult<()> {
    let Some(text) = msg.text() else {
        return Ok(());
    };
    if msg.chat.is_private() {
        let uid = msg.chat.id.0;
        let target=sqlx::query_scalar::<_,i32>("SELECT admin_msg_id FROM message_map WHERE user_id=? AND user_msg_id=? AND direction='in' ORDER BY id DESC LIMIT 1").bind(uid).bind(msg.id.0).fetch_optional(&app.db).await.unwrap_or(None);
        if let (Some(_tid), Some(id)) = (crate::db::thread(&app.db, uid).await, target) {
            bot.edit_message_text(ChatId(app.cfg.admin_group_id), MessageId(id), text)
                .await
                .ok();
        }
    } else if msg.chat.id.0 == app.cfg.admin_group_id {
        if let Some(tid) = msg.thread_id {
            let uid = sqlx::query_scalar::<_, i64>("SELECT user_id FROM users WHERE thread_id=?")
                .bind(tid.0 .0)
                .fetch_optional(&app.db)
                .await
                .unwrap_or(None);
            if let Some(uid) = uid {
                let target=sqlx::query_scalar::<_,i32>("SELECT user_msg_id FROM message_map WHERE user_id=? AND admin_msg_id=? ORDER BY id DESC LIMIT 1").bind(uid).bind(msg.id.0).fetch_optional(&app.db).await.unwrap_or(None);
                if let Some(id) = target {
                    bot.edit_message_text(ChatId(uid), MessageId(id), text)
                        .await
                        .ok();
                }
            }
        }
    }
    Ok(())
}
