use rusqlite::Connection;
use serde_json::json;

use crate::chamber::{Chamber, ChamberManager};
use crate::http::{Request, Response};
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
    chambers: &ChamberManager,
) -> rusqlite::Result<Response> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/chambers") => list_chambers(request, database, chambers),
        ("GET", "/api/jobs") => list_jobs(request, database, chambers),
        _ if request.path.starts_with("/api/chambers/") => {
            chamber_route(request, database, chambers)
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
) -> rusqlite::Result<Response> {
    let segments = request.path.split('/').collect::<Vec<_>>();
    match segments.as_slice() {
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
