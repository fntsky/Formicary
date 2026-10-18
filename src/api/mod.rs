use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Deserialize;
use serde_json::json;

use crate::http::{Request, Response};

const SESSION_TTL_SECONDS: u64 = 60 * 60 * 24;
const SESSION_COOKIE: &str = "formicary_session";

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

pub fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

pub fn cleanup_expired_sessions(database: &Connection) -> rusqlite::Result<()> {
    database.execute(
        "DELETE FROM sessions WHERE expires_at <= ?1",
        params![unix_time()],
    )?;
    Ok(())
}

pub fn user_count(database: &Connection) -> rusqlite::Result<i64> {
    database.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
}

pub fn create_initial_account(
    database: &mut Connection,
) -> rusqlite::Result<Option<(String, String)>> {
    let transaction = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let count: i64 = transaction.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?;
    if count > 0 {
        transaction.commit()?;
        return Ok(None);
    }

    let username = "admin".to_owned();
    let password = initial_password();
    let password_hash = password_auth::generate_hash(&password);
    transaction.execute(
        "INSERT INTO users (username, password_hash, created_at) VALUES (?1, ?2, ?3)",
        params![username, password_hash, unix_time()],
    )?;
    transaction.commit()?;
    Ok(Some((username, password)))
}

pub fn handle(request: &Request, database: &Connection) -> rusqlite::Result<Response> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api") => Ok(Response::json(
            "200 OK",
            json!({
                "service": "Formicary API",
                "endpoints": ["/api/register", "/api/login", "/api/me", "/api/logout"]
            }),
        )),
        ("POST", "/api/register") => register(request, database),
        ("POST", "/api/login") => login(request, database),
        ("GET", "/api/me") => current_user(request, database),
        ("POST", "/api/logout") => logout(request, database),
        (_, "/api/register" | "/api/login" | "/api/me" | "/api/logout") => Ok(Response::json(
            "405 Method Not Allowed",
            json!({"error": "请求方法不支持。"}),
        )),
        _ => Ok(Response::json(
            "404 Not Found",
            json!({"error": "API 端点不存在。"}),
        )),
    }
}

fn register(request: &Request, database: &Connection) -> rusqlite::Result<Response> {
    let credentials = match parse_credentials(request) {
        Ok(credentials) => credentials,
        Err(response) => return Ok(response),
    };

    if !valid_username(&credentials.username) {
        return Ok(Response::json(
            "400 Bad Request",
            json!({"error": "用户名需为 3 到 32 位字母、数字、下划线、点或短横线。"}),
        ));
    }
    let password_length = credentials.password.chars().count();
    if !(8..=128).contains(&password_length) {
        return Ok(Response::json(
            "400 Bad Request",
            json!({"error": "密码长度需为 8 到 128 个字符。"}),
        ));
    }

    let password_hash = password_auth::generate_hash(&credentials.password);
    let inserted = database.execute(
        "INSERT INTO users (username, password_hash, created_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(username) DO NOTHING",
        params![credentials.username, password_hash, unix_time()],
    )?;
    if inserted == 0 {
        return Ok(Response::json(
            "409 Conflict",
            json!({"error": "这个用户名已被注册。"}),
        ));
    }

    let user_id = database.last_insert_rowid();
    create_session_response(database, user_id, &credentials.username, "201 Created")
}

fn login(request: &Request, database: &Connection) -> rusqlite::Result<Response> {
    let credentials = match parse_credentials(request) {
        Ok(credentials) => credentials,
        Err(response) => return Ok(response),
    };

    let user = database
        .query_row(
            "SELECT id, password_hash FROM users WHERE username = ?1",
            params![credentials.username],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;

    match user {
        Some((user_id, password_hash))
            if password_auth::verify_password(&credentials.password, &password_hash).is_ok() =>
        {
            create_session_response(database, user_id, &credentials.username, "200 OK")
        }
        _ => Ok(Response::json(
            "401 Unauthorized",
            json!({"error": "用户名或密码不正确。"}),
        )),
    }
}

fn current_user(request: &Request, database: &Connection) -> rusqlite::Result<Response> {
    let Some(token) = session_token(request) else {
        return Ok(unauthorized_response());
    };
    let user = database
        .query_row(
            "SELECT users.username
             FROM sessions JOIN users ON users.id = sessions.user_id
             WHERE sessions.token = ?1 AND sessions.expires_at > ?2",
            params![token, unix_time()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;

    match user {
        Some(username) => Ok(Response::json(
            "200 OK",
            json!({"authenticated": true, "username": username}),
        )),
        None => Ok(unauthorized_response()),
    }
}

fn logout(request: &Request, database: &Connection) -> rusqlite::Result<Response> {
    if let Some(token) = session_token(request) {
        database.execute("DELETE FROM sessions WHERE token = ?1", params![token])?;
    }

    let mut response = Response::json("200 OK", json!({"ok": true}));
    response.headers.push(format!(
        "Set-Cookie: {SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT"
    ));
    Ok(response)
}

fn create_session_response(
    database: &Connection,
    user_id: i64,
    username: &str,
    status: &'static str,
) -> rusqlite::Result<Response> {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let now = unix_time();
    database.execute("DELETE FROM sessions WHERE expires_at <= ?1", params![now])?;
    let expires_at = now + SESSION_TTL_SECONDS as i64;
    database.execute(
        "INSERT INTO sessions (token, user_id, expires_at) VALUES (?1, ?2, ?3)",
        params![token, user_id, expires_at],
    )?;

    let mut response = Response::json(status, json!({"ok": true, "username": username}));
    response.headers.push(format!(
        "Set-Cookie: {SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_TTL_SECONDS}"
    ));
    Ok(response)
}

fn parse_credentials(request: &Request) -> Result<Credentials, Response> {
    if request.body.is_empty() {
        return Err(Response::json(
            "400 Bad Request",
            json!({"error": "请填写用户名和密码。"}),
        ));
    }

    serde_json::from_slice(&request.body).map_err(|_| {
        Response::json(
            "400 Bad Request",
            json!({"error": "请求内容必须是有效的 JSON。"}),
        )
    })
}

fn valid_username(username: &str) -> bool {
    (3..=32).contains(&username.len())
        && username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn session_token(request: &Request) -> Option<&str> {
    request
        .headers
        .get("cookie")?
        .split(';')
        .find_map(|cookie| {
            let (name, value) = cookie.trim().split_once('=')?;
            (name == SESSION_COOKIE && !value.is_empty()).then_some(value)
        })
}

fn unauthorized_response() -> Response {
    Response::json("401 Unauthorized", json!({"error": "尚未登录。"}))
}

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn initial_password() -> String {
    let bytes = *uuid::Uuid::new_v4().as_bytes();
    let random_value = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    (random_value % 900_000 + 100_000).to_string()
}
