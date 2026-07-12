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

        /// Save check results to history
        #[arg(long)]
        track: bool,

        /// Compare current run against the last tracked run (human-format; not
        /// combinable with --json)
        #[arg(long, conflicts_with = "json")]
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

    /// Claude Code session summary across every repo: token accounting, an
    /// HTML treemap, or JSON/CSV export
    ClaudeSessions(ClaudeSessionsArgs),

    /// GitHub utilities
    Gh(GhArgs),

    /// Create a pull request from the current branch (alias for `gh pr`)
    Pr(PrArgs),

    /// List my open PRs with CI status across repos (alias for `gh pr-list`)
    Prs,

    /// Agentboard: manage the watched-repo list (app + collectors read it)
    #[command(alias = "ag")]
    Agentboard(AgentboardArgs),

    /// Collect dashboard data into the local store (calendar, issues, PRs)
    Collect(CollectArgs),

    /// MCP server exposing the local store and agent sessions to claude
    Mcp(McpArgs),
}

#[derive(Args)]
#[command(disable_help_subcommand = true)]
pub struct McpArgs {
    #[command(subcommand)]
    pub command: McpCommands,
}

#[derive(Subcommand)]
pub enum McpCommands {
    /// Serve MCP over stdio (register with: `claude mcp add tt -- ttr mcp serve`)
    Serve {
        /// Path to the store database (defaults to the standard tt.db location)
        #[arg(long, value_name = "FILE")]
        store: Option<PathBuf>,
    },
}

#[derive(Args)]
#[command(disable_help_subcommand = true)]
pub struct CollectArgs {
    #[command(subcommand)]
    pub command: CollectCommands,
}

#[derive(Subcommand)]
pub enum CollectCommands {
    /// Collect today's calendar events via `claude -p` (next-meeting countdown)
    Calendar,

    /// Collect open issues assigned to me across tracked repos via `gh`
    Issues,

    /// Collect open and review-requested pull requests via `gh`
    Prs,

    /// Poll the watched Slack DM via the Slack Web API (needs a token in settings)
    Slack,

    /// Run every collector (calendar, issues, PRs, Slack)
    All,

    /// Show each collector's enabled state and last-run health (no collection)
    Status(CollectStatusArgs),
}

#[derive(Args)]
pub struct CollectStatusArgs {
    /// Emit structured JSON instead of the human table
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
#[command(disable_help_subcommand = true)]
pub struct AgentboardArgs {
    #[command(subcommand)]
    pub command: AgentboardCommands,
}

#[derive(Subcommand)]
pub enum AgentboardCommands {
    /// Manage the watched-repo list (repos.json) that feeds the app + collectors
    Repos(ReposArgs),

    /// Manage a folder's PTY sessions (sessions.json)
    Sessions(SessionsArgs),
}

#[derive(Args)]
#[command(disable_help_subcommand = true)]
pub struct SessionsArgs {
    #[command(subcommand)]
    pub command: Option<SessionsCommands>,
}

#[derive(Subcommand)]
pub enum SessionsCommands {
    /// Add a PTY session to a folder (a watched checkout)
    Add {
        /// Path to the folder/checkout (must exist)
        path: String,
        /// Optional session name (defaults to "shell N")
        #[arg(long)]
        name: Option<String>,
    },

    /// Rename a session by id
    Rename {
        /// Session id (from `sessions list`)
        id: String,
        /// New name
        name: String,
    },

    /// Remove a session by id
    Remove {
        /// Session id (from `sessions list`)
        id: String,
    },
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
pub struct ClaudeSessionsArgs {
    /// Session ID to analyze (shows all sessions if not provided)
    #[arg(long, short = 's')]
    pub session: Option<String>,

    /// Filter to sessions from the last N days (0 = no limit)
    #[arg(long, default_value_t = 7)]
    pub days: i64,

    /// Output format: html, json, csv, or md
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

    /// List my open PRs across tracked repos with CI check status
    #[command(alias = "prs")]
    PrList,

    /// Assign an open issue to a sibling slot checkout of this repo
    /// (hard-fails unless the slot is clean: no changes, no stashes)
    Assign(AssignArgs),
}

#[derive(Args)]
pub struct AssignArgs {
    /// Issue number to assign
    pub issue: u64,

    /// Target slot checkout directory (a clone of this same repo)
    #[arg(long, short = 's')]
    pub slot: std::path::PathBuf,
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

    /// Append a timestamped bullet to today's daily note without opening an editor
    Jot {
        /// Text to capture. Use `-` (or omit) to read the bullet from stdin.
        text: Option<String>,
    },

    /// Open the most recent journal entry in the editor
    Open {
        /// Open the most recent entry (the default; accepted for explicitness)
        #[arg(long)]
        last: bool,

        /// Filter by entry type: daily-notes, meeting, note
        #[arg(long, short = 't')]
        r#type: Option<String>,

        /// Print the entry's absolute path instead of opening it in an editor
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

        /// Emit entries as a JSON array instead of a table
        #[arg(long)]
        json: bool,
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

        /// Emit matches as a JSON array instead of grouped text
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
