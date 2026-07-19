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
//! back a validated object in its `--output-format json` envelope. The text
//! paths below only exist for a CLI that ignores those flags. And if the call
//! fails outright, the branch/goal are derived locally rather than surfaced as
//! an error — a "Suggest" button that can only ever fill the fields in.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

/// Generous — a cold `claude` CLI (auth check, MCP startup) can take a while,
/// but this is a manual, one-shot user action, not a background poll.
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(60);

/// How much of the goal the local fallback slugs into a branch name — mirrors
/// the dialog's own `BRANCH_SLUG_SOURCE_CHARS`, so a fallback branch looks
/// like the one the field already had rather than a surprise.
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Suggestion {
    pub branch: String,
    pub goal: String,
}

/// What [`suggest`] hands back: always a usable suggestion. `fallback` is
/// `Some(why)` when `claude` couldn't be reached or answered unusably and the
/// branch/goal were derived locally instead — the dialog shows that as a note,
/// not an error, because the fields still got filled with something sane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggested {
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
/// Never fails while the user gave us anything to work with: if `claude` is
/// missing, times out, or answers unusably, the branch/goal are derived
/// locally from the goal text and returned with a `fallback` note. Only an
/// image-only brief — nothing to slug — can still error, and even then the
/// dialog's typed fields are left untouched.
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
    let args = [
        "-p",
        &prompt,
        "--output-format",
        "json",
        "--json-schema",
        SUGGESTION_SCHEMA,
    ];
    let out = tt_exec::run_in_dir_with_timeout("claude", &args, cwd, CLAUDE_TIMEOUT)
        .map_err(|e| SuggestError::Exec(e.to_string()))?;
    if !out.ok() {
        return Err(SuggestError::Failed(out.stderr.trim().to_string()));
    }
    parse_response(&out.stdout)
}

/// The `--output-format json` envelope. Only the three fields that decide the
/// outcome are named; everything else (usage, cost, session id) is ignored.
#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    is_error: bool,
    /// The schema-validated object, when the CLI enforced `--json-schema`.
    #[serde(default)]
    structured_output: Option<Suggestion>,
    /// The assistant's final text — also the error message when `is_error`.
    #[serde(default)]
    result: Option<String>,
}

/// Prefer the CLI's schema-validated `structured_output`; fall back through
/// the envelope's `result` text and then the raw stdout, so an older `claude`
/// that ignores `--json-schema` (or prints plain text) still works.
fn parse_response(stdout: &str) -> Result<Suggestion> {
    let Ok(env) = serde_json::from_str::<Envelope>(stdout.trim()) else {
        return parse_suggestion(stdout).ok_or(SuggestError::Unparseable);
    };
    if env.is_error {
        return Err(SuggestError::Failed(env.result.unwrap_or_default().trim().to_string()));
    }
    env.structured_output
        .or_else(|| env.result.as_deref().and_then(parse_suggestion))
        .filter(|s| !s.branch.trim().is_empty() && !s.goal.trim().is_empty())
        .map(|s| Suggestion {
            branch: s.branch.trim().to_string(),
            goal: s.goal.trim().to_string(),
        })
        .ok_or(SuggestError::Unparseable)
}

/// Derive a suggestion without `claude` at all: the goal as typed, and the
/// same `feat/<slug>` the dialog's branch field already derives. Not clever,
/// but it's what the user would have shipped anyway — far better than an
/// error that leaves the button looking broken.
fn local_fallback(goal: &str) -> Option<Suggestion> {
    let goal = goal.trim();
    let slug = slugify(goal.chars().take(BRANCH_SLUG_SOURCE_CHARS).collect::<String>().trim());
    (!slug.is_empty())
        .then(|| Suggestion { branch: format!("feat/{slug}"), goal: goal.to_string() })
}

/// Lowercase, spaces to `-`, anything outside `[0-9a-z_-]` to `-`, collapse
/// runs, strip trailing — the same rules as the dialog's `slugify` and
/// tt-git's branch-name slug.
fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.to_lowercase().chars() {
        let c = if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
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

/// Last-resort text path, only reached when the CLI didn't hand us a
/// schema-validated object: strip an optional ```json fence, and failing that
/// carve out the outermost `{...}` so a "Sure! {...}" preamble still parses.
/// Strictness bought nothing here — the alternative to a lenient read is a
/// dead button.
fn parse_suggestion(raw: &str) -> Option<Suggestion> {
    let text = raw.trim();
    let text = text.strip_prefix("```json").or_else(|| text.strip_prefix("```")).unwrap_or(text);
    let text = text.strip_suffix("```").unwrap_or(text).trim();
    if let Ok(s) = serde_json::from_str::<Suggestion>(text) {
        return Some(s);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(text.get(start..=end)?).ok()
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
    fn prose_around_json_is_still_extracted() {
        // The old behavior — refusing anything but bare JSON — is what put
        // "couldn't parse a suggestion" in front of the user. This path only
        // runs when the CLI didn't enforce the schema, so read it leniently.
        let s = parse_suggestion("Sure! {\"branch\": \"x\", \"goal\": \"y\"} — hope that helps");
        assert_eq!(s, Some(Suggestion { branch: "x".into(), goal: "y".into() }));
    }

    #[test]
    fn the_call_asks_the_cli_to_enforce_the_schema() {
        let schema: serde_json::Value = serde_json::from_str(SUGGESTION_SCHEMA).unwrap();
        assert_eq!(schema["required"], serde_json::json!(["branch", "goal"]));
        assert_eq!(schema["additionalProperties"], serde_json::json!(false));
    }

    #[test]
    fn prefers_the_envelopes_validated_structured_output() {
        let raw = r#"{"type":"result","is_error":false,
            "result":"{\"branch\":\"ignored\",\"goal\":\"ignored\"}",
            "structured_output":{"branch":"feat/a","goal":"do a"},
            "total_cost_usd":0.28}"#;
        let s = parse_response(raw).unwrap();
        assert_eq!(s, Suggestion { branch: "feat/a".into(), goal: "do a".into() });
    }

    #[test]
    fn falls_back_to_the_envelopes_result_text() {
        // An older CLI that ignores --json-schema still answers in `result`.
        let raw = r#"{"type":"result","is_error":false,
            "result":"```json\n{\"branch\":\"fix/b\",\"goal\":\"fix b\"}\n```"}"#;
        let s = parse_response(raw).unwrap();
        assert_eq!(s, Suggestion { branch: "fix/b".into(), goal: "fix b".into() });
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
