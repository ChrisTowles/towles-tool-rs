//! Output data types for the Claude Sessions aggregates.
//!
//! The Claude Code transcript **input** schema (`TranscriptEntry`, `Message`,
//! `Usage`, `Content`) lives in the shared [`tt_claude_code`] crate — the
//! single quarantine for that internal, version-volatile format. This module
//! keeps only this crate's own output types, serialized camelCase for the
//! frontend.

use serde::Serialize;

/// An individual tool call with token attribution, used in tooltips and
/// treemap children.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolData {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// One project's aggregated tokens within a [`BarChartDay`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBar {
    pub project: String,
    pub total_tokens: i64,
}

/// One model's aggregated tokens across a set of sessions. Ports the "spend by
/// model" view: total tokens billed to Opus/Sonnet/Haiku over the selected
/// window.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelBar {
    pub model: String,
    pub total_tokens: i64,
}

/// One day of the stacked bar chart: per-project token totals.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BarChartDay {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub projects: Vec<ProjectBar>,
}
