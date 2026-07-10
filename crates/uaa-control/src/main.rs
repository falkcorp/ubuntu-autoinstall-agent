// file: crates/uaa-control/src/main.rs
// version: 1.0.0
// guid: 608d78f5-ac4c-43a5-b29f-e9008a300858
// last-edited: 2026-07-10

//! Thin entrypoint for the `uaa-control` daemon. All logic lives in `uaa_control`
//! (the library) so `cargo test --lib --offline` exercises it. `serve` is the default
//! subcommand; `import`/`export`/`audit` are placeholders their follower tasks fill.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "uaa-control", about = "uaa constellation control daemon (spec C3)")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Bind the four listeners and serve (default).
    Serve,
    /// Import a registry export (filled by control TASK-02).
    Import,
    /// Export the registry (filled by control TASK-02).
    Export,
    /// Inspect / verify the audit chain (filled by control TASK-04).
    Audit,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber_init();
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            uaa_control::listeners::serve(uaa_control::listeners::ServeConfig::default()).await
        }
        Command::Import => not_yet_implemented("import", "control TASK-02"),
        Command::Export => not_yet_implemented("export", "control TASK-02"),
        Command::Audit => not_yet_implemented("audit", "control TASK-04"),
    }
}

fn not_yet_implemented(cmd: &str, owner: &str) -> ! {
    eprintln!("uaa-control {cmd}: not yet implemented — see {owner}");
    std::process::exit(1);
}

/// Minimal tracing init without pulling extra deps into the workspace table.
fn tracing_subscriber_init() {
    // uaa-core already depends on tracing-subscriber; uaa-control keeps the binary
    // thin and lets tracing default to a no-op subscriber if none is installed. A
    // richer subscriber is a runtime/deploy concern (Bucket-3), not scaffold code.
}
