//! Distinguish a deliberate sidebar-divider drag from tmux's proportional
//! rescale echo after a terminal resize. Ports slot-1
//! `runtime/server/sidebar-width-sync.ts` (pure; clock injected as `now_ms`).
//!
//! After the terminal window changes size, tmux proportionally rescales every
//! pane and emits a burst of follow-up resize events at the already-settled
//! window width. Pane-width changes within [`WINDOW_RESIZE_COOLDOWN_MS`] of a
//! window resize are proportional echoes (enforce, don't adopt) — mistaking
//! them for a drag is what made the sidebar ratchet smaller on every terminal
//! resize.

use indexmap::IndexMap;

use crate::tmux::SidebarPane;

pub const WINDOW_RESIZE_COOLDOWN_MS: i64 = 750;

/// Parsed body of an `after-resize-pane` hook POST.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidebarResizeContext {
    pub pane_id: Option<String>,
    pub session_name: Option<String>,
    pub window_id: Option<String>,
    pub width: Option<u32>,
    pub window_width: Option<u32>,
}

/// Last observed sidebar/window widths for one window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarWindowSnapshot {
    pub width: u32,
    pub window_width: Option<u32>,
}

/// A width the server itself just enforced on a pane; the matching echo event
/// must not be re-adopted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarResizeSuppression {
    pub width: u32,
    pub expires_at: i64,
}

pub fn snapshot_sidebar_windows(panes: &[SidebarPane]) -> IndexMap<String, SidebarWindowSnapshot> {
    let mut snapshots = IndexMap::new();
    for pane in panes {
        snapshots.insert(
            pane.window_id.clone(),
            SidebarWindowSnapshot { width: pane.width, window_width: pane.window_width },
        );
    }
    snapshots
}

/// Decide whether a resize event is a deliberate divider drag whose width
/// should be adopted (`Some(width)`), or noise to ignore/enforce (`None`).
pub fn resolve_sidebar_width_from_resize_context(
    ctx: Option<&SidebarResizeContext>,
    panes: &[SidebarPane],
    previous_by_window: &IndexMap<String, SidebarWindowSnapshot>,
    suppressed_by_pane: &mut IndexMap<String, SidebarResizeSuppression>,
    window_resize_cooldown: &mut IndexMap<String, i64>,
    now_ms: i64,
) -> Option<u32> {
    let pane_id = ctx?.pane_id.as_deref()?;
    let pane = panes.iter().find(|candidate| candidate.pane_id == pane_id)?;

    let width = ctx?.width.unwrap_or(pane.width);
    let window_width = ctx?.window_width.or(pane.window_width)?;

    if let Some(suppressed) = suppressed_by_pane.get(&pane.pane_id).copied() {
        if suppressed.width == width && suppressed.expires_at >= now_ms {
            suppressed_by_pane.shift_remove(&pane.pane_id);
            return None;
        }
        if suppressed.expires_at < now_ms || suppressed.width != width {
            suppressed_by_pane.shift_remove(&pane.pane_id);
        }
    }

    let previous = previous_by_window.get(&pane.window_id)?;
    let previous_window_width = previous.window_width?;

    // The terminal window itself changed size: tmux's resulting pane widths
    // are proportional rescales, not a user dragging the divider — never
    // adopt them, and open a cooldown so the echo events that follow are
    // ignored too.
    if previous_window_width != window_width {
        window_resize_cooldown.insert(pane.window_id.clone(), now_ms + WINDOW_RESIZE_COOLDOWN_MS);
        return None;
    }

    if previous.width == width {
        return None;
    }

    // Same window width but a different pane width. This is a deliberate
    // divider drag only when no terminal resize happened recently; otherwise
    // it's a leftover proportional echo and must not be adopted.
    if let Some(cooldown_until) = window_resize_cooldown.get(&pane.window_id).copied() {
        if cooldown_until >= now_ms {
            return None;
        }
        window_resize_cooldown.shift_remove(&pane.window_id);
    }

    Some(width)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sidebar(pane_id: &str, window_id: &str, width: u32, window_width: u32) -> SidebarPane {
        SidebarPane {
            pane_id: pane_id.into(),
            session_name: "main".into(),
            window_id: window_id.into(),
            width,
            window_width: Some(window_width),
        }
    }

    fn ctx(pane_id: &str, width: u32, window_width: u32) -> SidebarResizeContext {
        SidebarResizeContext {
            pane_id: Some(pane_id.into()),
            width: Some(width),
            window_width: Some(window_width),
            ..Default::default()
        }
    }

    #[test]
    fn deliberate_drag_is_adopted() {
        let panes = vec![sidebar("%2", "@1", 45, 160)];
        let previous = snapshot_sidebar_windows(&[sidebar("%2", "@1", 40, 160)]);
        let mut suppressed = IndexMap::new();
        let mut cooldown = IndexMap::new();
        let width = resolve_sidebar_width_from_resize_context(
            Some(&ctx("%2", 45, 160)),
            &panes,
            &previous,
            &mut suppressed,
            &mut cooldown,
            1_000,
        );
        assert_eq!(width, Some(45));
    }

    #[test]
    fn window_resize_is_never_adopted_and_opens_cooldown() {
        let panes = vec![sidebar("%2", "@1", 30, 120)];
        let previous = snapshot_sidebar_windows(&[sidebar("%2", "@1", 40, 160)]);
        let mut suppressed = IndexMap::new();
        let mut cooldown = IndexMap::new();
        // Window shrank 160 → 120: proportional rescale, not a drag.
        let width = resolve_sidebar_width_from_resize_context(
            Some(&ctx("%2", 30, 120)),
            &panes,
            &previous,
            &mut suppressed,
            &mut cooldown,
            1_000,
        );
        assert_eq!(width, None);
        assert_eq!(cooldown.get("@1").copied(), Some(1_000 + WINDOW_RESIZE_COOLDOWN_MS));

        // Echo at the settled window width, still inside the cooldown: ignore.
        let previous = snapshot_sidebar_windows(&[sidebar("%2", "@1", 40, 120)]);
        let width = resolve_sidebar_width_from_resize_context(
            Some(&ctx("%2", 30, 120)),
            &panes,
            &previous,
            &mut suppressed,
            &mut cooldown,
            1_200,
        );
        assert_eq!(width, None);

        // After the cooldown expires, the same change is a real drag.
        let width = resolve_sidebar_width_from_resize_context(
            Some(&ctx("%2", 30, 120)),
            &panes,
            &previous,
            &mut suppressed,
            &mut cooldown,
            1_000 + WINDOW_RESIZE_COOLDOWN_MS + 1,
        );
        assert_eq!(width, Some(30));
        assert!(cooldown.is_empty());
    }

    #[test]
    fn suppressed_enforced_width_is_consumed_once() {
        let panes = vec![sidebar("%2", "@1", 40, 160)];
        let previous = snapshot_sidebar_windows(&[sidebar("%2", "@1", 35, 160)]);
        let mut suppressed = IndexMap::new();
        suppressed
            .insert("%2".to_string(), SidebarResizeSuppression { width: 40, expires_at: 2_000 });
        let mut cooldown = IndexMap::new();
        // The echo of the server's own resize-pane: swallowed, entry consumed.
        let width = resolve_sidebar_width_from_resize_context(
            Some(&ctx("%2", 40, 160)),
            &panes,
            &previous,
            &mut suppressed,
            &mut cooldown,
            1_000,
        );
        assert_eq!(width, None);
        assert!(suppressed.is_empty());

        // The same event again (suppression gone) is treated as a drag.
        let width = resolve_sidebar_width_from_resize_context(
            Some(&ctx("%2", 40, 160)),
            &panes,
            &previous,
            &mut suppressed,
            &mut cooldown,
            1_100,
        );
        assert_eq!(width, Some(40));
    }

    #[test]
    fn expired_or_mismatched_suppression_is_dropped_not_applied() {
        let panes = vec![sidebar("%2", "@1", 40, 160)];
        let previous = snapshot_sidebar_windows(&[sidebar("%2", "@1", 35, 160)]);
        let mut cooldown = IndexMap::new();

        // Expired suppression: removed, event evaluated normally (drag).
        let mut suppressed = IndexMap::new();
        suppressed
            .insert("%2".to_string(), SidebarResizeSuppression { width: 40, expires_at: 500 });
        let width = resolve_sidebar_width_from_resize_context(
            Some(&ctx("%2", 40, 160)),
            &panes,
            &previous,
            &mut suppressed,
            &mut cooldown,
            1_000,
        );
        assert_eq!(width, Some(40));
        assert!(suppressed.is_empty());

        // Different width than suppressed: suppression dropped, drag adopted.
        suppressed
            .insert("%2".to_string(), SidebarResizeSuppression { width: 50, expires_at: 9_000 });
        let width = resolve_sidebar_width_from_resize_context(
            Some(&ctx("%2", 40, 160)),
            &panes,
            &previous,
            &mut suppressed,
            &mut cooldown,
            1_000,
        );
        assert_eq!(width, Some(40));
        assert!(suppressed.is_empty());
    }

    #[test]
    fn missing_context_pane_or_previous_snapshot_yields_none() {
        let panes = vec![sidebar("%2", "@1", 40, 160)];
        let previous = IndexMap::new();
        let mut suppressed = IndexMap::new();
        let mut cooldown = IndexMap::new();
        assert_eq!(
            resolve_sidebar_width_from_resize_context(
                None,
                &panes,
                &previous,
                &mut suppressed,
                &mut cooldown,
                0
            ),
            None
        );
        // Unknown pane id.
        assert_eq!(
            resolve_sidebar_width_from_resize_context(
                Some(&ctx("%9", 40, 160)),
                &panes,
                &previous,
                &mut suppressed,
                &mut cooldown,
                0
            ),
            None
        );
        // No previous snapshot for the window.
        assert_eq!(
            resolve_sidebar_width_from_resize_context(
                Some(&ctx("%2", 45, 160)),
                &panes,
                &previous,
                &mut suppressed,
                &mut cooldown,
                0
            ),
            None
        );
    }
}
