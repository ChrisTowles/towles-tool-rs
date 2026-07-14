//! `claude -p`-backed branch-name/goal suggestion for the new-slot dialog.
//!
//! Manual, user-triggered only (never runs on a timer or keystroke) — the
//! dialog fills its editable fields with the result and the user can still
//! edit or undo before creating the slot, so this never writes anything
//! itself. Read-only by construction: the prompt tells `claude -p` not to
//! read or write repo files, just answer from the goal text and its own
//! knowledge of the repo it's pointed at (cwd = the repo checkout the dialog
//! is open for, so it has real CLAUDE.md/branch-convention context).

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

/// Generous — a cold `claude` CLI (auth check, MCP startup) can take a while,
/// but this is a manual, one-shot user action, not a background poll.
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Suggestion {
    pub branch: String,
    pub goal: String,
}

#[derive(Debug, Error)]
pub enum SuggestError {
    #[error("claude: {0}")]
    Exec(String),
    #[error("claude -p failed:\n{0}")]
    Failed(String),
    #[error("couldn't parse a suggestion out of claude's response")]
    Unparseable,
}

pub type Result<T> = std::result::Result<T, SuggestError>;

/// Ask `claude -p` (run with cwd = `cwd`) to propose a cleaned-up goal and a
/// legal, kebab-case branch name for it.
pub fn suggest(cwd: &Path, goal: &str) -> Result<Suggestion> {
    let prompt = prompt_for(goal);
    let out = tt_exec::run_in_dir_with_timeout("claude", &["-p", &prompt], cwd, CLAUDE_TIMEOUT)
        .map_err(|e| SuggestError::Exec(e.to_string()))?;
    if !out.ok() {
        return Err(SuggestError::Failed(out.stderr.trim().to_string()));
    }
    parse_suggestion(&out.stdout).ok_or(SuggestError::Unparseable)
}

fn prompt_for(goal: &str) -> String {
    format!(
        "You are naming a git branch and tidying a one-line task goal for a \
         new git worktree in this repository. Given the goal below, reply \
         with ONLY a JSON object (no prose, no markdown code fences) of the \
         shape {{\"branch\": \"feat/short-kebab-slug\", \"goal\": \"a clear, \
         concise restatement of the goal\"}}. The branch must be a legal git \
         ref name: lowercase, kebab-case, prefixed with feat/, fix/, or \
         chore/ as fits the goal. Do not read or write any files and do not \
         run any commands — just answer from the goal text and what you \
         already know about this repository's conventions.\n\nGoal: {goal}"
    )
}

/// Strip an optional ```json fence, then parse. `claude -p` is asked for bare
/// JSON, but models reach for a fence out of habit often enough to bother.
fn parse_suggestion(raw: &str) -> Option<Suggestion> {
    let text = raw.trim();
    let text = text.strip_prefix("```json").or_else(|| text.strip_prefix("```")).unwrap_or(text);
    let text = text.strip_suffix("```").unwrap_or(text).trim();
    serde_json::from_str(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_json() {
        let s = parse_suggestion(r#"{"branch": "feat/add-thing", "goal": "add the thing"}"#);
        assert_eq!(
            s,
            Some(Suggestion { branch: "feat/add-thing".into(), goal: "add the thing".into() })
        );
    }

    #[test]
    fn strips_a_json_code_fence() {
        let raw = "```json\n{\"branch\": \"fix/bug\", \"goal\": \"fix the bug\"}\n```";
        let s = parse_suggestion(raw);
        assert_eq!(s, Some(Suggestion { branch: "fix/bug".into(), goal: "fix the bug".into() }));
    }

    #[test]
    fn prose_around_json_fails_to_parse() {
        // Deliberately strict: this is a controlled prompt to our own model,
        // not third-party output, so no lenient extraction is worth the
        // complexity — a malformed reply just surfaces as "unparseable" and
        // the user's typed text is untouched.
        assert_eq!(parse_suggestion("Sure! {\"branch\": \"x\", \"goal\": \"y\"}"), None);
    }
}
