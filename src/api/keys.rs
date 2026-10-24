use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::http_types::{Request, Response};

use super::{
    authenticated_user_id, parse_json_body, unauthorized_response, verify_current_password,
};

#[derive(Deserialize)]
struct CreateKey {
    name: String,
    current_password: String,
}

struct KeyRecord {
    id: i64,
    name: String,
    key_id: String,
    created_at: i64,
    last_used_at: Option<i64>,
    revoked_at: Option<i64>,
}

impl KeyRecord {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            key_id: row.get(2)?,
            created_at: row.get(3)?,
            last_used_at: row.get(4)?,
            revoked_at: row.get(5)?,
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "key_id": self.key_id,
            "created_at": rfc3339(self.created_at),
            "last_used_at": self.last_used_at.map(rfc3339),
            "revoked_at": self.revoked_at.map(rfc3339),
            "status": if self.revoked_at.is_some() { "revoked" } else { "active" },
        })
    }
}

pub(super) fn matches_path(path: &str) -> bool {
    path == "/api/keys" || path.starts_with("/api/keys/")
}

pub(super) fn handle(request: &Request, database: &Connection) -> rusqlite::Result<Response> {
    let Some(user_id) = authenticated_user_id(request, database)? else {
        return Ok(unauthorized_response());
    };

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/keys") => list_keys(database, user_id),
        ("POST", "/api/keys") => create_key(request, database, user_id),
        ("DELETE", path) if path.starts_with("/api/keys/") => {
            let Some(key_id) = key_row_id(path) else {
                return Ok(not_found_response());
            };
            revoke_key(database, user_id, key_id)
        }
        (_, "/api/keys") => Ok(method_not_allowed_response()),
        (_, path) if path.starts_with("/api/keys/") => Ok(method_not_allowed_response()),
        _ => Ok(not_found_response()),
    }
}

fn list_keys(database: &Connection, user_id: i64) -> rusqlite::Result<Response> {
    let mut statement = database.prepare(
        "SELECT id, name, key_id, created_at, last_used_at, revoked_at
         FROM api_keys
         WHERE user_id = ?1
         ORDER BY created_at DESC, id DESC",
    )?;
    let keys = statement
        .query_map(params![user_id], KeyRecord::from_row)?
        .map(|record| record.map(|record| record.to_json()))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(no_store(Response::json("200 OK", json!({"keys": keys}))))
}

fn create_key(
    request: &Request,
    database: &Connection,
    user_id: i64,
) -> rusqlite::Result<Response> {
    if !has_json_content_type(request) {
        return Ok(Response::json(
            "415 Unsupported Media Type",
            json!({"error": "请求内容必须是 application/json。", "code": "json_required"}),
        ));
    }
    let input = match parse_json_body::<CreateKey>(request) {
        Ok(input) => input,
        Err(response) => return Ok(response),
    };

    let name = input.name.trim();
    if !valid_key_name(name) {
        return Ok(Response::json(
            "400 Bad Request",
            json!({"error": "密钥名称需为 1 到 64 个字符，且不能包含控制字符。", "code": "invalid_key_name"}),
        ));
    }
    if !verify_current_password(database, user_id, &input.current_password)? {
        return Ok(Response::json(
            "401 Unauthorized",
            json!({"error": "当前密码不正确。", "code": "invalid_password"}),
        ));
    }

    let generated = crate::auth::generate_key();
    let created_at = crate::auth::unix_time();
    database.execute(
        "INSERT INTO api_keys (user_id, name, key_id, secret_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            user_id,
            name,
            generated.key_id,
            generated.secret_hash.as_slice(),
            created_at
        ],
    )?;
    let id = database.last_insert_rowid();

    let key = KeyRecord {
        id,
        name: name.to_owned(),
        key_id: generated.key_id,
        created_at,
        last_used_at: None,
        revoked_at: None,
    };
    Ok(no_store(Response::json(
        "201 Created",
        json!({
            "key": key.to_json(),
            "token": generated.token,
            "warning": "这是唯一一次显示完整密钥，请立即保存；该密钥永不过期，只能撤销。",
        }),
    )))
}

fn revoke_key(database: &Connection, user_id: i64, key_id: i64) -> rusqlite::Result<Response> {
    let mut record = database
        .query_row(
            "SELECT id, name, key_id, created_at, last_used_at, revoked_at
             FROM api_keys
             WHERE id = ?1 AND user_id = ?2",
            params![key_id, user_id],
            KeyRecord::from_row,
        )
        .optional()?;
    let Some(record_ref) = record.as_mut() else {
        return Ok(not_found_response());
    };

    if record_ref.revoked_at.is_none() {
        let revoked_at = crate::auth::unix_time();
        database.execute(
            "UPDATE api_keys SET revoked_at = ?1 WHERE id = ?2 AND user_id = ?3",
            params![revoked_at, key_id, user_id],
        )?;
        record_ref.revoked_at = Some(revoked_at);
    }

    Ok(no_store(Response::json(
        "200 OK",
        json!({"key": record_ref.to_json()}),
    )))
}

fn key_row_id(path: &str) -> Option<i64> {
    let segments = path.split('/').collect::<Vec<_>>();
    let ["", "api", "keys", raw_id] = segments.as_slice() else {
        return None;
    };
    raw_id.parse().ok()
}

fn has_json_content_type(request: &Request) -> bool {
    request
        .headers
        .get("content-type")
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn valid_key_name(name: &str) -> bool {
    let length = name.chars().count();
    (1..=64).contains(&length) && !name.chars().any(char::is_control)
}

fn rfc3339(timestamp: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)
        .map(|value| value.to_rfc3339())
        .unwrap_or_default()
}

fn no_store(mut response: Response) -> Response {
    response.headers.push("Cache-Control: no-store".into());
    response
}

fn not_found_response() -> Response {
    Response::json(
        "404 Not Found",
        json!({"error": "密钥不存在。", "code": "key_not_found"}),
    )
}

fn method_not_allowed_response() -> Response {
    Response::json(
        "405 Method Not Allowed",
        json!({"error": "请求方法不支持。", "code": "method_not_allowed"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn database() -> Connection {
        let mut database = Connection::open_in_memory().unwrap();
        database.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::migrations::migrate(&mut database).unwrap();
        database
    }

    fn session_request(method: &str, path: &str, body: &[u8]) -> Request {
        Request {
            method: method.into(),
            target: path.into(),
            path: path.into(),
            headers: HashMap::from([
                ("cookie".into(), "formicary_session=test-session".into()),
                ("content-type".into(), "application/json".into()),
            ]),
            body: body.to_vec(),
        }
    }

    #[test]
    fn creates_lists_and_revokes_a_key() {
        let database = database();
        let password_hash = password_auth::generate_hash("correct horse");
        database
            .execute(
                "INSERT INTO users (id, username, password_hash, created_at) VALUES (1, 'admin', ?1, 0)",
                params![password_hash],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO sessions (token, user_id, expires_at) VALUES ('test-session', 1, 9999999999)",
                [],
            )
            .unwrap();

        let create = session_request(
            "POST",
            "/api/keys",
            br#"{"name":"ci","current_password":"correct horse"}"#,
        );
        let response = handle(&create, &database).unwrap();
        assert_eq!(response.status, "201 Created");
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        let token = body["token"].as_str().unwrap();
        assert!(token.starts_with("fmk_"));

        let list = session_request("GET", "/api/keys", &[]);
        let response = handle(&list, &database).unwrap();
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["keys"].as_array().unwrap().len(), 1);
        assert!(body["keys"][0].get("token").is_none());
        assert_eq!(body["keys"][0]["status"], "active");

        assert!(matches!(
            crate::auth::authenticate_key(
                &Request {
                    method: "GET".into(),
                    target: "/api/me".into(),
                    path: "/api/me".into(),
                    headers: HashMap::from([("authorization".into(), format!("Bearer {token}"))]),
                    body: Vec::new(),
                },
                &database,
            )
            .unwrap(),
            crate::auth::KeyAuthentication::Valid(_)
        ));

        let revoke = session_request("DELETE", "/api/keys/1", &[]);
        let response = handle(&revoke, &database).unwrap();
        assert_eq!(response.status, "200 OK");
        let body: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(body["key"]["status"], "revoked");

        assert!(matches!(
            crate::auth::authenticate_key(
                &Request {
                    method: "GET".into(),
                    target: "/api/me".into(),
                    path: "/api/me".into(),
                    headers: HashMap::from([("authorization".into(), format!("Bearer {token}"))]),
                    body: Vec::new(),
                },
                &database,
            )
            .unwrap(),
            crate::auth::KeyAuthentication::Invalid
        ));
    }

    #[test]
    fn requires_a_session_and_a_valid_password() {
        let database = database();
        let missing = Request {
            method: "GET".into(),
            target: "/api/keys".into(),
            path: "/api/keys".into(),
            headers: HashMap::new(),
            body: Vec::new(),
        };
        assert_eq!(
            handle(&missing, &database).unwrap().status,
            "401 Unauthorized"
        );

        let password_hash = password_auth::generate_hash("right");
        database
            .execute(
                "INSERT INTO users (id, username, password_hash, created_at) VALUES (1, 'admin', ?1, 0)",
                params![password_hash],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO sessions (token, user_id, expires_at) VALUES ('test-session', 1, 9999999999)",
                [],
            )
            .unwrap();

        let wrong = session_request(
            "POST",
            "/api/keys",
            br#"{"name":"ci","current_password":"wrong"}"#,
        );
        assert_eq!(
            handle(&wrong, &database).unwrap().status,
            "401 Unauthorized"
        );
    }
}
