use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::thread::JoinHandle;

use rusqlite::Connection;
use serde_json::json;

use crate::http::{Request, Response};
use crate::{api, chamber::ChamberManager, http, migrations, ui};

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 53212;
const DEFAULT_DATABASE_PATH: &str = "data/auth.sqlite3";
const DEFAULT_CHAMBERS_DIR: &str = "./chamber";

static GLOBAL_APP: OnceLock<App> = OnceLock::new();

#[derive(Debug, Clone)]
struct StaticDirectory {
    path: PathBuf,
    prefix: String,
}

#[derive(Debug, Clone, Default)]
pub struct App {
    static_directories: Arc<RwLock<Vec<StaticDirectory>>>,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the shared App instance used by the default `app::run()` entry point.
    pub fn global() -> &'static Self {
        GLOBAL_APP.get_or_init(Self::new)
    }

    /// Mounts a directory at a URL prefix, for example `("./assets", "/assets")`.
    pub fn register_static_directory(
        &self,
        path: impl AsRef<std::path::Path>,
        prefix: impl AsRef<str>,
    ) -> io::Result<()> {
        let prefix = normalize_static_prefix(prefix.as_ref())?;
        let path = fs::canonicalize(path)?;
        if !path.is_dir() {
            return Err(invalid_config(format!(
                "static path is not a directory: {}",
                path.display()
            )));
        }

        let mut static_directories = self
            .static_directories
            .write()
            .map_err(|_| io::Error::other("static directory registry is unavailable"))?;
        if static_directories
            .iter()
            .any(|mount| mount.prefix == prefix)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("static URL prefix {prefix:?} is already registered"),
            ));
        }

        static_directories.push(StaticDirectory { path, prefix });
        Ok(())
    }

    /// Unmounts the directory registered at `prefix`; returns `false` if absent.
    pub fn unregister_static_directory(&self, prefix: impl AsRef<str>) -> io::Result<bool> {
        let prefix = normalize_static_prefix(prefix.as_ref())?;
        let mut static_directories = self
            .static_directories
            .write()
            .map_err(|_| io::Error::other("static directory registry is unavailable"))?;
        let original_len = static_directories.len();
        static_directories.retain(|mount| mount.prefix != prefix);
        Ok(static_directories.len() != original_len)
    }

    /// Runs the server on a background thread using a shared, live mount registry.
    pub fn spawn(&self) -> JoinHandle<io::Result<()>> {
        let app = self.clone();
        std::thread::spawn(move || app.run())
    }

    pub fn run(&self) -> io::Result<()> {
        run_server(self)
    }
}

#[derive(Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_path: PathBuf,
    pub chambers_dir: PathBuf,
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

    let chambers_dir = match env::var_os("FORMICARY_CHAMBERS_DIR") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        Some(_) => return Err(invalid_config("FORMICARY_CHAMBERS_DIR cannot be empty")),
        None => PathBuf::from(DEFAULT_CHAMBERS_DIR),
    };

    Ok(Config {
        host,
        port,
        database_path,
        chambers_dir,
    })
}

pub fn run() -> io::Result<()> {
    App::global().run()
}

fn run_server(app: &App) -> io::Result<()> {
    let config = load_config()?;
    let chambers = ChamberManager::load(&config.chambers_dir)
        .map_err(|error| io::Error::other(format!("could not load Chambers: {error}")))?;
    for mount in chambers.static_web_mounts() {
        app.register_static_directory(&mount.directory, &mount.route)
            .map_err(|error| {
                io::Error::other(format!(
                    "{}: could not register staticweb route {:?}: {error}",
                    mount.config_path.display(),
                    mount.route
                ))
            })?;
    }
    let mut database = initialize(&config)
        .map_err(|error| io::Error::other(format!("could not initialize application: {error}")))?;

    let listener = TcpListener::bind((config.host.as_str(), config.port))?;
    let address = listener.local_addr()?;
    println!("HTTP server listening on http://{address}");
    let control_host = match config.host.as_str() {
        "0.0.0.0" | "::" => "localhost",
        host => host.trim_matches(['[', ']']),
    };
    let control_host = if control_host.contains(':') {
        format!("[{control_host}]")
    } else {
        control_host.to_owned()
    };
    println!("管理控制面板: http://{control_host}:{}/ui", address.port());

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
                let static_directories = match app.static_directories.read() {
                    Ok(static_directories) => static_directories.clone(),
                    Err(_) => {
                        eprintln!("Static directory registry is unavailable");
                        let response = Response::json(
                            "500 Internal Server Error",
                            json!({"error": "服务器内部错误。"}),
                        );
                        http::write_response(&mut stream, &response, request.method == "HEAD")?;
                        continue;
                    }
                };
                let response =
                    handle_request(&request, &mut database, &chambers, &static_directories);
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

fn handle_request(
    request: &Request,
    database: &mut Connection,
    chambers: &ChamberManager,
    static_directories: &[StaticDirectory],
) -> Response {
    if api::is_api_path(&request.path) {
        return match api::handle(request, database, chambers) {
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

    if let Some(response) = handle_static_directory(&request.path, static_directories) {
        return response;
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

pub(crate) fn normalize_static_prefix(prefix: &str) -> io::Result<String> {
    let prefix = prefix.trim_matches('/');
    let invalid = prefix.is_empty()
        || prefix.split('/').any(|segment| {
            segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.contains('\\')
                || segment
                    .chars()
                    .any(|character| matches!(character, '?' | '#'))
        });
    if invalid {
        return Err(invalid_config(
            "static URL prefix must contain valid, non-empty path segments",
        ));
    }

    if matches!(prefix.split('/').next(), Some("api" | "ui")) {
        return Err(invalid_config(
            "static URL prefixes cannot use the reserved /api or /ui paths",
        ));
    }

    Ok(format!("/{prefix}"))
}

fn handle_static_directory(path: &str, mounts: &[StaticDirectory]) -> Option<Response> {
    let mount = mounts
        .iter()
        .filter(|mount| {
            path == mount.prefix
                || path
                    .strip_prefix(&mount.prefix)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
        .max_by_key(|mount| mount.prefix.len())?;

    let relative_path = path
        .strip_prefix(&mount.prefix)
        .unwrap_or_default()
        .trim_start_matches('/');
    Some(serve_static_file(mount, relative_path))
}

fn serve_static_file(mount: &StaticDirectory, relative_path: &str) -> Response {
    let mut file_path = mount.path.clone();
    for segment in relative_path.split('/') {
        if segment.is_empty() {
            continue;
        }
        if segment == "." || segment == ".." || segment.contains('\\') {
            return static_not_found();
        }
        file_path.push(segment);
    }

    match fs::metadata(&file_path) {
        Ok(metadata) if metadata.is_dir() => file_path.push("index.html"),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return static_not_found(),
        Err(_) => return static_file_error(),
    }

    let canonical_path = match fs::canonicalize(&file_path) {
        Ok(path) if path.starts_with(&mount.path) => path,
        Ok(_) => return static_not_found(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return static_not_found(),
        Err(_) => return static_file_error(),
    };

    match fs::read(&canonical_path) {
        Ok(body) => Response::new("200 OK", ui::content_type(&canonical_path), body),
        Err(error) if error.kind() == io::ErrorKind::NotFound => static_not_found(),
        Err(_) => static_file_error(),
    }
}

fn static_not_found() -> Response {
    Response::new(
        "404 Not Found",
        "text/plain; charset=utf-8",
        b"Not found\n".to_vec(),
    )
}

fn static_file_error() -> Response {
    Response::new(
        "500 Internal Server Error",
        "text/plain; charset=utf-8",
        b"Unable to read static file\n".to_vec(),
    )
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
