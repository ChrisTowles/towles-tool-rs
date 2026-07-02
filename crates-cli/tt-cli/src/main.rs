mod cli;
mod commands;
mod ui;

use clap::Parser;
use cli::{Cli, Commands};

fn main() {
    let Cli { verbose, config_dir, command } = Cli::parse();

    init_logging(verbose);

    let exit_code = match command {
        Commands::Config(args) => commands::config::run(args.command, config_dir.as_deref()),
        Commands::Doctor { json } => commands::doctor::run(json),
    };

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

/// Configure `env_logger` from the `-v` count. `RUST_LOG` still overrides when set.
fn init_logging(verbose: u8) {
    let default_level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level))
        .init();
}
