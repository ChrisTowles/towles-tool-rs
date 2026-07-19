//! Persisted per-repo *identity*: the icon and color the user chose for a
//! tracked repo, so the same repo is recognizable at a glance in the rail, on
//! the Board's task cards, and anywhere else its work shows up.
//!
//! Stored in `<agentboard_shared_dir>/repo_meta.json`, keyed by the repo's
//! absolute dir — the same key `repos.json` lists. **Shared, not slot-scoped**:
//! which repos exist and what they look like is a fact about the machine, the
//! same rationale as [`crate::repos`]. (Contrast [`crate::folder_meta`], which
//! is per-checkout state and therefore slot-scoped.)
//!
//! Identity lives here rather than as fields on `repos.json`'s `repoPaths`
//! because that list is a plain `Vec<String>` read by three other crates; a
//! sibling keyed store adds the axis without churning every reader.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How strongly a repo's color shows up in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepoAccentStyle {
    /// Tint the icon and draw a thin edge. The restrained default — applied by
    /// the client when `RepoMeta::style` is absent, so there's no Rust-side
    /// `Default` to drift from it.
    Accent,
    /// Also wash the row/card surface with a low-alpha fill.
    Tint,
}

/// A `#rrggbb` color, lowercased. Parsed at the edge so nothing downstream
/// has to re-check that the string is safe to interpolate into a style.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HexColor(String);

impl HexColor {
    /// Parse `#rgb` or `#rrggbb` (leading `#` optional, case-insensitive) into
    /// the canonical `#rrggbb` form. `None` when it isn't a hex color.
    pub fn parse(raw: &str) -> Option<Self> {
        let body = raw.trim().trim_start_matches('#');
        if !body.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let expanded = match body.len() {
            3 => body.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 => body.to_string(),
            _ => return None,
        };
        Some(Self(format!("#{}", expanded.to_ascii_lowercase())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identity for one repo. Every field optional: an untouched repo has no entry
/// at all, and the UI falls back to its generic icon and neutral color.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoMeta {
    /// A lucide icon name from the client's allowlist (`REPO_ICONS`). Stored as
    /// a bare string because the icon set is a frontend concern — Rust only
    /// round-trips it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<HexColor>,
    /// Absent ⇒ [`RepoAccentStyle::Accent`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<RepoAccentStyle>,
}

impl RepoMeta {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// On-disk shape: `{ "repos": { "<repoDir>": { "icon": "...", ... } } }`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct RepoMetaConfig {
    #[serde(default)]
    repos: HashMap<String, RepoMeta>,
}

/// Default location: `<agentboard_shared_dir>/repo_meta.json`.
pub fn default_repo_meta_path() -> PathBuf {
    tt_config::agentboard_shared_dir_lossy().join("repo_meta.json")
}

/// Owns the repo→meta map plus its file path. Mirrors
/// [`crate::folder_meta::FolderMetaStore`], including its merge-on-save: this
/// file is shared across every Agentboard window, so a save only overwrites the
/// entries this instance actually touched.
#[derive(Debug, Default)]
pub struct RepoMetaStore {
    path: Option<PathBuf>,
    repos: HashMap<String, RepoMeta>,
    /// Repo dirs mutated since the last successful `save()`.
    dirty: HashSet<String>,
}

impl RepoMetaStore {
    /// Load from `path` (empty on missing/corrupt). `None` = in-memory only (tests).
    pub fn new(path: Option<PathBuf>) -> Self {
        let repos = match &path {
            Some(p) => load(p),
            None => HashMap::new(),
        };
        Self { path, repos, dirty: HashSet::new() }
    }

    /// This repo's identity, if the user has set any of it.
    pub fn meta_for(&self, dir: &str) -> Option<&RepoMeta> {
        self.repos.get(dir).filter(|m| !m.is_empty())
    }

    /// Replace a repo's identity wholesale — the picker always submits every
    /// field, and an all-empty `meta` clears the entry ("reset to default").
    /// Returns whether anything changed; caller persists on `true`.
    pub fn set_meta(&mut self, dir: &str, meta: RepoMeta) -> bool {
        // An empty `RepoMeta` is never stored (below), so "absent" and "empty"
        // are the same state — one comparison covers both.
        if self.repos.get(dir).map_or(meta.is_empty(), |m| *m == meta) {
            return false;
        }
        if meta.is_empty() {
            self.repos.remove(dir);
        } else {
            self.repos.insert(dir.to_string(), meta);
        }
        self.dirty.insert(dir.to_string());
        true
    }

    /// Forget one repo's identity, on an explicit user removal only.
    ///
    /// Deliberately not a poll-driven `prune(&dirs)` like the sibling stores:
    /// those hold derived state that regenerates, this holds a hand-picked
    /// choice with no undo. Reaping it on a churny dirs-set (see the note in
    /// `Engine::poll`) would silently destroy the user's work; a repo that
    /// merely goes missing keeps its identity, so retracking restores it.
    /// Returns whether anything was forgotten; caller persists on `true`.
    pub fn forget(&mut self, dir: &str) -> bool {
        if self.repos.remove(dir).is_none() {
            return false;
        }
        self.dirty.insert(dir.to_string());
        true
    }

    /// Persist the repos touched since the last save, rereading the file fresh
    /// so a concurrent window's edits to *other* repos survive.
    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        if self.dirty.is_empty() {
            return Ok(());
        }
        let dirty: Vec<String> = self.dirty.drain().collect();
        let mut on_disk = load(&path);
        for dir in &dirty {
            match self.repos.get(dir) {
                Some(meta) => {
                    on_disk.insert(dir.clone(), meta.clone());
                }
                None => {
                    on_disk.remove(dir);
                }
            }
        }
        let config = RepoMetaConfig { repos: on_disk };
        let json = serde_json::to_string_pretty(&config).unwrap_or_else(|_| "{}".to_string());
        crate::persist::write_atomic(&path, &format!("{json}\n"))?;
        // Adopt the merged result, same as `FolderMetaStore::save`: another
        // window's edits to repos we didn't touch are now part of our view, so
        // `meta_for` stops serving an icon this instance never saw change.
        // Only on a successful write — a failed one must not make us believe
        // we're in sync with a file we didn't manage to update.
        self.repos = config.repos;
        Ok(())
    }
}

/// Read the map, defaulting to empty on a missing or unparseable file.
fn load(path: &Path) -> HashMap<String, RepoMeta> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str::<RepoMetaConfig>(&text).map(|c| c.repos).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(icon: &str, color: &str) -> RepoMeta {
        RepoMeta {
            icon: Some(icon.to_string()),
            color: HexColor::parse(color),
            style: Some(RepoAccentStyle::Accent),
        }
    }

    #[test]
    fn hex_color_parses_both_lengths_and_canonicalizes() {
        assert_eq!(HexColor::parse("#AbC").unwrap().as_str(), "#aabbcc");
        assert_eq!(HexColor::parse("7C3AED").unwrap().as_str(), "#7c3aed");
        assert_eq!(HexColor::parse("  #7c3aed  ").unwrap().as_str(), "#7c3aed");
    }

    #[test]
    fn hex_color_rejects_non_colors() {
        for raw in ["", "#12", "#12345", "#gggggg", "red", "#7c3aed7c"] {
            assert!(HexColor::parse(raw).is_none(), "expected {raw:?} to be rejected");
        }
    }

    #[test]
    fn set_meta_reports_change_and_clears_on_empty() {
        let mut store = RepoMetaStore::new(None);
        assert!(store.set_meta("/r", meta("Rocket", "#7c3aed")));
        assert!(!store.set_meta("/r", meta("Rocket", "#7c3aed")), "no-op resave");
        assert_eq!(store.meta_for("/r").unwrap().icon.as_deref(), Some("Rocket"));

        assert!(store.set_meta("/r", RepoMeta::default()), "clearing is a change");
        assert!(store.meta_for("/r").is_none());
        assert!(!store.set_meta("/r", RepoMeta::default()), "clearing an absent repo is a no-op");
    }

    #[test]
    fn save_merges_with_a_concurrent_windows_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repo_meta.json");

        let mut a = RepoMetaStore::new(Some(path.clone()));
        a.set_meta("/a", meta("Rocket", "#7c3aed"));
        a.save().unwrap();

        // Second window loads, edits only its own repo, saves.
        let mut b = RepoMetaStore::new(Some(path.clone()));
        b.set_meta("/b", meta("Bug", "#f59e0b"));
        b.save().unwrap();

        // A's repo survives B's save.
        let reloaded = RepoMetaStore::new(Some(path));
        assert_eq!(reloaded.meta_for("/a").unwrap().icon.as_deref(), Some("Rocket"));
        assert_eq!(reloaded.meta_for("/b").unwrap().icon.as_deref(), Some("Bug"));
    }

    #[test]
    fn save_adopts_another_windows_edits_into_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repo_meta.json");

        let mut a = RepoMetaStore::new(Some(path.clone()));
        a.set_meta("/a", meta("Rocket", "#7c3aed"));
        a.save().unwrap();

        // Another window themes a repo A has never touched.
        let mut b = RepoMetaStore::new(Some(path.clone()));
        b.set_meta("/b", meta("Bug", "#f59e0b"));
        b.save().unwrap();

        // A's next save merges — and must leave A *reading* B's repo too,
        // rather than serving a view that predates it until restart.
        a.set_meta("/a", meta("Cloud", "#0ea5e9"));
        a.save().unwrap();
        assert_eq!(
            a.meta_for("/b").and_then(|m| m.icon.as_deref()),
            Some("Bug"),
            "a save must adopt the merged state, not just write it"
        );
    }

    #[test]
    fn forget_drops_only_the_named_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repo_meta.json");
        let mut store = RepoMetaStore::new(Some(path.clone()));
        store.set_meta("/keep", meta("Rocket", "#7c3aed"));
        store.set_meta("/drop", meta("Bug", "#f59e0b"));
        store.save().unwrap();

        assert!(store.forget("/drop"));
        assert!(!store.forget("/drop"), "forgetting an absent repo is a no-op");
        store.save().unwrap();

        let reloaded = RepoMetaStore::new(Some(path));
        assert!(reloaded.meta_for("/keep").is_some());
        assert!(reloaded.meta_for("/drop").is_none(), "a forgotten repo must not survive the save");
    }

    #[test]
    fn identity_survives_a_repo_going_missing() {
        // The regression this store's lack of a poll-driven prune exists to
        // prevent: a repo dropping out of the rail's dirs set (unmounted disk,
        // transient stat failure, untrack-then-retrack) must not cost the user
        // a hand-picked icon they can't get back with an undo.
        let mut store = RepoMetaStore::new(None);
        store.set_meta("/gone", meta("Rocket", "#7c3aed"));
        assert!(
            store.meta_for("/gone").is_some(),
            "only an explicit removal may forget an identity"
        );
    }

    #[test]
    fn serializes_to_the_shape_the_client_reads() {
        // The client types this as `{ icon?, color?, style?: "accent" | "tint" }`
        // and an unset field must be *absent*, not null — pin both.
        let full = RepoMeta {
            icon: Some("Rocket".into()),
            color: HexColor::parse("#7c3aed"),
            style: Some(RepoAccentStyle::Tint),
        };
        assert_eq!(
            serde_json::to_string(&full).unwrap(),
            r##"{"icon":"Rocket","color":"#7c3aed","style":"tint"}"##
        );
        assert_eq!(serde_json::to_string(&RepoMeta::default()).unwrap(), "{}");

        // And it round-trips back, including an absent style.
        let partial: RepoMeta = serde_json::from_str(r#"{"icon":"Bug"}"#).unwrap();
        assert_eq!(partial.icon.as_deref(), Some("Bug"));
        assert!(partial.style.is_none(), "absent style ⇒ the client's Accent default");
    }

    #[test]
    fn corrupt_file_loads_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repo_meta.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(RepoMetaStore::new(Some(path)).meta_for("/a").is_none());
    }
}
