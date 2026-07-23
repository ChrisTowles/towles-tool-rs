//! macOS-only: a native `NSEvent` local monitor that catches bare Control+C
//! before WKWebView's own Cocoa text-editing layer can eat it.
//!
//! macOS's default text-editing key bindings (the same table that gives
//! Safari/WKWebView text fields their Emacs-style Ctrl+A/Ctrl+E navigation)
//! bind Control+C to `insertNewline:` — Cocoa treats it like pressing Return,
//! not as an ordinary keystroke. WKWebView applies that native
//! `NSTextInputClient`/`interpretKeyEvents:` machinery to editable DOM
//! elements (the terminal's hidden `<textarea>` keystroke sink,
//! `apps/client/src/components/terminal-view.tsx`), and that native
//! resolution isn't reliably gated by the DOM keydown handler's
//! `preventDefault()` the way an ordinary default action would be — so
//! Ctrl+C never reaches the frontend's `term_key` IPC call at all on macOS.
//! WebKitGTK on Linux has no such table, which is why the bug is Mac-only.
//!
//! The fix intercepts at the AppKit level, before WKWebView's text-input
//! layer ever sees the event, and forwards it straight into the focused
//! terminal's PTY via [`crate::terminal::TermState::send_key_to_focused`].
//! Scoped narrowly to bare Ctrl+C (no Shift/Option/Command) — the one chord
//! reported broken — rather than every Cocoa-bound Ctrl combo, to keep the
//! blast radius on everything else small.

#[cfg(target_os = "macos")]
mod imp {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags};
    use std::ptr::NonNull;
    use tauri::{AppHandle, Manager};
    use tt_vt::{KeyAction, KeyEvent};

    use crate::terminal::TermState;

    /// Installs the monitor for the app's lifetime. There is no natural
    /// teardown point — the app quitting tears down the whole process,
    /// monitor included — so the handle is deliberately leaked rather than
    /// stored and later removed via `NSEvent::removeMonitor`.
    pub fn install(app: &AppHandle) {
        let app = app.clone();
        let block = block2::RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: a local monitor handler is always called by AppKit with
            // a valid event, for the duration of this call only.
            let event_ref = unsafe { event.as_ref() };
            if !is_bare_ctrl_c(event_ref) {
                return event.as_ptr();
            }
            app.state::<TermState>().send_key_to_focused(KeyEvent {
                code: "KeyC".into(),
                key: "c".into(),
                action: KeyAction::Press,
                shift: false,
                alt: false,
                ctrl: true,
                meta: false,
                caps_lock: false,
                num_lock: false,
            });
            // Consuming the event (returning null) keeps it from ever
            // reaching WKWebView's Cocoa text-editing layer — the layer that
            // would otherwise turn it into `insertNewline:`.
            std::ptr::null_mut()
        });
        // SAFETY: `block` matches the required `Fn(NonNull<NSEvent>) -> *mut
        // NSEvent` signature, and always returns either a valid pointer
        // (unmodified) or null.
        let monitor: Option<Retained<AnyObject>> = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block)
        };
        std::mem::forget(monitor);
        std::mem::forget(block);
    }

    /// Bare Control+C: Control held, Command/Option/Shift not — matches the
    /// physical chord a shell reads as SIGINT, independent of keyboard layout
    /// (`charactersIgnoringModifiers` still honors layout, just not modifiers).
    fn is_bare_ctrl_c(event: &NSEvent) -> bool {
        let mods = event.modifierFlags();
        let extra = NSEventModifierFlags::Command
            | NSEventModifierFlags::Option
            | NSEventModifierFlags::Shift;
        if !mods.contains(NSEventModifierFlags::Control) || mods.intersects(extra) {
            return false;
        }
        event.charactersIgnoringModifiers().is_some_and(|s| s.to_string() == "c")
    }
}

#[cfg(target_os = "macos")]
pub use imp::install;

#[cfg(not(target_os = "macos"))]
pub fn install(_app: &tauri::AppHandle) {}
