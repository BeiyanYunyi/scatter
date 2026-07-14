mod app;
mod astronomy;
mod cli;
mod config;
#[cfg(target_os = "macos")]
mod macos;
mod projection;
mod renderer;
mod solar;
mod stars;

use std::error::Error;

type AppResult<T> = Result<T, Box<dyn Error>>;

fn main() -> AppResult<()> {
    let Some(options) = cli::parse_startup_options(std::env::args_os().skip(1))
        .map_err(|error| format!("{error}\nRun with --help for usage."))?
    else {
        cli::print_usage();
        return Ok(());
    };
    let runtime_config = config::RuntimeConfig::load(options.config_path)?;
    eprintln!("loaded config from {}", runtime_config.path().display());
    app::run(options.window_mode, runtime_config)
}
