//! Renderer for the `{tt:...}` env-template grammar.
//!
//! Tokens (anywhere in a non-comment line):
//! - `{tt:port A-B}`  — port-pool claim: reuse the slot's existing in-range
//!   claim when no sibling holds it, else the first port in `A..=B` that no
//!   sibling claims and that passes the caller's `port_free` probe.
//! - `{tt:slot}`      — the slot number, e.g. `2`
//! - `{tt:slot-name}` — the slot directory basename, e.g. `blog-slot-2`
//! - `{tt:base}`      — the base branch this slot's work PRs into
//! - `{tt:var NAME}`  — the rendered value of `NAME` from an earlier line
//!
//! Unknown or malformed tokens are hard errors (typos must not render as
//! literal text into a config file). Comment lines pass through untouched so
//! templates can show example tokens without claiming ports.
//!
//! Rendering is idempotent by construction: the caller passes the slot's
//! current `.env` assignments as `existing`, and in-range claims are reused
//! instead of re-picked — re-rendering a slot never rotates its ports.

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::envfile;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TemplateError {
    #[error("line {line}: port pool {lo}-{hi} exhausted — widen the range in the template")]
    PoolExhausted { line: usize, lo: u16, hi: u16 },

    #[error("line {line}: invalid port range {lo}-{hi}")]
    InvalidRange { line: usize, lo: u16, hi: u16 },

    #[error("line {line}: {{tt:var {name}}} referenced before {name} is defined")]
    VarBeforeDef { line: usize, name: String },

    #[error("line {line}: unknown or malformed token `{token}`")]
    UnknownToken { line: usize, token: String },
}

/// Identity of the slot being rendered.
pub struct SlotContext<'a> {
    pub slot_name: &'a str,
    pub slot_number: u32,
    pub base_branch: &'a str,
}

/// A finished render: the output text plus which ports were freshly claimed
/// and which were reused from the slot's existing `.env`.
#[derive(Debug, Default)]
pub struct RenderOutcome {
    pub text: String,
    pub claimed: Vec<(String, u16)>,
    pub reused: Vec<(String, u16)>,
}

/// Render `template` for one slot. `existing` is the slot's current `.env`
/// (reuse source), `sibling_claims` the union of every *other* slot's port
/// claims — the caller must exclude the slot itself, or re-renders would see
/// their own claims as taken and rotate every port. `port_free` is the
/// machine-level availability probe (a real bind test at the CLI layer);
/// it is consulted only for fresh claims, never for reuses (the slot's own
/// running server would otherwise block its own re-render).
pub fn render(
    template: &str,
    ctx: &SlotContext<'_>,
    existing: &BTreeMap<String, String>,
    sibling_claims: &BTreeSet<u16>,
    mut port_free: impl FnMut(u16) -> bool,
) -> Result<RenderOutcome, TemplateError> {
    let mut out = RenderOutcome::default();
    let mut session: BTreeSet<u16> = BTreeSet::new();
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    let mut reuse_spent: BTreeSet<String> = BTreeSet::new();

    for (idx, raw_line) in template.lines().enumerate() {
        let line_no = idx + 1;
        if raw_line.trim_start().starts_with('#') {
            out.text.push_str(raw_line);
            out.text.push('\n');
            continue;
        }
        let key = envfile::line_key(raw_line).map(str::to_string);
        let mut line = raw_line.to_string();

        while let Some(start) = line.find("{tt:") {
            let Some(rel_end) = line[start..].find('}') else {
                return Err(TemplateError::UnknownToken {
                    line: line_no,
                    token: line[start..].to_string(),
                });
            };
            let end = start + rel_end;
            let token = line[start..=end].to_string();
            let inner = &line[start + 4..end];

            let replacement = if let Some(range) = inner.strip_prefix("port ") {
                let (lo, hi) = parse_range(range).ok_or_else(|| TemplateError::UnknownToken {
                    line: line_no,
                    token: token.clone(),
                })?;
                if lo > hi || lo == 0 {
                    return Err(TemplateError::InvalidRange { line: line_no, lo, hi });
                }
                let port = pick_port(
                    lo,
                    hi,
                    key.as_deref(),
                    existing,
                    sibling_claims,
                    &mut session,
                    &mut reuse_spent,
                    &mut port_free,
                )
                .ok_or(TemplateError::PoolExhausted { line: line_no, lo, hi })?;
                match port {
                    Picked::Reused(p) => {
                        out.reused.push((key.clone().unwrap_or_default(), p));
                        p.to_string()
                    }
                    Picked::Fresh(p) => {
                        out.claimed.push((key.clone().unwrap_or_default(), p));
                        p.to_string()
                    }
                }
            } else if inner == "slot" {
                ctx.slot_number.to_string()
            } else if inner == "slot-name" {
                ctx.slot_name.to_string()
            } else if inner == "base" {
                ctx.base_branch.to_string()
            } else if let Some(name) = inner.strip_prefix("var ") {
                vars.get(name).cloned().ok_or_else(|| TemplateError::VarBeforeDef {
                    line: line_no,
                    name: name.to_string(),
                })?
            } else {
                return Err(TemplateError::UnknownToken { line: line_no, token });
            };

            line.replace_range(start..=end, &replacement);
        }

        if let Some(k) = envfile::line_key(&line) {
            let value = line.trim_start()[k.len() + 1..].to_string();
            vars.insert(k.to_string(), value);
        }
        out.text.push_str(&line);
        out.text.push('\n');
    }
    Ok(out)
}

enum Picked {
    Reused(u16),
    Fresh(u16),
}

fn parse_range(range: &str) -> Option<(u16, u16)> {
    let (lo, hi) = range.split_once('-')?;
    Some((lo.trim().parse().ok()?, hi.trim().parse().ok()?))
}

#[allow(clippy::too_many_arguments)]
fn pick_port(
    lo: u16,
    hi: u16,
    key: Option<&str>,
    existing: &BTreeMap<String, String>,
    sibling_claims: &BTreeSet<u16>,
    session: &mut BTreeSet<u16>,
    reuse_spent: &mut BTreeSet<String>,
    port_free: &mut impl FnMut(u16) -> bool,
) -> Option<Picked> {
    if let Some(k) = key
        && !reuse_spent.contains(k)
        && let Some(prev) = existing.get(k).and_then(|v| v.parse::<u16>().ok())
        && (lo..=hi).contains(&prev)
        && !sibling_claims.contains(&prev)
        && !session.contains(&prev)
    {
        session.insert(prev);
        reuse_spent.insert(k.to_string());
        return Some(Picked::Reused(prev));
    }
    let fresh =
        (lo..=hi).find(|p| !sibling_claims.contains(p) && !session.contains(p) && port_free(*p))?;
    session.insert(fresh);
    Some(Picked::Fresh(fresh))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> SlotContext<'static> {
        SlotContext { slot_name: "blog-slot-2", slot_number: 2, base_branch: "main" }
    }

    fn render_ok(
        template: &str,
        existing: &[(&str, &str)],
        sibling: &[u16],
    ) -> Result<RenderOutcome, TemplateError> {
        let existing: BTreeMap<String, String> =
            existing.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let sibling: BTreeSet<u16> = sibling.iter().copied().collect();
        render(template, &ctx(), &existing, &sibling, |_| true)
    }

    #[test]
    fn claims_first_free_port_and_substitutes_identity() {
        let out = render_ok(
            "UI_PORT={tt:port 3000-3009}\nNAME={tt:slot-name}\nN={tt:slot}\nBASE={tt:base}\n",
            &[],
            &[3000, 3001],
        )
        .unwrap();
        assert!(out.text.contains("UI_PORT=3002"));
        assert!(out.text.contains("NAME=blog-slot-2"));
        assert!(out.text.contains("N=2"));
        assert!(out.text.contains("BASE=main"));
        assert_eq!(out.claimed, vec![("UI_PORT".to_string(), 3002)]);
    }

    #[test]
    fn rerender_reuses_existing_claim_even_when_listening() {
        // port_free = false everywhere: the slot's own server is listening on
        // 3005, but reuse must still win — re-renders never rotate ports.
        let existing: BTreeMap<String, String> =
            [("UI_PORT".to_string(), "3005".to_string())].into();
        let out =
            render("UI_PORT={tt:port 3000-3009}\n", &ctx(), &existing, &BTreeSet::new(), |_| false)
                .unwrap();
        assert!(out.text.contains("UI_PORT=3005"));
        assert_eq!(out.reused, vec![("UI_PORT".to_string(), 3005)]);
        assert!(out.claimed.is_empty());
    }

    #[test]
    fn sibling_claim_beats_reuse() {
        let out =
            render_ok("UI_PORT={tt:port 3000-3009}\n", &[("UI_PORT", "3005")], &[3005]).unwrap();
        assert!(out.text.contains("UI_PORT=3000"), "got: {}", out.text);
        assert_eq!(out.claimed.len(), 1);
    }

    #[test]
    fn two_claims_on_the_same_pool_do_not_collide() {
        let out = render_ok("A={tt:port 4000-4009}\nB={tt:port 4000-4009}\n", &[], &[]).unwrap();
        assert!(out.text.contains("A=4000"));
        assert!(out.text.contains("B=4001"));
    }

    #[test]
    fn pool_exhaustion_is_a_hard_error() {
        let err = render_ok("A={tt:port 5000-5001}\n", &[], &[5000, 5001]).unwrap_err();
        assert_eq!(err, TemplateError::PoolExhausted { line: 1, lo: 5000, hi: 5001 });
    }

    #[test]
    fn var_reference_uses_rendered_value() {
        let out = render_ok(
            "DB_PORT={tt:port 5432-5441}\nURL=postgres://localhost:{tt:var DB_PORT}/db\n",
            &[("DB_PORT", "5439")],
            &[],
        )
        .unwrap();
        assert!(out.text.contains("URL=postgres://localhost:5439/db"));
    }

    #[test]
    fn var_before_definition_errors() {
        let err = render_ok("URL={tt:var DB_PORT}\nDB_PORT=5432\n", &[], &[]).unwrap_err();
        assert_eq!(err, TemplateError::VarBeforeDef { line: 1, name: "DB_PORT".to_string() });
    }

    #[test]
    fn unknown_token_is_a_hard_error() {
        let err = render_ok("X={tt:prot 3000-3010}\n", &[], &[]).unwrap_err();
        assert!(matches!(err, TemplateError::UnknownToken { line: 1, .. }));
    }

    #[test]
    fn comment_lines_pass_through_without_claiming() {
        let out =
            render_ok("# example: UI_PORT={tt:port 3000-3000}\nA={tt:port 3000-3000}\n", &[], &[])
                .unwrap();
        assert!(out.text.contains("# example: UI_PORT={tt:port 3000-3000}"));
        assert!(out.text.contains("A=3000"));
    }

    #[test]
    fn invalid_range_errors() {
        let err = render_ok("A={tt:port 9000-8000}\n", &[], &[]).unwrap_err();
        assert_eq!(err, TemplateError::InvalidRange { line: 1, lo: 9000, hi: 8000 });
    }
}
