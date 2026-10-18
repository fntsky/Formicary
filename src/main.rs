mod api;
pub mod app;
pub mod chamber;
mod http;
mod migrations;
pub mod service;
mod ui;

fn main() -> std::io::Result<()> {
    app::run()
}
