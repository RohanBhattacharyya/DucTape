use std::time::Duration;
use std::{ptr, thread};

use x11::keysym::{XK_Return, XK_backslash, XK_bracketleft, XK_bracketright, XK_space};
use x11::xlib::{XCloseDisplay, XInitThreads, XKeysymToKeycode, XOpenDisplay, XQueryKeymap};

use crate::controller::{Controller, Source};

/// Polls the X11 keymap. Returns false when global hotkeys aren't possible, in which case the
/// UI handles Enter and Space itself while its window is focused.
pub fn bind_keys(ctrl: Controller) -> bool {
    // Wayland compositors don't let clients observe keys pressed in other windows
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland");
    if wayland {
        tracing::info!(
            "global hotkeys are unavailable on Wayland; Enter/Space work while DucTape is focused"
        );
        return false;
    }

    // SAFETY: XInitThreads must precede other Xlib calls; the display is only used on the
    // polling thread below.
    let display = unsafe {
        XInitThreads();
        XOpenDisplay(ptr::null())
    };
    if display.is_null() {
        tracing::warn!("could not open the X display, global hotkeys are unavailable");
        return false;
    }
    let display = display as usize;

    thread::Builder::new()
        .name("hotkeys".into())
        .spawn(move || {
            let display = display as *mut x11::xlib::Display;
            // SAFETY: display is a valid connection owned by this thread for its lifetime
            let code = |sym: u32| unsafe { XKeysymToKeycode(display, sym.into()) };
            let return_code = code(XK_Return);
            let space_code = code(XK_space);
            let enable_codes = [
                code(XK_bracketleft),
                code(XK_bracketright),
                code(XK_backslash),
            ];

            // Only act on the transition to pressed
            let mut return_was = false;
            let mut space_was = false;
            let mut enable_was = true;

            loop {
                let mut keymap = [0 as std::ffi::c_char; 32];
                // SAFETY: XQueryKeymap writes exactly 32 bytes
                unsafe { XQueryKeymap(display, keymap.as_mut_ptr()) };
                let pressed = |code: u8| keymap[code as usize / 8] as u8 & (1 << (code % 8)) != 0;

                let ret = pressed(return_code);
                let space = pressed(space_code);
                let enable = enable_codes.iter().all(|&c| pressed(c));

                if ret && !return_was {
                    ctrl.disable(Source::Hotkey);
                }
                if space && !space_was && ctrl.ds().enabled() {
                    ctrl.estop(Source::Hotkey);
                }
                if enable
                    && !enable_was
                    && let Err(e) = ctrl.enable(Source::Hotkey, None)
                {
                    ctrl.notice(format!("Could not enable: {e}"));
                }

                return_was = ret;
                space_was = space;
                enable_was = enable;
                thread::sleep(Duration::from_millis(20));
            }
            #[allow(unreachable_code)]
            // SAFETY: closing the display we opened
            unsafe {
                XCloseDisplay(display);
            }
        })
        .is_ok()
}
