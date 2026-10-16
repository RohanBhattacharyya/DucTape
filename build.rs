//! Builds the web UI (`ui/`) with npm so that `cargo build` is all anyone needs to run.
//!
//! Set `DUCTAPE_SKIP_UI_BUILD=1` to use whatever is already in `ui/dist` (packagers who build
//! the UI separately, or Rust-only development). Without npm, a placeholder page is embedded and
//! everything except the web UI (including MCP) still works.

use std::path::Path;
use std::process::Command;

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>DucTape</title></head>
<body style="font-family:sans-serif;background:#222;color:#eee;padding:2rem">
<h1>DucTape's web UI was not built</h1>
<p>Install Node.js 20 or newer and rebuild DucTape. The MCP server works without it.</p>
</body></html>
"#;

fn npm() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}

fn run(dir: &Path, args: &[&str]) -> bool {
    match Command::new(npm()).args(args).current_dir(dir).status() {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

fn placeholder(dist: &Path) {
    std::fs::create_dir_all(dist).expect("create ui/dist");
    for page in ["index.html", "console.html"] {
        let path = dist.join(page);
        if !path.exists() {
            std::fs::write(path, PLACEHOLDER).expect("write placeholder UI");
        }
    }
}

fn main() {
    let ui = Path::new("ui");
    let dist = ui.join("dist");
    for path in [
        "ui/src",
        "ui/public",
        "ui/index.html",
        "ui/console.html",
        "ui/package.json",
        "ui/package-lock.json",
        "ui/vite.config.ts",
    ] {
        println!("cargo::rerun-if-changed={path}");
    }
    println!("cargo::rerun-if-env-changed=DUCTAPE_SKIP_UI_BUILD");

    if std::env::var_os("DUCTAPE_SKIP_UI_BUILD").is_some_and(|v| v != "0") {
        placeholder(&dist);
        return;
    }

    let has_npm = Command::new(npm())
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_npm {
        if !dist.join("index.html").exists() {
            println!("cargo::warning=npm not found; embedding a placeholder instead of the web UI");
        }
        placeholder(&dist);
        return;
    }

    if !ui.join("node_modules").exists() {
        let args: &[&str] = if ui.join("package-lock.json").exists() {
            &["ci", "--no-audit", "--no-fund"]
        } else {
            &["install", "--no-audit", "--no-fund"]
        };
        if !run(ui, args) {
            panic!("`npm {}` failed in ui/", args.join(" "));
        }
    }
    if !run(ui, &["run", "build"]) {
        panic!("`npm run build` failed in ui/ (set DUCTAPE_SKIP_UI_BUILD=1 to skip)");
    }
}
