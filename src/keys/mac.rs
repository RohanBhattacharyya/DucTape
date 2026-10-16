use std::thread;
use std::time::Duration;

use crate::controller::{Controller, Source};

#[path = "mac/mgr.rs"]
mod mgr;

use mgr::InputManager;

pub fn bind_keys(ctrl: Controller) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = thread::Builder::new()
        .name("hotkeys".into())
        .spawn(move || {
            let Some(mgr) = InputManager::new() else {
                tracing::warn!(
                    "could not create the HID input manager, global hotkeys are unavailable"
                );
                let _ = tx.send(false);
                return;
            };
            let _ = tx.send(true);
            let mut return_was = false;
            let mut space_was = false;
            loop {
                let ret = mgr.poll_enter();
                let space = mgr.poll_spacebar();
                if ret && !return_was {
                    ctrl.disable(Source::Hotkey);
                }
                if space && !space_was && ctrl.ds().enabled() {
                    ctrl.estop(Source::Hotkey);
                }
                return_was = ret;
                space_was = space;
                thread::sleep(Duration::from_millis(20));
            }
        });
    spawned.is_ok() && rx.recv().unwrap_or(false)
}
