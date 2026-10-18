mod api;
mod http;
mod ui;

use std::io;
use std::net::TcpListener;

use http::{Request, Response};
use serde_json::json;

fn main() -> io::Result<()> {
    let database = api::open_database().map_err(|error| {
        io::Error::other(format!("could not initialize SQLite database: {error}"))
    })?;

    let listener = TcpListener::bind("0.0.0.0:53212")?;
    println!("HTTP server listening on http://0.0.0.0:53212");

    for connection in listener.incoming() {
        let mut stream = match connection {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("HTTP connection error: {error}");
                continue;
            }
        };

        match http::read_request(&stream) {
            Ok(Some(request)) => {
                let response = handle_request(&request, &database);
                http::write_response(&mut stream, &response, request.method == "HEAD")?;
            }
            Ok(None) => {}
            Err(error) => {
                let response = if error.kind() == io::ErrorKind::FileTooLarge {
                    Response::json("413 Payload Too Large", json!({"error": "请求内容过大。"}))
                } else {
                    Response::json("400 Bad Request", json!({"error": "无法读取请求。"}))
                };
                http::write_response(&mut stream, &response, false)?;
            }
        }
    }

    Ok(())
}

fn handle_request(request: &Request, database: &rusqlite::Connection) -> Response {
    if api::is_api_path(&request.path) {
        return match api::handle(request, database) {
            Ok(response) => response,
            Err(error) => {
                eprintln!("API database error: {error}");
                Response::json(
                    "500 Internal Server Error",
                    json!({"error": "服务器内部错误。"}),
                )
            }
        };
    }

    if request.method != "GET" && request.method != "HEAD" {
        return Response::new(
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            b"Method not allowed\n".to_vec(),
        );
    }

    if ui::is_ui_path(&request.path) {
        return ui::handle(&request.path);
    }

    if request.path == "/" {
        Response::new(
            "200 OK",
            "text/plain; charset=utf-8",
            b"Hello from Formicary\n".to_vec(),
        )
    } else {
        Response::new(
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"Not found\n".to_vec(),
        )
    }
}
