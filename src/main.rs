mod api;
mod app;
mod http;
mod migrations;
mod ui;

fn main() -> std::io::Result<()> {
    app::run()
}
