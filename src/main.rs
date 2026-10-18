mod api;
mod app;
pub mod chamber;
pub mod chamber_manager;
mod http;
pub mod job;
pub mod job_service;
mod migrations;
pub mod shell_runner;
mod ui;

fn main() -> std::io::Result<()> {
    app::run()
}
