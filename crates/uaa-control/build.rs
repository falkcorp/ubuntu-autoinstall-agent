// file: crates/uaa-control/build.rs
// version: 1.0.0
// guid: 6f3f5f9e-6f0a-4b6e-8a4c-9d2f0b1c3e7a
// last-edited: 2026-07-12

//! Builds `web/` (the operator SPA) before this crate compiles, so
//! `operator::web_ui`'s `#[derive(RustEmbed)]` (`#[folder = "../../web/dist"]`)
//! always has fresh assets to embed — the "one binary, no file copying"
//! deploy story. Set `UAA_SKIP_WEB_BUILD=1` to skip (e.g. no Node.js
//! available): `web/dist` must then already be populated from a prior build.

use std::path::Path;
use std::process::Command;

fn main() {
    let web_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web");

    // Only rerun `npm` when the SPA's own inputs change — never on every
    // Rust-only edit, and never re-triggered by `dist/` itself (which would
    // self-loop).
    for rel in [
        "src",
        "index.html",
        "package.json",
        "package-lock.json",
        "vite.config.ts",
        "tsconfig.json",
    ] {
        println!("cargo:rerun-if-changed={}", web_dir.join(rel).display());
    }
    println!("cargo:rerun-if-env-changed=UAA_SKIP_WEB_BUILD");

    if std::env::var_os("UAA_SKIP_WEB_BUILD").is_some() {
        println!(
            "cargo:warning=UAA_SKIP_WEB_BUILD set — skipping `npm run build` in web/; \
             web/dist must already be populated from a prior build"
        );
        return;
    }

    run(&web_dir, "npm", &["ci"]);
    run(&web_dir, "npm", &["run", "build"]);
}

fn run(dir: &Path, cmd: &str, args: &[&str]) {
    let status = Command::new(cmd)
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "failed to spawn `{cmd} {}` in {}: {e} (Node.js/npm is required to build \
             uaa-control; set UAA_SKIP_WEB_BUILD=1 to skip and use a pre-built web/dist instead)",
                args.join(" "),
                dir.display()
            )
        });
    if !status.success() {
        panic!(
            "`{cmd} {}` in {} exited with {status}",
            args.join(" "),
            dir.display()
        );
    }
}
