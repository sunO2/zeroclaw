//! Synthetic mouse click capability — uses CoreGraphics CGEvent to post a
//! click at a global display coordinate. Gated by the Accessibility TCC
//! permission. Coordinates are top-left origin of the primary display, in
//! points (not pixels) — matching what the agent sees in screenshots taken
//! via `take_screenshot`.

#[cfg(target_os = "macos")]
use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
#[cfg(target_os = "macos")]
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
#[cfg(target_os = "macos")]
use core_graphics::geometry::CGPoint;

/// Post a left/right/middle mouse click at global display coordinates (x, y).
/// `button` defaults to "left". Sends both press and release events.
#[tauri::command]
pub fn click(x: f64, y: f64, button: Option<String>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use crate::macos::permissions;
        if permissions::check_accessibility() != "granted" {
            return Err("permission_denied(accessibility)".into());
        }

        let (down_type, up_type, mouse_button) = match button.as_deref().unwrap_or("left") {
            "left" => (
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGMouseButton::Left,
            ),
            "right" => (
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGMouseButton::Right,
            ),
            "middle" => (
                CGEventType::OtherMouseDown,
                CGEventType::OtherMouseUp,
                CGMouseButton::Center,
            ),
            other => return Err(format!("unknown button: {other}")),
        };

        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| "failed to create CGEventSource")?;
        let position = CGPoint::new(x, y);

        let down = CGEvent::new_mouse_event(source.clone(), down_type, position, mouse_button)
            .map_err(|_| "failed to create mouse-down event")?;
        down.post(CGEventTapLocation::HID);

        let up = CGEvent::new_mouse_event(source, up_type, position, mouse_button)
            .map_err(|_| "failed to create mouse-up event")?;
        up.post(CGEventTapLocation::HID);

        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (x, y, button);
        Err("Click capability is currently macOS-only".into())
    }
}
