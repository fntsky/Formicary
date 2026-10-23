mod api;
pub mod app;
pub mod chamber;
mod http_types;
mod migrations;
mod proxy_http;
pub mod service;
mod ui;

fn main() -> std::io::Result<()> {
    app::run()
}
