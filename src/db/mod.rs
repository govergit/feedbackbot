use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use teloxide::types::User;
pub async fn connect(url: &str) -> Result<SqlitePool, sqlx::Error> {
    if let Some(path) = url
        .strip_prefix("sqlite://")
        .and_then(|x| x.split('?').next())
    {
        if path != ":memory:" {
            if let Some(p) = std::path::Path::new(path).parent() {
                std::fs::create_dir_all(p).ok();
            }
        }
    }
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
pub async fn ensure_user(db: &SqlitePool, u: &User) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO users(user_id,username,first_name,language_code,is_premium) VALUES(?,?,?,?,?) ON CONFLICT(user_id) DO UPDATE SET username=excluded.username,first_name=excluded.first_name,language_code=excluded.language_code,is_premium=excluded.is_premium,last_active_at=CURRENT_TIMESTAMP")
 .bind(u.id.0 as i64).bind(&u.username).bind(&u.first_name).bind(&u.language_code).bind(u.is_premium).execute(db).await?;
    Ok(())
}
pub async fn banned(db: &SqlitePool, id: i64) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT is_banned FROM users WHERE user_id=?")
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap_or(false)
}
pub async fn thread(db: &SqlitePool, id: i64) -> Option<i32> {
    sqlx::query_scalar::<_, Option<i32>>("SELECT thread_id FROM users WHERE user_id=?")
        .bind(id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .flatten()
}
pub async fn set_thread(db: &SqlitePool, id: i64, t: i32) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET thread_id=? WHERE user_id=?")
        .bind(t)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}
pub async fn map_add(
    db: &SqlitePool,
    uid: i64,
    um: i32,
    am: i32,
    d: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO message_map(user_id,user_msg_id,admin_msg_id,direction) VALUES(?,?,?,?)",
    )
    .bind(uid)
    .bind(um)
    .bind(am)
    .bind(d)
    .execute(db)
    .await?;
    Ok(())
}
pub async fn user_reply(db: &SqlitePool, uid: i64, admin_msg: i32) -> Option<i32> {
    sqlx::query_scalar("SELECT user_msg_id FROM message_map WHERE user_id=? AND admin_msg_id=? AND direction='in' ORDER BY id DESC LIMIT 1").bind(uid).bind(admin_msg).fetch_optional(db).await.ok().flatten()
}
pub async fn admin_reply(db: &SqlitePool, user_id: i64, user_msg: i32) -> Option<i32> {
    sqlx::query_scalar(
        "SELECT admin_msg_id FROM message_map WHERE user_id=? AND user_msg_id=? AND direction='out' ORDER BY id DESC LIMIT 1",
    )
    .bind(user_id)
    .bind(user_msg)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_create_all_required_tables() {
        let db = connect("sqlite::memory:").await.expect("SQLite migrations");
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('users','message_map','templates','settings','captcha_completions')",
        )
        .fetch_one(&db)
        .await
        .expect("schema query");
        assert_eq!(count, 5);

        sqlx::query("INSERT INTO users(user_id,first_name,thread_id) VALUES(1,'Test',NULL)")
            .execute(&db)
            .await
            .expect("insert user with no topic");
        assert_eq!(thread(&db, 1).await, None);
        sqlx::query("UPDATE users SET captcha_question='2 + 3 = ?',captcha_answer=5,captcha_options='5,7,8',captcha_id='test-1',captcha_expires_at=datetime('now','+15 minutes') WHERE user_id=1")
            .execute(&db)
            .await
            .expect("persist captcha challenge");
        let challenge: Option<(String, i32, String, String)> = sqlx::query_as(
            "SELECT captcha_question,captcha_answer,captcha_options,captcha_id FROM users WHERE user_id=1 AND captcha_expires_at>datetime('now')",
        )
        .fetch_optional(&db)
        .await
        .expect("load persisted captcha");
        assert_eq!(
            challenge,
            Some(("2 + 3 = ?".into(), 5, "5,7,8".into(), "test-1".into()))
        );
        sqlx::query("INSERT INTO captcha_completions(user_id) VALUES(1)")
            .execute(&db)
            .await
            .expect("persist completed captcha");
        let completed: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM captcha_completions WHERE user_id=?)")
                .bind(1_i64)
                .fetch_one(&db)
                .await
                .expect("load captcha completion");
        assert!(completed);
        let is_pinned: bool =
            sqlx::query_scalar("SELECT dossier_pinned FROM users WHERE user_id=1")
                .fetch_one(&db)
                .await
                .expect("read default dossier pin status");
        assert!(!is_pinned);
        set_thread(&db, 1, 123).await.expect("save topic id");
        assert_eq!(thread(&db, 1).await, Some(123));
    }
}
