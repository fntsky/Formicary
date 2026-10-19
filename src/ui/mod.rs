use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::http::Response;

pub fn is_ui_path(path: &str) -> bool {
    path == "/ui" || path.starts_with("/ui/")
}

pub fn handle(path: &str) -> Response {
    let Some(file_path) = map_path(path) else {
        return not_found();
    };

    match fs::read(&file_path) {
        Ok(body) => Response::new("200 OK", content_type(&file_path), body),
        Err(error) if error.kind() == io::ErrorKind::NotFound => not_found(),
        Err(_) => Response::new(
            "500 Internal Server Error",
            "text/plain; charset=utf-8",
            b"Unable to read UI file\n".to_vec(),
        ),
    }
}

fn map_path(request_path: &str) -> Option<PathBuf> {
    let relative_path = request_path.strip_prefix("/ui")?;
    if !relative_path.is_empty() && !relative_path.starts_with('/') {
        return None;
    }

    let mut file_path = ui_directory();
    for segment in relative_path.trim_start_matches('/').split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." || segment.contains('\\') {
            return None;
        }
        file_path.push(segment);
    }

    if file_path.is_dir() {
        file_path.push("index.html");
    }
    Some(file_path)
}

fn ui_directory() -> PathBuf {
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        let adjacent_ui = directory.join("ui");
        if adjacent_ui.is_dir() {
            return adjacent_ui;
        }
    }

    let output_ui = PathBuf::from("Output/ui");
    if output_ui.is_dir() {
        output_ui
    } else {
        PathBuf::from("ui/dist")
    }
}

fn not_found() -> Response {
    Response::new(
        "404 Not Found",
        "text/plain; charset=utf-8",
        b"Not found\n".to_vec(),
    )
}

pub(crate) fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}
