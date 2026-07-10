// file: crates/uaa/src/cli/enroll.rs
// version: 1.0.0
// guid: ff6b7df8-50d5-47a6-bfe5-3063d426ffe9
// last-edited: 2026-07-10

//! `uaa enroll` — stub, filled exclusively by pki/TASK-02 (PK-02).
#[derive(Debug, clap::Args)]
pub struct EnrollArgs {}
pub async fn enroll_command(_args: EnrollArgs) -> uaa_core::Result<()> {
    todo!("constellation: filled by pki/TASK-02 (PK-02)")
}
