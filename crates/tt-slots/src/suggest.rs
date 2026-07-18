//! `claude -p`-backed branch-name/goal suggestion for the new-slot dialog.
//!
//! Manual, user-triggered only (never runs on a timer or keystroke) — the
//! dialog fills its editable fields with the result and the user can still
//! edit or undo before creating the slot, so this never writes anything
//! itself. Read-only by construction: the prompt tells `claude -p` not to
//! read or write repo files, just answer from the goal text and its own
//! knowledge of the repo it's pointed at (cwd = the repo checkout the dialog
//! is open for, so it has real CLAUDE.md/branch-convention context).
//!
//! The one carve-out is attached screenshots, which are named by path and
//! explicitly readable — a pasted image is frequently the entire brief, and
//! without it an image-only request yields a generic suggestion.

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
///
/// `images` are absolute paths to screenshots the user attached (already
/// staged by [`crate::pasted`]). A screenshot is often the *whole* brief —
/// "make it look like this" with no typed goal at all — so they're named in
/// the prompt and reading them is explicitly allowed, unlike every other
/// file.
pub fn suggest(cwd: &Path, goal: &str, images: &[String]) -> Result<Suggestion> {
    let prompt = prompt_for(goal, images);
    let out = tt_exec::run_in_dir_with_timeout("claude", &["-p", &prompt], cwd, CLAUDE_TIMEOUT)
        .map_err(|e| SuggestError::Exec(e.to_string()))?;
    if !out.ok() {
        return Err(SuggestError::Failed(out.stderr.trim().to_string()));
    }
    parse_suggestion(&out.stdout).ok_or(SuggestError::Unparseable)
}

fn prompt_for(goal: &str, images: &[String]) -> String {
    // Reading the attached screenshots is the one carve-out from the
    // otherwise blanket "touch nothing" rule: without it the model answers
    // from the goal text alone and an image-only brief yields a generic
    // suggestion. The carve-out is enumerated by path, not a general
    // permission to read the repo.
    let (image_rule, image_task) = if images.is_empty() {
        (
            "Do not read or write any files and do not run any commands — just answer \
             from the goal text and what you already know about this repository's \
             conventions."
                .to_string(),
            String::new(),
        )
    } else {
        let list = images.join(" ");
        (
            format!(
                "Read ONLY these attached image files, which describe the task: {list}. \
                 Do not read or write any other file and do not run any commands — \
                 otherwise answer from the images, the goal text, and what you already \
                 know about this repository's conventions."
            ),
            format!(
                " The attached image{} {} the task; base the goal on what {} show{}.",
                if images.len() == 1 { "" } else { "s" },
                if images.len() == 1 { "describes" } else { "describe" },
                if images.len() == 1 { "it" } else { "they" },
                if images.len() == 1 { "s" } else { "" },
            ),
        )
    };
    let goal_line = if goal.trim().is_empty() { "(no goal text — use the images)" } else { goal };
    format!(
        "You are naming a git branch and tidying a one-line task goal for a \
         new git worktree in this repository. Reply with ONLY a JSON object (no \
         prose, no markdown code fences) of the shape {{\"branch\": \
         \"feat/short-kebab-slug\", \"goal\": \"a clear, concise restatement of the \
         goal\"}}. The branch must be a legal git ref name: lowercase, kebab-case, \
         prefixed with feat/, fix/, or chore/ as fits the goal.{image_task} \
         {image_rule}\n\nGoal: {goal_line}"
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
    fn without_images_the_prompt_forbids_touching_anything() {
        let p = prompt_for("add a thing", &[]);
        assert!(p.contains("Do not read or write any files"));
        assert!(p.contains("Goal: add a thing"));
    }

    #[test]
    fn with_images_the_prompt_names_them_and_allows_reading_only_them() {
        let images = vec!["/stage/paste-1.png".to_string()];
        let p = prompt_for("match this", &images);
        assert!(p.contains("/stage/paste-1.png"), "the path must be in the prompt");
        assert!(p.contains("Read ONLY these attached image files"));
        // The carve-out must stay a carve-out — still no general repo access.
        assert!(p.contains("Do not read or write any other file"));
        assert!(!p.contains("Do not read or write any files"));
    }

    #[test]
    fn an_image_only_brief_still_asks_for_a_goal() {
        // Pasting a screenshot with no typed text is a complete brief; the
        // prompt has to say so rather than sending an empty "Goal:" line that
        // reads like a mistake.
        let p = prompt_for("   ", &["/stage/paste-1.png".to_string()]);
        assert!(p.contains("(no goal text — use the images)"));
        assert!(p.contains("base the goal on what it shows"));
    }

    #[test]
    fn several_images_read_as_plural() {
        let images = vec!["/a/paste-1.png".to_string(), "/a/paste-2.png".to_string()];
        let p = prompt_for("compare", &images);
        assert!(p.contains("/a/paste-1.png /a/paste-2.png"));
        assert!(p.contains("images describe the task"));
        assert!(p.contains("what they show."));
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
