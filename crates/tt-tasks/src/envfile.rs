//! Dotenv-lite parsing and merging for task `.env` files.
//!
//! Deliberately minimal: `KEY=VALUE` lines, `#` comments, no quote handling —
//! values pass through untouched so a merge can never mangle a secret. The
//! rendered `.env` doubles as the task's port-claim record, so [`port_claims`]
//! is how sibling tasks learn which ports are taken.

use std::collections::{BTreeMap, BTreeSet};

/// The `KEY` of a `KEY=VALUE` line, when the line is a well-formed assignment
/// (ASCII identifier key, not a comment).
pub fn line_key(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    let mut bytes = key.bytes();
    let first = bytes.next()?;
    (first.is_ascii_alphabetic() || first == b'_')
        .then_some(())
        .filter(|_| key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .map(|_| key)
}

/// All `KEY=VALUE` assignments in order. Comments and malformed lines are
/// skipped; values are raw (no quote stripping, no trimming).
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let key = line_key(line)?;
            let value = &line.trim_start()[key.len() + 1..];
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

/// Ports this env file claims, keyed by the assignment's `KEY`: every `*PORT`
/// assignment whose value is a bare decimal port number. The key filter is
/// load-bearing: identity values like `TASK=3` are small bare numbers too, and
/// treating them as claims made the removal guard report "port 3 in use"
/// (binding low ports always fails without root). The rendered `.env` is the
/// claim record — stable until the task is removed (or re-rendered), at which
/// point the ports self-release. Keeping the key lets a drift check report
/// *which* var's port changed (e.g. `UI_PORT 3001 -> 3007`), not just that the
/// claimed set differs.
pub fn port_claims_by_key(text: &str) -> BTreeMap<String, u16> {
    parse(text)
        .into_iter()
        .filter(|(k, v)| {
            k.ends_with("PORT")
                && (1..=5).contains(&v.len())
                && v.bytes().all(|b| b.is_ascii_digit())
        })
        .filter_map(|(k, v)| v.parse::<u16>().ok().filter(|&p| p > 0).map(|p| (k, p)))
        .collect()
}

/// Ports this env file claims, as a flat set — [`port_claims_by_key`] without
/// the keys, for callers that only care what's taken (sibling-claim scanning,
/// the removal guard).
pub fn port_claims(text: &str) -> BTreeSet<u16> {
    port_claims_by_key(text).into_values().collect()
}

/// Merge `src`'s assignments into `dst` without disturbing anything `dst`
/// already sets: fill keys that are present-but-empty (`KEY=`), append keys
/// `dst` lacks entirely, and never touch a key with a value (rendered ports,
/// template-filled values). Returns the merged text and how many keys moved.
///
/// This is both the secrets-inheritance path (new task copies a sibling's API
/// keys) and the re-render preservation path (a re-rendered `.env` keeps keys
/// the template doesn't know about) — re-renders must be idempotent.
pub fn merge_missing_keys(dst: &str, src: &str) -> (String, usize) {
    let mut lines: Vec<String> = dst.lines().map(str::to_string).collect();
    let mut added = 0;
    for (key, value) in parse(src) {
        if value.is_empty() {
            continue;
        }
        match lines.iter().position(|l| line_key(l) == Some(key.as_str())) {
            Some(i) => {
                let existing = &lines[i].trim_start()[key.len() + 1..];
                if existing.is_empty() {
                    lines[i] = format!("{key}={value}");
                    added += 1;
                }
            }
            None => {
                lines.push(format!("{key}={value}"));
                added += 1;
            }
        }
    }
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    (text, added)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skips_comments_and_junk() {
        let text = "# comment\nUI_PORT=3000\nbad line\nEMPTY=\n  INDENTED=x\n1BAD=y\n";
        let parsed = parse(text);
        assert_eq!(
            parsed,
            vec![
                ("UI_PORT".into(), "3000".into()),
                ("EMPTY".into(), "".into()),
                ("INDENTED".into(), "x".into()),
            ]
        );
    }

    #[test]
    fn values_are_raw() {
        let parsed = parse("URL=postgres://u:p@h:5432/db?a=b\nQUOTED=\"kept\"\n");
        assert_eq!(parsed[0].1, "postgres://u:p@h:5432/db?a=b");
        assert_eq!(parsed[1].1, "\"kept\"");
    }

    #[test]
    fn port_claims_only_port_keys_with_bare_numbers() {
        let text = "UI_PORT=3000\nDB_PORT=5439\nTASK=3\nNAME=task-3\nURL=http://x:9999/\nZERO_PORT=0\nBIG_PORT=99999\n";
        let claims = port_claims(text);
        assert!(claims.contains(&3000) && claims.contains(&5439));
        assert!(!claims.contains(&3), "TASK=3 is identity, not a port claim");
        assert!(!claims.contains(&9999), "numbers inside URLs are not claims");
        assert!(!claims.contains(&0));
        assert_eq!(claims.len(), 2);
    }

    #[test]
    fn port_claims_by_key_keeps_the_owning_var_name() {
        let text = "UI_PORT=3000\nDB_PORT=5439\nTASK=3\nURL=http://x:9999/\n";
        let claims = port_claims_by_key(text);
        assert_eq!(claims.get("UI_PORT"), Some(&3000));
        assert_eq!(claims.get("DB_PORT"), Some(&5439));
        assert_eq!(claims.len(), 2, "TASK and URL are not port claims");
        // Consistent with the flat set view.
        assert_eq!(port_claims(text), claims.into_values().collect());
    }

    #[test]
    fn merge_fills_empty_appends_missing_keeps_set() {
        let dst = "UI_PORT=3001\nAPI_KEY=\nKEPT=already\n";
        let src = "UI_PORT=9999\nAPI_KEY=sekrit\nEXTRA=added\nBLANK=\n";
        let (merged, count) = merge_missing_keys(dst, src);
        assert_eq!(merged, "UI_PORT=3001\nAPI_KEY=sekrit\nKEPT=already\nEXTRA=added\n");
        assert_eq!(count, 2);
    }

    #[test]
    fn merge_never_mangles_special_chars() {
        let (merged, _) = merge_missing_keys("A=\n", "A=p&ss|w\\rd$1\n");
        assert_eq!(merged, "A=p&ss|w\\rd$1\n");
    }

    // Property tests: every port claim and inherited secret passes through
    // parse/merge, so these hold the invariants prose tests can only sample.

    use proptest::prelude::*;

    /// Valid assignment keys, as `line_key` defines them.
    fn any_key() -> impl Strategy<Value = String> {
        "[A-Za-z_][A-Za-z0-9_]{0,8}"
    }

    /// Values: printable, no newline (a newline would split the assignment —
    /// the format genuinely can't represent one; quoting is out of scope by
    /// design). Leading/trailing spaces and `=`/`#` inside values are all
    /// fair game and must survive raw.
    fn any_value() -> impl Strategy<Value = String> {
        "[ -~]{0,20}"
    }

    fn any_env() -> impl Strategy<Value = BTreeMap<String, String>> {
        proptest::collection::btree_map(any_key(), any_value(), 0..8)
    }

    fn render(env: &BTreeMap<String, String>) -> String {
        env.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
    }

    proptest! {
        #[test]
        fn parse_round_trips_rendered_assignments(env in any_env()) {
            let parsed: BTreeMap<String, String> = parse(&render(&env)).into_iter().collect();
            prop_assert_eq!(parsed, env);
        }

        #[test]
        fn merge_never_touches_a_set_value_and_fills_the_rest(
            dst in any_env(),
            src in any_env(),
        ) {
            let (merged_text, added) = merge_missing_keys(&render(&dst), &render(&src));
            let merged: BTreeMap<String, String> =
                parse(&merged_text).into_iter().collect();

            let mut expected_added = 0;
            for (k, v) in &dst {
                if !v.is_empty() {
                    // A key dst sets is untouchable — this is what keeps a
                    // re-render from clobbering rendered ports.
                    prop_assert_eq!(merged.get(k), Some(v));
                }
            }
            for (k, v) in &src {
                if v.is_empty() {
                    continue; // blank src keys never move
                }
                match dst.get(k) {
                    Some(existing) if !existing.is_empty() => {}
                    _ => expected_added += 1, // filled (dst blank) or appended
                }
                prop_assert!(merged.contains_key(k), "src key {} must survive", k);
            }
            prop_assert_eq!(added, expected_added);
        }

        #[test]
        fn merge_is_idempotent(dst in any_env(), src in any_env()) {
            let (once, _) = merge_missing_keys(&render(&dst), &render(&src));
            let (twice, added_again) = merge_missing_keys(&once, &render(&src));
            prop_assert_eq!(&twice, &once);
            prop_assert_eq!(added_again, 0);
        }

        #[test]
        fn port_claims_are_a_subset_of_parsed_port_vars(env in any_env()) {
            let text = render(&env);
            for (key, port) in port_claims_by_key(&text) {
                prop_assert!(key.ends_with("PORT"));
                prop_assert_eq!(env.get(&key), Some(&port.to_string()));
            }
        }
    }
}
