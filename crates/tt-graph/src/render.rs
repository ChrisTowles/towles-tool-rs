//! HTML rendering from the embedded treemap template. Ports
//! `src/commands/graph/render.ts`.
//!
//! The template is embedded at compile time with `include_str!` (the TS reads
//! it from disk relative to the module). Placeholders `{{WIDTH}}`, `{{HEIGHT}}`,
//! `{{DATA}}`, and `{{BAR_CHART_DATA}}` are substituted with literal values.

use crate::types::{BarChartData, TreemapNode};

const TEMPLATE: &str = include_str!("graph-template.html");

/// Generate HTML from treemap data and bar-chart data using the template.
/// Ports `generateTreemapHtml`.
pub fn generate_treemap_html(data: &TreemapNode, bar_chart_data: &BarChartData) -> String {
    let width = 1200;
    let height = 800;
    let data_json = serde_json::to_string(data).unwrap_or_else(|_| "{}".to_string());
    let bar_json = serde_json::to_string(bar_chart_data).unwrap_or_else(|_| "{}".to_string());

    TEMPLATE
        .replace("{{WIDTH}}", &width.to_string())
        .replace("{{HEIGHT}}", &height.to_string())
        .replace("{{DATA}}", &data_json)
        .replace("{{BAR_CHART_DATA}}", &bar_json)
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
}
