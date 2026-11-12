use crate::{db, App};
use chrono::Utc;
use rand::Rng;
use sqlx::Row;
use std::sync::atomic::Ordering;
use std::time::Duration;
use teloxide::{prelude::*, types::*};

async fn collect_album(msg: &Message, app: &App) -> Vec<Message> {
    let Some(group) = msg.media_group_id() else {
        return vec![msg.clone()];
    };
    let key = format!("{}:{group}", msg.chat.id);
    if let Ok(mut albums) = app.albums.lock() {
        albums.entry(key.clone()).or_default().push(msg.clone());
    }
    tokio::time::sleep(Duration::from_millis(900)).await;
    app.albums
        .lock()
        .ok()
        .and_then(|mut albums| albums.remove(&key))
        .unwrap_or_default()
}

pub async fn ensure_help_topic(
    bot: &Bot,
    db: &sqlx::SqlitePool,
    group_id: i64,
) -> ResponseResult<()> {
    let saved_group: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key='help_group_id'")
            .fetch_optional(db)
            .await
            .unwrap_or(None);
    let saved_thread: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key='help_thread_id'")
            .fetch_optional(db)
            .await
            .unwrap_or(None);
    let same_group = saved_group.as_deref() == Some(&group_id.to_string());
    let saved_message: Option<String> = if same_group {
        sqlx::query_scalar("SELECT value FROM settings WHERE key='help_message_id'")
            .fetch_optional(db)
            .await
            .unwrap_or(None)
    } else {
        None
    };
    let is_pinned = if same_group {
        sqlx::query_scalar::<_, bool>("SELECT value='true' FROM settings WHERE key='help_pinned'")
            .fetch_optional(db)
            .await
            .unwrap_or(None)
            .unwrap_or(false)
    } else {
        false
    };
    if let (Some(thread), Some(_message)) = (saved_thread.as_ref(), saved_message.as_ref()) {
        if same_group && is_pinned {
            return Ok(());
        }
        let thread_id = thread.parse::<i32>().unwrap_or_default();
        let message_id = _message.parse::<i32>().unwrap_or_default();
        if message_id > 0 {
            bot.pin_chat_message(ChatId(group_id), MessageId(message_id))
                .await?;
            save_setting(db, "help_pinned", "true").await;
            tracing::info!(
                group_id,
                thread_id,
                message_id,
                "pinned Help command reference"
            );
            return Ok(());
        }
    }

    let thread_id = if same_group {
        saved_thread
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0)
    } else {
        0
    };
    let thread_id = if thread_id > 0 {
        thread_id
    } else {
        let topic = bot
            .create_forum_topic(ChatId(group_id), "Help", 0x6FB9F0, "")
            .await?;
        let new_id = topic.thread_id.0 .0;
        save_setting(db, "help_group_id", &group_id.to_string()).await;
        save_setting(db, "help_thread_id", &new_id.to_string()).await;
        new_id
    };
    let help = concat!(
        "СПРАВКА ПО БОТУ ПОДДЕРЖКИ\n\n",
        "НАСТРОЙКА ГРУППЫ\n",
        "/chatid — показать ID группы.\n",
        "/addchat — подключить эту форумную супергруппу (только администратор бота). Нужны права бота: управление темами, закрепление и удаление сообщений. Если прав не хватает, выдайте их и повторите команду.\n\n",
        "КОМАНДЫ В ТОПИКЕ ОБРАЩЕНИЯ\n",
        "/ban [причина] — заблокировать пользователя и закрыть тему.\n",
        "/unban — снять блокировку и открыть тему.\n",
        "/close — закрыть обращение и уведомить пользователя.\n",
        "/note <текст> — сохранить заметку в карточке пользователя.\n",
        "/tpl_add <slug> <текст> — создать или обновить HTML-шаблон ответа.\n",
        "/tpl_del <slug> — удалить шаблон.\n",
        "/tpl_list — показать список шаблонов.\n",
        "/<slug> — отправить пользователю шаблон с таким slug.\n",
        "/stats — статистика; команда работает в General.\n\n",
        "СООБЩЕНИЯ\n",
        "Сообщения в топике отправляются пользователю. Текстовые правки синхронизируются в обе стороны. Сообщения с точкой или # в начале остаются внутренними комментариями менеджеров."
    );
    let message_id = if let Some(saved) = saved_message.and_then(|x| x.parse::<i32>().ok()) {
        saved
    } else {
        let message = bot
            .send_message(ChatId(group_id), help)
            .message_thread_id(ThreadId(MessageId(thread_id)))
            .await?;
        save_setting(db, "help_message_id", &message.id.0.to_string()).await;
        message.id.0
    };
    bot.pin_chat_message(ChatId(group_id), MessageId(message_id))
        .await?;
    save_setting(db, "help_pinned", "true").await;
    tracing::info!(
        group_id,
        thread_id,
        message_id,
        "pinned Help command reference"
    );
    Ok(())
}

async fn save_setting(db: &sqlx::SqlitePool, key: &str, value: &str) {
    if let Err(error) = sqlx::query(
        "INSERT INTO settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(db)
    .await
    {
        tracing::error!(%error, setting = key, "failed to persist settings value");
    }
}

#[derive(Clone, Copy)]
enum TicketStatus {
    Ready,
    Sending,
    Waiting,
    Answered,
    Closed,
    Banned,
    Unavailable,
    Failed,
}

impl TicketStatus {
    fn key(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Sending => "sending",
            Self::Waiting => "waiting",
            Self::Answered => "answered",
            Self::Closed => "closed",
            Self::Banned => "banned",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::Ready => "👋 <b>Служба поддержки готова</b>\nОтправьте сообщение, чтобы открыть обращение.",
            Self::Sending => "⏳ <b>Передаём сообщение команде поддержки</b>\nЭто обычно занимает несколько секунд.",
            Self::Waiting => "✅ <b>Сообщение доставлено специалистам</b>\n⏳ Обращение ожидает ответа. Ответ появится здесь, в этом чате.\n\nМожете отправить сюда дополнительные сведения или файлы — они попадут в это же обращение.",
            Self::Answered => "✅ <b>Поддержка ответила</b>\nОтвет специалиста находится выше в этом чате. Если вопрос ещё не решён, просто ответьте новым сообщением.",
            Self::Closed => "🔒 <b>Обращение закрыто</b>\nЕсли понадобится помощь по этому или другому вопросу, отправьте сообщение — мы откроем новое обращение.",
            Self::Banned => "⛔ <b>Доступ к поддержке ограничен</b>\nЕсли вы считаете, что это произошло по ошибке, свяжитесь с администратором поддержки.",
            Self::Unavailable => "⚙️ <b>Поддержка пока не подключена</b>\nВаше сообщение ещё не передано. Попробуйте написать позже.",
            Self::Failed => "⚠️ <b>Не удалось доставить сообщение</b>\nОно не попало специалистам. Попробуйте отправить его ещё раз чуть позже.",
        }
    }
}

pub async fn current_captcha(
    app: &App,
    uid: i64,
) -> Result<Option<(String, String, String)>, sqlx::Error> {
    let _guard = app.topic_lock.lock().await;
    let passed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM captcha_completions WHERE user_id=?) OR (SELECT captcha_passed FROM users WHERE user_id=?)",
    )
        .bind(uid)
        .bind(uid)
        .fetch_one(&app.db)
        .await?;
    if passed {
        return Ok(None);
    }
    let existing: Option<(String, String, String)> = sqlx::query_as(
        "SELECT captcha_question,captcha_options,captcha_id FROM users WHERE user_id=? AND captcha_expires_at>datetime('now')",
    )
    .bind(uid)
    .fetch_optional(&app.db)
    .await?;
    if existing.is_some() {
        return Ok(existing);
    }

    let (a, b, answer, choices) = {
        let mut rng = rand::thread_rng();
        let a = rng.gen_range(1..10);
        let b = rng.gen_range(1..10);
        let answer = a + b;
        let mut choices = vec![answer];
        while choices.len() < 3 {
            let candidate = rng.gen_range(2..19);
            if !choices.contains(&candidate) {
                choices.push(candidate);
            }
        }
        (a, b, answer, choices)
    };
    let question = format!("{a} + {b} = ?");
    let options = choices
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let challenge_id = rand::thread_rng().gen::<u64>().to_string();
    sqlx::query(
        "UPDATE users SET captcha_question=?,captcha_answer=?,captcha_options=?,captcha_id=?,captcha_expires_at=datetime('now','+15 minutes') WHERE user_id=?",
    )
    .bind(&question)
    .bind(answer)
    .bind(&options)
    .bind(&challenge_id)
    .bind(uid)
    .execute(&app.db)
    .await?;
    Ok(Some((question, options, challenge_id)))
}

async fn update_user_status(bot: &Bot, app: &App, uid: i64, status: TicketStatus) {
    update_user_status_message(bot, app, uid, status, false).await;
}

async fn replace_user_status(bot: &Bot, app: &App, uid: i64, status: TicketStatus) {
    update_user_status_message(bot, app, uid, status, true).await;
}

async fn update_user_status_message(
    bot: &Bot,
    app: &App,
    uid: i64,
    status: TicketStatus,
    force_new: bool,
) {
    let status_text = format!(
        "💬 <b>Обращение в поддержку</b>\n\n{}\n\n🕒 Обновлено: {} UTC",
        status.text(),
        Utc::now().format("%Y-%m-%d %H:%M")
    );
    let saved: Option<(Option<i32>, bool)> =
        sqlx::query_as("SELECT user_status_msg_id,user_status_pinned FROM users WHERE user_id=?")
            .bind(uid)
            .fetch_optional(&app.db)
            .await
            .ok()
            .flatten();

    let old_message_id = saved.as_ref().and_then(|(id, _)| *id);
    let (message_id, pinned) = match saved {
        Some((Some(id), pinned)) if !force_new => {
            match bot
                .edit_message_text(ChatId(uid), MessageId(id), status_text.clone())
                .parse_mode(ParseMode::Html)
                .await
            {
                Ok(_) => (id, pinned),
                Err(error) => {
                    tracing::warn!(user_id = uid, message_id = id, %error, "could not edit user status message; replacing it");
                    match bot
                        .send_message(ChatId(uid), status_text)
                        .parse_mode(ParseMode::Html)
                        .await
                    {
                        Ok(message) => (message.id.0, false),
                        Err(error) => {
                            tracing::error!(user_id = uid, %error, "could not send user status message");
                            return;
                        }
                    }
                }
            }
        }
        _ => match bot
            .send_message(ChatId(uid), status_text)
            .parse_mode(ParseMode::Html)
            .await
        {
            Ok(message) => (message.id.0, false),
            Err(error) => {
                tracing::error!(user_id = uid, %error, "could not send user status message");
                return;
            }
        },
    };

    let mut is_pinned = pinned;
    if !pinned {
        match bot
            .pin_chat_message(ChatId(uid), MessageId(message_id))
            .await
        {
            Ok(_) => is_pinned = true,
            Err(error) => {
                tracing::warn!(user_id = uid, message_id, %error, "could not pin user status message")
            }
        }
    }
    if let Err(error) = sqlx::query(
        "UPDATE users SET user_status_msg_id=?,user_status=?,user_status_pinned=?,status_updated_at=CURRENT_TIMESTAMP WHERE user_id=?",
    )
    .bind(message_id)
    .bind(status.key())
    .bind(is_pinned)
    .bind(uid)
    .execute(&app.db)
    .await
    {
        tracing::error!(user_id = uid, %error, "could not persist user status");
    }
    if force_new && is_pinned {
        if let Some(old_id) = old_message_id.filter(|old_id| *old_id != message_id) {
            if let Err(error) = bot.delete_message(ChatId(uid), MessageId(old_id)).await {
                tracing::warn!(user_id = uid, old_message_id = old_id, %error, "could not delete previous pinned status message");
            }
        }
    }
}

pub async fn initialize_user_statuses(bot: &Bot, app: &App) {
    let users = sqlx::query_scalar::<_, i64>(
        "SELECT user_id FROM users WHERE thread_id IS NOT NULL AND user_status_msg_id IS NULL",
    )
    .fetch_all(&app.db)
    .await
    .unwrap_or_default();
    for uid in users {
        update_user_status(bot, app, uid, TicketStatus::Waiting).await;
    }
}

pub async fn private(bot: &Bot, msg: &Message, user: &User, app: &App) -> ResponseResult<()> {
    let uid = user.id.0 as i64;
    if db::ensure_user(&app.db, user).await.is_err() || db::banned(&app.db, uid).await {
        return Ok(());
    }
    if msg
        .text()
        .is_some_and(|text| text.split_whitespace().next() == Some("/start"))
    {
        let status = if db::thread(&app.db, uid).await.is_some() {
            TicketStatus::Waiting
        } else {
            TicketStatus::Ready
        };
        replace_user_status(bot, app, uid, status).await;
        return Ok(());
    }
    if app.admin_group_id() == 0 {
        update_user_status(bot, app, uid, TicketStatus::Unavailable).await;
        return Ok(());
    }
    match msg.media_group_id() {
        Some(group) => {
            let key = format!("{uid}:{group}");
            if let Some(allowed) = app.rate_album_seen.get(&key).await {
                if !allowed {
                    return Ok(());
                }
            } else {
                let allowed = !rate_limited(bot, uid, app).await;
                app.rate_album_seen.insert(key, allowed).await;
                if !allowed {
                    return Ok(());
                }
            }
        }
        None => {
            if rate_limited(bot, uid, app).await {
                return Ok(());
            }
        }
    }
    if app.cfg.captcha_enabled {
        match current_captcha(app, uid).await {
            Ok(Some((question, options, challenge_id))) => {
                let options = options
                    .split(',')
                    .filter_map(|value| value.parse::<i32>().ok())
                    .collect::<Vec<_>>();
                let kb = InlineKeyboardMarkup::new(vec![options
                    .iter()
                    .map(|v| {
                        InlineKeyboardButton::callback(
                            v.to_string(),
                            format!("cap:{challenge_id}:{v}"),
                        )
                    })
                    .collect::<Vec<_>>()]);
                let sent = bot
                    .send_message(
                        msg.chat.id,
                        format!("Чтобы начать, решите пример: {question}"),
                    )
                    .reply_markup(kb)
                    .await?;
                sqlx::query("UPDATE users SET captcha_message_id=? WHERE user_id=?")
                    .bind(sent.id.0)
                    .bind(uid)
                    .execute(&app.db)
                    .await
                    .ok();
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(user_id = uid, %error, "failed to load captcha challenge");
                return Ok(());
            }
        }
    }
    let tid = ensure_topic(bot, msg, user, app).await?;
    tracing::info!(
        user_id = uid,
        admin_group_id = app.admin_group_id(),
        forum_thread_id = tid,
        user_message_id = msg.id.0,
        "routing private message to support topic"
    );
    update_user_status(bot, app, uid, TicketStatus::Sending).await;
    let batch = collect_album(msg, app).await;
    if batch.is_empty() {
        return Ok(());
    }
    if batch.len() > 1 {
        let ids = batch.iter().map(|m| m.id).collect::<Vec<_>>();
        match bot
            .copy_messages(ChatId(app.admin_group_id()), msg.chat.id, ids)
            .message_thread_id(ThreadId(MessageId(tid)))
            .await
        {
            Ok(copied) => {
                for (source, destination) in batch.iter().zip(copied) {
                    let _ = db::map_add(&app.db, uid, source.id.0, destination.0, "in").await;
                }
                update_user_status(bot, app, uid, TicketStatus::Waiting).await;
            }
            Err(e) => {
                tracing::error!(%e, "failed to copy inbound album");
                update_user_status(bot, app, uid, TicketStatus::Failed).await;
            }
        }
        if app.cfg.work_hours_enabled && outside_hours(app) && should_send_ooh(uid, app).await {
            bot.send_message(msg.chat.id, &app.cfg.out_of_hours_text)
                .await?;
        }
        return Ok(());
    }
    let reply = if let Some(r) = msg.reply_to_message() {
        db::admin_reply(&app.db, uid, r.id.0).await
    } else {
        None
    };
    let mut req = bot.copy_message(ChatId(app.admin_group_id()), msg.chat.id, msg.id);
    req = req.message_thread_id(ThreadId(MessageId(tid)));
    if let Some(id) = reply {
        req = req.reply_parameters(ReplyParameters::new(MessageId(id)));
    }
    match req.await {
        Ok(copied) => {
            tracing::info!(
                user_id = uid,
                forum_thread_id = tid,
                admin_message_id = copied.0,
                "copied private message into support topic"
            );
            let _ = db::map_add(&app.db, uid, msg.id.0, copied.0, "in").await;
            update_user_status(bot, app, uid, TicketStatus::Waiting).await;
        }
        Err(e)
            if e.to_string().to_lowercase().contains("thread not found")
                || e.to_string()
                    .to_lowercase()
                    .contains("message thread not found") =>
        {
            let _guard = app.topic_lock.lock().await;
            let current_tid = db::thread(&app.db, uid).await;
            let new_tid = if let Some(current_tid) = current_tid.filter(|id| *id != tid) {
                current_tid
            } else {
                sqlx::query("UPDATE users SET thread_id=NULL WHERE user_id=?")
                    .bind(uid)
                    .execute(&app.db)
                    .await
                    .ok();
                ensure_topic_unlocked(bot, msg, user, app).await?
            };
            tracing::info!(
                user_id = uid,
                old_thread_id = tid,
                new_thread_id = new_tid,
                "recreated deleted support topic"
            );
            match bot
                .copy_message(ChatId(app.admin_group_id()), msg.chat.id, msg.id)
                .message_thread_id(ThreadId(MessageId(new_tid)))
                .await
            {
                Ok(copied) => {
                    let _ = db::map_add(&app.db, uid, msg.id.0, copied.0, "in").await;
                    update_user_status(bot, app, uid, TicketStatus::Waiting).await;
                }
                Err(error) => {
                    tracing::error!(user_id = uid, thread_id = new_tid, %error, "inbound message delivery failed after topic recovery");
                    update_user_status(bot, app, uid, TicketStatus::Failed).await;
                    let _ = bot.send_message(ChatId(uid), format!("Тема создана, но сообщение пока не удалось доставить в поддержку: {error}"))
                        .await;
                }
            }
        }
        Err(e) => {
            tracing::error!(user_id = uid, thread_id = tid, %e, "failed to copy inbound message");
            update_user_status(bot, app, uid, TicketStatus::Failed).await;
            let _ = bot
                .send_message(
                    ChatId(uid),
                    format!("Не удалось отправить сообщение в службу поддержки: {e}"),
                )
                .await;
        }
    }
    if app.cfg.work_hours_enabled && outside_hours(app) && should_send_ooh(uid, app).await {
        bot.send_message(msg.chat.id, &app.cfg.out_of_hours_text)
            .await?;
    }
    Ok(())
}
async fn rate_limited(bot: &Bot, uid: i64, app: &App) -> bool {
    let now = Utc::now().timestamp();
    let window = app.cfg.rate_limit_window_sec as i64;
    let mut v = app.limits.get(&uid).await.unwrap_or_default();
    v.retain(|x| now - *x < window);
    v.push(now);
    app.limits.insert(uid, v.clone()).await;
    if v.len() <= app.cfg.rate_limit_max_msg as usize {
        app.strikes.insert(uid, 0).await;
        false
    } else {
        let n = app.strikes.get(&uid).await.unwrap_or(0).saturating_add(1);
        app.strikes.insert(uid, n).await;
        if n == 3 {
            let _ = bot
                .send_message(
                    ChatId(uid),
                    "Пожалуйста, не отправляйте сообщения так часто.",
                )
                .await;
        }
        true
    }
}
async fn ensure_topic(bot: &Bot, msg: &Message, u: &User, app: &App) -> ResponseResult<i32> {
    let _guard = app.topic_lock.lock().await;
    ensure_topic_unlocked(bot, msg, u, app).await
}

async fn ensure_topic_unlocked(
    bot: &Bot,
    msg: &Message,
    u: &User,
    app: &App,
) -> ResponseResult<i32> {
    let uid = u.id.0 as i64;
    if let Some(t) = db::thread(&app.db, uid).await {
        tracing::info!(
            user_id = uid,
            forum_thread_id = t,
            "using existing support topic"
        );
        ensure_dossier(bot, u, app, t).await;
        return Ok(t);
    }
    let mut name = format!(
        "{} (@{}) [{}]",
        u.first_name,
        u.username.as_deref().unwrap_or("—"),
        uid
    );
    name.truncate(128);
    let group_id = app.admin_group_id();
    let topic = match bot
        .create_forum_topic(ChatId(group_id), name, 0x6FB9F0, "")
        .await
    {
        Ok(topic) => topic,
        Err(error) => {
            tracing::error!(user_id = uid, admin_group_id = group_id, %error, "Telegram rejected forum topic creation");
            update_user_status(bot, app, uid, TicketStatus::Failed).await;
            let _ = bot
                .send_message(
                    ChatId(uid),
                    format!("Не удалось создать тему поддержки в группе. Проверьте, что Topics включены и у бота есть право «Управление темами». Ошибка Telegram: {error}"),
                )
                .await;
            return Err(error);
        }
    };
    let id = topic.thread_id.0 .0;
    if let Err(error) = db::set_thread(&app.db, uid, id).await {
        tracing::error!(user_id = uid, forum_thread_id = id, %error, "failed to save support topic id");
        let _ = bot
            .send_message(
                ChatId(uid),
                format!("Тема создана, но не удалось сохранить её в базе данных: {error}"),
            )
            .await;
    }
    tracing::info!(
        user_id = uid,
        admin_group_id = group_id,
        forum_thread_id = id,
        "created support topic"
    );
    ensure_dossier(bot, u, app, id).await;
    let _ = msg;
    Ok(id)
}

async fn ensure_dossier(bot: &Bot, user: &User, app: &App, thread_id: i32) {
    let uid = user.id.0 as i64;
    let saved: Option<(Option<i32>, bool)> =
        sqlx::query_as("SELECT dossier_msg_id,dossier_pinned FROM users WHERE user_id=?")
            .bind(uid)
            .fetch_optional(&app.db)
            .await
            .ok()
            .flatten();
    let dossier_id = if let Some((Some(message_id), pinned)) = saved {
        if pinned {
            return;
        }
        message_id
    } else {
        let text = dossier_html(&app.db, uid).await;
        match bot
            .send_message(ChatId(app.admin_group_id()), text)
            .parse_mode(ParseMode::Html)
            .message_thread_id(ThreadId(MessageId(thread_id)))
            .await
        {
            Ok(message) => {
                if let Err(error) = sqlx::query(
                    "UPDATE users SET dossier_msg_id=?,dossier_pinned=0 WHERE user_id=?",
                )
                .bind(message.id.0)
                .bind(uid)
                .execute(&app.db)
                .await
                {
                    tracing::error!(user_id = uid, %error, "failed to save client dossier message id");
                }
                message.id.0
            }
            Err(error) => {
                tracing::error!(user_id = uid, thread_id, %error, "failed to post client dossier");
                return;
            }
        }
    };

    match bot
        .pin_chat_message(ChatId(app.admin_group_id()), MessageId(dossier_id))
        .await
    {
        Ok(_) => {
            if let Err(error) = sqlx::query("UPDATE users SET dossier_pinned=1 WHERE user_id=?")
                .bind(uid)
                .execute(&app.db)
                .await
            {
                tracing::error!(user_id = uid, %error, "failed to save dossier pin status");
            }
            tracing::info!(
                user_id = uid,
                thread_id,
                dossier_message_id = dossier_id,
                "pinned client dossier in topic"
            );
        }
        Err(error) => {
            tracing::error!(user_id = uid, thread_id, dossier_message_id = dossier_id, %error, "failed to pin client dossier");
            let _ = bot
                .send_message(
                    ChatId(app.admin_group_id()),
                    "Не удалось закрепить карточку клиента. Проверьте право бота «Закрепление сообщений» и повторите попытку новым сообщением пользователя.",
                )
                .message_thread_id(ThreadId(MessageId(thread_id)))
                .await;
        }
    }
}
fn outside_hours(app: &App) -> bool {
    let now = Utc::now().format("%H:%M").to_string();
    now < app.cfg.work_hours_start || now >= app.cfg.work_hours_end
}
async fn should_send_ooh(uid: i64, app: &App) -> bool {
    let ready = sqlx::query_scalar::<_, bool>("SELECT last_out_of_hours_at IS NULL OR last_out_of_hours_at <= datetime('now','-6 hours') FROM users WHERE user_id=?")
        .bind(uid).fetch_one(&app.db).await.unwrap_or(false);
    if ready {
        sqlx::query("UPDATE users SET last_out_of_hours_at=CURRENT_TIMESTAMP WHERE user_id=?")
            .bind(uid)
            .execute(&app.db)
            .await
            .is_ok()
    } else {
        false
    }
}

pub async fn admin(bot: &Bot, msg: &Message, app: &App) -> ResponseResult<()> {
    if msg.from.as_ref().is_some_and(|sender| sender.is_bot) {
        return Ok(());
    }
    let command_name = msg
        .text()
        .and_then(|text| text.split_whitespace().next())
        .map(|command| command.split('@').next().unwrap_or(command));
    if command_name == Some("/chatid") {
        let reply = format!("ID этой группы: <code>{}</code>", msg.chat.id.0);
        let req = bot
            .send_message(msg.chat.id, reply)
            .parse_mode(ParseMode::Html);
        if let Some(thread) = msg.thread_id {
            req.message_thread_id(thread).await?;
        } else {
            req.await?;
        }
        return Ok(());
    }
    if command_name == Some("/addchat") {
        let Some(sender) = msg.from.as_ref() else {
            return Ok(());
        };
        if sender.id.0 as i64 != app.cfg.bot_owner_id {
            bot.send_message(msg.chat.id, "Только владелец бота может подключить группу.")
                .await?;
            return Ok(());
        }
        if !msg.chat.is_supergroup() {
            bot.send_message(msg.chat.id, "Подключить можно только супергруппу с темами.")
                .await?;
            return Ok(());
        }
        let full_chat = match bot.get_chat(msg.chat.id).await {
            Ok(chat) => chat,
            Err(error) => {
                tracing::warn!(%error, "could not verify support group topics");
                bot.send_message(msg.chat.id, "Не удалось проверить настройки группы. Убедитесь, что это супергруппа, включите Topics и повторите /addchat.")
                    .await?;
                return Ok(());
            }
        };
        let has_topics = matches!(
            full_chat.kind,
            ChatKind::Public(ChatPublic {
                kind: PublicChatKind::Supergroup(ref group),
                ..
            }) if group.is_forum
        );
        if !has_topics {
            bot.send_message(
                msg.chat.id,
                "Сначала включите темы (Topics) в настройках супергруппы.",
            )
            .await?;
            return Ok(());
        }
        let bot_id = match bot.get_me().await {
            Ok(me) => me.id,
            Err(error) => {
                tracing::warn!(%error, "could not identify bot for permission check");
                bot.send_message(
                    msg.chat.id,
                    "Не удалось проверить права бота. Проверьте связь и повторите /addchat.",
                )
                .await?;
                return Ok(());
            }
        };
        let missing_permissions = match bot.get_chat_member(msg.chat.id, bot_id).await {
            Ok(member) => match member.kind {
                ChatMemberKind::Administrator(admin) => {
                    let mut missing = Vec::new();
                    if !admin.can_manage_topics {
                        missing.push("Управление темами");
                    }
                    if !admin.can_pin_messages {
                        missing.push("Закрепление сообщений");
                    }
                    if !admin.can_delete_messages {
                        missing.push("Удаление сообщений");
                    }
                    missing
                }
                ChatMemberKind::Owner(_) => Vec::new(),
                _ => vec![
                    "Назначить бота администратором",
                    "Управление темами",
                    "Закрепление сообщений",
                    "Удаление сообщений",
                ],
            },
            Err(error) => {
                tracing::warn!(%error, "could not verify bot permissions");
                vec![
                    "Назначить бота администратором",
                    "Управление темами",
                    "Закрепление сообщений",
                    "Удаление сообщений",
                ]
            }
        };
        if !missing_permissions.is_empty() {
            let checklist = missing_permissions
                .iter()
                .map(|permission| format!("• {permission}"))
                .collect::<Vec<_>>()
                .join("\n");
            bot.send_message(
                msg.chat.id,
                format!("Группа пока не подключена. Выдайте боту права:\n{checklist}\nПосле этого отправьте /addchat ещё раз."),
            )
            .await?;
            return Ok(());
        }
        if let Err(error) = sqlx::query("INSERT INTO settings(key,value) VALUES('admin_group_id',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
            .bind(msg.chat.id.0.to_string())
            .execute(&app.db)
            .await
        {
            tracing::error!(%error, "failed to save admin group");
            bot.send_message(msg.chat.id, "Не удалось сохранить группу в базе данных.")
                .await?;
            return Ok(());
        }
        app.admin_group_id.store(msg.chat.id.0, Ordering::Relaxed);
        if let Err(error) = ensure_help_topic(bot, &app.db, msg.chat.id.0).await {
            tracing::error!(%error, "could not create Help topic");
            bot.send_message(
                msg.chat.id,
                format!("Группа подключена, но топик Help создать не удалось: {error}"),
            )
            .await?;
            return Ok(());
        }
        bot.send_message(msg.chat.id, "Группа подключена как служба поддержки. Пользовательские обращения будут приходить сюда.")
            .await?;
        return Ok(());
    }
    if app.admin_group_id() == 0 || msg.chat.id.0 != app.admin_group_id() {
        return Ok(());
    }
    let Some(tid) = msg.thread_id.map(|x| x.0 .0) else {
        return Ok(());
    };
    let Some(sender) = msg.from.as_ref() else {
        return Ok(());
    };
    if sender.is_bot {
        return Ok(());
    }
    if msg.text().is_some_and(|text| text.starts_with('/')) {
        match bot.get_chat_member(msg.chat.id, sender.id).await {
            Ok(member) if member.kind.is_privileged() => {}
            Ok(_) => {
                bot.send_message(
                    msg.chat.id,
                    "Команды доступны только администраторам группы.",
                )
                .message_thread_id(ThreadId(MessageId(tid)))
                .await?;
                return Ok(());
            }
            Err(error) => {
                tracing::warn!(%error, "could not verify admin command sender");
                return Ok(());
            }
        }
    }
    if tid == 1
        && msg
            .text()
            .is_some_and(|text| text.split_whitespace().next() == Some("/stats"))
    {
        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&app.db)
            .await
            .unwrap_or(0);
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM users WHERE last_active_at>=datetime('now','-1 day')",
        )
        .fetch_one(&app.db)
        .await
        .unwrap_or(0);
        let banned: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_banned=1")
            .fetch_one(&app.db)
            .await
            .unwrap_or(0);
        let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message_map")
            .fetch_one(&app.db)
            .await
            .unwrap_or(0);
        bot.send_message(msg.chat.id, format!("Пользователей: {users}\nАктивных за 24 ч: {active}\nЗабанено: {banned}\nСообщений: {messages}"))
            .message_thread_id(ThreadId(MessageId(tid))).await?;
        return Ok(());
    }
    let uid = sqlx::query_scalar::<_, i64>("SELECT user_id FROM users WHERE thread_id=?")
        .bind(tid)
        .fetch_optional(&app.db)
        .await
        .unwrap_or(None);
    let Some(uid) = uid else { return Ok(()) };
    let text = msg.text().unwrap_or("");
    if text.starts_with('.') || text.starts_with('#') {
        return Ok(());
    }
    if text.starts_with('/') {
        if command(bot, msg, uid, tid, text, app).await? {
            return Ok(());
        }
    }
    let batch = collect_album(msg, app).await;
    if batch.is_empty() {
        return Ok(());
    }
    if batch.len() > 1 {
        let ids = batch.iter().map(|m| m.id).collect::<Vec<_>>();
        match bot.copy_messages(ChatId(uid), msg.chat.id, ids).await {
            Ok(copied) => {
                for (source, destination) in batch.iter().zip(copied) {
                    let _ = db::map_add(&app.db, uid, destination.0, source.id.0, "out").await;
                }
                update_user_status(bot, app, uid, TicketStatus::Answered).await;
            }
            Err(e) => tracing::error!(%e, "failed to copy outbound album"),
        }
        return Ok(());
    }
    let reply = msg.reply_to_message().and_then(|r| Some(r.id.0));
    let reply_id = if let Some(aid) = reply {
        db::user_reply(&app.db, uid, aid).await
    } else {
        None
    };
    let mut req = bot.copy_message(ChatId(uid), msg.chat.id, msg.id);
    if let Some(id) = reply_id {
        req = req.reply_parameters(ReplyParameters::new(MessageId(id)));
    }
    match req.await {
        Ok(sent) => {
            db::map_add(&app.db, uid, sent.0, msg.id.0, "out")
                .await
                .ok();
            update_user_status(bot, app, uid, TicketStatus::Answered).await;
        }
        Err(error) => tracing::error!(user_id = uid, %error, "failed to deliver admin reply"),
    }
    Ok(())
}
async fn command(
    bot: &Bot,
    msg: &Message,
    uid: i64,
    tid: i32,
    text: &str,
    app: &App,
) -> ResponseResult<bool> {
    let mut p = text.splitn(2, ' ');
    let cmd = p
        .next()
        .unwrap_or("")
        .split('@')
        .next()
        .unwrap_or("")
        .trim_start_matches('/');
    let arg = p.next().unwrap_or("").trim();
    let chat = msg.chat.id;
    match cmd {
        "ban" => {
            sqlx::query("UPDATE users SET is_banned=1 WHERE user_id=?")
                .bind(uid)
                .execute(&app.db)
                .await
                .ok();
            bot.send_message(chat, format!("Пользователь заблокирован. {}", arg))
                .message_thread_id(ThreadId(MessageId(tid)))
                .await
                .ok();
            bot.close_forum_topic(chat, ThreadId(MessageId(tid)))
                .await
                .ok();
            update_user_status(bot, app, uid, TicketStatus::Banned).await;
            return Ok(true);
        }
        "unban" => {
            sqlx::query("UPDATE users SET is_banned=0 WHERE user_id=?")
                .bind(uid)
                .execute(&app.db)
                .await
                .ok();
            bot.reopen_forum_topic(chat, ThreadId(MessageId(tid)))
                .await
                .ok();
            bot.send_message(chat, "Блокировка снята.")
                .message_thread_id(ThreadId(MessageId(tid)))
                .await?;
            update_user_status(bot, app, uid, TicketStatus::Waiting).await;
            return Ok(true);
        }
        "close" => {
            bot.send_message(ChatId(uid), "Ваше обращение закрыто.")
                .await
                .ok();
            update_user_status(bot, app, uid, TicketStatus::Closed).await;
            bot.close_forum_topic(chat, ThreadId(MessageId(tid)))
                .await
                .ok();
            sqlx::query(
                "UPDATE users SET thread_id=NULL,dossier_msg_id=NULL,dossier_pinned=0 WHERE user_id=?",
            )
            .bind(uid)
            .execute(&app.db)
            .await
            .ok();
            return Ok(true);
        }
        "note" => {
            sqlx::query("UPDATE users SET admin_note=? WHERE user_id=?")
                .bind(arg)
                .bind(uid)
                .execute(&app.db)
                .await
                .ok();
            let d =
                sqlx::query_scalar::<_, i32>("SELECT dossier_msg_id FROM users WHERE user_id=?")
                    .bind(uid)
                    .fetch_optional(&app.db)
                    .await
                    .ok()
                    .flatten();
            if let Some(d) = d {
                let body = dossier_html(&app.db, uid).await;
                bot.edit_message_text(chat, MessageId(d), body)
                    .parse_mode(ParseMode::Html)
                    .await
                    .ok();
            }
            return Ok(true);
        }
        "tpl_add" => {
            if let Some((slug, content)) = arg.split_once(' ') {
                sqlx::query("INSERT INTO templates(slug,content) VALUES(?,?) ON CONFLICT(slug) DO UPDATE SET content=excluded.content").bind(slug).bind(content).execute(&app.db).await.ok();
                bot.send_message(chat, "Шаблон сохранён.")
                    .message_thread_id(ThreadId(MessageId(tid)))
                    .await?;
            }
            return Ok(true);
        }
        "tpl_del" => {
            sqlx::query("DELETE FROM templates WHERE slug=?")
                .bind(arg)
                .execute(&app.db)
                .await
                .ok();
            return Ok(true);
        }
        "tpl_list" => {
            let rows = sqlx::query("SELECT slug FROM templates ORDER BY slug")
                .fetch_all(&app.db)
                .await
                .unwrap_or_default();
            let s = rows
                .iter()
                .filter_map(|r| r.try_get::<String, _>("slug").ok())
                .collect::<Vec<_>>()
                .join(", ");
            bot.send_message(
                chat,
                if s.is_empty() {
                    "Шаблонов нет.".into()
                } else {
                    s
                },
            )
            .message_thread_id(ThreadId(MessageId(tid)))
            .await?;
            return Ok(true);
        }
        "stats" => {
            let u: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
                .fetch_one(&app.db)
                .await
                .unwrap_or(0);
            let a: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM users WHERE last_active_at>=datetime('now','-1 day')",
            )
            .fetch_one(&app.db)
            .await
            .unwrap_or(0);
            let b: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_banned=1")
                .fetch_one(&app.db)
                .await
                .unwrap_or(0);
            let m: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message_map")
                .fetch_one(&app.db)
                .await
                .unwrap_or(0);
            bot.send_message(
                chat,
                format!("Пользователей: {u}\nАктивных за 24 ч: {a}\nЗабанено: {b}\nСообщений: {m}"),
            )
            .message_thread_id(ThreadId(MessageId(tid)))
            .await?;
            return Ok(true);
        }
        _ => {
            if let Some(body) =
                sqlx::query_scalar::<_, String>("SELECT content FROM templates WHERE slug=?")
                    .bind(cmd)
                    .fetch_optional(&app.db)
                    .await
                    .unwrap_or(None)
            {
                bot.send_message(ChatId(uid), body.clone())
                    .parse_mode(ParseMode::Html)
                    .await
                    .ok();
                update_user_status(bot, app, uid, TicketStatus::Answered).await;
                bot.send_message(chat, body)
                    .message_thread_id(ThreadId(MessageId(tid)))
                    .await?;
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

async fn dossier_html(db: &sqlx::SqlitePool, uid: i64) -> String {
    let row = sqlx::query("SELECT username, first_name, language_code, is_premium, created_at, admin_note FROM users WHERE user_id=?")
        .bind(uid).fetch_optional(db).await.ok().flatten();
    let Some(row) = row else {
        return format!("👤 <b>Карточка клиента</b>\nID: <code>{uid}</code>");
    };
    let username: Option<String> = row.try_get("username").ok().flatten();
    let first_name: String = row.try_get("first_name").unwrap_or_default();
    let language: Option<String> = row.try_get("language_code").ok().flatten();
    let premium: bool = row.try_get("is_premium").unwrap_or(false);
    let created_at: String = row.try_get("created_at").unwrap_or_default();
    let note: Option<String> = row.try_get("admin_note").ok().flatten();
    let username = username
        .map(|x| format!("@{x}"))
        .unwrap_or_else(|| "—".into());
    let language = language.unwrap_or_else(|| "—".into());
    let created_at = created_at.replace('T', " ");
    let note = note
        .filter(|x| !x.trim().is_empty())
        .unwrap_or_else(|| "нет".into());
    format!("👤 <b>Карточка клиента</b>\nID: <code>{uid}</code>\nИмя: {}\nUsername: {}\nЯзык: {} | Premium: {}\nПервое обращение: {} UTC\n📝 Заметка: <i>{}</i>", html_escape(&first_name), html_escape(&username), html_escape(&language), if premium {"Да"} else {"Нет"}, html_escape(&created_at), html_escape(&note))
}
