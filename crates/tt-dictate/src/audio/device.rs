//! Input device resolution. Ported from scribed `src/audio/device.rs`,
//! which mirrors `claude_stt/engines/_audio.py:19-77`.
//!
//! The user can leave `input_device` empty (use the host default) or supply a
//! substring. We match the substring case-insensitively against device names
//! returned by cpal.

use cpal::traits::{DeviceTrait, HostTrait};

use super::AudioError;

/// A resolved input device, plus the name we'll log for the user.
pub struct ResolvedInput {
    pub device: cpal::Device,
    pub name: String,
}

/// Find a cpal input device. If `substring` is empty, use the host's default
/// input. Otherwise iterate all input devices and return the first whose name
/// contains the substring (case-insensitive).
pub fn resolve(substring: &str) -> Result<ResolvedInput, AudioError> {
    let host = cpal::default_host();
    if substring.is_empty() {
        let device = host.default_input_device().ok_or(AudioError::NoInputDevice)?;
        let name = device.name().unwrap_or_else(|_| "<unknown>".into());
        return Ok(ResolvedInput { device, name });
    }
    let needle = substring.to_ascii_lowercase();
    let devices = host.input_devices().map_err(|e| AudioError::Cpal(e.to_string()))?;
    for device in devices {
        let name = device.name().unwrap_or_default();
        if name.to_ascii_lowercase().contains(&needle) {
            return Ok(ResolvedInput { device, name });
        }
    }
    Err(AudioError::DeviceNotFound(substring.to_string()))
}

/// Enumerate device names. Used by the `dictation_devices` Tauri command.
pub fn list_names() -> Result<Vec<String>, AudioError> {
    let host = cpal::default_host();
    let devices = host.input_devices().map_err(|e| AudioError::Cpal(e.to_string()))?;
    Ok(devices.filter_map(|d| d.name().ok()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Empty substring resolution either succeeds (default device exists) or
    /// returns `NoInputDevice` (headless CI). Either is acceptable; we just
    /// shouldn't panic.
    #[test]
    fn empty_substring_uses_default() {
        match resolve("") {
            Ok(r) => assert!(!r.name.is_empty()),
            Err(AudioError::NoInputDevice) => {}
            Err(e) => panic!("unexpected: {e}"),
        }
    }

    #[test]
    fn nonexistent_substring_errors() {
        // This needle is unlikely to match any real device.
        match resolve("zzzz-unlikely-device-name-zzzz") {
            Err(AudioError::DeviceNotFound(s)) => {
                assert_eq!(s, "zzzz-unlikely-device-name-zzzz");
            }
            // On a host with zero input devices, we get a different shape;
            // ignore that case in CI.
            Err(_) => {}
            Ok(_) => panic!("matched a device that shouldn't exist"),
        }
    }

    #[test]
    fn list_names_does_not_panic() {
        let _ = list_names();
    }
}
