use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ttr")]
#[command(about = "towles-tool (Rust) - developer utilities, config, and diagnostics")]
#[command(version)]
#[command(disable_help_subcommand = true)]
pub struct Cli {
    /// Enable verbose logging (repeat for more detail: -v info, -vv debug, -vvv trace)
    #[arg(long, short, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Override the config directory (defaults to ~/.config/towles-tool)
    #[arg(long, global = true, value_name = "DIR")]
    pub config_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage towles-tool configuration
    Config(ConfigArgs),

    /// Check system dependencies and environment
    Doctor {
        /// Emit results as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
#[command(disable_help_subcommand = true)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommands,
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show current settings and the settings file path
    Show,

    /// Validate the settings file against the config schema
    Validate,

    /// Print the settings JSON schema
    Schema,

    /// Reset settings to defaults
    Reset {
        /// Confirm the reset (required to actually write)
        #[arg(long)]
        confirm: bool,
    },
}
