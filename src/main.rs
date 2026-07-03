use thunk::app;

fn main() -> app::Result<()> {
    // Capture cwd before any framework (WebKit/AppKit via the gui feature) can chdir.
    let start_dir = std::env::current_dir().ok();
    let cli = app::cli::Cli::parse();
    app::run(cli, start_dir)
}
