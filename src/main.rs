use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("0.0.0.0:53212")?;
    println!("HTTP server listening on http://0.0.0.0:53212");

    for connection in listener.incoming() {
        let mut stream = connection?;
        let mut request_line = String::new();
        BufReader::new(&mut stream).read_line(&mut request_line)?;
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let path = parts
            .next()
            .unwrap_or_default()
            .split('?')
            .next()
            .unwrap_or_default();

        if method != "GET" && method != "HEAD" {
            respond(
                &mut stream,
                "405 Method Not Allowed",
                "text/plain; charset=utf-8",
                b"Method not allowed\n",
                method == "HEAD",
            )?;
            continue;
        }

        if let Some(file_path) = ui_file_path(path) {
            match fs::read(&file_path) {
                Ok(body) => respond(
                    &mut stream,
                    "200 OK",
                    content_type(&file_path),
                    &body,
                    method == "HEAD",
                )?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    respond(
                        &mut stream,
                        "404 Not Found",
                        "text/plain; charset=utf-8",
                        b"Not found\n",
                        method == "HEAD",
                    )?;
                }
                Err(error) => return Err(error),
            }
        } else if path == "/" {
            let body = b"Hello from Formicary\n";
            respond(
                &mut stream,
                "200 OK",
                "text/plain; charset=utf-8",
                body,
                method == "HEAD",
            )?;
        } else {
            respond(
                &mut stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                b"Not found\n",
                method == "HEAD",
            )?;
        }
    }

    Ok(())
}

fn ui_file_path(request_path: &str) -> Option<PathBuf> {
    let relative_path = request_path.strip_prefix("/ui")?;
    if !relative_path.is_empty() && !relative_path.starts_with('/') {
        return None;
    }

    let mut file_path = PathBuf::from("Ouput/ui");
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

fn content_type(path: &std::path::Path) -> &'static str {
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

fn respond(
    stream: &mut std::net::TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if !head_only {
        stream.write_all(body)?;
    }
    Ok(())
}
