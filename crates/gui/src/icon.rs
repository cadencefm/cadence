//! Embedded cross-platform application icon for `cadence-gui`.
//!
//! Embeds the canonical 256×256 PNG derived from `assets/icons/cadence.svg`:
//! - Linux & Windows: decoded to RGBA8 and attached via `winit::window::WindowAttributes`
//!   (and Windows taskbar icon extension).
//! - macOS: installed on `NSApplication::sharedApplication` so Dock and Cmd-Tab show
//!   the Cadence icon even when launched unbundled from a terminal.

use winit::window::{Icon, WindowAttributes};

/// Embedded 256×256 PNG application icon (derived from `assets/icons/cadence.svg`).
pub const APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/cadence-256.png");

/// Decode the embedded PNG icon into a `winit::window::Icon`.
pub fn load_window_icon() -> Option<Icon> {
    let img = image::load_from_memory_with_format(APP_ICON_PNG, image::ImageFormat::Png).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

/// Attach the Cadence window icon (and Windows taskbar icon) to `WindowAttributes`.
pub fn apply_window_icon(mut attrs: WindowAttributes) -> WindowAttributes {
    if let Some(icon) = load_window_icon() {
        #[cfg(target_os = "windows")]
        {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs.with_taskbar_icon(Some(icon.clone()));
        }
        attrs = attrs.with_window_icon(Some(icon));
    }
    attrs
}

/// Install the macOS Dock / Cmd-Tab application icon on the main thread.
///
/// No-op on non-macOS platforms (where [`apply_window_icon`] supplies the icon).
pub fn install_app_icon() {
    #[cfg(target_os = "macos")]
    {
        use objc2::ClassType;
        use objc2_app_kit::{NSApplication, NSImage};
        use objc2_foundation::{MainThreadMarker, NSData};

        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let data = NSData::with_bytes(APP_ICON_PNG);
        if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
            let app = NSApplication::sharedApplication(mtm);
            // SAFETY: Called on the main thread via `MainThreadMarker` with a valid retained `NSImage`.
            unsafe {
                app.setApplicationIconImage(Some(&image));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_app_icon_decodes_to_256x256_rgba() {
        let img = image::load_from_memory_with_format(APP_ICON_PNG, image::ImageFormat::Png)
            .expect("embedded cadence-256.png must decode cleanly");
        let rgba = img.to_rgba8();
        assert_eq!(rgba.dimensions(), (256, 256));
        assert!(
            rgba.pixels().any(|p| p.0[3] > 0),
            "app icon must contain visible non-transparent pixels"
        );
        assert!(
            load_window_icon().is_some(),
            "winit Icon::from_rgba must succeed for embedded app icon"
        );
    }
}
