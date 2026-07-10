// file: crates/uaa-core/src/iso/image_build.rs
// version: 1.1.0
// guid: 10dd9cce-f46d-4c77-97bc-2f6d7a6f74e0
// last-edited: 2026-07-10

//! ISO image build — Rust port of `scripts/build-installer-image.sh` (v1.0.0).
//!
//! Builds the custom ZFS-on-LUKS installer image (Option 2) by OVERLAYING the
//! Ubuntu live-server squashfs with the static `uaa` agent + boot automation,
//! masking the stock installer's autostart unit, and repacking with
//! `mksquashfs -comp zstd`. Canonical's signed casper kernel/initrd are reused
//! unchanged (Secure Boot friendly); only the root squashfs is rewritten.
//!
//! Every external tool invocation (`unsquashfs`, `install`, `mkdir`, `ln`,
//! `rm`, `mksquashfs`, `du`, plus the `id -u` / `command -v unsquashfs`
//! preflight probes) runs through [`CommandExecutor`] so tests can inject a
//! recording mock and assert on exact command text — no real squashfs-tools
//! binary is ever required to exercise this module.
//!
//! Two steps carry **VERIFY-ON-VM** markers inherited verbatim from the shell
//! script: the exact stock-installer autostart unit name on 26.04
//! live-server is unconfirmed (all three candidates are masked), and whether
//! `debootstrap`/`sgdisk`/`zpool`/`cryptsetup`/`dracut`/`clevis` are present
//! in the live rootfs (missing tools WARN, never fail the build).
//! `scripts/vm-validate.sh` stage 3 greps for these exact strings — keep them
//! byte-for-byte identical if this module ever changes.

use std::path::PathBuf;

use crate::error::AutoInstallError;
use crate::network::CommandExecutor;
use crate::Result;

/// VERIFY-ON-VM: the exact stock-installer autostart unit on 26.04
/// live-server is unconfirmed; all three candidates are masked. vm-validate
/// stage 3 resolves this marker.
pub const MASK_UNITS: [&str; 3] = [
    "subiquity-server.service",
    "serial-subiquity@.service",
    "snap.subiquity.subiquity-server.service",
];

/// VERIFY-ON-VM: these must exist in the live rootfs or be baked in.
pub const REQUIRED_LIVE_TOOLS: [&str; 6] = [
    "debootstrap",
    "sgdisk",
    "zpool",
    "cryptsetup",
    "dracut",
    "clevis",
];

/// Prefixes searched (in this order) for each of [`REQUIRED_LIVE_TOOLS`]
/// inside the unpacked squashfs root, mirroring the script's three `[ -e
/// ... ]` checks (`usr/sbin`, `sbin`, `usr/bin`).
const TOOL_SEARCH_PREFIXES: [&str; 3] = ["usr/sbin", "sbin", "usr/bin"];

/// Inputs for [`image_build`], mirroring `build-installer-image.sh`'s
/// required `--src-squashfs` / `--agent` / `--out` flags plus the overlay
/// assets directory (`installer-image/` relative to the repo root in the
/// script; passed explicitly here so the module has no cwd assumption).
#[derive(Debug, Clone)]
pub struct ImageBuildOptions {
    /// Source live-server squashfs to unpack (must exist).
    pub src_squashfs: PathBuf,
    /// Static `uaa` agent binary to inject (must exist).
    pub agent_bin: PathBuf,
    /// Output squashfs path (overwritten if it already exists).
    pub out: PathBuf,
    /// Directory containing the overlay assets `uaa-autoinstall.sh` and
    /// `uaa-autoinstall.service` (default: `installer-image/`).
    pub overlay_dir: PathBuf,
}

/// Result of a successful [`image_build`] run.
#[derive(Debug, Clone, Default)]
pub struct ImageBuildReport {
    /// The output squashfs path (same as [`ImageBuildOptions::out`]).
    pub out: PathBuf,
    /// [`REQUIRED_LIVE_TOOLS`] entries not found in the live rootfs at any
    /// of [`TOOL_SEARCH_PREFIXES`] — WARN-only, never fails the build.
    pub missing_tools: Vec<String>,
    /// The stock-installer autostart units masked this run (always all of
    /// [`MASK_UNITS`] — masking an absent unit is a tolerated no-op).
    pub masked_units: Vec<String>,
}

/// Quote `s` for safe interpolation into a `bash -c` command string. Paths
/// made only of the usual filesystem-safe characters pass through unquoted
/// (keeps generated commands readable); anything else is single-quoted with
/// embedded quotes escaped. Mirrors `iso::remaster`'s helper of the same
/// name.
fn sh_quote(s: &str) -> String {
    let is_safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':' | '@'));
    if is_safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

fn path_arg(p: &std::path::Path) -> String {
    sh_quote(&p.display().to_string())
}

/// Overlay `opts.src_squashfs` with the static agent + boot automation and
/// repack it to `opts.out`, returning a report of what happened.
///
/// Preflight runs fail-closed and strictly BEFORE any pipeline command, in
/// this order: root check (`id -u` via the executor), `unsquashfs` on PATH
/// (via the executor), then plain filesystem existence checks for
/// `src_squashfs`, `agent_bin`, and both overlay assets. A preflight failure
/// never issues any pipeline command (unpack/inject/enable/mask/check/repack).
pub async fn image_build(
    executor: &mut dyn CommandExecutor,
    opts: &ImageBuildOptions,
) -> Result<ImageBuildReport> {
    // ── Preflight (fail-closed, before any pipeline command) ───────────────
    let uid_out = executor.execute_with_output("id -u").await?;
    let uid = uid_out.trim();
    if uid != "0" {
        return Err(AutoInstallError::ConfigError(format!(
            "must run as root (unsquashfs/mksquashfs + chroot bits); id -u = {uid}"
        )));
    }

    if !executor.check_silent("command -v unsquashfs").await? {
        return Err(AutoInstallError::ConfigError(
            "install squashfs-tools (unsquashfs not found)".to_string(),
        ));
    }

    if !opts.src_squashfs.is_file() {
        return Err(AutoInstallError::ConfigError(format!(
            "--src-squashfs missing/not found: {}",
            opts.src_squashfs.display()
        )));
    }
    if !opts.agent_bin.is_file() {
        return Err(AutoInstallError::ConfigError(format!(
            "--agent missing/not found: {}",
            opts.agent_bin.display()
        )));
    }
    let overlay_script = opts.overlay_dir.join("uaa-autoinstall.sh");
    let overlay_service = opts.overlay_dir.join("uaa-autoinstall.service");
    if !overlay_script.is_file() {
        return Err(AutoInstallError::ConfigError(format!(
            "overlay asset missing: {}",
            overlay_script.display()
        )));
    }
    if !overlay_service.is_file() {
        return Err(AutoInstallError::ConfigError(format!(
            "overlay asset missing: {}",
            overlay_service.display()
        )));
    }

    let work = tempfile::TempDir::new().map_err(AutoInstallError::IoError)?;
    let root = work.path().join("squashfs-root");

    // ── 1. Unpack ────────────────────────────────────────────────────────
    tracing::info!("==> Unpacking {}", opts.src_squashfs.display());
    let unsquashfs_cmd = format!(
        "unsquashfs -d {} {}",
        path_arg(&root),
        path_arg(&opts.src_squashfs)
    );
    executor.execute(&unsquashfs_cmd).await?;

    // ── 2. Inject agent + boot automation ──────────────────────────────────
    tracing::info!("==> Injecting agent + boot automation");
    let install_agent_cmd = format!(
        "install -m 0755 {} {}",
        path_arg(&opts.agent_bin),
        path_arg(&root.join("usr/local/bin/uaa"))
    );
    executor.execute(&install_agent_cmd).await?;

    let install_script_cmd = format!(
        "install -m 0755 {} {}",
        path_arg(&overlay_script),
        path_arg(&root.join("usr/local/bin/uaa-autoinstall.sh"))
    );
    executor.execute(&install_script_cmd).await?;

    let install_service_cmd = format!(
        "install -m 0644 {} {}",
        path_arg(&overlay_service),
        path_arg(&root.join("etc/systemd/system/uaa-autoinstall.service"))
    );
    executor.execute(&install_service_cmd).await?;

    // ── 3. Enable uaa-autoinstall.service ───────────────────────────────────
    tracing::info!("==> Enabling uaa-autoinstall.service (multi-user.target.wants)");
    let wants_dir = root.join("etc/systemd/system/multi-user.target.wants");
    let mkdir_cmd = format!("mkdir -p {}", path_arg(&wants_dir));
    executor.execute(&mkdir_cmd).await?;

    let enable_link = wants_dir.join("uaa-autoinstall.service");
    let ln_enable_cmd = format!(
        "ln -sf ../uaa-autoinstall.service {}",
        path_arg(&enable_link)
    );
    executor.execute(&ln_enable_cmd).await?;

    // ── 4. VERIFY-ON-VM marker 1: mask stock installer autostart ──────────
    // VERIFY-ON-VM: mask whatever autostarts the stock installer on 26.04
    // live-server. On recent server ISOs this is subiquity-server.service
    // (snap-wrapped variants exist). Masking is a no-op if the unit is
    // absent, so mask the likely candidates.
    tracing::warn!(
        "VERIFY-ON-VM: masking stock installer autostart (VERIFY unit name on VM) — \
         candidates: {MASK_UNITS:?}"
    );
    let systemd_dir = root.join("etc/systemd/system");
    let mut masked_units = Vec::with_capacity(MASK_UNITS.len());
    for unit in MASK_UNITS {
        let unit_path = systemd_dir.join(unit);
        let mask_cmd = format!("ln -sf /dev/null {}", path_arg(&unit_path));
        // Mirrors the script's `|| true`: a failed mask is logged, not fatal.
        if let Err(e) = executor.execute(&mask_cmd).await {
            tracing::warn!("mask of {unit} failed (tolerated, no-op semantics): {e}");
        }
        masked_units.push(unit.to_string());
    }

    // ── 5. VERIFY-ON-VM marker 2: live-rootfs install tools ────────────────
    // VERIFY-ON-VM: the agent needs debootstrap + gdisk in the LIVE rootfs
    // (casper has cryptsetup + zfs already). If absent, they must be baked
    // into the overlay. We can't apt-install offline here reliably, so flag
    // it loudly rather than silently shipping a broken image.
    tracing::warn!("VERIFY-ON-VM: checking live-rootfs install tools: {REQUIRED_LIVE_TOOLS:?}");
    let mut missing_tools = Vec::new();
    for tool in REQUIRED_LIVE_TOOLS {
        let mut found = false;
        for prefix in TOOL_SEARCH_PREFIXES {
            let candidate = root.join(prefix).join(tool);
            let check_cmd = format!("test -e {}", path_arg(&candidate));
            if executor.check_silent(&check_cmd).await? {
                found = true;
                break;
            }
        }
        if !found {
            tracing::warn!(
                "VERIFY-ON-VM: '{tool}' not found in live rootfs — bake it into the overlay"
            );
            missing_tools.push(tool.to_string());
        }
    }

    // ── 6. Repack ───────────────────────────────────────────────────────
    tracing::info!("==> Repacking squashfs -> {}", opts.out.display());
    let rm_cmd = format!("rm -f {}", path_arg(&opts.out));
    executor.execute(&rm_cmd).await?;

    let mksquashfs_cmd = format!(
        "mksquashfs {} {} -comp zstd -no-progress",
        path_arg(&root),
        path_arg(&opts.out)
    );
    executor
        .execute(&mksquashfs_cmd)
        .await
        .map_err(|e| AutoInstallError::SystemError(format!("mksquashfs failed: {e}")))?;

    let du_cmd = format!("du -h {}", path_arg(&opts.out));
    let size_out = executor
        .execute_with_output(&du_cmd)
        .await
        .unwrap_or_default();
    let size = size_out.split_whitespace().next().unwrap_or("?");
    tracing::info!("==> Done: {} ({size})", opts.out.display());

    Ok(ImageBuildReport {
        out: opts.out.clone(),
        missing_tools,
        masked_units,
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashSet;

    /// Recording mock: logs every command it receives so tests can assert on
    /// exact command order/text, and no real `unsquashfs`/`mksquashfs`
    /// binary is ever invoked.
    struct MockExecutor {
        commands: Vec<String>,
        uid: String,
        unsquashfs_present: bool,
        /// Tools reported absent at ALL THREE search prefixes.
        absent_tools: HashSet<&'static str>,
        /// MASK_UNITS entries whose `ln -sf /dev/null` mock-fails.
        fail_mask_units: HashSet<&'static str>,
    }

    impl Default for MockExecutor {
        fn default() -> Self {
            Self {
                commands: Vec::new(),
                uid: "0".to_string(),
                unsquashfs_present: true,
                absent_tools: HashSet::new(),
                fail_mask_units: HashSet::new(),
            }
        }
    }

    #[async_trait]
    impl CommandExecutor for MockExecutor {
        async fn connect(&mut self, _host: &str, _username: &str) -> Result<()> {
            Ok(())
        }

        async fn execute(&mut self, command: &str) -> Result<()> {
            self.commands.push(command.to_string());
            if command.starts_with("ln -sf /dev/null") {
                let last_segment = command.rsplit('/').next().unwrap_or_default();
                if self.fail_mask_units.contains(last_segment) {
                    return Err(AutoInstallError::SystemError(format!(
                        "mock: ln failed for {last_segment}"
                    )));
                }
            }
            Ok(())
        }

        async fn execute_with_output(&mut self, command: &str) -> Result<String> {
            self.commands.push(command.to_string());
            if command == "id -u" {
                return Ok(self.uid.clone());
            }
            if command.starts_with("du -h") {
                return Ok("42M\t/out/path\n".to_string());
            }
            Ok(String::new())
        }

        async fn execute_with_error_collection(
            &mut self,
            command: &str,
            _description: &str,
        ) -> Result<(i32, String, String)> {
            self.commands.push(command.to_string());
            Ok((0, String::new(), String::new()))
        }

        async fn check_silent(&mut self, command: &str) -> Result<bool> {
            self.commands.push(command.to_string());
            if command.contains("command -v unsquashfs") {
                return Ok(self.unsquashfs_present);
            }
            if command.starts_with("test -e") {
                for tool in REQUIRED_LIVE_TOOLS {
                    if command.ends_with(tool) {
                        return Ok(!self.absent_tools.contains(tool));
                    }
                }
                return Ok(true);
            }
            Ok(true)
        }

        async fn collect_debug_info(&mut self) -> Result<String> {
            Ok(String::new())
        }

        async fn upload_file(&mut self, _local: &str, _remote: &str) -> Result<()> {
            Ok(())
        }

        async fn download_file(&mut self, _remote: &str, _local: &str) -> Result<()> {
            Ok(())
        }

        fn disconnect(&mut self) {}
    }

    /// A tempdir with a fake src squashfs, agent binary, and overlay assets
    /// so filesystem-existence preflight checks pass.
    struct Fixture {
        _dir: tempfile::TempDir,
        opts: ImageBuildOptions,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let src_squashfs = dir.path().join("src.squashfs");
        std::fs::write(&src_squashfs, b"fake squashfs").unwrap();
        let agent_bin = dir.path().join("uaa-agent");
        std::fs::write(&agent_bin, b"fake elf").unwrap();
        let overlay_dir = dir.path().join("installer-image");
        std::fs::create_dir_all(&overlay_dir).unwrap();
        std::fs::write(overlay_dir.join("uaa-autoinstall.sh"), b"#!/bin/sh\n").unwrap();
        std::fs::write(overlay_dir.join("uaa-autoinstall.service"), b"[Unit]\n").unwrap();
        let out = dir.path().join("out.squashfs");

        let opts = ImageBuildOptions {
            src_squashfs,
            agent_bin,
            out,
            overlay_dir,
        };
        Fixture { _dir: dir, opts }
    }

    #[tokio::test]
    async fn test_preflight_fails_closed() {
        // Non-root.
        let fx = fixture();
        let mut mock = MockExecutor {
            uid: "1000".to_string(),
            ..Default::default()
        };
        let result = image_build(&mut mock, &fx.opts).await;
        assert!(result.is_err());
        assert!(!mock.commands.iter().any(|c| c.contains("unsquashfs -d")));

        // Missing --agent file.
        let fx2 = fixture();
        std::fs::remove_file(&fx2.opts.agent_bin).unwrap();
        let mut mock2 = MockExecutor::default();
        let result2 = image_build(&mut mock2, &fx2.opts).await;
        assert!(result2.is_err());
        assert!(!mock2.commands.iter().any(|c| c.contains("unsquashfs -d")));
    }

    #[tokio::test]
    async fn test_pipeline_command_order() {
        let fx = fixture();
        let mut mock = MockExecutor::default();
        let result = image_build(&mut mock, &fx.opts).await;
        assert!(result.is_ok(), "image_build failed: {:?}", result.err());

        let idx = |needle: &str| {
            mock.commands
                .iter()
                .position(|c| c.contains(needle))
                .unwrap_or_else(|| panic!("command containing {needle:?} not recorded"))
        };

        let unsquashfs = idx("unsquashfs -d");
        let install_agent = idx("install -m 0755");
        let install_service = idx("install -m 0644");
        let mkdir_wants = idx("mkdir -p");
        let enable_ln = mock
            .commands
            .iter()
            .position(|c| c.starts_with("ln -sf ../uaa-autoinstall.service"))
            .expect("enable ln recorded");
        let first_mask = mock
            .commands
            .iter()
            .position(|c| c.starts_with("ln -sf /dev/null"))
            .expect("mask ln recorded");
        let first_tool_check = mock
            .commands
            .iter()
            .position(|c| c.starts_with("test -e"))
            .expect("tool check recorded");
        let rm = idx("rm -f");
        let mksquashfs = idx("mksquashfs ");

        assert!(unsquashfs < install_agent);
        assert!(install_agent < install_service);
        assert!(install_service < mkdir_wants);
        assert!(mkdir_wants < enable_ln);
        assert!(enable_ln < first_mask);
        assert!(first_mask < first_tool_check);
        assert!(first_tool_check < rm);
        assert!(rm < mksquashfs);
    }

    #[tokio::test]
    async fn test_masks_all_three_units() {
        let fx = fixture();
        let mut mock = MockExecutor {
            fail_mask_units: HashSet::from(["serial-subiquity@.service"]),
            ..Default::default()
        };
        let result = image_build(&mut mock, &fx.opts).await;
        assert!(
            result.is_ok(),
            "a failing mask ln must not abort the build: {:?}",
            result.err()
        );

        for unit in MASK_UNITS {
            let matches: Vec<&String> = mock
                .commands
                .iter()
                .filter(|c| {
                    c.starts_with("ln -sf /dev/null")
                        && c.rsplit('/').next() == Some(unit)
                })
                .collect();
            assert_eq!(matches.len(), 1, "expected exactly one mask command for {unit}");
        }
    }

    #[tokio::test]
    async fn test_mksquashfs_zstd() {
        let fx = fixture();
        let out_display = fx.opts.out.display().to_string();
        let mut mock = MockExecutor::default();
        let result = image_build(&mut mock, &fx.opts).await;
        assert!(result.is_ok(), "image_build failed: {:?}", result.err());

        let final_cmd = mock.commands.last().expect("at least one command");
        // The final recorded command is the du -h size report; the
        // mksquashfs invocation is the one immediately before it.
        let mksquashfs_cmd = &mock.commands[mock.commands.len() - 2];
        assert!(mksquashfs_cmd.contains("mksquashfs"));
        assert!(mksquashfs_cmd.contains("-comp zstd"));
        assert!(mksquashfs_cmd.contains("-no-progress"));
        assert!(mksquashfs_cmd.contains(&out_display));
        assert!(final_cmd.contains("du -h"));
    }

    #[tokio::test]
    async fn test_missing_tool_warns_not_fails() {
        let fx = fixture();
        let mut mock = MockExecutor {
            absent_tools: HashSet::from(["debootstrap"]),
            ..Default::default()
        };
        let result = image_build(&mut mock, &fx.opts).await;
        let report = result.expect("missing tool must warn, not fail");
        assert_eq!(report.missing_tools, vec!["debootstrap".to_string()]);

        // All three prefixes were checked for the missing tool.
        let debootstrap_checks = mock
            .commands
            .iter()
            .filter(|c| c.starts_with("test -e") && c.ends_with("debootstrap"))
            .count();
        assert_eq!(debootstrap_checks, 3);
    }

    #[test]
    fn test_verify_on_vm_markers_present() {
        let src = include_str!("image_build.rs");
        assert!(
            src.matches("VERIFY-ON-VM").count() >= 2,
            "both VERIFY-ON-VM markers must be preserved"
        );
        for unit in MASK_UNITS {
            assert!(src.contains(unit), "mask unit {unit} missing from source");
        }
        for tool in REQUIRED_LIVE_TOOLS {
            assert!(src.contains(tool), "required tool {tool} missing from source");
        }
    }

    #[tokio::test]
    async fn test_agent_installed_0755() {
        // Anti-over-suppression: a valid build must clear every preflight
        // guard and reach the final mksquashfs — not just "not error".
        let fx = fixture();
        let agent_display = fx.opts.agent_bin.display().to_string();
        let mut mock = MockExecutor::default();
        let result = image_build(&mut mock, &fx.opts).await;
        assert!(result.is_ok(), "image_build failed: {:?}", result.err());

        let install_agent_cmd = mock
            .commands
            .iter()
            .find(|c| c.starts_with("install -m 0755") && c.contains(&agent_display))
            .expect("install -m 0755 <agent> ... recorded");
        assert!(install_agent_cmd.contains("usr/local/bin/uaa"));
        assert!(mock.commands.iter().any(|c| c.contains("mksquashfs")));
    }
}
