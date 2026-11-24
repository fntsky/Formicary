mod api;
pub mod app;
mod auth;
pub mod chamber;
mod github_auth;
mod http_types;
mod migrations;
mod proxy_http;
pub mod service;
mod ui;

fn main() -> std::io::Result<()> {
    app::run()
}
