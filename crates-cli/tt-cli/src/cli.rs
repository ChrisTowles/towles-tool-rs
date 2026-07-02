use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "tt")]
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

        /// Save check results to history
        #[arg(long)]
        track: bool,

        /// Compare current run against the last tracked run
        #[arg(long)]
        diff: bool,
    },

    /// Configure Claude Code settings and ensure required plugins
    Install {
        /// Show OTEL observability setup instructions
        #[arg(long, short = 'o')]
        observability: bool,
    },

    /// Journal and note-taking commands
    Journal(JournalArgs),

    /// Open this week's daily-notes file (alias for `journal daily-notes`)
    Today {
        /// Create the file but do not open it in an editor
        #[arg(long)]
        no_open: bool,
    },

    /// Generate an interactive HTML treemap from session token data
    Graph(GraphArgs),

    /// GitHub utilities
    Gh(GhArgs),

    /// Create a pull request from the current branch (alias for `gh pr`)
    Pr(PrArgs),

    /// Manage the agentboard desktop app's watched repos
    #[command(alias = "ag")]
    Agentboard(AgentboardArgs),
}

#[derive(Args)]
#[command(disable_help_subcommand = true)]
pub struct AgentboardArgs {
    #[command(subcommand)]
    pub command: AgentboardCommands,
}

#[derive(Subcommand)]
pub enum AgentboardCommands {
    /// Manage the watched-repo list (repos.json)
    Repos(ReposArgs),
}

#[derive(Args)]
#[command(disable_help_subcommand = true)]
pub struct ReposArgs {
    #[command(subcommand)]
    pub command: Option<ReposCommands>,
}

#[derive(Subcommand)]
pub enum ReposCommands {
    /// Add a repo directory to the watch list
    Add {
        /// Path to the repo (must exist; a warning is printed if it isn't a git repo)
        path: String,
    },

    /// Remove a repo from the watch list by session name or path
    Remove {
        /// Session name (dir basename) or the exact configured path
        target: String,
    },
}

#[derive(Args)]
pub struct GraphArgs {
    /// Session ID to analyze (shows all sessions if not provided)
    #[arg(long, short = 's')]
    pub session: Option<String>,

    /// Filter to sessions from the last N days (0 = no limit)
    #[arg(long, default_value_t = 7)]
    pub days: i64,

    /// Output format: html, json, or csv
    #[arg(long, short = 'f', default_value = "html")]
    pub format: String,

    /// Open the report in a browser after generating (the default)
    #[arg(long, short = 'o')]
    pub open: bool,

    /// Do not open the report in a browser
    #[arg(long, conflicts_with = "open")]
    pub no_open: bool,
}

#[derive(Args)]
#[command(disable_help_subcommand = true)]
pub struct GhArgs {
    #[command(subcommand)]
    pub command: GhCommands,
}

#[derive(Subcommand)]
pub enum GhCommands {
    /// Create a git branch from a GitHub issue
    Branch {
        /// Only show issues assigned to me
        #[arg(long, short = 'a')]
        assigned_to_me: bool,
    },

    /// Delete local branches that have been merged into main
    BranchClean(BranchCleanArgs),

    /// Create a pull request from the current branch
    Pr(PrArgs),
}

#[derive(Args)]
pub struct BranchCleanArgs {
    /// Skip confirmation prompt
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Preview branches without deleting
    #[arg(long)]
    pub dry_run: bool,

    /// Base branch to check against
    #[arg(long, short = 'b', default_value = "main")]
    pub base: String,
}

#[derive(Args)]
pub struct PrArgs {
    /// Create as draft PR
    #[arg(long, short = 'D')]
    pub draft: bool,

    /// Base branch for the PR
    #[arg(long, short = 'b', default_value = "main")]
    pub base: String,

    /// Skip confirmation prompt
    #[arg(long, short = 'y')]
    pub yes: bool,
}

#[derive(Args)]
#[command(disable_help_subcommand = true)]
pub struct JournalArgs {
    #[command(subcommand)]
    pub command: JournalCommands,
}

#[derive(Subcommand)]
pub enum JournalCommands {
    /// Weekly files with daily sections for ongoing work and notes
    DailyNotes {
        /// Create the file but do not open it in an editor
        #[arg(long)]
        no_open: bool,
    },

    /// General-purpose notes with structured sections
    Note {
        /// Note title (prompted for if omitted)
        title: Option<String>,

        /// Create the file but do not open it in an editor
        #[arg(long)]
        no_open: bool,
    },

    /// Structured meeting notes with agenda and action items
    Meeting {
        /// Meeting title (prompted for if omitted)
        title: Option<String>,

        /// Create the file but do not open it in an editor
        #[arg(long)]
        no_open: bool,
    },

    /// List recent journal entries
    List {
        /// Filter by entry type: daily-notes, meeting, note
        #[arg(long, short = 't')]
        r#type: Option<String>,

        /// Maximum number of entries to show (default: 20)
        #[arg(long, short = 'l')]
        limit: Option<String>,

        /// Sort by: date, name (default: date)
        #[arg(long, short = 's')]
        sort: Option<String>,
    },

    /// Search journal entries for matching text
    Search {
        /// Text to search for
        query: String,

        /// Filter by entry type: daily-notes, meeting, note
        #[arg(long, short = 't')]
        r#type: Option<String>,

        /// Filter by date range: YYYY-MM-DD..YYYY-MM-DD
        #[arg(long, short = 'r')]
        range: Option<String>,
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
