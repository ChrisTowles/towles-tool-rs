//! DOM `KeyboardEvent.code` → libghostty [`key::Key`] mapping.
//!
//! libghostty's `Key` enum is the W3C UI-Events physical-key `code` set, so
//! the mapping is a verbatim string match — the only rename is the letters,
//! where the DOM says `"KeyA"` and the enum says `A`. Unknown codes map to
//! [`Key::Unidentified`]; the encoder then falls back to the event's utf8
//! text, so an exotic key still types its character even without a physical
//! identity.

use libghostty_vt::key::Key;

/// Map a DOM `KeyboardEvent.code` to the libghostty key identity.
pub fn key_from_dom_code(code: &str) -> Key {
    // Letters: DOM "KeyA".."KeyZ" → A..Z.
    if let Some(letter) = code.strip_prefix("Key") {
        return match letter {
            "A" => Key::A,
            "B" => Key::B,
            "C" => Key::C,
            "D" => Key::D,
            "E" => Key::E,
            "F" => Key::F,
            "G" => Key::G,
            "H" => Key::H,
            "I" => Key::I,
            "J" => Key::J,
            "K" => Key::K,
            "L" => Key::L,
            "M" => Key::M,
            "N" => Key::N,
            "O" => Key::O,
            "P" => Key::P,
            "Q" => Key::Q,
            "R" => Key::R,
            "S" => Key::S,
            "T" => Key::T,
            "U" => Key::U,
            "V" => Key::V,
            "W" => Key::W,
            "X" => Key::X,
            "Y" => Key::Y,
            "Z" => Key::Z,
            _ => Key::Unidentified,
        };
    }
    match code {
        "Backquote" => Key::Backquote,
        "Backslash" => Key::Backslash,
        "BracketLeft" => Key::BracketLeft,
        "BracketRight" => Key::BracketRight,
        "Comma" => Key::Comma,
        "Digit0" => Key::Digit0,
        "Digit1" => Key::Digit1,
        "Digit2" => Key::Digit2,
        "Digit3" => Key::Digit3,
        "Digit4" => Key::Digit4,
        "Digit5" => Key::Digit5,
        "Digit6" => Key::Digit6,
        "Digit7" => Key::Digit7,
        "Digit8" => Key::Digit8,
        "Digit9" => Key::Digit9,
        "Equal" => Key::Equal,
        "IntlBackslash" => Key::IntlBackslash,
        "IntlRo" => Key::IntlRo,
        "IntlYen" => Key::IntlYen,
        "Minus" => Key::Minus,
        "Period" => Key::Period,
        "Quote" => Key::Quote,
        "Semicolon" => Key::Semicolon,
        "Slash" => Key::Slash,
        "AltLeft" => Key::AltLeft,
        "AltRight" => Key::AltRight,
        "Backspace" => Key::Backspace,
        "CapsLock" => Key::CapsLock,
        "ContextMenu" => Key::ContextMenu,
        "ControlLeft" => Key::ControlLeft,
        "ControlRight" => Key::ControlRight,
        "Enter" => Key::Enter,
        "MetaLeft" => Key::MetaLeft,
        "MetaRight" => Key::MetaRight,
        "ShiftLeft" => Key::ShiftLeft,
        "ShiftRight" => Key::ShiftRight,
        "Space" => Key::Space,
        "Tab" => Key::Tab,
        "Convert" => Key::Convert,
        "KanaMode" => Key::KanaMode,
        "NonConvert" => Key::NonConvert,
        "Delete" => Key::Delete,
        "End" => Key::End,
        "Help" => Key::Help,
        "Home" => Key::Home,
        "Insert" => Key::Insert,
        "PageDown" => Key::PageDown,
        "PageUp" => Key::PageUp,
        "ArrowDown" => Key::ArrowDown,
        "ArrowLeft" => Key::ArrowLeft,
        "ArrowRight" => Key::ArrowRight,
        "ArrowUp" => Key::ArrowUp,
        "NumLock" => Key::NumLock,
        "Numpad0" => Key::Numpad0,
        "Numpad1" => Key::Numpad1,
        "Numpad2" => Key::Numpad2,
        "Numpad3" => Key::Numpad3,
        "Numpad4" => Key::Numpad4,
        "Numpad5" => Key::Numpad5,
        "Numpad6" => Key::Numpad6,
        "Numpad7" => Key::Numpad7,
        "Numpad8" => Key::Numpad8,
        "Numpad9" => Key::Numpad9,
        "NumpadAdd" => Key::NumpadAdd,
        "NumpadBackspace" => Key::NumpadBackspace,
        "NumpadClear" => Key::NumpadClear,
        "NumpadClearEntry" => Key::NumpadClearEntry,
        "NumpadComma" => Key::NumpadComma,
        "NumpadDecimal" => Key::NumpadDecimal,
        "NumpadDivide" => Key::NumpadDivide,
        "NumpadEnter" => Key::NumpadEnter,
        "NumpadEqual" => Key::NumpadEqual,
        "NumpadMemoryAdd" => Key::NumpadMemoryAdd,
        "NumpadMemoryClear" => Key::NumpadMemoryClear,
        "NumpadMemoryRecall" => Key::NumpadMemoryRecall,
        "NumpadMemoryStore" => Key::NumpadMemoryStore,
        "NumpadMemorySubtract" => Key::NumpadMemorySubtract,
        "NumpadMultiply" => Key::NumpadMultiply,
        "NumpadParenLeft" => Key::NumpadParenLeft,
        "NumpadParenRight" => Key::NumpadParenRight,
        "NumpadSubtract" => Key::NumpadSubtract,
        "Escape" => Key::Escape,
        "F1" => Key::F1,
        "F2" => Key::F2,
        "F3" => Key::F3,
        "F4" => Key::F4,
        "F5" => Key::F5,
        "F6" => Key::F6,
        "F7" => Key::F7,
        "F8" => Key::F8,
        "F9" => Key::F9,
        "F10" => Key::F10,
        "F11" => Key::F11,
        "F12" => Key::F12,
        "F13" => Key::F13,
        "F14" => Key::F14,
        "F15" => Key::F15,
        "F16" => Key::F16,
        "F17" => Key::F17,
        "F18" => Key::F18,
        "F19" => Key::F19,
        "F20" => Key::F20,
        "F21" => Key::F21,
        "F22" => Key::F22,
        "F23" => Key::F23,
        "F24" => Key::F24,
        "Fn" => Key::Fn,
        "FnLock" => Key::FnLock,
        "PrintScreen" => Key::PrintScreen,
        "ScrollLock" => Key::ScrollLock,
        "Pause" => Key::Pause,
        "BrowserBack" => Key::BrowserBack,
        "BrowserFavorites" => Key::BrowserFavorites,
        "BrowserForward" => Key::BrowserForward,
        "BrowserHome" => Key::BrowserHome,
        "BrowserRefresh" => Key::BrowserRefresh,
        "BrowserSearch" => Key::BrowserSearch,
        "BrowserStop" => Key::BrowserStop,
        "Eject" => Key::Eject,
        "LaunchApp1" => Key::LaunchApp1,
        "LaunchApp2" => Key::LaunchApp2,
        "LaunchMail" => Key::LaunchMail,
        "MediaPlayPause" => Key::MediaPlayPause,
        "MediaSelect" => Key::MediaSelect,
        "MediaStop" => Key::MediaStop,
        "MediaTrackNext" => Key::MediaTrackNext,
        "MediaTrackPrevious" => Key::MediaTrackPrevious,
        "Power" => Key::Power,
        "Sleep" => Key::Sleep,
        "AudioVolumeDown" => Key::AudioVolumeDown,
        "AudioVolumeMute" => Key::AudioVolumeMute,
        "AudioVolumeUp" => Key::AudioVolumeUp,
        "WakeUp" => Key::WakeUp,
        "Copy" => Key::Copy,
        "Cut" => Key::Cut,
        "Paste" => Key::Paste,
        _ => Key::Unidentified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_lose_the_dom_key_prefix() {
        assert_eq!(key_from_dom_code("KeyA"), Key::A);
        assert_eq!(key_from_dom_code("KeyZ"), Key::Z);
    }

    #[test]
    fn non_letter_codes_map_verbatim() {
        assert_eq!(key_from_dom_code("Digit1"), Key::Digit1);
        assert_eq!(key_from_dom_code("ArrowUp"), Key::ArrowUp);
        assert_eq!(key_from_dom_code("NumpadEnter"), Key::NumpadEnter);
        assert_eq!(key_from_dom_code("Semicolon"), Key::Semicolon);
        assert_eq!(key_from_dom_code("ShiftLeft"), Key::ShiftLeft);
        assert_eq!(key_from_dom_code("Enter"), Key::Enter);
    }

    #[test]
    fn unknown_codes_are_unidentified_not_errors() {
        assert_eq!(key_from_dom_code(""), Key::Unidentified);
        assert_eq!(key_from_dom_code("KeyÅ"), Key::Unidentified);
        assert_eq!(key_from_dom_code("Gamepad3"), Key::Unidentified);
    }
}
