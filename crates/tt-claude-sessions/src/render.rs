//! HTML rendering from the embedded treemap template.
//!
//! The template is embedded at compile time with `include_str!`. Placeholders
//! `{{WIDTH}}`, `{{HEIGHT}}`, `{{DATA}}`, and `{{BAR_CHART_DATA}}` are filled in a
//! single left-to-right pass ([`fill_template`]) so text inside one payload can
//! never be mistaken for a later placeholder. The two JSON payloads sit inside a
//! `<script>` block, so they are escaped ([`escape_json_for_script`]) to stop a
//! `</script>` (or a stray `<`) in the data from breaking out of the element.

use crate::types::{BarChartData, TreemapNode};

const TEMPLATE: &str = include_str!("graph-template.html");

/// Generate HTML from treemap data and bar-chart data using the template.
///
pub fn generate_treemap_html(data: &TreemapNode, bar_chart_data: &BarChartData) -> String {
    let width = 1200;
    let height = 800;
    let data_json =
        escape_json_for_script(&serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string()));
    let bar_json = escape_json_for_script(
        &serde_json::to_string(bar_chart_data).unwrap_or_else(|_| "{}".to_string()),
    );

    fill_template(
        TEMPLATE,
        &[
            ("WIDTH", &width.to_string()),
            ("HEIGHT", &height.to_string()),
            ("DATA", &data_json),
            ("BAR_CHART_DATA", &bar_json),
        ],
    )
}

/// Escape a serialized-JSON payload for safe embedding inside an HTML `<script>`
/// block: `<` and `/` become `\uXXXX` escapes. These are valid inside JSON/JS
/// string literals and decode back to the original characters, so a value
/// containing `</script>` can no longer terminate the surrounding script element.
/// Both characters occur only inside JSON string values (never in structural
/// positions), so the payload stays a valid JS object literal.
fn escape_json_for_script(json: &str) -> String {
    json.replace('<', "\\u003c").replace('/', "\\u002f")
}

/// Substitute `{{KEY}}` placeholders in a single left-to-right pass over
/// `template`. Inserted values are appended to the output and never re-scanned,
/// so a payload that itself contains the literal text of another placeholder
/// (e.g. `{{BAR_CHART_DATA}}`) survives verbatim instead of being clobbered by a
/// later substitution. An unknown or unterminated placeholder is left as-is.
fn fill_template(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            // Unterminated `{{` — emit it verbatim and stop scanning.
            out.push_str("{{");
            rest = after;
            break;
        };
        let key = &after[..close];
        match vars.iter().find(|entry| entry.0 == key) {
            Some(entry) => out.push_str(entry.1),
            None => {
                out.push_str("{{");
                out.push_str(key);
                out.push_str("}}");
            }
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_all_placeholders() {
        let data = TreemapNode { name: "All Sessions".to_string(), ..Default::default() };
        let bar = BarChartData { days: Vec::new() };
        let html = generate_treemap_html(&data, &bar);
        assert!(!html.contains("{{WIDTH}}"));
        assert!(!html.contains("{{HEIGHT}}"));
        assert!(!html.contains("{{DATA}}"));
        assert!(!html.contains("{{BAR_CHART_DATA}}"));
        assert!(html.contains("1200"));
        assert!(html.contains("All Sessions"));
        assert!(html.contains("\"days\":[]"));
    }

    #[test]
    fn escapes_script_closing_in_data_payload() {
        // A session name that tries to break out of the <script> block must be
        // neutralized: the raw `</script>` breakout cannot survive in the payload.
        let data = TreemapNode {
            name: "</script><script>alert(1)</script>".to_string(),
            ..Default::default()
        };
        let bar = BarChartData { days: Vec::new() };
        let html = generate_treemap_html(&data, &bar);
        assert!(!html.contains("</script><script>alert(1)"));
        // It survives only as an inert, unicode-escaped JS string.
        assert!(html.contains("\\u003cscript>alert(1)"));
    }

    #[test]
    fn placeholder_in_data_is_not_rescanned() {
        // A datum equal to a later placeholder must survive verbatim: single-pass
        // filling never re-scans inserted content for further placeholders.
        let data = TreemapNode { name: "{{BAR_CHART_DATA}}".to_string(), ..Default::default() };
        let bar = BarChartData { days: Vec::new() };
        let html = generate_treemap_html(&data, &bar);
        assert!(html.contains("{{BAR_CHART_DATA}}"));
        // The real bar-chart placeholder was still substituted with its payload.
        assert!(html.contains("const barChartData = {\"days\":[]}"));
    }
}
