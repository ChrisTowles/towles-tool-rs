mod cli;
mod commands;
mod ui;

use clap::Parser;
use cli::{Cli, Commands, JournalCommands};

fn main() {
    let Cli { verbose, config_dir, command } = Cli::parse();

    init_logging(verbose);

    let exit_code = match command {
        Commands::Journal(args) => commands::journal::run(args.command, config_dir.as_deref()),
        Commands::Today { no_open } => {
            commands::journal::run(JournalCommands::DailyNotes { no_open }, config_dir.as_deref())
        }
        Commands::Collect(args) => commands::collect::run(args.command, config_dir.as_deref()),
        Commands::Mcp(args) => commands::mcp::run(args.command, config_dir.as_deref()),
        Commands::Slot(args) => commands::slot::run(args.command),
    };

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

/// Install telemetry, mapping the `-v` count onto the stderr level. `RUST_LOG`
/// still overrides when set. The `-v` count only affects what reaches the
/// terminal — the on-disk event log always records at debug (see `tt_otel`).
///
/// A failure here is ignored: telemetry must never stop the CLI from running
/// its command.
fn init_logging(verbose: u8) {
    let default_level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let _ = tt_otel::init("tt", default_level);
}
