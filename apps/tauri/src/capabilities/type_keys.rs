//! Synthetic keyboard input capability — uses CoreGraphics CGEvent to type
//! a UTF-8 string into the currently focused application. Gated by the
//! Accessibility TCC permission. Sends each character via
//! `set_string_from_utf16` so unicode (emoji, accents, CJK) works without
//! needing virtual keycodes.

#[cfg(target_os = "macos")]
use core_graphics::event::CGEvent;
#[cfg(target_os = "macos")]
use core_graphics::event::CGEventTapLocation;
#[cfg(target_os = "macos")]
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

/// Type the given text into the focused application by synthesizing
/// keyboard events. Handles arbitrary unicode via UTF-16 input.
#[tauri::command]
pub fn type_keys(text: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use crate::macos::permissions;
        if permissions::check_accessibility() != "granted" {
            return Err("permission_denied(accessibility)".into());
        }

        if text.is_empty() {
            return Ok(());
        }

        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| "failed to create CGEventSource")?;

        let down = CGEvent::new_keyboard_event(source.clone(), 0, true)
            .map_err(|_| "failed to create key-down event")?;
        down.set_string(&text);
        down.post(CGEventTapLocation::HID);

        let up = CGEvent::new_keyboard_event(source, 0, false)
            .map_err(|_| "failed to create key-up event")?;
        up.set_string(&text);
        up.post(CGEventTapLocation::HID);

        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        Err("Keyboard input capability is currently macOS-only".into())
    }
}
