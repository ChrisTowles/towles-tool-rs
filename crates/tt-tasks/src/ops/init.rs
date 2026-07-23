//! One-shot repo onboarding (`tt task init`): template choice, `.gitignore`,
//! the Claude Code worktree hooks, and the primary checkout's first render.

use std::fs;
use std::path::PathBuf;

use super::render::{RenderSummary, init_template_sidecar, render_task_env, template_sidecar_path};
use super::{OpsError, Result, TaskRoot, git_checkout};
use crate::layout;

/// The two Claude Code worktree hooks `tt task init` wires so `claude
/// --worktree` and background sessions route through the task machinery.
pub(crate) const WORKTREE_HOOKS: [(&str, &str); 2] = [
    ("WorktreeCreate", "tt task hook-create"),
    ("WorktreeRemove", "tt task hook-remove"),
];

/// What [`init_repo`] did (every step is idempotent, so re-runs report
/// mostly `false`/unchanged).
pub struct InitReport {
    /// The template tasks will render from: the repo's tokenized
    /// `.env.example`, or the `.claude/task-env.template` sidecar.
    pub template: PathBuf,
    pub sidecar_created: bool,
    /// `.env` was appended to the repo's `.gitignore`.
    pub gitignore_added: bool,
    /// The worktree hooks were added to `.claude/settings.json`.
    pub hooks_wired: bool,
    pub settings_path: PathBuf,
    /// The primary checkout's `.env` render (it claims ports like any task).
    pub render: RenderSummary,
}

/// Onboard a repo onto the task convention in one idempotent shot: pick (or
/// create) the env template, gitignore `.env`, wire the Claude Code
/// WorktreeCreate/WorktreeRemove hooks into `.claude/settings.json`, and
/// render the primary checkout's `.env` so it claims its ports. The hook
/// wiring only takes effect in new worktrees once the settings file is
/// committed — the caller surfaces that reminder.
pub fn init_repo(sr: &TaskRoot) -> Result<InitReport> {
    // Template: the committed tokenized .env.example wins; otherwise make
    // sure the sidecar exists (empty-but-explained when freshly created).
    let repo_template = sr.checkout.join(".env.example");
    let has_tokenized_example =
        fs::read_to_string(&repo_template).is_ok_and(|text| text.contains("${tt:"));
    let (template, sidecar_created) = if has_tokenized_example {
        (repo_template, false)
    } else {
        let existed = template_sidecar_path(sr).is_file();
        (init_template_sidecar(sr)?, !existed)
    };

    // Gitignore `.env` only when git says it is definitely not ignored
    // (check-ignore exits 1); an errored probe (128 — odd repo state) must
    // not append blindly.
    let mut gitignore_added = false;
    if let Ok(out) = git_checkout(&sr.checkout, &["check-ignore", "-q", ".env"])
        && out.exit_code == 1
    {
        let gitignore = sr.checkout.join(".gitignore");
        let mut current = fs::read_to_string(&gitignore).unwrap_or_default();
        if !current.is_empty() && !current.ends_with('\n') {
            current.push('\n');
        }
        current.push_str(".env\n");
        fs::write(&gitignore, current)
            .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", gitignore.display())))?;
        gitignore_added = true;
    }

    // Hooks: merge into the committed settings file, preserving everything
    // already there.
    let settings_path = sr.checkout.join(layout::CLAUDE_DIR).join("settings.json");
    let current = fs::read_to_string(&settings_path).unwrap_or_default();
    let (wired_text, hooks_wired) = wire_worktree_hooks(&current)?;
    if hooks_wired {
        let claude_dir = sr.checkout.join(layout::CLAUDE_DIR);
        fs::create_dir_all(&claude_dir)
            .map_err(|e| OpsError::Io(format!("cannot create {}: {e}", claude_dir.display())))?;
        fs::write(&settings_path, wired_text)
            .map_err(|e| OpsError::Io(format!("cannot write {}: {e}", settings_path.display())))?;
    }

    let render = render_task_env(sr, &sr.checkout, None)?;
    Ok(InitReport {
        template,
        sidecar_created,
        gitignore_added,
        hooks_wired,
        settings_path,
        render,
    })
}

/// Merge the [`WORKTREE_HOOKS`] into a `.claude/settings.json` document,
/// preserving every existing key/hook. Returns the new JSON text and whether
/// anything changed (an event already carrying its `tt task hook-*` command
/// anywhere in its entries is left alone). Empty input starts from `{}`;
/// malformed JSON is an error — never clobber a file we can't parse.
pub fn wire_worktree_hooks(settings: &str) -> Result<(String, bool)> {
    let mut doc: serde_json::Value = if settings.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(settings)
            .map_err(|e| OpsError::Io(format!(".claude/settings.json is not valid JSON: {e}")))?
    };
    if !doc.is_object() {
        return Err(OpsError::Io(".claude/settings.json is not a JSON object".to_string()));
    }

    let mut changed = false;
    let hooks = doc
        .as_object_mut()
        .expect("checked is_object above")
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        return Err(OpsError::Io(".claude/settings.json `hooks` is not an object".to_string()));
    }
    for (event, command) in WORKTREE_HOOKS {
        let entries = hooks
            .as_object_mut()
            .expect("checked is_object above")
            .entry(event)
            .or_insert_with(|| serde_json::json!([]));
        if !entries.is_array() {
            return Err(OpsError::Io(format!(
                ".claude/settings.json `hooks.{event}` is not an array"
            )));
        }
        let already = entries.as_array().expect("checked is_array above").iter().any(|entry| {
            entry.get("hooks").and_then(|h| h.as_array()).is_some_and(|hs| {
                hs.iter().any(|h| h.get("command").and_then(|c| c.as_str()) == Some(command))
            })
        });
        if !already {
            entries
                .as_array_mut()
                .expect("checked is_array above")
                .push(serde_json::json!({ "hooks": [{ "type": "command", "command": command }] }));
            changed = true;
        }
    }

    let mut text = serde_json::to_string_pretty(&doc)
        .map_err(|e| OpsError::Io(format!("cannot serialize settings.json: {e}")))?;
    text.push('\n');
    Ok((text, changed))
}
