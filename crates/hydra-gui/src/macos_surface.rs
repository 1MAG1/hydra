//! Window colour space, so presenting the software-rendered surface does not
//! cost a full-window colour conversion per frame.
//!
//! The default renderer is tiny-skia (see `main`): iced rasterises into a
//! CPU buffer that softbuffer hands to CoreAnimation as a `CGImage` tagged
//! `CGColorSpaceCreateDeviceRGB`. If the window's backing store is in a
//! different space — and on any modern Mac it is, the display profile being
//! P3 — CoreAnimation cannot use that image directly: every present goes
//! through `CA::Render::prepare_image` → ColorSync → `vImageConvert_AnyToAny`
//! over the whole window, on the main thread. Profiling a 60 Hz redraw on a
//! Retina display put ~35% of main-thread time there, and it is paid per
//! present no matter how little of the window actually changed.
//!
//! Declaring the window's own colour space to be the one the buffer is
//! already in removes the match: the pixels go to the backing store as they
//! are, and the display profile is applied by the compositor at scan-out
//! like it is for every other window. `HYDRA_WINDOW_COLORSPACE=off` skips
//! this, `=device` picks DeviceRGB instead of sRGB, for comparing.

#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2_app_kit::{NSColorSpace, NSView};

use iced::window::raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Pin `window`'s colour space to the one its software surface is drawn in.
/// Safe to call on any window and on any platform build; a handle that is
/// not AppKit, or a view with no window yet, is simply left alone.
pub fn pin_color_space(handle: &dyn HasWindowHandle) {
    let space = match std::env::var("HYDRA_WINDOW_COLORSPACE").as_deref() {
        Ok("off") => return,
        // DeviceRGB is what softbuffer literally tags the image with; sRGB is
        // what that means on a colour-managed Mac, and keeps the colours the
        // interface was drawn for.
        Ok("device") => NSColorSpace::deviceRGBColorSpace(),
        _ => NSColorSpace::sRGBColorSpace(),
    };
    let Ok(handle) = handle.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // SAFETY: iced hands out the handle of a live window; `ns_view` is that
    // window's content view, and this runs on the main thread (the iced
    // update loop), which is where AppKit requires it.
    let view: Retained<NSView> =
        unsafe { Retained::retain(appkit.ns_view.as_ptr().cast()) }.expect("live NSView");
    let Some(window) = view.window() else {
        return;
    };
    window.setColorSpace(Some(&space));
    crate::log::debug("window colour space pinned to the surface's");
}
