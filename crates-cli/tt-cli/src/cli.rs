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

    /// Worktree slots: a primary checkout (<root>/<repo>-primary, always the
    /// default branch) plus branch-named worktrees under <root>/slots/, each
    /// with rendered per-slot ports/env so concurrent slots never collide
    Slot(SlotArgs),
}

#[derive(Args)]
#[command(disable_help_subcommand = true)]
pub struct SlotArgs {
    #[command(subcommand)]
    pub command: SlotCommands,
}

#[derive(Subcommand)]
pub enum SlotCommands {
    /// Create the slot for a branch: worktree under .claude/worktrees/ +
    /// rendered .env (port claims, inherited sibling secrets) + setup step
    /// (TT_SLOT_SETUP from the rendered .env, else lockfile-detected install)
    New {
        /// Branch to create and check out (the slot folder is the slugged branch,
        /// e.g. feat/thing -> feat-thing)
        #[arg(long, short = 'b')]
        branch: String,

        /// Base ref for the new branch (default: the main checkout's branch)
        #[arg(long, value_name = "REF")]
        base: Option<String>,

        /// Emit the created slot as JSON
        #[arg(long)]
        json: bool,

        /// Repo checkout (default: walk up from cwd to the nearest git checkout)
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },

    /// List the main checkout and slots with branch, dirty count, and claimed ports
    Ls {
        /// Emit checkouts as a JSON array
        #[arg(long)]
        json: bool,

        /// Repo checkout (default: walk up from cwd to the nearest git checkout)
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },

    /// Remove a slot: guarded (clean tree, no commits unreachable from a
    /// branch or remote, nothing foreign on its ports), then docker compose
    /// down -v, anchored container/volume sweep, worktree remove
    Rm {
        /// Slot directory name under .claude/worktrees/, e.g. slot-migrate
        name: String,

        /// Skip guards (each skip is printed) and force worktree removal
        #[arg(long)]
        force: bool,

        /// Repo checkout (default: walk up from cwd to the nearest git checkout)
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },

    /// (Re)render a checkout's .env from the template — idempotent: existing
    /// port claims and keys the template doesn't know are preserved
    Env {
        /// Slot directory name under .claude/worktrees/, or `primary` for the
        /// main checkout
        name: String,

        /// Repo checkout (default: walk up from cwd to the nearest git checkout)
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },

    /// Claude Code WorktreeCreate hook shell: reads the hook JSON on stdin,
    /// creates (or reuses) the slot, prints its path on stdout — wire it as
    /// the repo's WorktreeCreate hook so `claude --worktree` and background
    /// sessions land in tt-managed slots
    #[command(hide = true)]
    HookCreate,

    /// Claude Code WorktreeRemove hook shell: reads the hook JSON on stdin
    /// and runs the same guarded removal as `tt slot rm`
    #[command(hide = true)]
    HookRemove,

    /// Remove every slot whose branch's work has landed (merged into the
    /// main checkout's branch, or upstream deleted after a squash/rebase
    /// merge) — same guards as rm, never forced — then sweep the
    /// per-checkout state dirs and agentboard windows/sessions left behind
    /// by checkouts that no longer exist
    Clean {
        /// Report what would be removed/swept without touching anything
        #[arg(long)]
        dry_run: bool,

        /// Emit the report as JSON
        #[arg(long)]
        json: bool,

        /// Repo checkout (default: walk up from cwd to the nearest git checkout)
        #[arg(long, value_name = "DIR")]
        root: Option<PathBuf>,
    },
}

#[derive(Args)]
#[command(disable_help_subcommand = true)]
pub struct McpArgs {
    #[command(subcommand)]
    pub command: McpCommands,
}

#[derive(Subcommand)]
pub enum McpCommands {
    /// Serve MCP over stdio (register with: `claude mcp add tt -- tt mcp serve`)
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

    /// Touch a collector's nudge file so a running app instance in this
    /// checkout refreshes that data immediately instead of on its next poll
    Nudge(NudgeArgs),

    /// Show each collector's enabled state and last-run health (no collection)
    Status(CollectStatusArgs),
}

#[derive(Args)]
pub struct NudgeArgs {
    /// Which collector to eagerly refresh
    #[arg(value_enum)]
    pub target: NudgeTarget,
}

/// Which collector `tt collect nudge` eagerly refreshes. Mirrors the two
/// collectors the app's scheduler nudge-dir watch polls for
/// (`crates-tauri/tt-app/src/scheduler.rs`) — see [`NudgeTarget::file_name`]
/// for the shared filename contract between the two.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum NudgeTarget {
    Prs,
    Issues,
}

impl NudgeTarget {
    /// Filename inside the nudge dir this target touches.
    pub fn file_name(self) -> &'static str {
        match self {
            NudgeTarget::Prs => "prs",
            NudgeTarget::Issues => "issues",
        }
    }
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

    /// Get the current checkout current with main: fetch + rebase onto
    /// origin/<base> (hard-fails on a dirty tree or a rebase conflict)
    Sync(SyncArgs),

    /// Check out the branch for a PR number (resolves via `gh pr view`,
    /// guards on a clean tree, fetches the branch if needed)
    #[command(alias = "pr-checkout")]
    Co(CoArgs),
}

#[derive(Args)]
pub struct SyncArgs {
    /// Base branch to rebase onto
    #[arg(long, short = 'b', default_value = "main")]
    pub base: String,
}

#[derive(Args)]
pub struct CoArgs {
    /// PR number whose head branch to check out
    pub number: u64,
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

    /// Clean branches whose upstream is gone (deleted on the remote) instead of
    /// ancestor-merged ones — catches GitHub rebase-and-merge branches. Deletes
    /// with `git branch -D` since they aren't ancestor-merged.
    #[arg(long)]
    pub gone: bool,

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

        /// Fuzzy-pick a recent entry from an interactive list (requires a TTY)
        #[arg(long)]
        pick: bool,

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
