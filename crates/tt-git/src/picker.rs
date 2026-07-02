//! Issue-picker column layout and line rendering, ported from `src/commands/gh/branch.ts`
//! and `src/lib/render.ts`. Pure functions only; the interactive prompt lives in the CLI.

use crate::Issue;

/// Column widths used to align the issue-picker rows. Mirrors `ColumnLayout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnLayout {
    pub longest_number: usize,
    pub longest_labels: usize,
    /// Space left for the title column. May be negative on a very narrow terminal, so it
    /// is kept signed (matching the TS, which can compute a negative width).
    pub description_length: i64,
}

/// Compute column widths for `issues` given the terminal width. Ports `computeColumnLayout`.
pub fn compute_column_layout(issues: &[Issue], terminal_columns: i64) -> ColumnLayout {
    let longest_number = issues.iter().map(|i| i.number.to_string().len()).max().unwrap_or(0);
    let longest_labels =
        issues.iter().map(|i| joined_label_names(i).chars().count()).max().unwrap_or(0);
    let line_max_length = terminal_columns.min(130);
    let description_length = line_max_length - longest_number as i64 - longest_labels as i64 - 15;

    ColumnLayout { longest_number, longest_labels, description_length }
}

fn joined_label_names(issue: &Issue) -> String {
    issue.labels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// The selectable value behind a picker choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceValue {
    Issue(u64),
    Cancel,
}

/// A single row in the issue picker. Mirrors the `Choice` objects built in `branch.ts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueChoice {
    pub title: String,
    pub value: ChoiceValue,
    pub description: Option<String>,
}

/// Build the picker choices for `issues`, appending a trailing `Cancel` choice.
/// Ports `buildIssueChoices`.
pub fn build_issue_choices(issues: &[Issue], layout: &ColumnLayout) -> Vec<IssueChoice> {
    let mut choices: Vec<IssueChoice> = issues
        .iter()
        .map(|i| {
            let label_text = i
                .labels
                .iter()
                .map(|l| print_with_hex_color(&l.name, &l.color))
                .collect::<Vec<_>>()
                .join(", ");
            let label_no_color = joined_label_names(i);
            let label_startpad = (layout.longest_labels as i64
                - label_no_color.chars().count() as i64)
                .max(0) as usize;
            let width = layout.description_length.max(0) as usize;
            let title_col = pad_end(&limit_text(&i.title, layout.description_length), width);
            IssueChoice {
                title: i.number.to_string(),
                value: ChoiceValue::Issue(i.number),
                description: Some(format!(
                    "{title_col} {}{label_text}",
                    " ".repeat(label_startpad)
                )),
            }
        })
        .collect();
    choices.push(IssueChoice {
        title: "Cancel".to_string(),
        value: ChoiceValue::Cancel,
        description: None,
    });
    choices
}

/// Pad `text` on the right with spaces to `width` (no-op if already wider). Like JS
/// `String.prototype.padEnd`.
fn pad_end(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width { text.to_string() } else { format!("{text}{}", " ".repeat(width - len)) }
}

/// Truncate `text` to `max_width`, appending a dimmed ellipsis. Ports `limitText`.
/// A non-positive width yields an empty string (the TS relies on positive widths).
pub fn limit_text(text: &str, max_width: i64) -> String {
    let len = text.chars().count() as i64;
    if len <= max_width {
        return text.to_string();
    }
    if max_width <= 1 {
        return String::new();
    }
    let keep = (max_width - 1) as usize;
    let head: String = text.chars().take(keep).collect();
    // consola `colors.dim("…")` — kept as raw ANSI so `strip_ansi` removes it.
    format!("{head}\u{1b}[2m…\u{1b}[22m")
}

/// Wrap `msg` in a 24-bit ANSI foreground color from a hex string. Ports
/// `printWithHexColor`.
pub fn print_with_hex_color(msg: &str, hex: &str) -> String {
    let clean = hex.strip_prefix('#').unwrap_or(hex);
    let r = u8::from_str_radix(clean.get(0..2).unwrap_or("0"), 16).unwrap_or(0);
    let g = u8::from_str_radix(clean.get(2..4).unwrap_or("0"), 16).unwrap_or(0);
    let b = u8::from_str_radix(clean.get(4..6).unwrap_or("0"), 16).unwrap_or(0);
    format!("\u{1b}[38;2;{r};{g};{b}m{msg}\u{1b}[0m")
}

/// Strip ANSI escape sequences (`\x1B[ ... <letter>`). Ports `stripAnsi`.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() || d == ';' {
                    chars.next();
                } else {
                    break;
                }
            }
            if let Some(&f) = chars.peek()
                && f.is_ascii_alphabetic()
            {
                chars.next(); // consume the final letter
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Label;

    fn issue(number: u64, title: &str, labels: &[(&str, &str)]) -> Issue {
        Issue {
            number,
            title: title.to_string(),
            state: "open".to_string(),
            labels: labels
                .iter()
                .map(|(n, c)| Label { name: n.to_string(), color: c.to_string() })
                .collect(),
        }
    }

    fn sample() -> Vec<Issue> {
        vec![
            issue(4, "Short bug", &[("bug", "d73a4a")]),
            issue(
                123,
                "Add authentication flow with OAuth",
                &[("enhancement", "a2eeef"), ("priority", "ff0000")],
            ),
            issue(7, "Docs update", &[]),
        ]
    }

    #[test]
    fn longest_number_from_widest() {
        assert_eq!(compute_column_layout(&sample(), 100).longest_number, 3);
    }

    #[test]
    fn longest_labels_from_widest_joined() {
        assert_eq!(
            compute_column_layout(&sample(), 100).longest_labels,
            "enhancement, priority".len()
        );
    }

    #[test]
    fn caps_line_width_at_130() {
        let narrow = compute_column_layout(&sample(), 80);
        let wide = compute_column_layout(&sample(), 200);
        assert_eq!(narrow.description_length, 80 - 3 - 21 - 15);
        assert_eq!(wide.description_length, 130 - 3 - 21 - 15);
    }

    #[test]
    fn single_issue_layout() {
        let single = vec![issue(1, "t", &[])];
        let layout = compute_column_layout(&single, 80);
        assert_eq!(layout.longest_number, 1);
        assert_eq!(layout.longest_labels, 0);
        // longest_number = 1, longest_labels = 0.
        assert_eq!(layout.description_length, 80 - 1 - 15);
    }

    #[test]
    fn choices_have_one_per_issue_plus_cancel() {
        let issues = sample();
        let layout = compute_column_layout(&issues, 100);
        let choices = build_issue_choices(&issues, &layout);
        assert_eq!(choices.len(), issues.len() + 1);
        let last = choices.last().unwrap();
        assert_eq!(last.title, "Cancel");
        assert_eq!(last.value, ChoiceValue::Cancel);
    }

    #[test]
    fn choices_use_number_as_title_and_value() {
        let issues = sample();
        let layout = compute_column_layout(&issues, 100);
        let choices = build_issue_choices(&issues, &layout);
        assert_eq!(choices[0].title, "4");
        assert_eq!(choices[0].value, ChoiceValue::Issue(4));
        assert_eq!(choices[1].title, "123");
        assert_eq!(choices[1].value, ChoiceValue::Issue(123));
    }

    #[test]
    fn description_contains_title_and_labels() {
        let issues = sample();
        let layout = compute_column_layout(&issues, 100);
        let choices = build_issue_choices(&issues, &layout);
        let desc0 = strip_ansi(choices[0].description.as_ref().unwrap());
        assert!(desc0.contains("Short bug"));
        let desc1 = strip_ansi(choices[1].description.as_ref().unwrap());
        assert!(desc1.contains("enhancement"));
        assert!(desc1.contains("priority"));
    }

    #[test]
    fn empty_issue_list_yields_only_cancel() {
        let layout = compute_column_layout(&[issue(0, "", &[])], 80);
        let choices = build_issue_choices(&[], &layout);
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].value, ChoiceValue::Cancel);
    }

    #[test]
    fn hex_color_roundtrips_through_strip() {
        let colored = print_with_hex_color("bug", "d73a4a");
        assert_ne!(colored, "bug");
        assert_eq!(strip_ansi(&colored), "bug");
    }

    #[test]
    fn limit_text_truncates_long_input() {
        let long = "abcdefghij";
        let out = limit_text(long, 5);
        // 4 kept chars + ellipsis, ANSI stripped.
        assert_eq!(strip_ansi(&out), "abcd…");
        // Short input is returned unchanged.
        assert_eq!(limit_text("hi", 5), "hi");
    }
}
