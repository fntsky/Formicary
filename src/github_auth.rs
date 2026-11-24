use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const CLIENT_ID_ENV: &str = "FORMICARY_GITHUB_CLIENT_ID";
const VERIFICATION_URL: &str = "https://github.com/login/device";
const REQUEST_FAILED: &str = "无法连接 GitHub 完成授权，请稍后重试。";

pub(crate) struct GithubAuth {
    client_id: Option<String>,
    sessions: HashMap<String, Authorization>,
}

impl fmt::Debug for GithubAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("GithubAuth").finish_non_exhaustive()
    }
}

impl Default for GithubAuth {
    fn default() -> Self {
        Self {
            client_id: std::env::var(CLIENT_ID_ENV)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            sessions: HashMap::new(),
        }
    }
}

enum Authorization {
    Pending(DeviceAuthorization),
    Connected(Account),
}

impl Authorization {
    fn expires_at(&self) -> Instant {
        match self {
            Self::Pending(pending) => pending.expires_at,
            Self::Connected(account) => account.expires_at,
        }
    }
}

struct DeviceAuthorization {
    device_code: String,
    user_code: String,
    expires_at: Instant,
    interval: Duration,
    next_poll: Instant,
}

struct Account {
    login: String,
    token: String,
    expires_at: Instant,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum Status<'a> {
    Unconfigured,
    Disconnected,
    Pending {
        user_code: &'a str,
        verification_url: &'static str,
        retry_after: u64,
    },
    Connected {
        login: &'a str,
        expires_in: u64,
    },
}

impl GithubAuth {
    pub(crate) fn status(&mut self, session: &str) -> Status<'_> {
        let now = Instant::now();
        self.sessions.retain(|_, entry| entry.expires_at() > now);
        match self.sessions.get(session) {
            Some(Authorization::Pending(pending)) => Status::Pending {
                user_code: &pending.user_code,
                verification_url: VERIFICATION_URL,
                retry_after: pending
                    .next_poll
                    .min(pending.expires_at)
                    .saturating_duration_since(now)
                    .as_millis()
                    .div_ceil(1000) as u64,
            },
            Some(Authorization::Connected(account)) => Status::Connected {
                login: &account.login,
                expires_in: account
                    .expires_at
                    .duration_since(now)
                    .as_millis()
                    .div_ceil(1000) as u64,
            },
            None if self.client_id.is_none() => Status::Unconfigured,
            None => Status::Disconnected,
        }
    }

    pub(crate) fn start(&mut self, session: &str) -> Result<Status<'_>, &'static str> {
        if matches!(self.status(session), Status::Disconnected) {
            let client_id = self.client_id.as_deref().unwrap();
            let pending = request_device_code(&client()?, client_id)?;
            self.sessions
                .insert(session.to_owned(), Authorization::Pending(pending));
        }
        Ok(self.status(session))
    }

    pub(crate) fn poll(&mut self, session: &str) -> Result<Status<'_>, &'static str> {
        if matches!(self.status(session), Status::Pending { retry_after: 0, .. })
            && let Some(Authorization::Pending(pending)) = self.sessions.get_mut(session)
        {
            let client_id = self.client_id.as_deref().unwrap();
            let client = client()?;
            pending.next_poll = Instant::now() + pending.interval;
            let exchange = exchange_device_code(&client, client_id, &pending.device_code)?;
            match exchange {
                Exchange::Pending => {}
                Exchange::SlowDown { interval } => {
                    pending.interval = (pending.interval + Duration::from_secs(5))
                        .max(Duration::from_secs(u64::from(interval.unwrap_or(0))));
                    pending.next_poll = Instant::now() + pending.interval;
                }
                Exchange::Rejected(error) => {
                    self.disconnect(session);
                    return Err(oauth_error(&error));
                }
                Exchange::Authorized {
                    access_token,
                    expires_in,
                } => {
                    self.disconnect(session);
                    let expires_at = Instant::now() + Duration::from_secs(u64::from(expires_in));
                    if expires_in == 0 || !access_token.starts_with("ghu_") {
                        return Err("请使用启用了设备流的 GitHub App，并开启用户令牌过期。");
                    }
                    let login = account_login(&client, &access_token)?;
                    self.sessions.insert(
                        session.to_owned(),
                        Authorization::Connected(Account {
                            login,
                            token: access_token,
                            expires_at,
                        }),
                    );
                }
            }
        }
        Ok(self.status(session))
    }

    pub(crate) fn token(&mut self, session: &str) -> Option<String> {
        if !matches!(self.status(session), Status::Connected { .. }) {
            return None;
        }
        match self.sessions.get(session) {
            Some(Authorization::Connected(account)) => Some(account.token.clone()),
            _ => None,
        }
    }

    pub(crate) fn disconnect(&mut self, session: &str) {
        self.sessions.remove(session);
    }

    pub(crate) fn clear(&mut self) {
        self.sessions.clear();
    }
}

fn client() -> Result<Client, &'static str> {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Formicary")
        .build()
        .map_err(|_| REQUEST_FAILED)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DeviceCodeResponse {
    Code {
        device_code: String,
        user_code: String,
        expires_in: u32,
        interval: u32,
    },
    Error {
        error: String,
    },
}

fn request_device_code(
    client: &Client,
    client_id: &str,
) -> Result<DeviceAuthorization, &'static str> {
    let response = client
        .post("https://github.com/login/device/code")
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[("client_id", client_id), ("scope", "offline_access")])
        .send()
        .map_err(|_| REQUEST_FAILED)?;
    match read_json(response)? {
        DeviceCodeResponse::Error { error } => Err(oauth_error(&error)),
        DeviceCodeResponse::Code {
            device_code,
            user_code,
            expires_in,
            interval,
        } => {
            let now = Instant::now();
            let interval = Duration::from_secs(u64::from(interval.max(1)));
            Ok(DeviceAuthorization {
                device_code,
                user_code,
                expires_at: now + Duration::from_secs(u64::from(expires_in)),
                interval,
                next_poll: now + interval,
            })
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TokenResponse {
    Error {
        error: String,
        interval: Option<u32>,
    },
    Token {
        access_token: String,
        #[serde(default)]
        expires_in: u32,
    },
}

enum Exchange {
    Pending,
    SlowDown {
        interval: Option<u32>,
    },
    Rejected(String),
    Authorized {
        access_token: String,
        expires_in: u32,
    },
}

fn exchange_device_code(
    client: &Client,
    client_id: &str,
    device_code: &str,
) -> Result<Exchange, &'static str> {
    let response = client
        .post("https://github.com/login/oauth/access_token")
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .map_err(|_| REQUEST_FAILED)?;
    Ok(match read_json(response)? {
        TokenResponse::Error { error, interval } => match error.as_str() {
            "authorization_pending" => Exchange::Pending,
            "slow_down" => Exchange::SlowDown { interval },
            _ => Exchange::Rejected(error),
        },
        TokenResponse::Token {
            access_token,
            expires_in,
        } => Exchange::Authorized {
            access_token,
            expires_in,
        },
    })
}

fn account_login(client: &Client, token: &str) -> Result<String, &'static str> {
    #[derive(Deserialize)]
    struct User {
        login: String,
    }

    let response = client
        .get("https://api.github.com/user")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .bearer_auth(token)
        .send()
        .and_then(Response::error_for_status)
        .map_err(|_| REQUEST_FAILED)?;
    read_json::<User>(response).map(|user| user.login)
}

fn read_json<T: DeserializeOwned>(response: Response) -> Result<T, &'static str> {
    response.json().map_err(|_| REQUEST_FAILED)
}

fn oauth_error(error: &str) -> &'static str {
    match error {
        "access_denied" => "已在 GitHub 拒绝授权，可重新登录。",
        "expired_token" | "token_expired" | "incorrect_device_code" | "bad_verification_code" => {
            "GitHub 授权码已过期，请重新登录。"
        }
        "device_flow_disabled" => "请在 GitHub App 设置中启用设备流。",
        "incorrect_client_credentials" => "GitHub App Client ID 无效，请检查服务器配置。",
        _ => "GitHub 未能完成授权，请重试并检查 GitHub App 设置。",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_codes_and_tokens_are_isolated_and_expire() {
        let now = Instant::now();
        let mut auth = GithubAuth {
            client_id: Some("test-app".into()),
            sessions: HashMap::from([
                (
                    "pending".into(),
                    Authorization::Pending(DeviceAuthorization {
                        device_code: "secret-device-code".into(),
                        user_code: "USER-CODE".into(),
                        expires_at: now + Duration::from_secs(900),
                        interval: Duration::from_secs(5),
                        next_poll: now + Duration::from_secs(5),
                    }),
                ),
                (
                    "connected".into(),
                    Authorization::Connected(Account {
                        login: "octocat".into(),
                        token: "secret-token".into(),
                        expires_at: now + Duration::from_secs(30),
                    }),
                ),
                (
                    "expired".into(),
                    Authorization::Connected(Account {
                        login: "octocat".into(),
                        token: "expired-token".into(),
                        expires_at: now,
                    }),
                ),
            ]),
        };

        let pending = serde_json::to_value(auth.poll("pending").unwrap()).unwrap();
        assert_eq!(pending["user_code"], "USER-CODE");
        assert!(pending["retry_after"].as_u64().unwrap() > 0);
        assert!(pending.get("device_code").is_none());
        assert!(auth.token("pending").is_none());
        assert!(auth.token("expired").is_none());
        assert!(!auth.sessions.contains_key("expired"));

        let connected = serde_json::to_value(auth.status("connected")).unwrap();
        assert_eq!(connected["login"], "octocat");
        assert!(connected.get("token").is_none());
        assert_eq!(auth.token("connected").as_deref(), Some("secret-token"));
        assert!(matches!(
            auth.status("another-session"),
            Status::Disconnected
        ));
        auth.disconnect("connected");
        assert!(auth.token("connected").is_none());
    }
}
