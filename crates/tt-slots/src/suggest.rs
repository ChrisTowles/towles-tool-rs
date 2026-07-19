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
//!
//! The shape of the answer is the CLI's problem, not ours: `--json-schema`
//! makes `claude` route the model through a structured-output tool and hand
//! back a validated object in its `--output-format json` envelope, so there's
//! no JSON-out-of-prose extraction here. Anything that still goes wrong lands
//! on [`local_fallback`] instead of an error — a "Suggest" button that can
//! only ever fill the fields in.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Generous — a cold `claude` CLI (auth check, MCP startup) can take a while,
/// but this is a manual, one-shot user action, not a background poll.
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(60);

/// Mirrors the dialog's own `BRANCH_SLUG_SOURCE_CHARS`.
const BRANCH_SLUG_SOURCE_CHARS: usize = 50;

/// JSON Schema handed to `claude -p --json-schema`, which makes the CLI itself
/// enforce the shape: the model answers through a structured-output tool and
/// the envelope carries a validated `structured_output` object. That's the
/// difference between "we asked nicely for JSON" and "the CLI guarantees it".
const SUGGESTION_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "branch": { "type": "string", "description": "legal git ref: lowercase kebab-case, prefixed feat/, fix/, or chore/" },
    "goal": { "type": "string", "description": "one clear, concise sentence restating the task" }
  },
  "required": ["branch", "goal"],
  "additionalProperties": false
}"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    pub branch: String,
    pub goal: String,
}

/// What [`suggest`] hands back: always a usable suggestion. `fallback` is
/// `Some(why)` when `claude` couldn't be reached or answered unusably and the
/// branch/goal were derived locally instead — the dialog shows that as a note,
/// not an error, because the fields still got filled with something sane.
///
/// Serializes flat (`{branch, goal, fallback}`) so the Tauri command can hand
/// it straight to the dialog instead of restating the fields in a parallel
/// payload type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suggested {
    #[serde(flatten)]
    pub suggestion: Suggestion,
    pub fallback: Option<String>,
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
///
/// Never fails while the user gave us anything to slug (see [`Suggested`]).
/// Only an image-only brief can still error, and even then the dialog's typed
/// fields are left untouched.
pub fn suggest(cwd: &Path, goal: &str, images: &[String]) -> Result<Suggested> {
    match ask_claude(cwd, goal, images) {
        Ok(suggestion) => Ok(Suggested { suggestion, fallback: None }),
        Err(e) => local_fallback(goal)
            .map(|suggestion| Suggested { suggestion, fallback: Some(e.to_string()) })
            .ok_or(e),
    }
}

fn ask_claude(cwd: &Path, goal: &str, images: &[String]) -> Result<Suggestion> {
    let prompt = prompt_for(goal, images);
    let out =
        tt_exec::run_in_dir_with_timeout("claude", &claude_args(&prompt), cwd, CLAUDE_TIMEOUT)
            .map_err(|e| SuggestError::Exec(e.to_string()))?;
    if !out.ok() {
        return Err(SuggestError::Failed(out.stderr.trim().to_string()));
    }
    parse_response(&out.stdout)
}

/// Split out so a test can assert the flags that make the answer's shape the
/// CLI's problem — asserting against [`SUGGESTION_SCHEMA`] alone would pass
/// even if `--json-schema` were dropped from the command.
fn claude_args(prompt: &str) -> [&str; 6] {
    [
        "-p",
        prompt,
        "--output-format",
        "json",
        "--json-schema",
        SUGGESTION_SCHEMA,
    ]
}

/// The `--output-format json` envelope. Only the three fields that decide the
/// outcome are named; everything else (usage, cost, session id) is ignored.
#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    is_error: bool,
    /// The schema-validated object `--json-schema` guarantees.
    #[serde(default)]
    structured_output: Option<Suggestion>,
    /// Only read for its error message, when `is_error`.
    #[serde(default)]
    result: Option<String>,
}

/// Read the envelope, and nothing else. A `claude` too old for these flags
/// exits non-zero on the unknown argument and never reaches here, so there is
/// no older-CLI text shape worth carrying — and a schema regression should
/// surface as a fallback the user can see, not be silently rescued by a
/// lenient re-read.
fn parse_response(stdout: &str) -> Result<Suggestion> {
    let env: Envelope =
        serde_json::from_str(stdout.trim()).map_err(|_| SuggestError::Unparseable)?;
    if env.is_error {
        return Err(SuggestError::Failed(env.result.unwrap_or_default().trim().to_string()));
    }
    env.structured_output
        .map(|s| Suggestion {
            branch: s.branch.trim().to_string(),
            goal: s.goal.trim().to_string(),
        })
        .filter(|s| !s.branch.is_empty() && !s.goal.is_empty())
        .ok_or(SuggestError::Unparseable)
}

/// Derive a suggestion without `claude` at all: the goal as typed, and the
/// same `feat/<slug>` the dialog's branch field already derives — same rules
/// and source-char budget, through the one shared slug helper, so the two
/// can't disagree about what the branch should be.
fn local_fallback(goal: &str) -> Option<Suggestion> {
    let goal = goal.trim();
    let slug =
        tt_git::branch_name::slug(&goal.chars().take(BRANCH_SLUG_SOURCE_CHARS).collect::<String>());
    (!slug.is_empty())
        .then(|| Suggestion { branch: format!("feat/{slug}"), goal: goal.to_string() })
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
         new git worktree in this repository. Answer with the required \
         structured output: a `branch` like \"feat/short-kebab-slug\" and a \
         `goal` that clearly and concisely restates the task. The branch must \
         be a legal git ref name: lowercase, kebab-case, prefixed with feat/, \
         fix/, or chore/ as fits the goal.{image_task} \
         {image_rule}\n\nGoal: {goal_line}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_call_makes_the_cli_enforce_the_schema() {
        // Asserting against SUGGESTION_SCHEMA alone would still pass with the
        // flags dropped, so check the command that actually gets run.
        let args = claude_args("the prompt");
        assert_eq!(args[2..4], ["--output-format", "json"]);
        assert_eq!(args[4], "--json-schema");
        let schema: serde_json::Value = serde_json::from_str(args[5]).unwrap();
        assert_eq!(schema["required"], serde_json::json!(["branch", "goal"]));
        assert_eq!(schema["additionalProperties"], serde_json::json!(false));
    }

    #[test]
    fn serializes_flat_for_the_dialog() {
        // The dialog reads {branch, goal, fallback}; `flatten` is what keeps
        // the Tauri command from needing a parallel payload struct.
        let s = Suggested {
            suggestion: Suggestion { branch: "feat/a".into(), goal: "do a".into() },
            fallback: None,
        };
        let json: serde_json::Value = serde_json::to_value(&s).unwrap();
        assert_eq!(json, serde_json::json!({"branch": "feat/a", "goal": "do a", "fallback": null}));
    }

    #[test]
    fn reads_the_envelopes_validated_structured_output() {
        let raw = r#"{"type":"result","is_error":false,
            "result":"{\"branch\":\"ignored\",\"goal\":\"ignored\"}",
            "structured_output":{"branch":"feat/a","goal":"do a"},
            "total_cost_usd":0.28}"#;
        let s = parse_response(raw).unwrap();
        assert_eq!(s, Suggestion { branch: "feat/a".into(), goal: "do a".into() });
    }

    #[test]
    fn an_envelope_without_structured_output_is_unparseable() {
        // Deliberately not re-read out of `result`: that hedge would silently
        // paper over a schema regression. It falls through to the local
        // fallback, which the user can see.
        let raw = r#"{"type":"result","is_error":false,
            "result":"Sure! {\"branch\":\"fix/b\",\"goal\":\"fix b\"}"}"#;
        assert!(matches!(parse_response(raw), Err(SuggestError::Unparseable)));
    }

    #[test]
    fn an_error_envelope_reports_its_message() {
        let raw = r#"{"type":"result","is_error":true,"result":"credit balance too low"}"#;
        let e = parse_response(raw).unwrap_err();
        assert!(e.to_string().contains("credit balance too low"), "{e}");
    }

    #[test]
    fn blank_fields_count_as_unparseable() {
        let raw = r#"{"is_error":false,"structured_output":{"branch":"  ","goal":"x"}}"#;
        assert!(matches!(parse_response(raw), Err(SuggestError::Unparseable)));
    }

    #[test]
    fn a_local_fallback_mirrors_the_dialogs_own_branch_slug() {
        let s = local_fallback("  I want All tasks: agentboard → kanban!  ").unwrap();
        assert_eq!(s.branch, "feat/i-want-all-tasks-agentboard-kanban");
        assert_eq!(s.goal, "I want All tasks: agentboard → kanban!");
    }

    #[test]
    fn a_long_goal_only_slugs_its_opening() {
        let s = local_fallback(&"word ".repeat(40)).unwrap();
        assert!(s.branch.len() <= "feat/".len() + BRANCH_SLUG_SOURCE_CHARS, "{}", s.branch);
    }

    #[test]
    fn nothing_to_slug_means_no_fallback() {
        // An image-only brief with no typed text: there is genuinely nothing
        // to derive, so the caller surfaces the real error instead.
        assert_eq!(local_fallback("   "), None);
        assert_eq!(local_fallback("!!!"), None);
    }
}
