// file: crates/uaa-core/src/iso/remaster.rs
// version: 1.1.0
// guid: 9cae93d1-edc1-4ff6-8234-1014a22fe1cf
// last-edited: 2026-07-10

//! ISO remaster — Rust port of `scripts/make-ssh-ready-iso.sh` (v1.2.0).
//!
//! Injects the NoCloud cloud-init seed (`installer-image/nocloud/`) into a
//! stock Ubuntu Server ISO's GRUB kernel cmdline so the LIVE installer
//! session boots with `openssh-server` on and the NoCloud datasource wired
//! (`ds=nocloud;s=/cdrom/nocloud/`), with no `autoinstall:` key baked in by
//! default. Opting in with `RemasterOptions::autoinstall` additionally bakes
//! the `uaa.autoinstall` token (+ optional `uaa.on_done=<action>`), which the
//! seed's `runcmd` gate uses to auto-run the install agent on boot.
//!
//! Every external tool invocation (`xorriso`) runs through [`CommandExecutor`]
//! so tests can inject a mock and assert on the exact command text — no real
//! ISO or `xorriso` binary is ever required to exercise this module. Cmdline
//! patching itself is pure `&str` -> `(String, bool)` logic, independently
//! testable without any executor at all.

use std::path::{Path, PathBuf};

use regex::Regex;

use crate::error::AutoInstallError;
use crate::network::CommandExecutor;
use crate::Result;

/// What the installed system should do once `uaa-usb-bootstrap.sh` finishes.
/// Only meaningful when [`RemasterOptions::autoinstall`] is set — mirrors the
/// script's `--on-done poweroff|reboot|shell` flag / `UAA_ON_DONE` env var.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OnDone {
    Poweroff,
    Reboot,
    Shell,
}

impl OnDone {
    /// The literal token value baked into `uaa.on_done=<value>`.
    pub fn as_str(self) -> &'static str {
        match self {
            OnDone::Poweroff => "poweroff",
            OnDone::Reboot => "reboot",
            OnDone::Shell => "shell",
        }
    }
}

/// Options for [`remaster`], mirroring `make-ssh-ready-iso.sh`'s flags/env
/// (`--autoinstall`/`UAA_AUTOINSTALL`, `--on-done`/`UAA_ON_DONE`,
/// `UAA_SEED_DIR`).
#[derive(Debug, Clone)]
pub struct RemasterOptions {
    /// Input ISO path, block device path, or an already-prefixed `stdio:...`.
    pub input: String,
    /// Output ISO path; defaults to `<input minus .iso>-ssh-ready.iso`.
    pub output: Option<String>,
    /// NoCloud seed directory (must contain `user-data` + `meta-data`).
    pub seed_dir: PathBuf,
    /// Opt-in: bake the `uaa.autoinstall` token.
    pub autoinstall: bool,
    /// Optional `uaa.on_done=<action>` token (only emitted when `autoinstall`).
    pub on_done: Option<OnDone>,
}

/// Resolve the `xorriso -indev` argument for `input`. `xorriso` addresses
/// non-MMC devices with a `stdio:` prefix.
///
/// - `stdio:*` passes through unchanged.
/// - `is_block_device` (probed by the caller — kept out of this pure function
///   so it stays unit-testable without a real block device) prefixes `stdio:`.
/// - otherwise `input` must be an existing regular file, used as-is.
/// - anything else (missing path) is a hard `ConfigError`.
pub fn resolve_input_dev(input: &str, is_block_device: bool) -> Result<String> {
    if input.starts_with("stdio:") {
        return Ok(input.to_string());
    }
    if is_block_device {
        return Ok(format!("stdio:{input}"));
    }
    if Path::new(input).is_file() {
        return Ok(input.to_string());
    }
    Err(AutoInstallError::ConfigError(format!(
        "input not found (need an .iso file or block device): {input}"
    )))
}

/// Refuse to write the output ISO to a device path — `stdio:*` or `/dev/*`.
pub fn validate_output_path(out: &str) -> Result<()> {
    if out.starts_with("stdio:") || out.starts_with("/dev/") {
        return Err(AutoInstallError::ConfigError(format!(
            "refusing to write output to a device ({out}); give a file path"
        )));
    }
    Ok(())
}

/// `<input minus .iso>-ssh-ready.iso`.
pub fn default_output_for(input: &str) -> String {
    let base = input.strip_suffix(".iso").unwrap_or(input);
    format!("{base}-ssh-ready.iso")
}

/// Matches the vmlinuz path on any `linux`/`linuxefi` GRUB boot line, e.g.
/// `linux /casper/vmlinuz` or `linuxefi /casper/vmlinuz`. Capture group 1 is
/// the whole matched prefix, used as the insertion point for both patch
/// functions below.
fn vmlinuz_regex() -> Regex {
    Regex::new(r"(linux(efi)?[[:space:]]+/casper/vmlinuz)")
        .expect("static vmlinuz regex is valid")
}

/// Insert the NoCloud cloud-init datasource tokens right after every
/// `linux`/`linuxefi /casper/vmlinuz` boot entry: ` ds=nocloud\;s=/cdrom/nocloud/
/// autoinstall=0`. The `;` MUST stay backslash-escaped — GRUB otherwise reads
/// it as a statement separator.
///
/// Idempotent: if `ds=nocloud` already appears anywhere in `cfg`, returns the
/// input unchanged and `false` (mirrors `patch_cfg()` in
/// make-ssh-ready-iso.sh). This idempotency guard is INDEPENDENT of
/// [`patch_autoinstall_tokens`]'s guard: each patch is applied/skipped solely
/// based on its own marker text, so re-running one does not affect the other.
pub fn patch_kernel_cmdline(cfg: &str) -> (String, bool) {
    if cfg.contains("ds=nocloud") {
        return (cfg.to_string(), false);
    }
    let patched = vmlinuz_regex()
        .replace_all(cfg, r"$1 ds=nocloud\;s=/cdrom/nocloud/ autoinstall=0")
        .into_owned();
    (patched, true)
}

/// Insert the opt-in autoinstall tokens right after every
/// `linux`/`linuxefi /casper/vmlinuz` boot entry: ` uaa.autoinstall` plus
/// ` uaa.on_done=<action>` when `on_done` is set. The seed's `runcmd` gate
/// checks for `uaa.autoinstall` on the kernel cmdline to auto-run
/// `uaa-usb-bootstrap.sh`.
///
/// Idempotent: if `uaa.autoinstall` already appears anywhere in `cfg`,
/// returns the input unchanged and `false` (mirrors `patch_autoinstall()`).
/// INDEPENDENT of [`patch_kernel_cmdline`]'s idempotency guard — re-running
/// with `--autoinstall` on an already-SSH-ready ISO only adds this token;
/// running it twice adds nothing.
pub fn patch_autoinstall_tokens(cfg: &str, on_done: Option<OnDone>) -> (String, bool) {
    if cfg.contains("uaa.autoinstall") {
        return (cfg.to_string(), false);
    }
    let tokens = match on_done {
        Some(action) => format!("uaa.autoinstall uaa.on_done={}", action.as_str()),
        None => "uaa.autoinstall".to_string(),
    };
    let replacement = format!("$1 {tokens}");
    let patched = vmlinuz_regex()
        .replace_all(cfg, replacement.as_str())
        .into_owned();
    (patched, true)
}

/// Quote `s` for safe interpolation into a `bash -c` command string. Paths
/// made only of the usual filesystem-safe characters pass through unquoted
/// (keeps generated commands readable); anything else is single-quoted with
/// embedded quotes escaped.
fn sh_quote(s: &str) -> String {
    let is_safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':'));
    if is_safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// `xorriso -osirrox on -indev <dev> -extract <iso_path> <dest>`.
fn extract_command(in_dev: &str, iso_path: &str, dest: &Path) -> String {
    format!(
        "xorriso -osirrox on -indev {} -extract {} {}",
        sh_quote(in_dev),
        sh_quote(iso_path),
        sh_quote(&dest.display().to_string())
    )
}

/// `xorriso -indev <dev> -outdev <out> -boot_image any replay -compliance
/// no_emul_toc -map <local> <iso_target> ...`. The `-boot_image any replay` +
/// `no_emul_toc` pair is load-bearing — a repack without them produces an
/// unbootable stick (El Torito boot catalog would not survive the repack).
fn repack_command(in_dev: &str, out_iso: &str, maps: &[(PathBuf, &str)]) -> String {
    let mut cmd = format!(
        "xorriso -indev {} -outdev {} -boot_image any replay -compliance no_emul_toc",
        sh_quote(in_dev),
        sh_quote(out_iso)
    );
    for (local, iso_target) in maps {
        cmd.push_str(&format!(
            " -map {} {}",
            sh_quote(&local.display().to_string()),
            sh_quote(iso_target)
        ));
    }
    cmd
}

/// Probe whether `path` is an existing block device (e.g. a USB stick like
/// `/dev/sdc`). Kept as a thin, non-pure wrapper around `std::fs` so
/// [`resolve_input_dev`] itself stays a pure, trivially-testable function.
#[cfg(unix)]
fn is_block_device_path(path: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path)
        .map(|m| m.file_type().is_block_device())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_block_device_path(_path: &str) -> bool {
    false
}

/// Re-master `opts.input` into `opts.output` (or the computed default),
/// returning the output path on success.
///
/// Preflight checks run fail-closed and strictly BEFORE any `xorriso`
/// invocation, in this order: seed dir contents, output-path device
/// refusal, then `xorriso` on `PATH` (the only preflight check that goes
/// through the executor — a missing seed or a device output path never
/// touches the executor at all).
pub async fn remaster(
    executor: &mut dyn CommandExecutor,
    opts: &RemasterOptions,
) -> Result<String> {
    let user_data = opts.seed_dir.join("user-data");
    let meta_data = opts.seed_dir.join("meta-data");
    if !user_data.is_file() || !meta_data.is_file() {
        return Err(AutoInstallError::ConfigError(format!(
            "seed missing in {}",
            opts.seed_dir.display()
        )));
    }

    let output = opts
        .output
        .clone()
        .unwrap_or_else(|| default_output_for(&opts.input));
    validate_output_path(&output)?;

    if !executor.check_silent("command -v xorriso").await? {
        return Err(AutoInstallError::ConfigError(
            "xorriso not found (apt install xorriso / brew install xorriso)".to_string(),
        ));
    }

    let is_block = is_block_device_path(&opts.input);
    let in_dev = resolve_input_dev(&opts.input, is_block)?;

    let tmp = tempfile::TempDir::new().map_err(AutoInstallError::IoError)?;
    let grub_path = tmp.path().join("grub.cfg");
    let loopback_path = tmp.path().join("loopback.cfg");

    let extract_grub_cmd = extract_command(&in_dev, "/boot/grub/grub.cfg", &grub_path);
    executor
        .execute(&extract_grub_cmd)
        .await
        .map_err(|_| AutoInstallError::ConfigError("no /boot/grub/grub.cfg in ISO".to_string()))?;
    let grub_cfg = std::fs::read_to_string(&grub_path).map_err(AutoInstallError::IoError)?;

    let extract_loopback_cmd = extract_command(&in_dev, "/boot/grub/loopback.cfg", &loopback_path);
    let have_loopback = executor.execute(&extract_loopback_cmd).await.is_ok();
    let loopback_cfg = if have_loopback {
        Some(std::fs::read_to_string(&loopback_path).map_err(AutoInstallError::IoError)?)
    } else {
        None
    };

    let (grub_cfg, _) = patch_kernel_cmdline(&grub_cfg);
    let loopback_cfg = loopback_cfg.map(|c| patch_kernel_cmdline(&c).0);

    let (grub_cfg, loopback_cfg) = if opts.autoinstall {
        let grub_cfg = patch_autoinstall_tokens(&grub_cfg, opts.on_done).0;
        let loopback_cfg = loopback_cfg.map(|c| patch_autoinstall_tokens(&c, opts.on_done).0);
        (grub_cfg, loopback_cfg)
    } else {
        (grub_cfg, loopback_cfg)
    };

    std::fs::write(&grub_path, &grub_cfg).map_err(AutoInstallError::IoError)?;
    let mut maps = vec![
        (grub_path.clone(), "/boot/grub/grub.cfg"),
        (opts.seed_dir.clone(), "/nocloud"),
    ];
    if let Some(ref lc) = loopback_cfg {
        std::fs::write(&loopback_path, lc).map_err(AutoInstallError::IoError)?;
        maps.push((loopback_path.clone(), "/boot/grub/loopback.cfg"));
    }

    let repack_cmd = repack_command(&in_dev, &output, &maps);
    executor
        .execute(&repack_cmd)
        .await
        .map_err(|e| AutoInstallError::SystemError(format!("repack failed: {e}")))?;

    Ok(output)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    /// Recording mock: logs every command it receives (so tests can assert
    /// fail-closed behavior recorded zero commands) and simulates the two
    /// real side effects `remaster()` depends on from `xorriso`: whether
    /// `xorriso` is "installed", and writing fixture cfg content to the
    /// destination path of an `-extract` command (a real `xorriso` would
    /// populate that path on disk; commands embed a randomly-generated
    /// tempdir path per run, so responses are matched by pattern rather than
    /// full-string equality like the simpler `MockExecutor` in verify.rs).
    #[derive(Default)]
    struct MockExecutor {
        commands: Vec<String>,
        xorriso_present: bool,
        fail_grub_extract: bool,
        fail_loopback_extract: bool,
        grub_cfg_fixture: String,
        loopback_cfg_fixture: String,
    }

    fn write_fixture_to_last_path(command: &str, content: &str) {
        if let Some(dest) = command.split_whitespace().last() {
            let dest = dest.trim_matches('\'');
            let _ = std::fs::write(dest, content);
        }
    }

    #[async_trait]
    impl CommandExecutor for MockExecutor {
        async fn connect(&mut self, _host: &str, _username: &str) -> Result<()> {
            Ok(())
        }

        async fn execute(&mut self, command: &str) -> Result<()> {
            self.commands.push(command.to_string());
            if command.contains("-extract") && command.contains("loopback.cfg") {
                if self.fail_loopback_extract {
                    return Err(AutoInstallError::SystemError(
                        "mock: no loopback.cfg in ISO".to_string(),
                    ));
                }
                write_fixture_to_last_path(command, &self.loopback_cfg_fixture);
                return Ok(());
            }
            if command.contains("-extract") && command.contains("grub.cfg") {
                if self.fail_grub_extract {
                    return Err(AutoInstallError::SystemError(
                        "mock: no grub.cfg in ISO".to_string(),
                    ));
                }
                write_fixture_to_last_path(command, &self.grub_cfg_fixture);
                return Ok(());
            }
            Ok(())
        }

        async fn execute_with_output(&mut self, command: &str) -> Result<String> {
            self.commands.push(command.to_string());
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
            if command.contains("command -v xorriso") {
                return Ok(self.xorriso_present);
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

    fn seeded_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("user-data"), "#cloud-config\n").unwrap();
        std::fs::write(dir.path().join("meta-data"), "").unwrap();
        dir
    }

    // ── Pure patch function tests ───────────────────────────────────────────

    #[test]
    fn test_patch_cmdline_inserts_tokens() {
        let cfg = "menuentry 'Ubuntu' {\n  linux\t/casper/vmlinuz quiet\n}\n\
                   menuentry 'UEFI' {\n  linuxefi /casper/vmlinuz quiet\n}\n";
        let (patched, changed) = patch_kernel_cmdline(cfg);
        assert!(changed);
        assert!(patched.contains("linux\t/casper/vmlinuz ds=nocloud\\;s=/cdrom/nocloud/ autoinstall=0 quiet"));
        assert!(patched.contains("linuxefi /casper/vmlinuz ds=nocloud\\;s=/cdrom/nocloud/ autoinstall=0 quiet"));
    }

    #[test]
    fn test_patch_cmdline_idempotent() {
        let cfg = "linux /casper/vmlinuz quiet\n";
        let (once, first_changed) = patch_kernel_cmdline(cfg);
        assert!(first_changed);
        let (twice, second_changed) = patch_kernel_cmdline(&once);
        assert_eq!(once, twice);
        assert!(!second_changed);
    }

    #[test]
    fn test_patch_autoinstall_independent_idempotency() {
        // Already has ds=nocloud (patch 1 applied) — patch_autoinstall must
        // still fire independently.
        let cfg = "linux /casper/vmlinuz ds=nocloud\\;s=/cdrom/nocloud/ autoinstall=0 quiet\n";
        let (patched, changed) = patch_autoinstall_tokens(cfg, Some(OnDone::Poweroff));
        assert!(changed);
        assert!(patched.contains("uaa.autoinstall"));
        assert!(patched.contains("uaa.on_done=poweroff"));

        let (patched_again, changed_again) = patch_autoinstall_tokens(&patched, Some(OnDone::Poweroff));
        assert_eq!(patched, patched_again);
        assert!(!changed_again);
    }

    #[test]
    fn test_semicolon_stays_escaped() {
        let cfg = "linux /casper/vmlinuz quiet\n";
        let (patched, _) = patch_kernel_cmdline(cfg);
        assert!(patched.contains("ds=nocloud\\;s="));
    }

    #[test]
    fn test_resolve_input_and_output_guards() {
        assert_eq!(
            resolve_input_dev("stdio:/dev/sdc", false).unwrap(),
            "stdio:/dev/sdc"
        );
        assert_eq!(
            resolve_input_dev("/dev/sdc", true).unwrap(),
            "stdio:/dev/sdc"
        );
        assert!(validate_output_path("/dev/sdc").is_err());
        assert!(validate_output_path("stdio:x").is_err());
    }

    // ── Orchestrator tests (mock executor, no real xorriso) ────────────────

    #[tokio::test]
    async fn test_remaster_fails_closed_before_xorriso() {
        // seed_dir intentionally empty (no user-data/meta-data written).
        let seed_dir = tempfile::tempdir().expect("tempdir");
        let opts = RemasterOptions {
            input: "does-not-matter.iso".to_string(),
            output: None,
            seed_dir: seed_dir.path().to_path_buf(),
            autoinstall: false,
            on_done: None,
        };
        let mut mock = MockExecutor::default();
        let result = remaster(&mut mock, &opts).await;
        assert!(result.is_err());
        assert_eq!(mock.commands.len(), 0);
    }

    #[tokio::test]
    async fn test_remaster_repack_preserves_el_torito() {
        let seed_dir = seeded_dir();
        let work_dir = tempfile::tempdir().expect("tempdir");
        let input_iso = work_dir.path().join("input.iso");
        std::fs::write(&input_iso, "fake iso bytes").unwrap();
        let output_iso = work_dir.path().join("output.iso");

        let opts = RemasterOptions {
            input: input_iso.display().to_string(),
            output: Some(output_iso.display().to_string()),
            seed_dir: seed_dir.path().to_path_buf(),
            autoinstall: false,
            on_done: None,
        };

        let mut mock = MockExecutor {
            xorriso_present: true,
            grub_cfg_fixture: "linux /casper/vmlinuz quiet\n".to_string(),
            loopback_cfg_fixture: "linuxefi /casper/vmlinuz quiet\n".to_string(),
            ..Default::default()
        };

        let result = remaster(&mut mock, &opts).await;
        assert!(result.is_ok(), "remaster failed: {:?}", result.err());

        let repack_cmd = mock.commands.last().expect("at least one command");
        assert!(repack_cmd.contains("-boot_image any replay"));
        assert!(repack_cmd.contains("-compliance no_emul_toc"));
        assert!(repack_cmd.contains("-map"));
        assert!(repack_cmd.contains("/nocloud"));
    }

    #[tokio::test]
    async fn test_remaster_no_loopback_tolerated() {
        let seed_dir = seeded_dir();
        let work_dir = tempfile::tempdir().expect("tempdir");
        let input_iso = work_dir.path().join("input.iso");
        std::fs::write(&input_iso, "fake iso bytes").unwrap();
        let output_iso = work_dir.path().join("output.iso");

        let opts = RemasterOptions {
            input: input_iso.display().to_string(),
            output: Some(output_iso.display().to_string()),
            seed_dir: seed_dir.path().to_path_buf(),
            autoinstall: false,
            on_done: None,
        };

        let mut mock = MockExecutor {
            xorriso_present: true,
            grub_cfg_fixture: "linux /casper/vmlinuz quiet\n".to_string(),
            fail_loopback_extract: true,
            ..Default::default()
        };

        let result = remaster(&mut mock, &opts).await;
        assert!(result.is_ok(), "remaster failed: {:?}", result.err());

        let repack_cmd = mock.commands.last().expect("at least one command");
        assert!(!repack_cmd.contains("loopback.cfg"));
    }
}
