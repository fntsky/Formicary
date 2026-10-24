use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::http_types::Request;

const KEY_PREFIX: &str = "fmk_";
const KEY_ID_BYTES: usize = 12;
const SECRET_BYTES: usize = 32;
const KEY_ID_HEX_LEN: usize = KEY_ID_BYTES * 2;
const SECRET_HEX_LEN: usize = SECRET_BYTES * 2;
const LAST_USED_UPDATE_INTERVAL: i64 = 60;

/// A successfully verified API key together with the owning account.
#[derive(Debug, Clone)]
pub(crate) struct ApiKeyIdentity {
    pub id: i64,
    pub username: String,
    pub name: String,
    pub key_id: String,
}

/// Result of checking the explicit API key credentials on a request.
pub(crate) enum KeyAuthentication {
    Absent,
    Invalid,
    Valid(ApiKeyIdentity),
}

pub(crate) struct GeneratedKey {
    pub token: String,
    pub key_id: String,
    pub secret_hash: [u8; SECRET_BYTES],
}

/// Generates a 256-bit Formicary key. Only the hash is stored; the token is
/// returned to the caller once and never persisted.
pub(crate) fn generate_key() -> GeneratedKey {
    let id_bytes = *uuid::Uuid::new_v4().as_bytes();
    let key_id = hex_encode(&id_bytes[..KEY_ID_BYTES]);

    let mut secret = [0u8; SECRET_BYTES];
    secret[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    secret[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());

    let token = format!("{KEY_PREFIX}{key_id}_{}", hex_encode(&secret));
    let secret_hash = hash_secret(&key_id, &secret);

    GeneratedKey {
        token,
        key_id,
        secret_hash,
    }
}

pub(crate) fn hash_secret(key_id: &str, secret: &[u8]) -> [u8; SECRET_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(b"formicary.api-key.v1");
    hasher.update([0u8]);
    hasher.update(key_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(secret);
    hasher.finalize().into()
}

/// Checks the explicit API key credentials on a request.
///
/// Missing credentials return `Absent`; malformed, revoked, or unknown
/// credentials return `Invalid`. Callers decide whether `Absent` is allowed.
pub(crate) fn authenticate_key(
    request: &Request,
    database: &Connection,
) -> rusqlite::Result<KeyAuthentication> {
    let token = match explicit_key(request) {
        Ok(Some(token)) => token,
        Ok(None) => return Ok(KeyAuthentication::Absent),
        Err(()) => return Ok(KeyAuthentication::Invalid),
    };

    let Some(parsed) = parse_token(token) else {
        return Ok(KeyAuthentication::Invalid);
    };

    let row = database
        .query_row(
            "SELECT api_keys.id,
                    users.username,
                    api_keys.name,
                    api_keys.key_id,
                    api_keys.secret_hash,
                    api_keys.revoked_at
             FROM api_keys
             JOIN users ON users.id = api_keys.user_id
             WHERE api_keys.key_id = ?1",
            params![parsed.key_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .optional()?;

    let Some((id, username, name, key_id, stored_hash, revoked_at)) = row else {
        return Ok(KeyAuthentication::Invalid);
    };
    if revoked_at.is_some() {
        return Ok(KeyAuthentication::Invalid);
    }
    let Ok(stored_hash) = <[u8; SECRET_BYTES]>::try_from(stored_hash) else {
        return Ok(KeyAuthentication::Invalid);
    };

    let computed_hash = hash_secret(&key_id, &parsed.secret);
    if computed_hash.ct_eq(&stored_hash).unwrap_u8() != 1 {
        return Ok(KeyAuthentication::Invalid);
    }

    let now = unix_time();
    if let Err(error) = database.execute(
        "UPDATE api_keys
         SET last_used_at = ?1
         WHERE key_id = ?2
           AND (last_used_at IS NULL OR last_used_at <= ?3)",
        params![now, key_id, now - LAST_USED_UPDATE_INTERVAL],
    ) {
        eprintln!("Could not update API key usage: {error}");
    }

    Ok(KeyAuthentication::Valid(ApiKeyIdentity {
        id,
        username,
        name,
        key_id,
    }))
}

fn explicit_key(request: &Request) -> Result<Option<&str>, ()> {
    let authorization = request
        .headers
        .get("authorization")
        .and_then(|value| parse_bearer(value));
    let api_key_header = request.headers.get("x-api-key").map(|value| value.trim());

    match (authorization, api_key_header) {
        (Some(authorization), Some(api_key)) => {
            if api_key.is_empty() || authorization != api_key {
                Err(())
            } else {
                Ok(Some(authorization))
            }
        }
        (Some(authorization), None) => Ok(Some(authorization)),
        (None, Some(api_key)) => {
            if api_key.is_empty() {
                Err(())
            } else {
                Ok(Some(api_key))
            }
        }
        (None, None) => Ok(None),
    }
}

/// Only `Bearer fmk_...` is treated as a Formicary key. Other Bearer tokens
/// belong to an upstream service or reverse proxy and are ignored here.
fn parse_bearer(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty() && token.starts_with(KEY_PREFIX)).then_some(token)
}

struct ParsedKey {
    key_id: String,
    secret: [u8; SECRET_BYTES],
}

fn parse_token(token: &str) -> Option<ParsedKey> {
    let remainder = token.strip_prefix(KEY_PREFIX)?;
    let (key_id, secret_hex) = remainder.split_once('_')?;
    if key_id.len() != KEY_ID_HEX_LEN || secret_hex.len() != SECRET_HEX_LEN {
        return None;
    }
    if !is_hex(key_id) || !is_hex(secret_hex) {
        return None;
    }

    let mut secret = [0u8; SECRET_BYTES];
    for (index, byte) in secret.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&secret_hex[index * 2..index * 2 + 2], 16).ok()?;
    }

    Some(ParsedKey {
        key_id: key_id.to_ascii_lowercase(),
        secret,
    })
}

fn is_hex(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

pub(crate) fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn request_with_authorization(token: &str) -> Request {
        Request {
            method: "GET".into(),
            target: "/api/me".into(),
            path: "/api/me".into(),
            headers: HashMap::from([("authorization".into(), format!("Bearer {token}"))]),
            body: Vec::new(),
        }
    }

    #[test]
    fn generated_key_round_trips_through_the_token_format() {
        let generated = generate_key();
        let parsed = parse_token(&generated.token).expect("generated token must parse");

        assert_eq!(parsed.key_id, generated.key_id);
        assert_eq!(
            hash_secret(&parsed.key_id, &parsed.secret),
            generated.secret_hash
        );
        assert!(generated.token.starts_with("fmk_"));
    }

    #[test]
    fn rejects_malformed_tokens() {
        for token in [
            "",
            "fmk_",
            "fmk_short_short",
            "fmk_zzzzzzzzzzzzzzzzzzzzzzzz_0000000000000000000000000000000000000000000000000000000000000000",
            "fmk_000000000000000000000000_000000000000000000000000000000000000000000000000000000000000000",
            "other_000000000000000000000000_0000000000000000000000000000000000000000000000000000000000000000",
        ] {
            assert!(
                parse_token(token).is_none(),
                "token should be rejected: {token:?}"
            );
        }
    }

    #[test]
    fn hash_binds_key_id_to_secret() {
        let secret = [7u8; SECRET_BYTES];
        assert_ne!(hash_secret("a", &secret), hash_secret("b", &secret));
    }

    #[test]
    fn authenticates_valid_key_and_tracks_usage() {
        let mut database = Connection::open_in_memory().unwrap();
        database.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::migrations::migrate(&mut database).unwrap();
        database
            .execute(
                "INSERT INTO users (id, username, password_hash, created_at) VALUES (1, 'admin', 'x', 0)",
                [],
            )
            .unwrap();

        let generated = generate_key();
        database
            .execute(
                "INSERT INTO api_keys (user_id, name, key_id, secret_hash, created_at)
                 VALUES (1, 'ci', ?1, ?2, 0)",
                params![generated.key_id, generated.secret_hash.as_slice()],
            )
            .unwrap();

        let request = request_with_authorization(&generated.token);
        let KeyAuthentication::Valid(identity) = authenticate_key(&request, &database).unwrap()
        else {
            panic!("expected a valid key");
        };
        assert_eq!(identity.username, "admin");
        assert_eq!(identity.key_id, generated.key_id);

        let last_used: Option<i64> = database
            .query_row(
                "SELECT last_used_at FROM api_keys WHERE key_id = ?1",
                params![generated.key_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(last_used.is_some());

        database
            .execute(
                "UPDATE api_keys SET revoked_at = 1 WHERE key_id = ?1",
                params![generated.key_id],
            )
            .unwrap();
        assert!(matches!(
            authenticate_key(&request, &database).unwrap(),
            KeyAuthentication::Invalid
        ));
    }

    #[test]
    fn distinguishes_missing_and_invalid_credentials() {
        let mut database = Connection::open_in_memory().unwrap();
        database.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::migrations::migrate(&mut database).unwrap();

        let missing = Request {
            method: "GET".into(),
            target: "/api/me".into(),
            path: "/api/me".into(),
            headers: HashMap::new(),
            body: Vec::new(),
        };
        assert!(matches!(
            authenticate_key(&missing, &database).unwrap(),
            KeyAuthentication::Absent
        ));

        let malformed = request_with_authorization("fmk_bad");
        assert!(matches!(
            authenticate_key(&malformed, &database).unwrap(),
            KeyAuthentication::Invalid
        ));
    }
}
