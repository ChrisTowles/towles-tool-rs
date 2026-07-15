//! `tt install`: configure Claude Code settings and ensure required plugins.
//!
//! Ports `src/commands/install.ts`. The settings read/write is pure logic in
//! [`crate::commands::claude_settings`]; this layer handles output styling, the
//! observability instructions, and the interactive plugin install.
//!
//! Deviations from the TS CLI (see docs/MIGRATION.md):
//! - Plugin install prompts require a TTY. When stdin is not a terminal we print a
//!   dim "skipped (non-interactive)" note instead of prompting, so CI/tests never
//!   hang and never run a real `claude plugin install`.
//! - The per-command `--debug` flag is replaced by the global `-v/--verbose` flag.

use crate::commands::claude_settings::{
    apply_recommended_settings, claude_settings_path, load_claude_settings, save_claude_settings,
};
use console::style;
use serde::Deserialize;
use std::io::IsTerminal;

/// A plugin `tt install` ensures is present.
struct RequiredPlugin {
    /// Fully-qualified plugin id, e.g. `tt@towles-tool`.
    id: &'static str,
    /// Short display name used in messages.
    name: &'static str,
    /// Marketplace to add before installing, if the plugin ships one.
    marketplace: Option<Marketplace>,
}

struct Marketplace {
    /// Marketplace label shown after a successful `marketplace add`.
    name: &'static str,
    /// URL passed to `claude plugin marketplace add`.
    url: &'static str,
}

const REQUIRED_PLUGINS: &[RequiredPlugin] = &[
    RequiredPlugin {
        id: "tt@towles-tool",
        name: "core",
        marketplace: Some(Marketplace {
            name: "towles-tool",
            url: "https://github.com/ChrisTowles/towles-tool",
        }),
    },
    RequiredPlugin {
        id: "towles-tool-app@towles-tool",
        name: "towles-tool-app",
        marketplace: Some(Marketplace {
            name: "towles-tool",
            url: "https://github.com/ChrisTowles/towles-tool",
        }),
    },
    RequiredPlugin {
        id: "code-simplifier@claude-plugins-official",
        name: "code-simplifier",
        marketplace: None,
    },
];

pub fn run(observability: bool) -> i32 {
    println!("\n{}\n", style("🔧 towles-tool install").bold());

    let path = claude_settings_path();
    let existing = load_claude_settings(&path);
    if existing.is_empty() {
        println!("{}", style("No Claude settings file found, will create one").dim());
    } else {
        println!(
            "{}",
            style(format!("Found existing Claude settings at {}", path.display())).dim()
        );
    }

    let (settings, changes) = apply_recommended_settings(existing);

    for change in &changes {
        println!("{}", style(format!("✓ {change}")).green());
    }
    if !changes.iter().any(|c| c.contains("cleanupPeriodDays")) {
        println!("{}", style("✓ cleanupPeriodDays already set to 99999").dim());
    }
    if !changes.iter().any(|c| c.contains("alwaysThinkingEnabled")) {
        println!("{}", style("✓ alwaysThinkingEnabled already set to true").dim());
    }

    if !changes.is_empty() {
        if let Err(e) = save_claude_settings(&path, &settings) {
            crate::ui::error(&format!("Failed to save Claude settings: {e}"));
            return 1;
        }
        println!("\n{}", style(format!("✓ Saved Claude settings to {}", path.display())).green());
    }

    if observability {
        println!("\n{}\n", style("📊 Observability Setup").bold());
        show_otel_instructions();
    }

    println!("\n{}\n", style("📦 Claude Plugins").bold());
    ensure_claude_plugins();

    println!("\n{}\n", style("🔌 MCP Server").bold());
    ensure_tt_mcp_server();

    println!("\n{}\n", style(style("✅ Installation complete!").bold()).green());
    0
}

/// Ensure the `tt` MCP server (`tt mcp serve`) is registered with Claude Code,
/// so any Claude session can reach the store + live agent sessions. Follows the
/// plugin-install pattern: registration mutates external state, so it's gated on
/// an interactive TTY — a non-interactive run prints a dim skip note and changes
/// nothing. Registered for the `user` scope so it applies to every project.
fn ensure_tt_mcp_server() {
    let registered = match tt_exec::run("claude", &["mcp", "list"]) {
        Ok(out) if out.ok() => tt_doctor::tt_mcp_registered(&out.stdout),
        _ => {
            println!("{}", style("⚠ Could not list Claude MCP servers").yellow());
            false
        }
    };

    if registered {
        println!("{}", style("✓ tt MCP server already registered").dim());
        return;
    }

    if !std::io::stdin().is_terminal() {
        println!("{}", style("  tt MCP server skipped (non-interactive)").dim());
        return;
    }

    let install = inquire::Confirm::new("Register the tt MCP server with Claude Code?")
        .with_default(true)
        .prompt()
        .unwrap_or(false);
    if !install {
        println!("{}", style("  Skipped tt MCP server").dim());
        return;
    }

    match tt_exec::run(
        "claude",
        &[
            "mcp", "add", "--scope", "user", "tt", "--", "tt", "mcp", "serve",
        ],
    ) {
        Ok(out) if out.ok() => {
            println!("{}", style("✓ tt MCP server registered").green());
        }
        Ok(out) => {
            if !out.stdout.is_empty() {
                println!("{}", out.stdout);
            }
            if !out.stderr.is_empty() {
                println!("{}", style(&out.stderr).dim());
            }
            println!(
                "{}",
                style(format!("⚠ tt MCP registration exited with code {}", out.exit_code)).yellow()
            );
        }
        Err(e) => {
            println!("{}", style(format!("⚠ tt MCP registration failed: {e}")).yellow());
        }
    }
}

#[derive(Deserialize)]
struct PluginEntry {
    id: String,
}

fn ensure_claude_plugins() {
    let installed_ids: Vec<String> = match tt_exec::run("claude", &["plugin", "list", "--json"]) {
        Ok(out) if out.ok() => serde_json::from_str::<Vec<PluginEntry>>(&out.stdout)
            .map(|plugins| plugins.into_iter().map(|p| p.id).collect())
            .unwrap_or_default(),
        _ => {
            println!("{}", style("⚠ Could not list Claude plugins").yellow());
            Vec::new()
        }
    };
    let is_installed = |id: &str| installed_ids.iter().any(|i| i == id);

    // Add marketplaces for any missing plugins that ship one (failures are ignored:
    // the marketplace may already be present).
    for plugin in REQUIRED_PLUGINS {
        if let Some(market) = &plugin.marketplace
            && !is_installed(plugin.id)
            && let Ok(out) = tt_exec::run("claude", &["plugin", "marketplace", "add", market.url])
            && out.ok()
        {
            println!("{}", style(format!("  Added marketplace: {}", market.name)).dim());
        }
    }

    for plugin in REQUIRED_PLUGINS {
        if is_installed(plugin.id) {
            println!("{}", style(format!("✓ {} already installed", plugin.name)).dim());
            continue;
        }

        if !std::io::stdin().is_terminal() {
            println!("{}", style(format!("  {} skipped (non-interactive)", plugin.name)).dim());
            continue;
        }

        let install = inquire::Confirm::new(&format!("Install {} plugin?", plugin.name))
            .with_default(true)
            .prompt()
            .unwrap_or(false);

        if !install {
            println!("{}", style(format!("  Skipped {}", plugin.name)).dim());
            continue;
        }

        match tt_exec::run("claude", &["plugin", "install", plugin.id, "--scope", "user"]) {
            Ok(out) if out.ok() => {
                println!("{}", style(format!("✓ {} installed", plugin.name)).green());
            }
            Ok(out) => {
                if !out.stdout.is_empty() {
                    println!("{}", out.stdout);
                }
                if !out.stderr.is_empty() {
                    println!("{}", style(&out.stderr).dim());
                }
                println!(
                    "{}",
                    style(format!("⚠ {} install exited with code {}", plugin.name, out.exit_code))
                        .yellow()
                );
            }
            Err(e) => {
                println!("{}", style(format!("⚠ {} install failed: {e}", plugin.name)).yellow());
            }
        }
    }
}

fn show_otel_instructions() {
    println!("{}\n", style("Add these environment variables to your shell profile:").cyan());
    let vars = "export CLAUDE_CODE_ENABLE_TELEMETRY=1
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317";
    for line in vars.lines() {
        println!("  {line}");
    }
    println!();
    println!(
        "{}",
        style("For more info, see: https://github.com/anthropics/claude-code-monitoring-guide")
            .dim()
    );
    println!();
    println!("{}", style("Quick cost analysis (no setup required):").cyan());
    println!("{}", style("  npx ccusage@latest --breakdown").dim());
}
