//! Scrollback search: pure case-insensitive substring matching over one
//! row's cells. The engine extracts cells (char + column + width) from the
//! grid and this module finds the query in them, so column positions stay
//! exact across wide (CJK/emoji) characters.

use serde::Serialize;

/// One search hit somewhere in the terminal's full screen (scrollback +
/// active area). `row` is absolute: 0 = oldest scrollback row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    /// Absolute row index (0 = top of scrollback).
    pub row: usize,
    /// Starting column of the match.
    pub col: u16,
    /// Match width in terminal columns (wide chars count 2).
    pub width: u16,
}

/// One cell of a row as extracted from the grid: a character at a starting
/// column with a column width (1, or 2 for wide cells).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowCell {
    pub ch: char,
    pub col: u16,
    pub width: u16,
}

/// Case-insensitive, non-overlapping occurrences of `query` in a row's
/// cells, as `(col, width)` column ranges. Empty queries match nothing.
pub fn match_row(cells: &[RowCell], query: &str) -> Vec<(u16, u16)> {
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() || cells.is_empty() {
        return Vec::new();
    }
    // Lowercased haystack, remembering which cell produced each char
    // (lowercasing can expand one char into several, e.g. 'İ').
    let mut hay: Vec<char> = Vec::with_capacity(cells.len());
    let mut owner: Vec<usize> = Vec::with_capacity(cells.len());
    for (i, cell) in cells.iter().enumerate() {
        for lc in cell.ch.to_lowercase() {
            hay.push(lc);
            owner.push(i);
        }
    }

    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == needle[..] {
            let first = cells[owner[i]];
            let last = cells[owner[i + needle.len() - 1]];
            out.push((first.col, last.col + last.width - first.col));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Narrow cells for plain text starting at column 0.
    fn cells(text: &str) -> Vec<RowCell> {
        text.chars().enumerate().map(|(i, ch)| RowCell { ch, col: i as u16, width: 1 }).collect()
    }

    #[test]
    fn finds_a_plain_substring() {
        assert_eq!(match_row(&cells("error: file not found"), "file"), vec![(7, 4)]);
    }

    #[test]
    fn is_case_insensitive_both_ways() {
        assert_eq!(match_row(&cells("Warning: DISK FULL"), "disk"), vec![(9, 4)]);
        assert_eq!(match_row(&cells("warning: disk full"), "DISK"), vec![(9, 4)]);
    }

    #[test]
    fn returns_every_non_overlapping_match() {
        assert_eq!(match_row(&cells("abab abab"), "ab"), vec![(0, 2), (2, 2), (5, 2), (7, 2)]);
        // Overlapping candidates collapse to non-overlapping hits.
        assert_eq!(match_row(&cells("aaaa"), "aa"), vec![(0, 2), (2, 2)]);
    }

    #[test]
    fn empty_query_or_row_matches_nothing() {
        assert!(match_row(&cells("text"), "").is_empty());
        assert!(match_row(&[], "text").is_empty());
        assert!(match_row(&cells("short"), "much longer than the row").is_empty());
    }

    #[test]
    fn wide_chars_shift_columns_and_widen_matches() {
        // "日本 x": 日 at col 0 (w2), 本 at col 2 (w2), space at 4, x at 5.
        let row = vec![
            RowCell { ch: '日', col: 0, width: 2 },
            RowCell { ch: '本', col: 2, width: 2 },
            RowCell { ch: ' ', col: 4, width: 1 },
            RowCell { ch: 'x', col: 5, width: 1 },
        ];
        assert_eq!(match_row(&row, "x"), vec![(5, 1)]);
        assert_eq!(match_row(&row, "本 x"), vec![(2, 4)]);
        assert_eq!(match_row(&row, "日本"), vec![(0, 4)]);
    }

    #[test]
    fn unicode_lowercasing_matches_accented_text() {
        assert_eq!(match_row(&cells("CAFÉ noir"), "café"), vec![(0, 4)]);
    }
}
