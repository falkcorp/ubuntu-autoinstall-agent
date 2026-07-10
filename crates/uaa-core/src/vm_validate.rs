// file: crates/uaa-core/src/vm_validate.rs
// version: 1.1.1
// guid: e3044431-725f-4fc3-a710-2f13497fca46
// last-edited: 2026-07-10

//! QEMU+swtpm VM validation gate (`uaa vm-validate`) — Rust port of
//! `scripts/vm-validate.sh` (v1.0.0). THIS SCRIPT PASSING IS THE GATE — no
//! hardware attempt or len-serv-003 wipe before it passes.
//!
//! Boots the remastered SSH-ready ISO in QEMU with OVMF UEFI firmware, a
//! virtio qcow2 target disk (the guest sees `/dev/vda`), and a swtpm socket
//! TPM2 device (`tpm-tis`). It copies the `uaa` binary + a VM test config
//! into the live session over SSH, runs `uaa install --config` there,
//! reboots from the installed disk, and asserts LUKS unlock + rpool/bpool
//! import + multi-user. Stage 3 additionally interrogates the live
//! environment to resolve BOTH VERIFY-ON-VM markers in
//! `scripts/build-installer-image.sh` and prints the answers in a
//! machine-greppable report at the end (see [`render_report`]).
//!
//! `scripts/vm-validate.sh` STAYS AUTHORITATIVE until TG-03 proves this
//! port — do not point `docs/vm-validation.md` at this module yet.
//!
//! Every external process (`qemu-system-x86_64`, `swtpm`, `qemu-img`, `ssh`,
//! `scp`, `command -v` probes, `uname`) goes through the [`CommandExecutor`]
//! seam so the whole harness is mockable — NO real VM is ever launched by
//! the unit tests below. Wait-loops (pidfile wait, socket wait, SSH wait,
//! qemu-exit wait) are encoded as single shell one-liners dispatched through
//! the executor rather than native `tokio::time::sleep` loops, so a mock
//! executor resolves them instantly instead of burning real wall-clock time.
//! Cleanup only ever kills pids this harness itself recorded — never
//! `pkill` (the host may be running other VMs).

use crate::error::AutoInstallError;
use crate::network::CommandExecutor;
use crate::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Options for a `uaa vm-validate` run. Defaults mirror
/// `scripts/vm-validate.sh`'s own flag defaults.
#[derive(Debug, Clone)]
pub struct VmValidateOptions {
    pub iso: PathBuf,
    pub agent: PathBuf,
    pub config: PathBuf,
    pub workdir: PathBuf,
    /// e.g. "40G"
    pub disk_size: String,
    pub ssh_port: u16,
    pub boot_timeout: u64,
    pub install_timeout: u64,
}

impl Default for VmValidateOptions {
    fn default() -> Self {
        Self {
            iso: PathBuf::new(),
            agent: PathBuf::new(),
            config: PathBuf::from("examples/configs/install/vm-test.yaml"),
            workdir: PathBuf::from("./vm-validate-work"),
            disk_size: "40G".to_string(),
            ssh_port: 10022,
            boot_timeout: 600,
            install_timeout: 3600,
        }
    }
}

/// Live-rootfs tool presence, as resolved by stage 3 (or left `Unknown` when
/// stage 3 was never reached because an earlier stage failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Present,
    Missing,
    Unknown,
}

impl ToolStatus {
    fn as_str(self) -> &'static str {
        match self {
            ToolStatus::Present => "present",
            ToolStatus::Missing => "MISSING",
            ToolStatus::Unknown => "UNKNOWN",
        }
    }
}

/// Overall gate outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateResult {
    Pass,
    Fail { first_failing_stage: String },
}

/// The full `==== VERIFY-ON-VM REPORT ====` payload. See [`render_report`]
/// for the byte-compatible rendering.
#[derive(Debug, Clone)]
pub struct VerifyOnVmReport {
    pub observed_units: Option<String>,
    pub marker72_verdict: Option<String>,
    pub tool_status: BTreeMap<String, ToolStatus>,
    pub gate: GateResult,
}

/// The exact 3-unit mask list from `scripts/build-installer-image.sh`'s
/// stock-installer autostart unit (marker :72). Duplicated here rather than
/// imported from `image_build.rs::MASK_UNITS` (TP-03) to avoid a cross-file
/// dependency that would collide with TP-03's wave.
pub const MASK_UNITS: [&str; 3] = [
    "subiquity-server.service",
    "serial-subiquity@.service",
    "snap.subiquity.subiquity-server.service",
];

/// Live-rootfs tools interrogated by stage 3 (marker :81), in report order.
pub const INTERROGATE_TOOLS: [&str; 6] =
    ["debootstrap", "sgdisk", "zpool", "cryptsetup", "dracut", "clevis"];

const SSH_USER: &str = "ubuntu-server";
const SSH_LIVE_PASSWORD: &str = "default";

const REQUIRED_TOOLS: [&str; 5] = ["qemu-system-x86_64", "swtpm", "qemu-img", "ssh", "scp"];
const OVMF_DIRS: [&str; 4] = [
    "/usr/share/OVMF",
    "/usr/share/qemu",
    "/usr/share/edk2/ovmf",
    "/usr/share/edk2-ovmf",
];

const LOG_00_PREFLIGHT: &str = "00-preflight.log";
const LOG_01_WORKSPACE: &str = "01-workspace.log";
const LOG_02_BOOT_ISO: &str = "02-boot-iso.log";
const LOG_03_INTERROGATE: &str = "03-interrogate.log";
const LOG_04_INSTALL: &str = "04-install.log";
const LOG_05_BOOT_DISK: &str = "05-boot-disk.log";
const LOG_06_ASSERT: &str = "06-assert.log";
const LOG_07_REPORT: &str = "07-report.log";

/// Byte-compatible re-implementation of `scripts/vm-validate.sh`'s
/// `print_report`. PURE — takes no executor, does no I/O.
pub fn render_report(r: &VerifyOnVmReport) -> String {
    let mut out = String::new();
    out.push_str("==== VERIFY-ON-VM REPORT ====\n");
    out.push_str("marker build-installer-image.sh:72 (stock-installer autostart unit):\n");
    out.push_str(&format!(
        "  observed-units: {}\n",
        r.observed_units
            .as_deref()
            .unwrap_or("UNKNOWN (stage 3 not reached)")
    ));
    out.push_str(&format!(
        "  masked-by-build-script: {}\n",
        MASK_UNITS.join(" ")
    ));
    out.push_str(&format!(
        "  verdict: {}\n",
        r.marker72_verdict
            .as_deref()
            .unwrap_or("UNKNOWN (stage 3 not reached)")
    ));
    out.push_str("marker build-installer-image.sh:81 (live-rootfs tools):\n");
    for tool in INTERROGATE_TOOLS {
        let status = r
            .tool_status
            .get(tool)
            .copied()
            .unwrap_or(ToolStatus::Unknown);
        out.push_str(&format!("  {:<12} {}\n", format!("{tool}:"), status.as_str()));
    }
    match &r.gate {
        GateResult::Pass => out.push_str("GATE: PASS\n"),
        GateResult::Fail {
            first_failing_stage,
        } => out.push_str(&format!("GATE: FAIL ({first_failing_stage})\n")),
    }
    out.push_str("=============================\n");
    out
}

// ---------------------------------------------------------------------
// Pure evaluators (stage-3 marker verdict, stage-4 install assertions,
// stage-6 disk-boot assertions, stage-0 placeholder die). Unit-testable
// without any process.
// ---------------------------------------------------------------------

/// Stage-3 marker-72 verdict: `COVERED` iff every unit in `observed_units`
/// (space-separated, or the literal `NONE`) is in [`MASK_UNITS`]; otherwise
/// `GAP (unit <u> not in mask list)` for the LAST non-masked unit seen
/// (mirrors the shell loop's last-write-wins semantics).
pub fn evaluate_marker72(observed_units: &str) -> String {
    if observed_units.trim().is_empty() || observed_units == "NONE" {
        return "COVERED".to_string();
    }
    let mut verdict = "COVERED".to_string();
    for u in observed_units.split_whitespace() {
        if !MASK_UNITS.contains(&u) {
            verdict = format!("GAP (unit {u} not in mask list)");
        }
    }
    verdict
}

/// Stage-4 install-log assertions: at least 7 `Phase completed:` lines AND a
/// `Phase 6: Final setup` line AND a case-insensitive `Installation
/// completed successfully` line. Returns the observed phase count on
/// success.
pub fn evaluate_install_log(log: &str) -> Result<u32> {
    let phase_count = log.matches("Phase completed:").count() as u32;
    let has_phase6 = log.contains("Phase 6: Final setup");
    let has_success = log
        .to_lowercase()
        .contains("installation completed successfully");
    if phase_count >= 7 && has_phase6 && has_success {
        Ok(phase_count)
    } else {
        Err(AutoInstallError::ValidationError(format!(
            "install log does not show all 7 phases completed (found {phase_count} 'Phase \
             completed:' lines, or missing the Phase 6 / final-success line)"
        )))
    }
}

/// Stage-6 disk-boot assertions: LUKS unlocked, both `rpool` and `bpool`
/// imported (exact-line match), and multi-user reached (either
/// `is-system-running` containing `running`/`degraded`, or the
/// `multi-user.target` fallback reporting exactly `active`).
pub fn evaluate_stage6(
    crypt_out: &str,
    zpool_out: &str,
    is_system_running: &str,
    multi_user_fallback: Option<&str>,
) -> Result<()> {
    if !crypt_out.contains("is active") {
        return Err(AutoInstallError::ValidationError(
            "cryptsetup status luks did not report 'is active'".to_string(),
        ));
    }

    let has_rpool = zpool_out.lines().any(|l| l.trim() == "rpool");
    let has_bpool = zpool_out.lines().any(|l| l.trim() == "bpool");
    if !(has_rpool && has_bpool) {
        return Err(AutoInstallError::ValidationError(
            "zpool list did not show both rpool and bpool".to_string(),
        ));
    }

    if is_system_running.contains("running") || is_system_running.contains("degraded") {
        return Ok(());
    }
    if multi_user_fallback == Some("active") {
        return Ok(());
    }
    Err(AutoInstallError::ValidationError(
        "system did not reach multi-user (is-system-running nor multi-user.target active)"
            .to_string(),
    ))
}

/// Stage-0 hard die: never install with an unsubstituted secret placeholder.
pub fn config_has_placeholder(text: &str) -> bool {
    text.contains("REPLACE_AT_PLACE_TIME")
}

// ---------------------------------------------------------------------
// Command-string builders (all executed via CommandExecutor::execute* —
// never a raw local process-spawn API). Kept as small pure functions so the
// test module below can build the same expected strings it asserts against.
// ---------------------------------------------------------------------

/// Single-quote-wrap a value for safe shell interpolation (POSIX): wrap in
/// single quotes, and replace each embedded single quote with '\'' .
fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn log_path_str(workdir: &Path, name: &str) -> String {
    format!("{}/logs/{name}", shq(&workdir.display().to_string()))
}

fn ssh_cmd(
    opts: &VmValidateOptions,
    have_sshpass: bool,
    user: &str,
    timeout_secs: u64,
    remote_cmd: &str,
) -> String {
    let ssh_opts = format!(
        "-p {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5",
        opts.ssh_port
    );
    let base = if have_sshpass && user == SSH_USER {
        format!("sshpass -p {SSH_LIVE_PASSWORD} ssh {ssh_opts} {user}@127.0.0.1")
    } else {
        format!("ssh {ssh_opts} {user}@127.0.0.1")
    };
    if timeout_secs > 0 {
        format!("timeout {timeout_secs} {base} \"{remote_cmd}\"")
    } else {
        format!("{base} \"{remote_cmd}\"")
    }
}

fn scp_cmd(
    opts: &VmValidateOptions,
    have_sshpass: bool,
    user: &str,
    local: &str,
    remote_dst: &str,
) -> String {
    let scp_opts = format!(
        "-P {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null",
        opts.ssh_port
    );
    let local = shq(local);
    let remote_dst = shq(remote_dst);
    if have_sshpass && user == SSH_USER {
        format!("sshpass -p {SSH_LIVE_PASSWORD} scp {scp_opts} {local} {user}@127.0.0.1:{remote_dst}")
    } else {
        format!("scp {scp_opts} {local} {user}@127.0.0.1:{remote_dst}")
    }
}

fn wait_for_ssh_cmd(opts: &VmValidateOptions, have_sshpass: bool, user: &str, overall: u64) -> String {
    let probe = ssh_cmd(opts, have_sshpass, user, 5, "true");
    format!(
        "waited=0; while [ $waited -lt {overall} ]; do {probe} && exit 0; sleep 5; waited=$((waited+5)); done; exit 1"
    )
}

fn firmware_args(ovmf_code: &str, ovmf_vars: Option<&str>, workdir: &Path) -> String {
    let ovmf_code = shq(ovmf_code);
    if ovmf_vars.is_some() {
        format!(
            " -drive if=pflash,format=raw,readonly=on,file={ovmf_code} -drive if=pflash,format=raw,file={}/OVMF_VARS.fd",
            shq(&workdir.display().to_string())
        )
    } else {
        format!(" -bios {ovmf_code}")
    }
}

// ---------------------------------------------------------------------
// Orchestration state carried across stages.
// ---------------------------------------------------------------------

#[derive(Default)]
struct Ctx {
    have_sshpass: bool,
    have_socat: bool,
    kvm_ok: bool,
    ovmf_code: String,
    ovmf_vars: Option<String>,
    disk_img: String,
    swtpm_sock: String,
    iso_qemu_pid: String,
    /// Every pid this harness itself started (swtpm, iso-boot qemu,
    /// disk-boot qemu), in launch order — the ONLY pids `cleanup` may kill.
    pids: Vec<String>,
    observed_units: Option<String>,
    marker72_verdict: Option<String>,
    tool_status: BTreeMap<String, ToolStatus>,
}

fn build_report(ctx: &Ctx, gate: GateResult) -> VerifyOnVmReport {
    VerifyOnVmReport {
        observed_units: ctx.observed_units.clone(),
        marker72_verdict: ctx.marker72_verdict.clone(),
        tool_status: ctx.tool_status.clone(),
        gate,
    }
}

/// Kill only pids THIS harness started. NEVER `pkill` by name — the host may
/// be running other VMs.
async fn cleanup(executor: &mut dyn CommandExecutor, pids: &[String]) {
    for pid in pids.iter().rev() {
        let alive = executor
            .check_silent(&format!("kill -0 {pid} 2>/dev/null"))
            .await
            .unwrap_or(false);
        if alive {
            let _ = executor.execute(&format!("kill {pid} 2>/dev/null || true")).await;
        }
    }
}

/// Render + (best-effort) log the final report, run cleanup, and convert the
/// gate outcome into the function's `Result`. Called exactly once per
/// `vm_validate` invocation, on every exit path.
async fn finish(
    executor: &mut dyn CommandExecutor,
    ctx: &Ctx,
    gate: GateResult,
) -> Result<VerifyOnVmReport> {
    let report = build_report(ctx, gate);
    println!("{}", render_report(&report));
    cleanup(executor, &ctx.pids).await;
    match &report.gate {
        GateResult::Pass => Ok(report),
        GateResult::Fail {
            first_failing_stage,
        } => Err(AutoInstallError::VmError(first_failing_stage.clone())),
    }
}

// ---------------------------------------------------------------------
// Stage 0: preflight
// ---------------------------------------------------------------------

async fn stage0_preflight(
    executor: &mut dyn CommandExecutor,
    opts: &VmValidateOptions,
    ctx: &mut Ctx,
) -> std::result::Result<(), String> {
    let log = log_path_str(&opts.workdir, LOG_00_PREFLIGHT);

    let uname = executor
        .execute_with_output("uname -s")
        .await
        .map_err(|e| format!("uname -s failed (see {log}): {e}"))?;
    if uname.trim() != "Linux" {
        return Err(
            "Linux host required (no KVM on macOS) — run on the server 172.16.2.30 or any amd64 Linux box"
                .to_string(),
        );
    }

    for bin in REQUIRED_TOOLS {
        let ok = executor
            .check_silent(&format!("command -v {bin} >/dev/null 2>&1"))
            .await
            .map_err(|e| e.to_string())?;
        if !ok {
            return Err(format!(
                "missing required tool '{bin}' — install it (e.g. qemu-system-x86, swtpm, openssh-client packages)"
            ));
        }
    }

    ctx.have_sshpass = executor
        .check_silent("command -v sshpass >/dev/null 2>&1")
        .await
        .unwrap_or(false);
    if !ctx.have_sshpass {
        tracing::warn!(
            "sshpass not found — falling back to key-only SSH auth for the live-session login (needs the operator key loaded, e.g. in an ssh-agent)"
        );
    }

    ctx.have_socat = executor
        .check_silent("command -v socat >/dev/null 2>&1")
        .await
        .unwrap_or(false);
    if !ctx.have_socat {
        tracing::warn!(
            "socat not found — cannot auto-answer a LUKS passphrase prompt on the disk-boot serial console (stage 5); if the reboot hangs there, install socat or ensure TPM2/Clevis auto-unlock is configured"
        );
    }

    for dir in OVMF_DIRS {
        for f in ["OVMF_CODE_4M.fd", "OVMF_CODE.fd"] {
            if ctx.ovmf_code.is_empty() {
                let path = format!("{dir}/{f}");
                if executor
                    .check_silent(&format!("test -f {path}"))
                    .await
                    .unwrap_or(false)
                {
                    ctx.ovmf_code = path;
                }
            }
        }
        for f in ["OVMF_VARS_4M.fd", "OVMF_VARS.fd"] {
            if ctx.ovmf_vars.is_none() {
                let path = format!("{dir}/{f}");
                if executor
                    .check_silent(&format!("test -f {path}"))
                    .await
                    .unwrap_or(false)
                {
                    ctx.ovmf_vars = Some(path);
                }
            }
        }
    }
    if ctx.ovmf_code.is_empty() {
        return Err(
            "OVMF firmware (OVMF_CODE*.fd) not found under /usr/share/OVMF or /usr/share/qemu — install the 'ovmf' package"
                .to_string(),
        );
    }

    ctx.kvm_ok = executor
        .check_silent("test -r /dev/kvm && test -w /dev/kvm")
        .await
        .unwrap_or(false);
    if !ctx.kvm_ok {
        tracing::warn!(
            "/dev/kvm not writable — falling back to TCG software emulation (slow); this is a WARN, not a failure"
        );
    }

    // Placeholders must never reach an install.
    let config_text = tokio::fs::read_to_string(&opts.config)
        .await
        .map_err(|e| format!("--config {} not found: {e}", opts.config.display()))?;
    if config_has_placeholder(&config_text) {
        return Err(format!(
            "--config {} still contains REPLACE_AT_PLACE_TIME placeholders — never install with unsubstituted secrets",
            opts.config.display()
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------
// Stage 1: workspace
// ---------------------------------------------------------------------

async fn stage1_workspace(
    executor: &mut dyn CommandExecutor,
    opts: &VmValidateOptions,
    ctx: &mut Ctx,
) -> std::result::Result<(), String> {
    let log = log_path_str(&opts.workdir, LOG_01_WORKSPACE);

    let disk_img = format!("{}/disk.qcow2", shq(&opts.workdir.display().to_string()));
    let create_cmd = format!(
        "qemu-img create -f qcow2 {disk_img} {size} >>{log} 2>&1",
        size = shq(&opts.disk_size)
    );
    executor
        .execute(&create_cmd)
        .await
        .map_err(|e| format!("qemu-img create failed — see {log}: {e}"))?;
    ctx.disk_img = disk_img;

    if let Some(vars_src) = ctx.ovmf_vars.clone() {
        let cp_cmd = format!(
            "cp {} {}/OVMF_VARS.fd",
            shq(&vars_src),
            shq(&opts.workdir.display().to_string())
        );
        executor
            .execute(&cp_cmd)
            .await
            .map_err(|e| format!("failed to copy OVMF_VARS — see {log}: {e}"))?;
    }

    let sock = format!("{}/swtpm.sock", shq(&opts.workdir.display().to_string()));
    let pidfile = format!("{}/swtpm.pid", shq(&opts.workdir.display().to_string()));
    let tpmstate = format!("{}/tpmstate", shq(&opts.workdir.display().to_string()));
    let launch_cmd = format!(
        "mkdir -p {tpmstate} && swtpm socket --tpmstate dir={tpmstate} --ctrl type=unixio,path={sock} --tpm2 --daemon --pid file={pidfile} >>{log} 2>&1; i=0; while [ ! -f {pidfile} ] && [ $i -lt 20 ]; do sleep 1; i=$((i+1)); done; cat {pidfile} 2>/dev/null"
    );
    let pid_out = executor
        .execute_with_output(&launch_cmd)
        .await
        .map_err(|_| format!("swtpm did not write a pid file at {pidfile}"))?;
    let pid = pid_out.trim().to_string();
    if pid.is_empty() {
        return Err(format!("swtpm pid file at {pidfile} was empty"));
    }
    // Recorded immediately — before the socket wait below — so `cleanup` can
    // always kill our own swtpm daemon even if that wait times out.
    ctx.pids.push(pid);

    let sock_wait_cmd = format!(
        "i=0; while [ ! -S {sock} ] && [ $i -lt 20 ]; do sleep 1; i=$((i+1)); done; test -S {sock}"
    );
    let sock_ok = executor
        .check_silent(&sock_wait_cmd)
        .await
        .map_err(|e| e.to_string())?;
    if !sock_ok {
        return Err(format!("swtpm socket never appeared at {sock}"));
    }
    ctx.swtpm_sock = sock;

    Ok(())
}

// ---------------------------------------------------------------------
// Stage 2: boot-iso
// ---------------------------------------------------------------------

async fn stage2_boot_iso(
    executor: &mut dyn CommandExecutor,
    opts: &VmValidateOptions,
    ctx: &Ctx,
) -> std::result::Result<String, String> {
    let log = log_path_str(&opts.workdir, LOG_02_BOOT_ISO);
    let serial_log = log_path_str(&opts.workdir, "02-boot-iso-serial.log");
    let firmware = firmware_args(&ctx.ovmf_code, ctx.ovmf_vars.as_deref(), &opts.workdir);
    let kvm = if ctx.kvm_ok { " -enable-kvm -cpu host" } else { "" };

    let launch_cmd = format!(
        "qemu-system-x86_64 -m 4096 -smp 2{firmware} -drive file={disk},if=virtio,format=qcow2 -cdrom {iso} -boot order=dc -chardev socket,id=chrtpm,path={sock} -tpmdev emulator,id=tpm0,chardev=chrtpm -device tpm-tis,tpmdev=tpm0 -netdev user,id=n0,hostfwd=tcp::{port}-:22 -device virtio-net-pci,netdev=n0 -serial file:{serial_log} -display none -no-reboot{kvm} >>{log} 2>&1 & echo $!",
        disk = ctx.disk_img,
        iso = shq(&opts.iso.display().to_string()),
        sock = ctx.swtpm_sock,
        port = opts.ssh_port,
    );
    let pid_out = executor
        .execute_with_output(&launch_cmd)
        .await
        .map_err(|e| format!("failed to launch qemu (iso boot): {e}"))?;
    let pid = pid_out.trim().to_string();
    if pid.is_empty() {
        return Err("qemu (iso boot) did not report a pid".to_string());
    }

    let wait_cmd = wait_for_ssh_cmd(opts, ctx.have_sshpass, SSH_USER, opts.boot_timeout);
    let ok = executor
        .check_silent(&wait_cmd)
        .await
        .map_err(|e| e.to_string())?;
    if !ok {
        return Err(format!(
            "SSH to the live installer session did not come up within {}s (see {log} and {serial_log})",
            opts.boot_timeout
        ));
    }
    let alive = executor
        .check_silent(&format!("kill -0 {pid} 2>/dev/null"))
        .await
        .unwrap_or(false);
    if !alive {
        return Err(format!("qemu (iso boot) exited unexpectedly — see {log}"));
    }

    Ok(pid)
}

// ---------------------------------------------------------------------
// Stage 3: interrogate (report-only — never fails the gate)
// ---------------------------------------------------------------------

async fn stage3_interrogate(executor: &mut dyn CommandExecutor, opts: &VmValidateOptions, ctx: &mut Ctx) {
    let _log = log_path_str(&opts.workdir, LOG_03_INTERROGATE);

    let units_cmd = ssh_cmd(
        opts,
        ctx.have_sshpass,
        SSH_USER,
        30,
        "systemctl list-units --all --no-legend '*subiquity*' 2>/dev/null; systemctl list-unit-files --no-legend '*subiquity*' 2>/dev/null",
    );
    let units_raw = executor
        .execute_with_output(&units_cmd)
        .await
        .unwrap_or_default();

    let mut units: Vec<&str> = units_raw
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    units.sort_unstable();
    units.dedup();
    let observed_units = if units.is_empty() {
        "NONE".to_string()
    } else {
        units.join(" ")
    };
    ctx.marker72_verdict = Some(evaluate_marker72(&observed_units));
    ctx.observed_units = Some(observed_units);

    for tool in INTERROGATE_TOOLS {
        let cmd = ssh_cmd(
            opts,
            ctx.have_sshpass,
            SSH_USER,
            15,
            &format!("command -v {tool} >/dev/null 2>&1"),
        );
        let present = executor.check_silent(&cmd).await.unwrap_or(false);
        ctx.tool_status.insert(
            tool.to_string(),
            if present {
                ToolStatus::Present
            } else {
                ToolStatus::Missing
            },
        );
    }
    // Stage-3 findings are report-only: a MISSING tool here does not fail
    // the gate — stage 4 will fail on it (if it actually blocks the
    // install), and the stage-7 report explains why.
}

// ---------------------------------------------------------------------
// Stage 4: install
// ---------------------------------------------------------------------

async fn stage4_install(
    executor: &mut dyn CommandExecutor,
    opts: &VmValidateOptions,
    ctx: &Ctx,
) -> std::result::Result<(), String> {
    let log = log_path_str(&opts.workdir, LOG_04_INSTALL);

    let scp_agent = scp_cmd(
        opts,
        ctx.have_sshpass,
        SSH_USER,
        &opts.agent.display().to_string(),
        "/tmp/uaa",
    );
    executor
        .execute(&scp_agent)
        .await
        .map_err(|e| format!("scp of agent binary failed — see {log}: {e}"))?;

    let chmod_cmd = ssh_cmd(opts, ctx.have_sshpass, SSH_USER, 15, "chmod +x /tmp/uaa");
    executor
        .execute(&chmod_cmd)
        .await
        .map_err(|e| format!("chmod +x /tmp/uaa over ssh failed: {e}"))?;

    let scp_config = scp_cmd(
        opts,
        ctx.have_sshpass,
        SSH_USER,
        &opts.config.display().to_string(),
        "/tmp/vm-test.yaml",
    );
    executor
        .execute(&scp_config)
        .await
        .map_err(|e| format!("scp of --config failed — see {log}: {e}"))?;

    // Deliberately NOT passing --hold-on-failure/--pause-after-storage: with
    // both false, install_command -> local_install_command routes through
    // perform_installation_with_options_and_pause, which itself
    // short-circuits straight to perform_installation()
    // (src/network/ssh_installer/installer.rs) — the variant whose
    // run_phase! macro actually logs "Phase completed: <label>" per phase
    // and the final "Installation completed successfully" line the
    // assertion below greps for. Passing either flag would route through
    // the other (silent-on-success) macro and break these assertions.
    let install_cmd = ssh_cmd(
        opts,
        ctx.have_sshpass,
        SSH_USER,
        opts.install_timeout,
        "sudo /tmp/uaa install --config /tmp/vm-test.yaml",
    );
    let (exit_code, stdout, stderr) = executor
        .execute_with_error_collection(&install_cmd, "uaa install --config")
        .await
        .map_err(|e| format!("uaa install invocation failed: {e}"))?;
    let combined_log = format!("{stdout}{stderr}");

    if exit_code == 124 {
        return Err(format!(
            "uaa install timed out after {}s (never a skip — this is a FAIL) — see {log}",
            opts.install_timeout
        ));
    }
    if exit_code != 0 {
        return Err(format!("uaa install exited nonzero ({exit_code}) — see {log}"));
    }

    // 7 phases total: Phase 0..Phase 6 ("Phase 6: Final setup" is the last).
    evaluate_install_log(&combined_log).map_err(|e| format!("{e} — see {log}"))?;

    Ok(())
}

// ---------------------------------------------------------------------
// Stage 5: boot-disk (reboot from the installed disk; same swtpm state)
// ---------------------------------------------------------------------

async fn extract_luks_key(config_path: &Path) -> Option<String> {
    let text = tokio::fs::read_to_string(config_path).await.ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("luks_key:") {
            let val = rest.trim().trim_matches('"');
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

async fn stage5_boot_disk(
    executor: &mut dyn CommandExecutor,
    opts: &VmValidateOptions,
    ctx: &Ctx,
) -> std::result::Result<String, String> {
    let log = log_path_str(&opts.workdir, LOG_05_BOOT_DISK);
    let serial_log = log_path_str(&opts.workdir, "05-boot-disk-serial.log");

    let poweroff_cmd = ssh_cmd(opts, ctx.have_sshpass, SSH_USER, 20, "sudo poweroff");
    let _ = executor.execute(&poweroff_cmd).await; // connection drop is expected

    let wait_exit_cmd = format!(
        "waited=0; while kill -0 {pid} 2>/dev/null; do sleep 2; waited=$((waited+2)); if [ $waited -ge {timeout} ]; then kill {pid} 2>/dev/null; sleep 2; kill -0 {pid} 2>/dev/null && kill -9 {pid} 2>/dev/null; break; fi; done; true",
        pid = ctx.iso_qemu_pid,
        timeout = opts.boot_timeout,
    );
    let _ = executor.execute(&wait_exit_cmd).await;

    // Best-effort branch: with socat, wire the serial console to a unix
    // socket and watch for a LUKS passphrase prompt, sending the throwaway
    // `luks_key` from --config if one appears. Without socat this is
    // skipped and documented as a known limitation (see
    // docs/vm-validation.md); a genuine hang here is caught by the
    // boot-timeout FAIL below regardless. Backgrounded (trailing `&`) so it
    // never blocks the SSH wait that follows.
    let serial_sock = format!("{}/serial-disk.sock", shq(&opts.workdir.display().to_string()));
    let serial_args = if ctx.have_socat {
        format!(" -chardev socket,id=serial0,path={serial_sock},server=on,wait=off -serial chardev:serial0")
    } else {
        format!(" -serial file:{serial_log}")
    };
    let firmware = firmware_args(&ctx.ovmf_code, ctx.ovmf_vars.as_deref(), &opts.workdir);
    let kvm = if ctx.kvm_ok { " -enable-kvm -cpu host" } else { "" };
    let launch_cmd = format!(
        "qemu-system-x86_64 -m 4096 -smp 2{firmware} -drive file={disk},if=virtio,format=qcow2 -boot order=c -chardev socket,id=chrtpm,path={sock} -tpmdev emulator,id=tpm0,chardev=chrtpm -device tpm-tis,tpmdev=tpm0 -netdev user,id=n0,hostfwd=tcp::{port}-:22 -device virtio-net-pci,netdev=n0{serial} -display none -no-reboot{kvm} >>{log} 2>&1 & echo $!",
        disk = ctx.disk_img,
        sock = ctx.swtpm_sock,
        port = opts.ssh_port,
        serial = serial_args,
    );
    let pid_out = executor
        .execute_with_output(&launch_cmd)
        .await
        .map_err(|e| format!("failed to launch qemu (disk boot): {e}"))?;
    let pid = pid_out.trim().to_string();
    if pid.is_empty() {
        return Err("qemu (disk boot) did not report a pid".to_string());
    }

    if ctx.have_socat {
        if let Some(luks_key) = extract_luks_key(&opts.config).await {
            let tries = opts.boot_timeout / 2 + 1;
            let inject_cmd = format!(
                "(i=0; while [ $i -lt {tries} ]; do if grep -qiE 'enter passphrase|please unlock disk' {serial_log} 2>/dev/null; then printf '%s\\n' '{luks_key}' | socat -u - UNIX-CONNECT:{serial_sock} 2>/dev/null; break; fi; sleep 2; i=$((i+1)); done) >/dev/null 2>&1 &"
            );
            let _ = executor.execute(&inject_cmd).await;
        }
    }

    let wait_cmd = wait_for_ssh_cmd(opts, false, "root", opts.boot_timeout);
    let ok = executor
        .check_silent(&wait_cmd)
        .await
        .map_err(|e| e.to_string())?;
    if !ok {
        return Err(format!(
            "SSH to the installed disk (root) did not come up within {}s (see {log} and {serial_log} — check for a stuck LUKS passphrase prompt)",
            opts.boot_timeout
        ));
    }
    let alive = executor
        .check_silent(&format!("kill -0 {pid} 2>/dev/null"))
        .await
        .unwrap_or(false);
    if !alive {
        return Err(format!("qemu (disk boot) exited unexpectedly — see {log}"));
    }

    Ok(pid)
}

// ---------------------------------------------------------------------
// Stage 6: assert (LUKS unlock + rpool/bpool import + multi-user)
// ---------------------------------------------------------------------

async fn stage6_assert(
    executor: &mut dyn CommandExecutor,
    opts: &VmValidateOptions,
) -> std::result::Result<(), String> {
    let _log = log_path_str(&opts.workdir, LOG_06_ASSERT);

    let crypt_cmd = ssh_cmd(opts, false, "root", 30, "cryptsetup status luks");
    let crypt_out = executor
        .execute_with_output(&crypt_cmd)
        .await
        .unwrap_or_default();

    let zpool_cmd = ssh_cmd(opts, false, "root", 30, "zpool list -H -o name");
    let zpool_out = executor
        .execute_with_output(&zpool_cmd)
        .await
        .unwrap_or_default();

    let mu_cmd = ssh_cmd(opts, false, "root", 30, "systemctl is-system-running --wait");
    let mu_out = executor.execute_with_output(&mu_cmd).await.unwrap_or_default();

    let fallback = if mu_out.contains("running") || mu_out.contains("degraded") {
        None
    } else {
        let alt_cmd = ssh_cmd(opts, false, "root", 15, "systemctl is-active multi-user.target");
        let alt = executor.execute_with_output(&alt_cmd).await.unwrap_or_default();
        Some(alt.trim().to_string())
    };

    evaluate_stage6(&crypt_out, &zpool_out, &mu_out, fallback.as_deref()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------
// Top-level orchestrator: stages 0-7.
// ---------------------------------------------------------------------

/// Run the full 8-stage QEMU+swtpm validation harness. Every external
/// process goes through `executor` (mockable — no real VM in unit tests).
/// A failing stage renders the `==== VERIFY-ON-VM REPORT ====` (with
/// `GATE: FAIL (stage N: <msg>)`) and returns `Err`; a clean run renders
/// `GATE: PASS` and returns `Ok`.
pub async fn vm_validate(
    executor: &mut dyn CommandExecutor,
    opts: &VmValidateOptions,
) -> Result<VerifyOnVmReport> {
    let _ = tokio::fs::create_dir_all(opts.workdir.join("logs")).await;
    let _ = tokio::fs::create_dir_all(opts.workdir.join("tpmstate")).await;

    let mut ctx = Ctx::default();

    if let Err(msg) = stage0_preflight(executor, opts, &mut ctx).await {
        return finish(
            executor,
            &ctx,
            GateResult::Fail {
                first_failing_stage: format!("stage 0: {msg}"),
            },
        )
        .await;
    }

    if let Err(msg) = stage1_workspace(executor, opts, &mut ctx).await {
        return finish(
            executor,
            &ctx,
            GateResult::Fail {
                first_failing_stage: format!("stage 1: {msg}"),
            },
        )
        .await;
    }

    match stage2_boot_iso(executor, opts, &ctx).await {
        Ok(pid) => {
            ctx.iso_qemu_pid = pid.clone();
            ctx.pids.push(pid);
        }
        Err(msg) => {
            return finish(
                executor,
                &ctx,
                GateResult::Fail {
                    first_failing_stage: format!("stage 2: {msg}"),
                },
            )
            .await;
        }
    }

    // Report-only: never fails the gate.
    stage3_interrogate(executor, opts, &mut ctx).await;

    if let Err(msg) = stage4_install(executor, opts, &ctx).await {
        return finish(
            executor,
            &ctx,
            GateResult::Fail {
                first_failing_stage: format!("stage 4: {msg}"),
            },
        )
        .await;
    }

    match stage5_boot_disk(executor, opts, &ctx).await {
        Ok(pid) => ctx.pids.push(pid),
        Err(msg) => {
            return finish(
                executor,
                &ctx,
                GateResult::Fail {
                    first_failing_stage: format!("stage 5: {msg}"),
                },
            )
            .await;
        }
    }

    if let Err(msg) = stage6_assert(executor, opts).await {
        return finish(
            executor,
            &ctx,
            GateResult::Fail {
                first_failing_stage: format!("stage 6: {msg}"),
            },
        )
        .await;
    }

    let _log07 = log_path_str(&opts.workdir, LOG_07_REPORT);
    finish(executor, &ctx, GateResult::Pass).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicI32, Ordering};

    // ── Recording mock executor ───────────────────────────────────────
    //
    // Returns pre-loaded (exit_code, stdout, stderr) triples keyed by exact
    // command string, defaulting to a silent success (0, "", "") for any
    // command not explicitly configured. Records every command it was asked
    // to run, in order, so tests can assert on command sequencing and on
    // fail-closed (zero-command) paths. NEVER launches a real process.
    struct MockExecutor {
        /// Exact-match responses, checked first.
        responses: HashMap<String, (i32, String, String)>,
        /// Substring-match fallback rules (first match wins), checked when no
        /// exact match exists — used for commands whose exact text embeds
        /// runtime-computed paths that are awkward to reproduce byte-for-byte
        /// in a test (e.g. the swtpm/qemu pid-capture launch commands).
        contains: Vec<(String, i32, String, String)>,
        recorded: Vec<String>,
    }

    impl MockExecutor {
        fn new() -> Self {
            Self {
                responses: HashMap::new(),
                contains: Vec::new(),
                recorded: Vec::new(),
            }
        }

        fn with(mut self, cmd: &str, exit_code: i32, stdout: &str) -> Self {
            self.responses
                .insert(cmd.to_string(), (exit_code, stdout.to_string(), String::new()));
            self
        }

        fn with_contains(mut self, needle: &str, exit_code: i32, stdout: &str) -> Self {
            self.contains
                .push((needle.to_string(), exit_code, stdout.to_string(), String::new()));
            self
        }

        fn get(&self, cmd: &str) -> (i32, String, String) {
            if let Some(v) = self.responses.get(cmd) {
                return v.clone();
            }
            for (needle, code, out, err) in &self.contains {
                if cmd.contains(needle.as_str()) {
                    return (*code, out.clone(), err.clone());
                }
            }
            (0, String::new(), String::new())
        }
    }

    #[async_trait]
    impl CommandExecutor for MockExecutor {
        async fn connect(&mut self, _host: &str, _username: &str) -> Result<()> {
            Ok(())
        }

        async fn execute(&mut self, command: &str) -> Result<()> {
            self.recorded.push(command.to_string());
            let (code, out, err) = self.get(command);
            if code == 0 {
                Ok(())
            } else {
                Err(AutoInstallError::ProcessError {
                    command: command.to_string(),
                    exit_code: Some(code),
                    stderr: if err.is_empty() { out } else { err },
                })
            }
        }

        async fn execute_with_output(&mut self, command: &str) -> Result<String> {
            self.recorded.push(command.to_string());
            let (code, out, err) = self.get(command);
            if code == 0 {
                Ok(out)
            } else {
                Err(AutoInstallError::ProcessError {
                    command: command.to_string(),
                    exit_code: Some(code),
                    stderr: if err.is_empty() { out } else { err },
                })
            }
        }

        async fn execute_with_error_collection(
            &mut self,
            command: &str,
            _description: &str,
        ) -> Result<(i32, String, String)> {
            self.recorded.push(command.to_string());
            Ok(self.get(command))
        }

        async fn check_silent(&mut self, command: &str) -> Result<bool> {
            self.recorded.push(command.to_string());
            Ok(self.get(command).0 == 0)
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

    static WORKDIR_SEQ: AtomicI32 = AtomicI32::new(0);

    fn temp_workdir() -> PathBuf {
        let n = WORKDIR_SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "uaa-vm-validate-test-{}-{}-{}",
            std::process::id(),
            n,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn happy_path_opts() -> VmValidateOptions {
        let workdir = temp_workdir();
        // Unique per test (parallel `cargo test` runs would otherwise race
        // on a shared config path).
        let config = workdir.join("vm-test.yaml");
        VmValidateOptions {
            iso: PathBuf::from("/tmp/ssh-ready.iso"),
            agent: PathBuf::from("/tmp/uaa-agent"),
            config,
            workdir,
            disk_size: "40G".to_string(),
            ssh_port: 10022,
            boot_timeout: 600,
            install_timeout: 3600,
        }
    }

    fn write_config(opts: &VmValidateOptions, contents: &str) {
        std::fs::write(&opts.config, contents).unwrap();
    }

    /// Builds a MockExecutor stubbed so a full stage-0..stage-6 run passes,
    /// given `opts`. Individual tests can further `.with(...)` overrides.
    fn happy_path_mock(opts: &VmValidateOptions) -> MockExecutor {
        let install_log = "\
Phase completed: Phase 0\n\
Phase completed: Phase 1\n\
Phase completed: Phase 2\n\
Phase completed: Phase 3\n\
Phase completed: Phase 4\n\
Phase completed: Phase 5\n\
Phase 6: Final setup\n\
Phase completed: Phase 6\n\
Installation completed successfully\n";

        let install_cmd = ssh_cmd(
            opts,
            false,
            SSH_USER,
            opts.install_timeout,
            "sudo /tmp/uaa install --config /tmp/vm-test.yaml",
        );
        // Stage 6 assertions run as root, never via sshpass (root is
        // key-only), so `have_sshpass` is always `false` here regardless of
        // stage 0's live-session sshpass detection.
        let cryptsetup_cmd = ssh_cmd(opts, false, "root", 30, "cryptsetup status luks");
        let zpool_cmd = ssh_cmd(opts, false, "root", 30, "zpool list -H -o name");
        let is_system_running_cmd = ssh_cmd(opts, false, "root", 30, "systemctl is-system-running --wait");

        MockExecutor::new()
            .with("uname -s", 0, "Linux")
            .with("command -v qemu-system-x86_64 >/dev/null 2>&1", 0, "")
            .with("command -v swtpm >/dev/null 2>&1", 0, "")
            .with("command -v qemu-img >/dev/null 2>&1", 0, "")
            .with("command -v ssh >/dev/null 2>&1", 0, "")
            .with("command -v scp >/dev/null 2>&1", 0, "")
            .with("command -v sshpass >/dev/null 2>&1", 1, "")
            .with("command -v socat >/dev/null 2>&1", 1, "")
            .with("test -f /usr/share/OVMF/OVMF_CODE_4M.fd", 0, "")
            .with("test -r /dev/kvm && test -w /dev/kvm", 1, "")
            // swtpm --daemon + pidfile cat, and both qemu `& echo $!` pid
            // captures (iso-boot and disk-boot) — exact command text embeds
            // the temp workdir, so match by substring instead.
            .with_contains("swtpm socket", 0, "12345\n")
            .with_contains("qemu-system-x86_64", 0, "20001\n")
            .with(&install_cmd, 0, install_log)
            .with(&cryptsetup_cmd, 0, "/dev/mapper/luks is active.")
            .with(&zpool_cmd, 0, "rpool\nbpool\n")
            .with(&is_system_running_cmd, 0, "running\n")
    }

    // ── render_report (pure, byte-compatible) ───────────────────────────

    #[test]
    fn test_render_report_pass_format() {
        let mut tool_status = BTreeMap::new();
        for tool in INTERROGATE_TOOLS {
            tool_status.insert(tool.to_string(), ToolStatus::Present);
        }
        let report = VerifyOnVmReport {
            observed_units: Some("subiquity-server.service".to_string()),
            marker72_verdict: Some("COVERED".to_string()),
            tool_status,
            gate: GateResult::Pass,
        };
        let rendered = render_report(&report);
        let lines: Vec<&str> = rendered.lines().collect();

        assert_eq!(lines.first(), Some(&"==== VERIFY-ON-VM REPORT ===="));
        assert_eq!(lines.last(), Some(&"============================="));
        assert!(rendered.contains("GATE: PASS"));

        // One line per tool, in mask/tool order.
        let mut last_idx = None;
        for tool in INTERROGATE_TOOLS {
            let needle = format!("{tool}:");
            let idx = rendered.find(&needle).unwrap_or_else(|| panic!("missing tool line for {tool}"));
            if let Some(prev) = last_idx {
                assert!(idx > prev, "tool {tool} out of order");
            }
            last_idx = Some(idx);
        }
    }

    #[test]
    fn test_render_report_fail_and_unknowns() {
        let report = VerifyOnVmReport {
            observed_units: None,
            marker72_verdict: None,
            tool_status: BTreeMap::new(),
            gate: GateResult::Fail {
                first_failing_stage: "stage 2: SSH did not come up within 600s".to_string(),
            },
        };
        let rendered = render_report(&report);

        assert!(rendered.contains("verdict: UNKNOWN (stage 3 not reached)"));
        assert!(rendered.contains("observed-units: UNKNOWN (stage 3 not reached)"));
        for tool in INTERROGATE_TOOLS {
            let needle = format!("{tool}:");
            assert!(
                rendered
                    .lines()
                    .any(|l| l.trim_start().starts_with(&needle) && l.trim_end().ends_with("UNKNOWN")),
                "tool {tool} line missing UNKNOWN"
            );
        }
        assert!(rendered.contains("GATE: FAIL (stage 2: SSH did not come up within 600s)"));
    }

    // ── shq / command-string quoting ────────────────────────────────────

    #[test]
    fn test_shq_wraps_and_escapes_embedded_quotes() {
        assert_eq!(shq("/tmp/plain"), "'/tmp/plain'");
        // The distinctive part: an embedded single quote must be escaped as
        // '\'' (close quote, escaped literal quote, reopen quote) — a naive
        // wrap-only implementation would leave the embedded quote
        // unescaped and break out of the quoted span.
        assert_eq!(shq("/tmp/a'b"), r#"'/tmp/a'\''b'"#);
    }

    #[test]
    fn test_scp_cmd_quotes_local_path_against_injection() {
        let opts = happy_path_opts();
        let malicious = "/tmp/a b; touch pwned";
        let cmd = scp_cmd(&opts, false, "root", malicious, "/tmp/uaa");

        // The dangerous `; touch pwned` substring must appear ONLY inside
        // the single-quoted span produced by `shq`, never as a bare,
        // shell-interpretable command separator.
        assert!(
            cmd.contains(&shq(malicious)),
            "expected the quoted local path in: {cmd}"
        );
        assert!(
            !cmd.contains("b; touch pwned '"),
            "the semicolon escaped the quoted span: {cmd}"
        );
    }

    // ── pure evaluators ─────────────────────────────────────────────────

    #[test]
    fn test_marker72_covered_and_gap() {
        assert_eq!(evaluate_marker72("subiquity-server.service"), "COVERED");
        assert_eq!(
            evaluate_marker72("weird.service"),
            "GAP (unit weird.service not in mask list)"
        );
        assert_eq!(evaluate_marker72("NONE"), "COVERED");
    }

    #[test]
    fn test_install_log_assertions() {
        let good = "\
Phase completed: 0\nPhase completed: 1\nPhase completed: 2\nPhase completed: 3\n\
Phase completed: 4\nPhase completed: 5\nPhase 6: Final setup\nPhase completed: 6\n\
Installation completed successfully\n";
        assert_eq!(evaluate_install_log(good).unwrap(), 7);

        let short = "\
Phase completed: 0\nPhase completed: 1\nPhase completed: 2\nPhase completed: 3\n\
Phase completed: 4\nPhase completed: 5\nPhase 6: Final setup\n\
Installation completed successfully\n";
        assert!(evaluate_install_log(short).is_err());

        let missing_success = "\
Phase completed: 0\nPhase completed: 1\nPhase completed: 2\nPhase completed: 3\n\
Phase completed: 4\nPhase completed: 5\nPhase 6: Final setup\nPhase completed: 6\n";
        assert!(evaluate_install_log(missing_success).is_err());
    }

    #[test]
    fn test_stage6_assertions() {
        assert!(evaluate_stage6("is active", "rpool\nbpool\n", "running", None).is_ok());
        assert!(evaluate_stage6("is active", "rpool\n", "running", None).is_err());
        assert!(evaluate_stage6("is active", "rpool\nbpool\n", "degraded", None).is_ok());
        assert!(evaluate_stage6("is active", "rpool\nbpool\n", "starting", Some("active")).is_ok());
        assert!(evaluate_stage6("is active", "rpool\nbpool\n", "starting", Some("inactive")).is_err());
        assert!(evaluate_stage6("not active", "rpool\nbpool\n", "running", None).is_err());
    }

    #[test]
    fn test_config_has_placeholder() {
        assert!(config_has_placeholder("luks_key: REPLACE_AT_PLACE_TIME"));
        assert!(!config_has_placeholder("luks_key: real-value"));
    }

    // ── full orchestrator (mocked — no real qemu/swtpm ever launches) ───

    #[tokio::test]
    async fn test_placeholder_config_dies_stage0() {
        let opts = happy_path_opts();
        write_config(&opts, "luks_key: REPLACE_AT_PLACE_TIME\n");
        let mut mock = happy_path_mock(&opts);

        let result = vm_validate(&mut mock, &opts).await;
        assert!(result.is_err());

        // Presence CHECKS (`command -v qemu-img ...`) are expected here —
        // only actual invocations of the VM tooling are forbidden.
        assert!(!mock.recorded.iter().any(|c| c.contains("qemu-img create")));
        assert!(!mock.recorded.iter().any(|c| c.contains("swtpm socket --tpmstate")));
        assert!(!mock.recorded.iter().any(|c| c.contains("qemu-system-x86_64 -m")));
    }

    #[tokio::test]
    async fn test_no_pkill_ever() {
        let opts = happy_path_opts();
        write_config(&opts, "luks_key: throwaway\n");
        let mut mock = happy_path_mock(&opts);

        let result = vm_validate(&mut mock, &opts).await;
        assert!(result.is_ok(), "expected a clean pass: {:?}", result.err());

        assert!(!mock.recorded.iter().any(|c| c.contains("pkill")));
        // Every `kill` reference must be to a pid this run actually
        // recorded — spot check the swtpm/qemu pids are the ones killed.
        for cmd in &mock.recorded {
            if let Some(rest) = cmd.strip_prefix("kill -0 ") {
                let pid = rest.split_whitespace().next().unwrap_or("");
                assert!(
                    pid == "12345" || pid == "20001",
                    "unexpected pid referenced in cleanup: {cmd}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_full_pass_command_sequence() {
        let opts = happy_path_opts();
        write_config(&opts, "luks_key: throwaway\n");
        let mut mock = happy_path_mock(&opts);

        let result = vm_validate(&mut mock, &opts).await;
        assert!(result.is_ok(), "expected a clean pass: {:?}", result.err());
        let report = result.unwrap();
        assert_eq!(report.gate, GateResult::Pass);

        // The guard stack does not over-suppress a clean pass: every stage's
        // signature command was actually issued, in order.
        let find_after = |from: usize, needle: &str| -> usize {
            mock.recorded
                .iter()
                .enumerate()
                .skip(from)
                .find(|(_, c)| c.contains(needle))
                .map(|(i, _)| i)
                .unwrap_or_else(|| panic!("command containing {needle:?} not found after index {from}"))
        };

        let i0 = find_after(0, "qemu-img create -f qcow2");
        let i1 = find_after(i0, "swtpm socket");
        let i2 = find_after(i1, "tpm-tis");
        assert!(
            mock.recorded[i2].contains("hostfwd=tcp::10022-:22"),
            "qemu launch missing hostfwd: {}",
            mock.recorded[i2]
        );
        let i3 = find_after(i2, "scp");
        let i4 = find_after(i3, "sudo /tmp/uaa install --config /tmp/vm-test.yaml");
        let i5 = find_after(i4, "cryptsetup status luks");
        let i6 = find_after(i5, "zpool list -H -o name");
        let _i7 = find_after(i6, "systemctl is-system-running --wait");
    }

    #[tokio::test]
    async fn test_stage2_ssh_failure_reports_correct_gate() {
        let opts = happy_path_opts();
        write_config(&opts, "luks_key: throwaway\n");
        let mut mock = happy_path_mock(&opts);
        // Force the stage-2 SSH wait loop to fail (have_sshpass is false in
        // happy_path_opts — stage 0's preflight stubs sshpass as absent).
        let wait_cmd = wait_for_ssh_cmd(&opts, false, SSH_USER, opts.boot_timeout);
        mock.responses.insert(wait_cmd, (1, String::new(), String::new()));

        let result = vm_validate(&mut mock, &opts).await;
        match result {
            Err(AutoInstallError::VmError(msg)) => {
                assert!(msg.starts_with("stage 2:"), "got: {msg}");
            }
            other => panic!("expected stage-2 VmError, got {other:?}"),
        }
    }
}
