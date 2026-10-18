use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;

use rusqlite::Connection;
use serde_json::json;

use crate::http::{Request, Response};
use crate::{api, http, migrations, ui};

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 53212;
const DEFAULT_DATABASE_PATH: &str = "data/auth.sqlite3";

#[derive(Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_path: PathBuf,
}

pub fn load_config() -> io::Result<Config> {
    let host = env_string("FORMICARY_HOST", DEFAULT_HOST)?;
    if host.trim().is_empty() {
        return Err(invalid_config("FORMICARY_HOST cannot be empty"));
    }

    let port = match env::var("FORMICARY_PORT") {
        Ok(value) => value.parse::<u16>().map_err(|_| {
            invalid_config("FORMICARY_PORT must be a valid port number from 0 to 65535")
        })?,
        Err(env::VarError::NotPresent) => DEFAULT_PORT,
        Err(error) => return Err(invalid_config(format!("invalid FORMICARY_PORT: {error}"))),
    };

    let database_path = match env::var_os("FORMICARY_DATABASE_PATH") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        Some(_) => return Err(invalid_config("FORMICARY_DATABASE_PATH cannot be empty")),
        None => PathBuf::from(DEFAULT_DATABASE_PATH),
    };

    Ok(Config {
        host,
        port,
        database_path,
    })
}

pub fn run() -> io::Result<()> {
    let config = load_config()?;
    let database = initialize(&config)
        .map_err(|error| io::Error::other(format!("could not initialize application: {error}")))?;

    let listener = TcpListener::bind((config.host.as_str(), config.port))?;
    println!("HTTP server listening on http://{}", listener.local_addr()?);

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

pub fn initialize(config: &Config) -> Result<Connection, Box<dyn Error>> {
    let database_existed = config.database_path.exists();
    if let Some(parent) = config
        .database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let mut database = Connection::open(&config.database_path)?;
    database.execute_batch("PRAGMA foreign_keys = ON;")?;
    migrations::migrate(&mut database)?;
    api::cleanup_expired_sessions(&database)?;

    if !database_existed || api::user_count(&database)? == 0 {
        if let Some((username, password)) = api::create_initial_account(&mut database)? {
            println!("首次启动已创建管理员账户：");
            println!("用户名: {username}");
            println!("密码: {password}");
        }
    }

    Ok(database)
}

fn handle_request(request: &Request, database: &Connection) -> Response {
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

fn env_string(name: &str, default: &str) -> io::Result<String> {
    match env::var(name) {
        Ok(value) => Ok(value),
        Err(env::VarError::NotPresent) => Ok(default.to_owned()),
        Err(error) => Err(invalid_config(format!("invalid {name}: {error}"))),
    }
}

fn invalid_config(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
