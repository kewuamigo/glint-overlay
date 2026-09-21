#![windows_subsystem = "windows"]

mod cli;
mod console;
mod elevate;
mod host;
mod launcher;

use anyhow::Result;

fn main() {
    if let Err(err) = run() {
        let _ = launcher::write_startup_error(&err);
        eprintln!("{err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--cli") {
        return cli::run_cli(&args[1..]);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let _ = glint_etw_cleanup::cleanup_glint_etw();
    let _ = launcher::ensure_log_dir();
    launcher::spawn_launcher_electron()
}
