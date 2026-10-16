//! Crash reporting for the GUI: write a report with human-panic and say where it is in a dialog.

use std::panic::PanicHookInfo;
use std::path::Path;

use human_panic::{Metadata, handle_dump};
use tinyfiledialogs::{MessageBoxIcon, message_box_ok};

pub fn install() {
    // With RUST_BACKTRACE set, keep the default hook for developers
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::panic::set_hook(Box::new(hook));
    }
}

fn hook(info: &PanicHookInfo<'_>) {
    let meta = Metadata::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
        .authors(env!("CARGO_PKG_AUTHORS").replace(':', ", "))
        .homepage(env!("CARGO_PKG_HOMEPAGE"));

    let file = handle_dump(&meta, info);
    let msg = message(file.as_deref());
    eprintln!("{msg}");
    message_box_ok("An Unexpected Error Occurred", &msg, MessageBoxIcon::Error);
}

fn message(file: Option<&Path>) -> String {
    format!(
        "Well, this is embarrassing.\n\n\
         Conductor had a problem and crashed. To help diagnose the problem you can send a crash report.\n\n\
         A report file has been generated at \"{}\". Submit an issue and include the details in the \
         report file so the error can be diagnosed.\n\n\
         Conductor does not perform automated error collection. Improving it relies on people \
         submitting reports.\n\nThank you kindly!",
        file.map_or_else(
            || "<Failed to store file to disk>".to_string(),
            |p| p.display().to_string()
        )
    )
}
