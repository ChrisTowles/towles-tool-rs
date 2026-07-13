//! Rendering for `tt gh pr-list` — a terminal view of your open pull requests
//! with their CI check status.
//!
//! The "needs you" semantics are the single Rust source of truth, mirroring the
//! app's Cockpit (`store-bits.tsx`: `prNeedsYou` / `prRank`): a PR demands your
//! attention when its checks are failing or your review has been requested. The
//! CLI fetches PR rows through the existing `tt_collect::collect_prs` collector
//! (one gh code path) and maps them into [`PrRow`]s for rendering here, so this
//! crate stays Tauri- and store-free.

/// The `review_state` value the PR collector writes when your review has been
/// requested (see `tt_collect::prs`); kept in sync with `tt_store::attention`.
pub const REVIEW_REQUESTED: &str = "review_requested";

/// One pull request to render. A lightweight view over `tt_store::PrItem` so
/// this crate need not depend on the store; the CLI maps the store rows in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRow {
    pub repo: String,
    pub number: i64,
    pub title: String,
    /// CI rollup: `"passing" | "failing" | "pending" | "none"`
    /// (see `tt_collect::prs::checks_status`).
    pub checks: String,
    /// `"review_requested"` when your review is requested, else empty/authored.
    pub review_state: String,
}

/// Whether a PR demands your attention: failing checks or a requested review.
/// Mirrors the app's `prNeedsYou`.
pub fn needs_you(pr: &PrRow) -> bool {
    pr.checks == "failing" || pr.review_state == REVIEW_REQUESTED
}

/// Ordering weight — failing checks outrank review-requested outrank the rest.
/// Mirrors the app's `prRank`.
pub fn rank(pr: &PrRow) -> u8 {
    if pr.checks == "failing" {
        2
    } else if pr.review_state == REVIEW_REQUESTED {
        1
    } else {
        0
    }
}

/// The glyph for a CI check rollup state.
fn checks_glyph(checks: &str) -> char {
    match checks {
        "passing" => '✓',
        "failing" => '✗',
        "pending" => '•',
        _ => '·', // "none" / unrecognized
    }
}

/// Render open PRs as aligned terminal lines: a needs-you marker, the CI glyph,
/// `repo#number`, and the title (with a `(review requested)` suffix where your
/// review is awaited), highest-attention first, plus a one-line summary footer.
/// Emits no ANSI so it is identical on and off a TTY. Empty input yields a
/// single "no open PRs" line.
pub fn render_pr_list(rows: &[PrRow]) -> String {
    if rows.is_empty() {
        return "No open pull requests.".to_string();
    }

    // Highest attention first, then a stable repo/number order.
    let mut sorted: Vec<&PrRow> = rows.iter().collect();
    sorted.sort_by(|a, b| {
        rank(b)
            .cmp(&rank(a))
            .then_with(|| a.repo.cmp(&b.repo))
            .then_with(|| a.number.cmp(&b.number))
    });

    let refs: Vec<String> = sorted.iter().map(|p| format!("{}#{}", p.repo, p.number)).collect();
    let ref_width = refs.iter().map(|r| r.chars().count()).max().unwrap_or(0);

    let mut lines: Vec<String> = Vec::with_capacity(sorted.len() + 2);
    for (pr, reference) in sorted.iter().zip(&refs) {
        let marker = if needs_you(pr) { '!' } else { ' ' };
        let glyph = checks_glyph(&pr.checks);
        let review = if pr.review_state == REVIEW_REQUESTED { "  (review requested)" } else { "" };
        lines.push(format!(
            "{marker} {glyph} {reference:<ref_width$}  {title}{review}",
            title = pr.title,
        ));
    }

    let total = sorted.len();
    let needs = sorted.iter().filter(|p| needs_you(p)).count();
    let pr_word = if total == 1 { "PR" } else { "PRs" };
    let need_verb = if needs == 1 { "needs" } else { "need" };
    lines.push(String::new());
    lines.push(format!("{total} open {pr_word} · {needs} {need_verb} you"));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(repo: &str, number: i64, title: &str, checks: &str, review: &str) -> PrRow {
        PrRow {
            repo: repo.to_string(),
            number,
            title: title.to_string(),
            checks: checks.to_string(),
            review_state: review.to_string(),
        }
    }

    #[test]
    fn needs_you_matches_failing_or_review_requested() {
        assert!(needs_you(&row("o/r", 1, "t", "failing", "")));
        assert!(needs_you(&row("o/r", 1, "t", "passing", REVIEW_REQUESTED)));
        assert!(!needs_you(&row("o/r", 1, "t", "passing", "")));
        assert!(!needs_you(&row("o/r", 1, "t", "pending", "")));
        assert!(!needs_you(&row("o/r", 1, "t", "none", "")));
    }

    #[test]
    fn rank_orders_failing_over_review_over_rest() {
        assert_eq!(rank(&row("o/r", 1, "t", "failing", REVIEW_REQUESTED)), 2);
        assert_eq!(rank(&row("o/r", 1, "t", "passing", REVIEW_REQUESTED)), 1);
        assert_eq!(rank(&row("o/r", 1, "t", "pending", "")), 0);
    }

    #[test]
    fn empty_list_renders_a_placeholder() {
        assert_eq!(render_pr_list(&[]), "No open pull requests.");
    }

    #[test]
    fn renders_aligned_rows_highest_attention_first() {
        let rows = vec![
            row("o/a", 1, "Passing PR", "passing", ""),
            row("o/b", 22, "Failing PR", "failing", ""),
            row("o/c", 3, "Review me", "passing", REVIEW_REQUESTED),
        ];
        let out = render_pr_list(&rows);
        assert_eq!(
            out,
            [
                "! ✗ o/b#22  Failing PR",
                "! ✓ o/c#3   Review me  (review requested)",
                "  ✓ o/a#1   Passing PR",
                "",
                "3 open PRs · 2 need you",
            ]
            .join("\n"),
        );
    }

    #[test]
    fn covers_pending_and_none_glyphs_and_singular_footer() {
        let rows = vec![
            row("o/a", 1, "Building", "pending", ""),
            row("o/b", 2, "No CI", "none", ""),
        ];
        let out = render_pr_list(&rows);
        assert!(out.contains("  • o/a#1  Building"));
        assert!(out.contains("  · o/b#2  No CI"));
        assert!(out.ends_with("2 open PRs · 0 need you"));
    }

    #[test]
    fn singular_footer_grammar() {
        let out = render_pr_list(&[row("o/a", 5, "Solo", "failing", "")]);
        assert!(out.ends_with("1 open PR · 1 needs you"));
    }
}
