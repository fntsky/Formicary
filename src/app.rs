use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::any;
use rusqlite::Connection;
use serde_json::json;
use tower::limit::ConcurrencyLimitLayer;

use crate::http_types::{Request, Response};
use crate::{api, chamber::ChamberManager, migrations, proxy_http::ProxyMount, ui};

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 53212;
const DEFAULT_DATABASE_PATH: &str = "data/auth.sqlite3";
const DEFAULT_CHAMBERS_DIR: &str = "./chamber";
const MAX_REQUEST_BODY: usize = 2 * 1024 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 32;

static GLOBAL_APP: OnceLock<App> = OnceLock::new();
static CTRL_C_REQUESTED: AtomicBool = AtomicBool::new(false);
static CTRL_C_HANDLER: OnceLock<Result<(), String>> = OnceLock::new();

#[derive(Debug, Clone)]
struct StaticDirectory {
    path: PathBuf,
    prefix: String,
}

#[derive(Debug, Clone, Default)]
pub struct App {
    mounts: Arc<RwLock<MountRegistry>>,
}

#[derive(Debug, Default)]
struct MountRegistry {
    static_directories: Vec<StaticDirectory>,
    proxy_http: Vec<ProxyMount>,
}

#[derive(Clone)]
struct HttpState {
    app: App,
    runtime: Arc<Mutex<RuntimeState>>,
}

struct RuntimeState {
    database: Connection,
    chambers: ChamberManager,
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
        let prefix = normalize_route_prefix(prefix.as_ref())?;
        let path = fs::canonicalize(path)?;
        if !path.is_dir() {
            return Err(invalid_config(format!(
                "static path is not a directory: {}",
                path.display()
            )));
        }

        let mut mounts = self
            .mounts
            .write()
            .map_err(|_| io::Error::other("mount registry is unavailable"))?;
        if mounts
            .static_directories
            .iter()
            .any(|mount| mount.prefix == prefix)
            || mounts.proxy_http.iter().any(|mount| mount.route == prefix)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("URL prefix {prefix:?} is already registered"),
            ));
        }

        mounts
            .static_directories
            .push(StaticDirectory { path, prefix });
        Ok(())
    }

    /// Unmounts the directory registered at `prefix`; returns `false` if absent.
    pub fn unregister_static_directory(&self, prefix: impl AsRef<str>) -> io::Result<bool> {
        let prefix = normalize_route_prefix(prefix.as_ref())?;
        let mut mounts = self
            .mounts
            .write()
            .map_err(|_| io::Error::other("mount registry is unavailable"))?;
        let original_len = mounts.static_directories.len();
        mounts
            .static_directories
            .retain(|mount| mount.prefix != prefix);
        Ok(mounts.static_directories.len() != original_len)
    }

    /// Starts and mounts a local service behind an HTTP route.
    ///
    /// `script` runs as shell text in `working_directory` and should keep the
    /// service in the foreground. Unregistering the route terminates its process
    /// group after requests already using the mount have finished.
    pub fn register_proxy_http(
        &self,
        working_directory: impl AsRef<std::path::Path>,
        script: impl AsRef<str>,
        route: impl AsRef<str>,
        upstream: impl AsRef<str>,
    ) -> io::Result<()> {
        let route = normalize_route_prefix(route.as_ref())?;
        let working_directory = fs::canonicalize(working_directory)?;
        if !working_directory.is_dir() {
            return Err(invalid_config(format!(
                "proxyhttp working path is not a directory: {}",
                working_directory.display()
            )));
        }

        let mut mounts = self
            .mounts
            .write()
            .map_err(|_| io::Error::other("mount registry is unavailable"))?;
        if mounts
            .static_directories
            .iter()
            .any(|mount| mount.prefix == route)
            || mounts.proxy_http.iter().any(|mount| mount.route == route)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("URL prefix {route:?} is already registered"),
            ));
        }

        let mount = ProxyMount::start(
            &working_directory,
            script.as_ref(),
            route,
            upstream.as_ref(),
        )?;
        mounts.proxy_http.push(mount);
        Ok(())
    }

    pub(crate) fn proxy_http_mount(&self, route: &str) -> io::Result<Option<ProxyMount>> {
        let mounts = self
            .mounts
            .read()
            .map_err(|_| io::Error::other("mount registry is unavailable"))?;
        Ok(mounts
            .proxy_http
            .iter()
            .find(|mount| mount.route == route)
            .cloned())
    }

    /// Unmounts a proxy route and stops its configured service process.
    pub fn unregister_proxy_http(&self, route: impl AsRef<str>) -> io::Result<bool> {
        let route = normalize_route_prefix(route.as_ref())?;
        let mut mounts = self
            .mounts
            .write()
            .map_err(|_| io::Error::other("mount registry is unavailable"))?;
        let original_len = mounts.proxy_http.len();
        mounts.proxy_http.retain(|mount| mount.route != route);
        Ok(mounts.proxy_http.len() != original_len)
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
    CTRL_C_REQUESTED.store(false, Ordering::Release);
    install_ctrl_c_handler()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let config = load_config()?;
    let chambers = ChamberManager::load(&config.chambers_dir)
        .map_err(|error| io::Error::other(format!("could not load Chambers: {error}")))?;
    let database = initialize(&config)
        .map_err(|error| io::Error::other(format!("could not initialize application: {error}")))?;
    let listener = TcpListener::bind((config.host.as_str(), config.port))?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let mut registered_static_routes = Vec::new();
    let mut registered_proxy_routes = Vec::new();
    for mount in chambers.static_web_mounts() {
        if let Err(error) = app.register_static_directory(&mount.directory, &mount.route) {
            rollback_mounts(app, &registered_static_routes, &registered_proxy_routes);
            return Err(io::Error::other(format!(
                "{}: could not register staticweb route {:?}: {error}",
                mount.config_path.display(),
                mount.route
            )));
        }
        registered_static_routes.push(mount.route.clone());
    }
    for mount in chambers.proxy_http_mounts() {
        if let Err(error) = app.register_proxy_http(
            &mount.working_directory,
            &mount.script,
            &mount.route,
            &mount.upstream,
        ) {
            rollback_mounts(app, &registered_static_routes, &registered_proxy_routes);
            return Err(io::Error::other(format!(
                "{}: could not register proxyhttp route {:?}: {error}",
                mount.config_path.display(),
                mount.route
            )));
        }
        registered_proxy_routes.push(mount.route.clone());
    }
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

    let runtime_state = Arc::new(Mutex::new(RuntimeState { database, chambers }));
    let router = http_router(HttpState {
        app: app.clone(),
        runtime: Arc::clone(&runtime_state),
    });
    let server_result = runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await
    });

    match runtime_state.lock() {
        Ok(state) => {
            for chamber in state.chambers.chambers() {
                if let Err(error) = chamber.close(app) {
                    eprintln!("Could not close Chamber {}: {error}", chamber.name);
                }
            }
        }
        Err(_) => eprintln!("Could not close Chambers because server state is unavailable"),
    }

    server_result
}

fn http_router(state: HttpState) -> Router {
    Router::new()
        .route("/api", any(dispatch_request))
        .route("/api/{*path}", any(dispatch_request))
        .fallback(any(dispatch_request))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY))
        .layer(ConcurrencyLimitLayer::new(MAX_CONCURRENT_REQUESTS))
        .with_state(state)
}

async fn shutdown_signal() {
    while !CTRL_C_REQUESTED.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn dispatch_request(
    State(state): State<HttpState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> axum::response::Response {
    let body = match body {
        Ok(body) => body,
        Err(rejection) => {
            let status = rejection.into_response().status();
            let response = if status == StatusCode::PAYLOAD_TOO_LARGE {
                Response::json("413 Payload Too Large", json!({"error": "请求内容过大。"}))
            } else {
                Response::json("400 Bad Request", json!({"error": "无法读取请求。"}))
            };
            return to_framework_response(response, method == Method::HEAD);
        }
    };

    let path = uri.path().to_owned();
    let target = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(uri.path())
        .to_owned();
    let headers = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
        })
        .collect();
    let request = Request {
        method: method.as_str().to_owned(),
        target,
        path,
        headers,
        body: body.to_vec(),
    };
    let app = state.app;
    let runtime = state.runtime;

    let response = tokio::task::spawn_blocking(move || {
        let mut runtime = match runtime.lock() {
            Ok(runtime) => runtime,
            Err(_) => {
                return Response::json(
                    "500 Internal Server Error",
                    json!({"error": "服务器内部错误。"}),
                );
            }
        };
        let mounts = match app.mounts.read() {
            Ok(mounts) => (mounts.static_directories.clone(), mounts.proxy_http.clone()),
            Err(_) => {
                eprintln!("Mount registry is unavailable");
                return Response::json(
                    "500 Internal Server Error",
                    json!({"error": "服务器内部错误。"}),
                );
            }
        };
        let RuntimeState { database, chambers } = &mut *runtime;
        handle_request(&request, database, chambers, &app, &mounts.0, &mounts.1)
    })
    .await
    .unwrap_or_else(|error| {
        eprintln!("HTTP request handler failed: {error}");
        Response::json(
            "500 Internal Server Error",
            json!({"error": "服务器内部错误。"}),
        )
    });

    to_framework_response(response, method == Method::HEAD)
}

fn to_framework_response(response: Response, head_only: bool) -> axum::response::Response {
    let status_code = response
        .status
        .split_ascii_whitespace()
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .and_then(|value| StatusCode::from_u16(value).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let content_length = response.content_length.unwrap_or(response.body.len());
    let mut framework_response = axum::response::Response::new(if head_only {
        axum::body::Body::empty()
    } else {
        axum::body::Body::from(response.body)
    });
    *framework_response.status_mut() = status_code;

    let headers = framework_response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&response.content_type) {
        headers.insert(axum::http::header::CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&content_length.to_string()) {
        headers.insert(axum::http::header::CONTENT_LENGTH, value);
    }
    for header in response.headers {
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.trim().as_bytes()),
            HeaderValue::from_str(value.trim()),
        ) else {
            continue;
        };
        headers.append(name, value);
    }
    framework_response
}

fn install_ctrl_c_handler() -> io::Result<()> {
    match CTRL_C_HANDLER.get_or_init(|| {
        ctrlc::set_handler(|| CTRL_C_REQUESTED.store(true, Ordering::Release))
            .map_err(|error| error.to_string())
    }) {
        Ok(()) => Ok(()),
        Err(error) => Err(io::Error::other(error.clone())),
    }
}

fn rollback_mounts(app: &App, static_routes: &[String], proxy_routes: &[String]) {
    for route in proxy_routes.iter().rev() {
        let _ = app.unregister_proxy_http(route);
    }
    for route in static_routes.iter().rev() {
        let _ = app.unregister_static_directory(route);
    }
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
    chambers: &mut ChamberManager,
    app: &App,
    static_directories: &[StaticDirectory],
    proxy_http: &[ProxyMount],
) -> Response {
    if api::is_api_path(&request.path) {
        return match api::handle(request, database, chambers, app) {
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

    enum Route<'a> {
        Static(&'a StaticDirectory),
        Proxy(&'a ProxyMount),
    }
    let best_static = static_directories
        .iter()
        .filter(|mount| matches_prefix(&request.path, &mount.prefix))
        .max_by_key(|mount| mount.prefix.len());
    let best_proxy = proxy_http
        .iter()
        .filter(|mount| mount.matches(&request.path))
        .max_by_key(|mount| mount.route.len());
    let route = match (best_static, best_proxy) {
        (Some(static_mount), Some(proxy_mount))
            if proxy_mount.route.len() > static_mount.prefix.len() =>
        {
            Some(Route::Proxy(proxy_mount))
        }
        (Some(static_mount), _) => Some(Route::Static(static_mount)),
        (None, Some(proxy_mount)) => Some(Route::Proxy(proxy_mount)),
        (None, None) => None,
    };
    if let Some(route) = route {
        return match route {
            Route::Proxy(mount) => {
                let username = match api::authenticated_username(request, database) {
                    Ok(username) => username,
                    Err(error) => {
                        eprintln!("Could not validate proxy request session: {error}");
                        return Response::json(
                            "500 Internal Server Error",
                            json!({"error": "服务器内部错误。"}),
                        );
                    }
                };
                mount.forward(request, username.as_deref())
            }
            Route::Static(mount) if request.method == "GET" || request.method == "HEAD" => {
                let relative_path = request
                    .path
                    .strip_prefix(&mount.prefix)
                    .unwrap_or_default()
                    .trim_start_matches('/');
                serve_static_file(mount, relative_path)
            }
            Route::Static(_) => Response::new(
                "405 Method Not Allowed",
                "text/plain; charset=utf-8",
                b"Method not allowed\n".to_vec(),
            ),
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

pub(crate) fn normalize_route_prefix(prefix: &str) -> io::Result<String> {
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
            "URL route must contain valid, non-empty path segments",
        ));
    }

    if matches!(prefix.split('/').next(), Some("api" | "ui")) {
        return Err(invalid_config(
            "URL routes cannot use the reserved /api or /ui paths",
        ));
    }

    Ok(format!("/{prefix}"))
}

fn matches_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
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
