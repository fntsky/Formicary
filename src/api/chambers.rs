use rusqlite::Connection;
use serde_json::json;

use crate::chamber::{Chamber, ChamberManager};
use crate::http_types::{Request, Response};
use crate::service::job::Job;
use crate::service::job_service::JobServiceError;

use super::{authenticated_user_id, unauthorized_response};

pub(super) fn matches_path(path: &str) -> bool {
    path == "/api/chambers"
        || path.starts_with("/api/chambers/")
        || path == "/api/jobs"
        || path.starts_with("/api/jobs/")
}

pub(super) fn handle(
    request: &Request,
    database: &Connection,
    chambers: &mut ChamberManager,
    app: &crate::app::App,
) -> rusqlite::Result<Response> {
    match (request.method.as_str(), request.path.as_str()) {
        ("POST", "/api/chambers/github/branches") => {
            if authenticated_user_id(request, database)?.is_none() {
                return Ok(unauthorized_response());
            }
            #[derive(serde::Deserialize)]
            struct Repository {
                repository: String,
            }
            let input: Repository = match super::parse_json_body(request) {
                Ok(value) => value,
                Err(response) => return Ok(response),
            };
            Ok(
                match crate::chamber::github::list_branches(&input.repository) {
                    Ok(branches) => Response::json("200 OK", json!(branches)),
                    Err(error) => Response::json("400 Bad Request", json!({"error": error})),
                },
            )
        }
        ("POST", "/api/chambers/import") => {
            if authenticated_user_id(request, database)?.is_none() {
                return Ok(unauthorized_response());
            }
            let download: crate::chamber::github::Download = match super::parse_json_body(request) {
                Ok(value) => value,
                Err(response) => return Ok(response),
            };
            Ok(match chambers.import_github(&download, app) {
                Ok(name) => Response::json("201 Created", json!({"name": name})),
                Err(error) => Response::json("400 Bad Request", json!({"error": error})),
            })
        }
        ("GET", "/api/chambers") => list_chambers(request, database, chambers),
        ("GET", "/api/jobs") => list_jobs(request, database, chambers),
        _ if request.path.starts_with("/api/chambers/") => {
            chamber_route(request, database, chambers, app)
        }
        _ if request.path.starts_with("/api/jobs/") => {
            job_action_route(request, database, chambers)
        }
        _ => Ok(method_not_allowed_response()),
    }
}

fn list_chambers(
    request: &Request,
    database: &Connection,
    chambers: &ChamberManager,
) -> rusqlite::Result<Response> {
    if authenticated_user_id(request, database)?.is_none() {
        return Ok(unauthorized_response());
    }

    let summaries = chambers
        .chambers()
        .iter()
        .map(|chamber| {
            let jobs = chamber.jobs();
            json!({
                "name": chamber.name,
                "directory": chamber.directory.display().to_string(),
                "job_count": jobs.len(),
                "enabled_job_count": jobs.iter().filter(|job| job.snapshot().enabled).count(),
                "static_web_count": chamber.static_web_mounts().len(),
                "proxy_http_count": chamber.proxy_http_mounts().len(),
            })
        })
        .collect::<Vec<_>>();

    Ok(Response::json("200 OK", json!({"chambers": summaries})))
}

fn list_jobs(
    request: &Request,
    database: &Connection,
    chambers: &ChamberManager,
) -> rusqlite::Result<Response> {
    if authenticated_user_id(request, database)?.is_none() {
        return Ok(unauthorized_response());
    }

    let jobs = chambers
        .list_jobs()
        .into_iter()
        .map(|(chamber, job)| job_json(&chamber, &job))
        .collect::<Vec<_>>();
    Ok(Response::json("200 OK", json!({"jobs": jobs})))
}

fn chamber_route(
    request: &Request,
    database: &Connection,
    chambers: &ChamberManager,
    app: &crate::app::App,
) -> rusqlite::Result<Response> {
    let segments = request.path.split('/').collect::<Vec<_>>();
    match segments.as_slice() {
        [
            "",
            "api",
            "chambers",
            encoded_name,
            "proxies",
            raw_index,
            "logs",
        ] => {
            if request.method != "GET" {
                return Ok(method_not_allowed_response());
            }
            if authenticated_user_id(request, database)?.is_none() {
                return Ok(unauthorized_response());
            }
            let Some(name) = decode_path_segment(encoded_name) else {
                return Ok(invalid_chamber_name_response());
            };
            let Some(chamber) = chambers.chambers().iter().find(|item| item.name == name) else {
                return Ok(not_found_response());
            };
            let Some(config) = raw_index
                .parse::<usize>()
                .ok()
                .and_then(|index| chamber.proxy_http_mounts().get(index))
            else {
                return Ok(not_found_response());
            };
            let proxy = match app.proxy_http_mount(&config.route) {
                Ok(Some(proxy)) => proxy,
                Ok(None) => return Ok(not_found_response()),
                Err(_) => {
                    return Ok(Response::json(
                        "500 Internal Server Error",
                        json!({"error": "无法读取代理日志。"}),
                    ));
                }
            };
            let entries = proxy
                .logs()
                .into_iter()
                .map(|entry| {
                    let timestamp: chrono::DateTime<chrono::Utc> = entry.timestamp.into();
                    json!({"timestamp": timestamp.to_rfc3339(), "stream": match entry.stream {
                    crate::service::shell_runner::ShellLogStream::Stdout => "stdout",
                    crate::service::shell_runner::ShellLogStream::Stderr => "stderr",
                }, "bytes": entry.bytes})
                })
                .collect::<Vec<_>>();
            let mut response =
                Response::json("200 OK", json!({"route": config.route, "entries": entries}));
            response.headers.push("Cache-Control: no-store".into());
            Ok(response)
        }
        ["", "api", "chambers", encoded_name] => {
            if request.method != "GET" {
                return Ok(method_not_allowed_response());
            }
            if authenticated_user_id(request, database)?.is_none() {
                return Ok(unauthorized_response());
            }

            let Some(name) = decode_path_segment(encoded_name) else {
                return Ok(invalid_chamber_name_response());
            };
            let Some(chamber) = chambers.chambers().iter().find(|item| item.name == name) else {
                return Ok(not_found_response());
            };
            Ok(Response::json(
                "200 OK",
                json!({"chamber": chamber_json(chamber)}),
            ))
        }
        ["", "api", "chambers", encoded_name, "jobs"] => {
            if request.method != "GET" {
                return Ok(method_not_allowed_response());
            }
            if authenticated_user_id(request, database)?.is_none() {
                return Ok(unauthorized_response());
            }

            let Some(name) = decode_path_segment(encoded_name) else {
                return Ok(invalid_chamber_name_response());
            };
            let Some(jobs) = chambers.jobs_for_chamber(&name) else {
                return Ok(not_found_response());
            };
            let jobs = jobs
                .iter()
                .map(|job| job_json(&name, job))
                .collect::<Vec<_>>();
            Ok(Response::json(
                "200 OK",
                json!({"chamber": name, "jobs": jobs}),
            ))
        }
        _ => Ok(not_found_response()),
    }
}

fn chamber_json(chamber: &Chamber) -> serde_json::Value {
    let chamber_jobs = chamber.jobs();
    let enabled_job_count = chamber_jobs
        .iter()
        .filter(|job| job.snapshot().enabled)
        .count();
    let jobs = chamber_jobs
        .iter()
        .map(|job| job_json(&chamber.name, job))
        .collect::<Vec<_>>();
    let static_web = chamber
        .static_web_mounts()
        .iter()
        .map(|mount| {
            json!({
                "route": mount.route,
                "directory": mount.directory.display().to_string(),
            })
        })
        .collect::<Vec<_>>();
    let proxy_http = chamber
        .proxy_http_mounts()
        .iter()
        .map(|mount| {
            json!({
                "route": mount.route,
                "upstream": mount.upstream,
                "working_directory": mount.working_directory.display().to_string(),
            })
        })
        .collect::<Vec<_>>();

    json!({
        "name": chamber.name,
        "directory": chamber.directory.display().to_string(),
        "job_count": jobs.len(),
        "enabled_job_count": enabled_job_count,
        "jobs": jobs,
        "static_web": static_web,
        "proxy_http": proxy_http,
    })
}

fn job_action_route(
    request: &Request,
    database: &Connection,
    chambers: &ChamberManager,
) -> rusqlite::Result<Response> {
    let segments = request.path.split('/').collect::<Vec<_>>();
    let ["", "api", "jobs", raw_id, action] = segments.as_slice() else {
        return Ok(not_found_response());
    };
    if !matches!(*action, "pause" | "start") {
        return Ok(not_found_response());
    }
    if request.method != "POST" {
        return Ok(method_not_allowed_response());
    }
    if authenticated_user_id(request, database)?.is_none() {
        return Ok(unauthorized_response());
    }
    let Ok(id) = raw_id.parse::<u64>() else {
        return Ok(not_found_response());
    };
    let Some((chamber, _)) = chambers
        .list_jobs()
        .into_iter()
        .find(|(_, candidate)| candidate.id() == id)
    else {
        return Ok(not_found_response());
    };

    let result = if *action == "pause" {
        chambers.pause_job(id)
    } else {
        chambers.start_job(id)
    };
    match result {
        Ok(job) => Ok(Response::json(
            "200 OK",
            json!({"job": job_json(&chamber, &job)}),
        )),
        Err(JobServiceError::NotFound(_)) => Ok(not_found_response()),
        Err(error) => {
            eprintln!("Could not change job {id} state: {error}");
            Ok(Response::json(
                "500 Internal Server Error",
                json!({"error": "无法更新 Job 状态。"}),
            ))
        }
    }
}

fn job_json(chamber: &str, job: &Job) -> serde_json::Value {
    let snapshot = job.snapshot();
    json!({
        "id": snapshot.id,
        "chamber": chamber,
        "name": snapshot.name,
        "cron": snapshot.cron_expression,
        "working_directory": snapshot.working_directory.display().to_string(),
        "command": snapshot.shell_command,
        "timeout_ms": snapshot.timeout.as_millis(),
        "enabled": snapshot.enabled,
    })
}

fn decode_path_segment(segment: &str) -> Option<String> {
    let source = segment.as_bytes();
    let mut decoded = Vec::with_capacity(source.len());
    let mut index = 0;
    while index < source.len() {
        if source[index] == b'%' {
            let high = *source.get(index + 1)?;
            let low = *source.get(index + 2)?;
            decoded.push((hex_digit(high)? << 4) | hex_digit(low)?);
            index += 3;
        } else {
            decoded.push(source[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn invalid_chamber_name_response() -> Response {
    Response::json(
        "400 Bad Request",
        json!({"error": "Chamber 名称编码无效。"}),
    )
}

fn not_found_response() -> Response {
    Response::json("404 Not Found", json!({"error": "API 端点不存在。"}))
}

fn method_not_allowed_response() -> Response {
    Response::json(
        "405 Method Not Allowed",
        json!({"error": "请求方法不支持。"}),
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        fs, thread,
        time::{Duration, Instant},
    };

    /// 验证代理日志接口需要登录，返回带时间戳的输出快照，并拒绝不存在的代理。
    /// Verifies that proxy log snapshots require authentication, contain timestamped output, and reject missing proxies.
    #[test]
    fn reads_authenticated_proxy_log_snapshot() {
        let root =
            std::env::temp_dir().join(format!("formicary-proxy-logs-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("test")).unwrap();
        fs::write(root.join("test/chamber.toml"), "[chamber]\nname = 'test'\nschema_version = 1\n[proxyhttp]\nscript = 'printf hello'\nroute = '/logs-test'\nupstream = 'http://127.0.0.1:1'\n").unwrap();
        let manager = ChamberManager::load(&root).unwrap();
        let app = crate::app::App::new();
        app.register_proxy_http(
            root.join("test"),
            "printf hello; printf error >&2",
            "/logs-test",
            "http://127.0.0.1:1",
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !app
            .proxy_http_mount("/logs-test")
            .unwrap()
            .unwrap()
            .shell()
            .is_finished()
        {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let database = Connection::open_in_memory().unwrap();
        database.execute_batch("CREATE TABLE sessions(token TEXT, user_id INTEGER, expires_at INTEGER); INSERT INTO sessions VALUES ('test-token', 1, 9999999999);").unwrap();
        let mut request = Request {
            method: "GET".into(),
            target: "/api/chambers/test/proxies/0/logs".into(),
            path: "/api/chambers/test/proxies/0/logs".into(),
            headers: HashMap::new(),
            body: Vec::new(),
        };
        assert_eq!(
            chamber_route(&request, &database, &manager, &app)
                .unwrap()
                .status,
            "401 Unauthorized"
        );
        request
            .headers
            .insert("cookie".into(), "formicary_session=test-token".into());
        let response = chamber_route(&request, &database, &manager, &app).unwrap();
        assert_eq!(response.status, "200 OK");
        assert!(
            response
                .headers
                .iter()
                .any(|header| header == "Cache-Control: no-store")
        );
        let data: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(data["route"], "/logs-test");
        let entries = data["entries"].as_array().unwrap();
        assert!(entries.iter().all(|entry| {
            chrono::DateTime::parse_from_rfc3339(entry["timestamp"].as_str().unwrap()).is_ok()
        }));
        for (stream, expected) in [
            ("stdout", b"hello".as_slice()),
            ("stderr", b"error".as_slice()),
        ] {
            let bytes: Vec<u8> = entries
                .iter()
                .filter(|entry| entry["stream"] == stream)
                .flat_map(|entry| {
                    entry["bytes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|byte| byte.as_u64().unwrap() as u8)
                })
                .collect();
            assert_eq!(bytes, expected);
        }
        request.path = "/api/chambers/test/proxies/1/logs".into();
        assert_eq!(
            chamber_route(&request, &database, &manager, &app)
                .unwrap()
                .status,
            "404 Not Found"
        );
        app.unregister_proxy_http("/logs-test").unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
