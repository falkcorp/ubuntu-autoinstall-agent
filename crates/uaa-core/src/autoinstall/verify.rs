// file: crates/uaa-core/src/autoinstall/verify.rs
// version: 1.2.2
// guid: c2d3e4f5-a6b7-8c9d-0e1f-2a3b4c5d6e7f
// last-edited: 2026-07-10

//! Post-install verification for Lenovo fleet hosts.
//!
//! Checks are split into two layers:
//!
//! 1. **Pure evaluators** (`evaluate_*`) — synchronous, take raw command output
//!    strings, return a [`CheckResult`]. These are the testable core: pass in the
//!    exact strings from our live probe and assert the result.
//!
//! 2. **Async orchestrator** ([`verify_host`]) — issues commands over SSH (or
//!    locally) via a [`crate::network::executor::CommandExecutor`] and calls each
//!    evaluator with the result.
//!
//! # Example
//! ```no_run
//! use uaa_core::autoinstall::{HostSpec, verify::verify_host};
//! use uaa_core::network::SshClient;
//! # async fn run() -> uaa_core::Result<()> {
//! let mut client = SshClient::new();
//! client.connect("172.16.3.96", "jdfalk").await?;
//! let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");
//! let report = verify_host(&mut client, &spec, "172.16.3.96").await?;
//! report.print();
//! # Ok(())
//! # }
//! ```

use crate::{
    autoinstall::host_spec::HostSpec,
    network::executor::CommandExecutor,
    Result,
};

// ── Fleet-wide constants used only for verification ──────────────────────────

/// The LUKS partition on all Lenovo NVMe hosts.
///
/// ZFS-on-LUKS layout (Path B): p1 ESP, p2 RESET, p3 bpool, **p4 LUKS** (holds
/// rpool). The old LVM layout put LUKS on p3 — retargeted to p4 here.
const LUKS_PARTITION: &str = "/dev/nvme0n1p4";

/// The NIC used on all Lenovo fleet hosts.
const LENSERV_NIC: &str = "enp1s0f0";

/// Tang servers that must all appear in the clevis SSS binding.
const TANG_URLS: &[&str] = &[
    "http://172.16.2.45",
    "http://172.16.2.46",
    "http://172.16.2.47",
];

/// The SSS JSON key that encodes the threshold (t=2).
const CLEVIS_THRESHOLD_STR: &str = "\"t\":2";

// ── Result types ──────────────────────────────────────────────────────────────

/// One named check with pass/fail and a human-readable detail string.
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

impl CheckResult {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self { name, passed: true, detail: detail.into() }
    }

    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self { name, passed: false, detail: detail.into() }
    }
}

/// All check results for one host.
#[derive(Debug)]
pub struct VerifyReport {
    /// SSH host or IP that was checked.
    pub host: String,
    pub checks: Vec<CheckResult>,
}

impl VerifyReport {
    pub fn all_passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }

    /// Print a human-readable table to stdout.
    pub fn print(&self) {
        println!("\n=== Verification report for {} ===", self.host);
        for c in &self.checks {
            let mark = if c.passed { "PASS" } else { "FAIL" };
            println!("  [{mark}] {}: {}", c.name, c.detail);
        }
        if self.all_passed() {
            println!("\nAll checks passed.");
        } else {
            let n = self.checks.iter().filter(|c| !c.passed).count();
            println!("\n{n} check(s) FAILED.");
        }
    }
}

// ── Pure evaluators ───────────────────────────────────────────────────────────

/// LUKS partition exists on the expected device.
///
/// Expects output of `lsblk -o NAME,TYPE,FSTYPE`.
pub fn evaluate_luks_partition(lsblk_output: &str) -> CheckResult {
    if lsblk_output.contains("crypto_LUKS") {
        CheckResult::pass("luks_partition", format!("{LUKS_PARTITION} is LUKS"))
    } else {
        CheckResult::fail(
            "luks_partition",
            format!("no crypto_LUKS device found — lsblk output: {lsblk_output:?}"),
        )
    }
}

/// Both ZFS pools (`rpool` + `bpool`) are imported.
///
/// Expects output of `zpool list -H -o name`.
pub fn evaluate_zfs_pools(zpool_output: &str) -> CheckResult {
    let has_rpool = zpool_output.lines().any(|l| l.trim() == "rpool");
    let has_bpool = zpool_output.lines().any(|l| l.trim() == "bpool");
    if has_rpool && has_bpool {
        CheckResult::pass("zfs_pools", "rpool + bpool imported")
    } else {
        let mut missing = vec![];
        if !has_rpool { missing.push("rpool"); }
        if !has_bpool { missing.push("bpool"); }
        CheckResult::fail("zfs_pools", format!("missing pool(s): {}", missing.join(", ")))
    }
}

/// Root filesystem is a ZFS dataset on `rpool/ROOT/…` (not LVM/ext4).
///
/// Expects output of `findmnt -n -o FSTYPE,SOURCE /`.
pub fn evaluate_zfs_root(findmnt_root: &str) -> CheckResult {
    let is_zfs = findmnt_root.contains("zfs");
    let on_rpool = findmnt_root.contains("rpool/ROOT/");
    if is_zfs && on_rpool {
        CheckResult::pass("zfs_root", "/ is a ZFS dataset on rpool/ROOT")
    } else {
        CheckResult::fail(
            "zfs_root",
            format!("/ is not ZFS-on-rpool — findmnt: {findmnt_root:?}"),
        )
    }
}

/// `/boot` is a ZFS dataset on `bpool/BOOT/…`.
///
/// Expects output of `findmnt -n -o FSTYPE,SOURCE /boot`.
pub fn evaluate_boot_on_bpool(findmnt_boot: &str) -> CheckResult {
    let is_zfs = findmnt_boot.contains("zfs");
    let on_bpool = findmnt_boot.contains("bpool/BOOT/");
    if is_zfs && on_bpool {
        CheckResult::pass("boot_on_bpool", "/boot is a ZFS dataset on bpool/BOOT")
    } else {
        CheckResult::fail(
            "boot_on_bpool",
            format!("/boot is not on bpool — findmnt: {findmnt_boot:?}"),
        )
    }
}

/// Regression guard: NO LVM present. The old installer produced LUKS+LVM+ext4;
/// the correct layout is LUKS+ZFS with no LVM anywhere.
///
/// Expects output of `lsblk -o NAME,TYPE,FSTYPE`.
pub fn evaluate_no_lvm(lsblk_output: &str) -> CheckResult {
    let has_lvm = lsblk_output
        .lines()
        .any(|l| l.split_whitespace().any(|f| f == "lvm"));
    if has_lvm {
        CheckResult::fail(
            "no_lvm",
            format!("LVM present — expected ZFS-on-LUKS, not LVM: {lsblk_output:?}"),
        )
    } else {
        CheckResult::pass("no_lvm", "no LVM devices (ZFS-on-LUKS as intended)")
    }
}

/// A TPM2 keyslot is enrolled (via `systemd-cryptenroll --tpm2-with-pin`).
///
/// Expects output of `cryptsetup luksDump <dev>` — systemd-cryptenroll records a
/// `systemd-tpm2` token. NOTE: enrolled on FIRST BOOT, so this only passes after
/// the target has booted at least once.
pub fn evaluate_tpm2_keyslot(luksdump_output: &str) -> CheckResult {
    if luksdump_output.contains("systemd-tpm2") {
        CheckResult::pass("tpm2_keyslot", "systemd-tpm2 keyslot present")
    } else {
        CheckResult::fail(
            "tpm2_keyslot",
            "no systemd-tpm2 token (first-boot enrollment not yet run?)",
        )
    }
}

/// A FIDO2/YubiKey keyslot is enrolled (via `systemd-cryptenroll --fido2-device`).
///
/// Expects output of `cryptsetup luksDump <dev>` — records a `systemd-fido2`
/// token. NOTE: enrolled MANUALLY post-install with the physical key, so this is
/// expected to fail until `register-fido2-luks.sh` has been run.
pub fn evaluate_fido2_keyslot(luksdump_output: &str) -> CheckResult {
    if luksdump_output.contains("systemd-fido2") {
        CheckResult::pass("fido2_keyslot", "systemd-fido2 keyslot present")
    } else {
        CheckResult::fail(
            "fido2_keyslot",
            "no systemd-fido2 token (run register-fido2-luks.sh with the YubiKey)",
        )
    }
}

/// The signed shim bootloader is installed so Secure Boot can be enabled.
///
/// Expects output of `ls -1 /boot/efi/EFI/ubuntu/`. We require `shimx64.efi`
/// (the signed first-stage loader) and `grubx64.efi` (the signed grub it
/// chainloads). Secure Boot may still be OFF in firmware — this only checks the
/// chain is in place so it CAN be turned on.
pub fn evaluate_shim_present(esp_listing: &str) -> CheckResult {
    let has_shim = esp_listing.lines().any(|l| l.trim() == "shimx64.efi");
    let has_grub = esp_listing.lines().any(|l| l.trim() == "grubx64.efi");
    if has_shim && has_grub {
        CheckResult::pass("shim_present", "shimx64.efi + grubx64.efi installed (Secure Boot ready)")
    } else {
        let mut missing = vec![];
        if !has_shim { missing.push("shimx64.efi"); }
        if !has_grub { missing.push("grubx64.efi"); }
        CheckResult::fail("shim_present", format!("missing: {}", missing.join(", ")))
    }
}

/// Clevis SSS Tang binding is present with t=2 and all 3 Tang URLs.
///
/// Expects output of `clevis luks list <dev>`.
pub fn evaluate_clevis_binding(clevis_output: &str) -> CheckResult {
    let has_sss = clevis_output.contains("sss");
    let has_threshold = clevis_output.contains(CLEVIS_THRESHOLD_STR);
    let missing_urls: Vec<&str> = TANG_URLS
        .iter()
        .copied()
        .filter(|url| !clevis_output.contains(url))
        .collect();

    if has_sss && has_threshold && missing_urls.is_empty() {
        CheckResult::pass("clevis_binding", format!("SSS t=2 with {} Tang servers", TANG_URLS.len()))
    } else {
        let mut reasons = vec![];
        if !has_sss { reasons.push("missing 'sss' pin"); }
        if !has_threshold { reasons.push("missing t=2 threshold"); }
        if !missing_urls.is_empty() {
            reasons.push("missing Tang URL(s)");
        }
        CheckResult::fail("clevis_binding", reasons.join("; "))
    }
}

/// `/etc/crypttab` exists and has at least one non-comment line.
///
/// Expects output of `cat /etc/crypttab` (empty string = file missing or empty).
pub fn evaluate_crypttab(crypttab_output: &str) -> CheckResult {
    let has_entry = crypttab_output
        .lines()
        .any(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));

    if has_entry {
        CheckResult::pass("crypttab_present", "crypttab has at least one entry")
    } else {
        CheckResult::fail("crypttab_present", "crypttab missing or empty")
    }
}

/// Dracut clevis.conf loads the `network` dracut module.
///
/// Expects output of `cat /etc/dracut.conf.d/clevis.conf`.
pub fn evaluate_dracut_network(dracut_conf: &str) -> CheckResult {
    if dracut_conf.contains("add_dracutmodules") && dracut_conf.contains("network") {
        CheckResult::pass("dracut_network_module", "add_dracutmodules includes network")
    } else {
        CheckResult::fail(
            "dracut_network_module",
            "clevis.conf missing add_dracutmodules+= \" network \"",
        )
    }
}

/// Dracut clevis.conf sets `kernel_cmdline` for rd.neednet + ip=dhcp.
///
/// Expects output of `cat /etc/dracut.conf.d/clevis.conf`.
pub fn evaluate_dracut_kernel_cmdline(dracut_conf: &str) -> CheckResult {
    let has_neednet = dracut_conf.contains("rd.neednet=1");
    let has_ip_dhcp = dracut_conf.contains("ip=dhcp");
    if has_neednet && has_ip_dhcp {
        CheckResult::pass("dracut_kernel_cmdline", "kernel_cmdline has rd.neednet=1 ip=dhcp")
    } else {
        let mut reasons = vec![];
        if !has_neednet { reasons.push("missing rd.neednet=1"); }
        if !has_ip_dhcp { reasons.push("missing ip=dhcp"); }
        CheckResult::fail("dracut_kernel_cmdline", reasons.join("; "))
    }
}

/// `/etc/default/grub` passes `rd.neednet=1 ip=dhcp` in `GRUB_CMDLINE_LINUX`.
///
/// Expects output of `cat /etc/default/grub`.
pub fn evaluate_grub_cmdline(grub_content: &str) -> CheckResult {
    let in_grub_line = grub_content
        .lines()
        .filter(|l| l.trim_start().starts_with("GRUB_CMDLINE_LINUX="))
        .any(|l| l.contains("rd.neednet=1") && l.contains("ip=dhcp"));

    if in_grub_line {
        CheckResult::pass("grub_cmdline", "GRUB_CMDLINE_LINUX has rd.neednet=1 ip=dhcp")
    } else {
        CheckResult::fail(
            "grub_cmdline",
            "GRUB_CMDLINE_LINUX missing rd.neednet=1 and/or ip=dhcp",
        )
    }
}

/// The running kernel was booted with `rd.neednet=1 ip=dhcp`.
///
/// Expects contents of `/proc/cmdline`.
pub fn evaluate_running_cmdline(proc_cmdline: &str) -> CheckResult {
    let has_neednet = proc_cmdline.contains("rd.neednet=1");
    let has_ip_dhcp = proc_cmdline.contains("ip=dhcp");
    if has_neednet && has_ip_dhcp {
        CheckResult::pass("running_cmdline", "boot cmdline has rd.neednet=1 ip=dhcp")
    } else {
        let mut reasons = vec![];
        if !has_neednet { reasons.push("missing rd.neednet=1"); }
        if !has_ip_dhcp { reasons.push("missing ip=dhcp"); }
        CheckResult::fail("running_cmdline", reasons.join("; "))
    }
}

/// Hostname matches the spec.
///
/// Expects trimmed output of `hostname`.
pub fn evaluate_hostname(hostname_output: &str, spec: &HostSpec) -> CheckResult {
    let got = hostname_output.trim();
    if got == spec.hostname {
        CheckResult::pass("hostname_matches", format!("hostname = {got}"))
    } else {
        CheckResult::fail(
            "hostname_matches",
            format!("expected '{}', got '{got}'", spec.hostname),
        )
    }
}

/// The NIC carries the expected IP address.
///
/// Expects output of `ip -br addr show <nic>`.
pub fn evaluate_ip_address(ip_br_output: &str, spec: &HostSpec) -> CheckResult {
    if ip_br_output.contains(&spec.network_address) {
        CheckResult::pass("ip_matches", format!("{} on {LENSERV_NIC}", spec.network_address))
    } else {
        CheckResult::fail(
            "ip_matches",
            format!(
                "expected {} on {LENSERV_NIC}, got: {ip_br_output:?}",
                spec.network_address
            ),
        )
    }
}

/// A systemd service is active.
///
/// Expects trimmed output of `systemctl is-active <svc>`.
pub fn evaluate_service(svc_name: &'static str, is_active_output: &str) -> CheckResult {
    if is_active_output.trim() == "active" {
        CheckResult::pass(svc_name, "active")
    } else {
        CheckResult::fail(svc_name, format!("not active: '{}'", is_active_output.trim()))
    }
}

// ── Async orchestrator ────────────────────────────────────────────────────────

/// SSH into the host described by `spec` and run all verification checks.
///
/// The caller must have already called `runner.connect(host, user).await?`.
pub async fn verify_host(
    runner: &mut dyn CommandExecutor,
    spec: &HostSpec,
    host_label: &str,
) -> Result<VerifyReport> {
    let mut checks = Vec::with_capacity(12);

    // 1. LUKS partition (+ no-LVM regression guard from the same lsblk)
    let lsblk = runner
        .execute_with_output("lsblk -o NAME,TYPE,FSTYPE")
        .await
        .unwrap_or_default();
    checks.push(evaluate_luks_partition(&lsblk));
    checks.push(evaluate_no_lvm(&lsblk));

    // 1b. ZFS layout: rpool+bpool imported, / on rpool/ROOT, /boot on bpool/BOOT
    let zpools = runner
        .execute_with_output("zpool list -H -o name")
        .await
        .unwrap_or_default();
    checks.push(evaluate_zfs_pools(&zpools));
    let findmnt_root = runner
        .execute_with_output("findmnt -n -o FSTYPE,SOURCE /")
        .await
        .unwrap_or_default();
    checks.push(evaluate_zfs_root(&findmnt_root));
    let findmnt_boot = runner
        .execute_with_output("findmnt -n -o FSTYPE,SOURCE /boot")
        .await
        .unwrap_or_default();
    checks.push(evaluate_boot_on_bpool(&findmnt_boot));

    // 1c. Signed shim chain present so Secure Boot can be enabled.
    let esp_listing = runner
        .execute_with_output("ls -1 /boot/efi/EFI/ubuntu/")
        .await
        .unwrap_or_default();
    checks.push(evaluate_shim_present(&esp_listing));

    // 2. Clevis SSS Tang binding
    let clevis = runner
        .execute_with_output(&format!("sudo -n clevis luks list -d {LUKS_PARTITION}"))
        .await
        .unwrap_or_default();
    checks.push(evaluate_clevis_binding(&clevis));

    // 2b. TPM2 (first-boot) + FIDO2 (manual) keyslots from luksDump
    let luksdump = runner
        .execute_with_output(&format!("sudo -n cryptsetup luksDump {LUKS_PARTITION}"))
        .await
        .unwrap_or_default();
    checks.push(evaluate_tpm2_keyslot(&luksdump));
    checks.push(evaluate_fido2_keyslot(&luksdump));

    // 3. crypttab
    let crypttab = runner
        .execute_with_output("cat /etc/crypttab")
        .await
        .unwrap_or_default();
    checks.push(evaluate_crypttab(&crypttab));

    // 4 & 5. Dracut clevis.conf (both checks from the same file)
    let dracut_conf = runner
        .execute_with_output("cat /etc/dracut.conf.d/clevis.conf")
        .await
        .unwrap_or_default();
    checks.push(evaluate_dracut_network(&dracut_conf));
    checks.push(evaluate_dracut_kernel_cmdline(&dracut_conf));

    // 6. GRUB cmdline — check both the main file and the grub.d drop-in written
    //    by the autoinstall late-command (50-clevis-network.cfg).
    let grub = runner
        .execute_with_output("cat /etc/default/grub /etc/default/grub.d/50-clevis-network.cfg 2>/dev/null || cat /etc/default/grub")
        .await
        .unwrap_or_default();
    checks.push(evaluate_grub_cmdline(&grub));

    // 7. Running cmdline
    let proc_cmdline = runner
        .execute_with_output("cat /proc/cmdline")
        .await
        .unwrap_or_default();
    checks.push(evaluate_running_cmdline(&proc_cmdline));

    // 8. Hostname
    let hostname_out = runner
        .execute_with_output("hostname")
        .await
        .unwrap_or_default();
    checks.push(evaluate_hostname(&hostname_out, spec));

    // 9. IP address
    let ip_out = runner
        .execute_with_output(&format!("ip -br addr show {LENSERV_NIC}"))
        .await
        .unwrap_or_default();
    checks.push(evaluate_ip_address(&ip_out, spec));

    // 10–12. Service health
    for svc in &["ssh", "rsyslog", "prometheus-node-exporter"] {
        let out = runner
            .execute_with_output(&format!("systemctl is-active {svc}"))
            .await
            .unwrap_or_else(|_| "unknown".to_string());
        // The service name string must have 'static lifetime; we map the 3 known names.
        let label: &'static str = match *svc {
            "ssh" => "svc_ssh",
            "rsyslog" => "svc_rsyslog",
            "prometheus-node-exporter" => "svc_node_exporter",
            _ => "svc_unknown",
        };
        checks.push(evaluate_service(label, &out));
    }

    Ok(VerifyReport {
        host: host_label.to_string(),
        checks,
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autoinstall::host_spec::HostSpec;
    use crate::network::executor::CommandExecutor;
    use crate::Result;
    use async_trait::async_trait;
    use std::collections::HashMap;

    /// Minimal mock executor: returns pre-loaded output strings keyed by command.
    /// Any command not in the map returns an empty string.
    struct MockExecutor {
        responses: HashMap<String, String>,
    }

    impl MockExecutor {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self {
                responses: pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            }
        }

        fn get(&self, cmd: &str) -> String {
            self.responses.get(cmd).cloned().unwrap_or_default()
        }
    }

    #[async_trait]
    impl CommandExecutor for MockExecutor {
        async fn connect(&mut self, _host: &str, _username: &str) -> Result<()> {
            Ok(())
        }
        async fn execute(&mut self, _command: &str) -> Result<()> {
            Ok(())
        }
        async fn execute_with_output(&mut self, command: &str) -> Result<String> {
            Ok(self.get(command))
        }
        async fn execute_with_error_collection(
            &mut self,
            command: &str,
            _description: &str,
        ) -> Result<(i32, String, String)> {
            Ok((0, self.get(command), String::new()))
        }
        async fn check_silent(&mut self, command: &str) -> Result<bool> {
            Ok(!self.get(command).is_empty())
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

    // ── Live-probe fixture strings (verbatim from len-serv-003) ──────────────

    // ZFS-on-LUKS layout: p1 ESP, p2 RESET, p3 bpool (zfs_member), p4 LUKS →
    // luks mapper (holds rpool). NO LVM anywhere.
    const LSBLK_003: &str = "\
NAME        TYPE  FSTYPE
nvme0n1     disk
nvme0n1p1   part  vfat
nvme0n1p2   part  ext4
nvme0n1p3   part  zfs_member
nvme0n1p4   part  crypto_LUKS
luks        crypt zfs_member";

    const ESP_LISTING_003: &str = "BOOTX64.CSV\ngrub.cfg\ngrubx64.efi\nmmx64.efi\nshimx64.efi\n";
    const ZPOOL_003: &str = "bpool\nrpool\n";
    const FINDMNT_ROOT_003: &str = "zfs   rpool/ROOT/ubuntu_3pvepx\n";
    const FINDMNT_BOOT_003: &str = "zfs   bpool/BOOT/ubuntu_3pvepx\n";
    // luksDump of a fully-provisioned host: clevis (Tang) + systemd-tpm2 (PIN) +
    // systemd-fido2 (YubiKey) tokens all enrolled.
    const LUKSDUMP_003: &str = "\
LUKS header information\nVersion:        2\nTokens:\n  0: clevis\n  1: systemd-tpm2\n  2: systemd-fido2\nKeyslots:\n  0: luks2\n";

    const CLEVIS_003: &str =
        "1: sss '{\"t\":2,\"pins\":{\"tang\":[{\"url\":\"http://172.16.2.45\"},{\"url\":\"http://172.16.2.46\"},{\"url\":\"http://172.16.2.47\"}]}}'";

    const CRYPTTAB_003: &str =
        "dm_crypt-0 UUID=210735c1-4b9d-45ff-a954-8d3648e17e1a none luks\n";

    const DRACUT_CONF_003: &str = "\
add_dracutmodules+=\" network \"\nkernel_cmdline=\"rd.neednet=1 ip=dhcp\"\n";

    const GRUB_003: &str = "\
GRUB_DEFAULT=0\nGRUB_TIMEOUT=5\nGRUB_DISTRIBUTOR=`lsb_release -i -s 2>/dev/null || echo Debian`\nGRUB_CMDLINE_LINUX=\"rd.neednet=1 ip=dhcp\"\n";

    const PROC_CMDLINE_003: &str =
        "BOOT_IMAGE=/vmlinuz-6.8.0-57-generic root=/dev/mapper/ubuntu-lv ro rd.neednet=1 ip=dhcp quiet splash\n";

    // ── Pure evaluator tests ─────────────────────────────────────────────────

    #[test]
    fn luks_partition_passes_on_crypto_luks() {
        assert!(evaluate_luks_partition(LSBLK_003).passed);
    }

    #[test]
    fn luks_partition_fails_when_missing() {
        let out = evaluate_luks_partition("nvme0n1 disk\nnvme0n1p1 part vfat");
        assert!(!out.passed);
    }

    #[test]
    fn clevis_binding_passes_on_live_fixture() {
        assert!(evaluate_clevis_binding(CLEVIS_003).passed);
    }

    #[test]
    fn clevis_binding_fails_without_sss() {
        let r = evaluate_clevis_binding("1: tang '{\"url\":\"http://172.16.2.45\"}'");
        assert!(!r.passed);
        assert!(r.detail.contains("sss"));
    }

    #[test]
    fn clevis_binding_fails_without_threshold() {
        // Missing the t=2 field
        let r = evaluate_clevis_binding(
            "1: sss '{\"t\":1,\"pins\":{\"tang\":[{\"url\":\"http://172.16.2.45\"},{\"url\":\"http://172.16.2.46\"},{\"url\":\"http://172.16.2.47\"}]}}'",
        );
        assert!(!r.passed);
        assert!(r.detail.contains("t=2"));
    }

    #[test]
    fn clevis_binding_fails_on_missing_tang_url() {
        let r = evaluate_clevis_binding(
            "1: sss '{\"t\":2,\"pins\":{\"tang\":[{\"url\":\"http://172.16.2.45\"},{\"url\":\"http://172.16.2.46\"}]}}'",
        );
        assert!(!r.passed);
    }

    #[test]
    fn crypttab_passes_with_entry() {
        assert!(evaluate_crypttab(CRYPTTAB_003).passed);
    }

    #[test]
    fn crypttab_fails_when_empty() {
        assert!(!evaluate_crypttab("# comment only\n").passed);
        assert!(!evaluate_crypttab("").passed);
    }

    #[test]
    fn dracut_network_passes_on_live_fixture() {
        assert!(evaluate_dracut_network(DRACUT_CONF_003).passed);
    }

    #[test]
    fn dracut_network_fails_without_network_module() {
        let r = evaluate_dracut_network("kernel_cmdline=\"rd.neednet=1 ip=dhcp\"\n");
        assert!(!r.passed);
    }

    #[test]
    fn dracut_kernel_cmdline_passes_on_live_fixture() {
        assert!(evaluate_dracut_kernel_cmdline(DRACUT_CONF_003).passed);
    }

    #[test]
    fn dracut_kernel_cmdline_fails_when_incomplete() {
        let r = evaluate_dracut_kernel_cmdline("kernel_cmdline=\"rd.neednet=1\"\n");
        assert!(!r.passed);
        assert!(r.detail.contains("ip=dhcp"));
    }

    #[test]
    fn grub_cmdline_passes_on_live_fixture() {
        assert!(evaluate_grub_cmdline(GRUB_003).passed);
    }

    #[test]
    fn grub_cmdline_fails_when_missing() {
        let r = evaluate_grub_cmdline("GRUB_CMDLINE_LINUX=\"quiet splash\"\n");
        assert!(!r.passed);
    }

    #[test]
    fn running_cmdline_passes_on_live_fixture() {
        assert!(evaluate_running_cmdline(PROC_CMDLINE_003).passed);
    }

    #[test]
    fn running_cmdline_fails_when_missing_params() {
        let r = evaluate_running_cmdline("BOOT_IMAGE=/vmlinuz root=/dev/sda ro quiet\n");
        assert!(!r.passed);
        assert!(r.detail.contains("rd.neednet=1"));
    }

    #[test]
    fn hostname_passes_on_match() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");
        assert!(evaluate_hostname("len-serv-003\n", &spec).passed);
    }

    #[test]
    fn hostname_fails_on_mismatch() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");
        let r = evaluate_hostname("len-serv-001\n", &spec);
        assert!(!r.passed);
        assert!(r.detail.contains("len-serv-001"));
    }

    #[test]
    fn ip_address_passes_when_present() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");
        let out = "enp1s0f0 UP 172.16.3.96/23 fe80::1/64";
        assert!(evaluate_ip_address(out, &spec).passed);
    }

    #[test]
    fn ip_address_fails_when_absent() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");
        let r = evaluate_ip_address("enp1s0f0 UP 172.16.3.92/23", &spec);
        assert!(!r.passed);
    }

    #[test]
    fn service_passes_when_active() {
        assert!(evaluate_service("svc_ssh", "active\n").passed);
    }

    #[test]
    fn service_fails_when_inactive() {
        let r = evaluate_service("svc_ssh", "inactive\n");
        assert!(!r.passed);
        assert!(r.detail.contains("inactive"));
    }

    // ── Integration test: full verify_host over MockExecutor ────────────────

    #[tokio::test]
    async fn verify_host_all_pass_for_len_serv_003() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");

        let mut mock = MockExecutor::new(&[
            ("lsblk -o NAME,TYPE,FSTYPE", LSBLK_003),
            ("zpool list -H -o name", ZPOOL_003),
            ("findmnt -n -o FSTYPE,SOURCE /", FINDMNT_ROOT_003),
            ("findmnt -n -o FSTYPE,SOURCE /boot", FINDMNT_BOOT_003),
            ("ls -1 /boot/efi/EFI/ubuntu/", ESP_LISTING_003),
            ("sudo -n clevis luks list -d /dev/nvme0n1p4", CLEVIS_003),
            ("sudo -n cryptsetup luksDump /dev/nvme0n1p4", LUKSDUMP_003),
            ("cat /etc/crypttab", CRYPTTAB_003),
            ("cat /etc/dracut.conf.d/clevis.conf", DRACUT_CONF_003),
            ("cat /etc/default/grub /etc/default/grub.d/50-clevis-network.cfg 2>/dev/null || cat /etc/default/grub", GRUB_003),
            ("cat /proc/cmdline", PROC_CMDLINE_003),
            ("hostname", "len-serv-003\n"),
            ("ip -br addr show enp1s0f0", "enp1s0f0 UP 172.16.3.96/23 fe80::1/64\n"),
            ("systemctl is-active ssh", "active"),
            ("systemctl is-active rsyslog", "active"),
            ("systemctl is-active prometheus-node-exporter", "active"),
        ]);

        let report = verify_host(&mut mock, &spec, "len-serv-003").await.unwrap();
        assert_eq!(report.checks.len(), 19);
        for c in &report.checks {
            assert!(c.passed, "check '{}' failed: {}", c.name, c.detail);
        }
        assert!(report.all_passed());
    }

    #[tokio::test]
    async fn verify_host_fails_on_wrong_hostname() {
        let spec = HostSpec::for_lenserv("len-serv-001", "172.16.3.92/23");

        let mut mock = MockExecutor::new(&[
            ("lsblk -o NAME,TYPE,FSTYPE", LSBLK_003),
            ("zpool list -H -o name", ZPOOL_003),
            ("findmnt -n -o FSTYPE,SOURCE /", FINDMNT_ROOT_003),
            ("findmnt -n -o FSTYPE,SOURCE /boot", FINDMNT_BOOT_003),
            ("ls -1 /boot/efi/EFI/ubuntu/", ESP_LISTING_003),
            ("sudo -n clevis luks list -d /dev/nvme0n1p4", CLEVIS_003),
            ("sudo -n cryptsetup luksDump /dev/nvme0n1p4", LUKSDUMP_003),
            ("cat /etc/crypttab", CRYPTTAB_003),
            ("cat /etc/dracut.conf.d/clevis.conf", DRACUT_CONF_003),
            ("cat /etc/default/grub /etc/default/grub.d/50-clevis-network.cfg 2>/dev/null || cat /etc/default/grub", GRUB_003),
            ("cat /proc/cmdline", PROC_CMDLINE_003),
            // hostname mismatch
            ("hostname", "len-serv-003\n"),
            ("ip -br addr show enp1s0f0", "enp1s0f0 UP 172.16.3.92/23 fe80::1/64\n"),
            ("systemctl is-active ssh", "active"),
            ("systemctl is-active rsyslog", "active"),
            ("systemctl is-active prometheus-node-exporter", "active"),
        ]);

        let report = verify_host(&mut mock, &spec, "172.16.3.92").await.unwrap();
        assert!(!report.all_passed());
        let hn_check = report.checks.iter().find(|c| c.name == "hostname_matches").unwrap();
        assert!(!hn_check.passed);
    }

    #[tokio::test]
    async fn verify_host_fails_on_missing_luks() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");

        let mut mock = MockExecutor::new(&[
            // No crypto_LUKS in output
            ("lsblk -o NAME,TYPE,FSTYPE", "nvme0n1 disk\nnvme0n1p1 part vfat\n"),
            ("zpool list -H -o name", ZPOOL_003),
            ("findmnt -n -o FSTYPE,SOURCE /", FINDMNT_ROOT_003),
            ("findmnt -n -o FSTYPE,SOURCE /boot", FINDMNT_BOOT_003),
            ("ls -1 /boot/efi/EFI/ubuntu/", ESP_LISTING_003),
            ("sudo -n clevis luks list -d /dev/nvme0n1p4", CLEVIS_003),
            ("sudo -n cryptsetup luksDump /dev/nvme0n1p4", LUKSDUMP_003),
            ("cat /etc/crypttab", CRYPTTAB_003),
            ("cat /etc/dracut.conf.d/clevis.conf", DRACUT_CONF_003),
            ("cat /etc/default/grub /etc/default/grub.d/50-clevis-network.cfg 2>/dev/null || cat /etc/default/grub", GRUB_003),
            ("cat /proc/cmdline", PROC_CMDLINE_003),
            ("hostname", "len-serv-003\n"),
            ("ip -br addr show enp1s0f0", "enp1s0f0 UP 172.16.3.96/23\n"),
            ("systemctl is-active ssh", "active"),
            ("systemctl is-active rsyslog", "active"),
            ("systemctl is-active prometheus-node-exporter", "active"),
        ]);

        let report = verify_host(&mut mock, &spec, "len-serv-003").await.unwrap();
        assert!(!report.all_passed());
        let luks_check = report.checks.iter().find(|c| c.name == "luks_partition").unwrap();
        assert!(!luks_check.passed);
    }

    // ── New ZFS / multikey evaluator tests ──────────────────────────────────

    #[test]
    fn zfs_pools_passes_with_both() {
        assert!(evaluate_zfs_pools(ZPOOL_003).passed);
    }

    #[test]
    fn zfs_pools_fails_missing_bpool() {
        let r = evaluate_zfs_pools("rpool\n");
        assert!(!r.passed);
        assert!(r.detail.contains("bpool"));
    }

    #[test]
    fn zfs_root_passes_on_rpool_dataset() {
        assert!(evaluate_zfs_root(FINDMNT_ROOT_003).passed);
    }

    #[test]
    fn zfs_root_fails_on_lvm_ext4() {
        // The old broken layout: root on an ext4 LVM LV.
        let r = evaluate_zfs_root("ext4  /dev/mapper/ubuntu--vg-ubuntu--lv");
        assert!(!r.passed);
    }

    #[test]
    fn boot_on_bpool_passes() {
        assert!(evaluate_boot_on_bpool(FINDMNT_BOOT_003).passed);
    }

    #[test]
    fn boot_on_bpool_fails_on_ext4_boot() {
        assert!(!evaluate_boot_on_bpool("ext4  /dev/nvme0n1p2").passed);
    }

    #[test]
    fn no_lvm_passes_on_zfs_fixture() {
        assert!(evaluate_no_lvm(LSBLK_003).passed);
    }

    #[test]
    fn no_lvm_fails_when_lvm_present() {
        // The exact regression we're guarding against (old len-serv-003 output).
        let lvm = "nvme0n1p4 part crypto_LUKS\ndm-0 crypt LVM2_member\nubuntu--lv lvm ext4";
        let r = evaluate_no_lvm(lvm);
        assert!(!r.passed);
        assert!(r.detail.contains("LVM"));
    }

    #[test]
    fn tpm2_keyslot_passes_when_token_present() {
        assert!(evaluate_tpm2_keyslot(LUKSDUMP_003).passed);
    }

    #[test]
    fn tpm2_keyslot_fails_before_first_boot() {
        // Only clevis enrolled — TPM2 first-boot unit hasn't run yet.
        assert!(!evaluate_tpm2_keyslot("Tokens:\n  0: clevis\n").passed);
    }

    #[test]
    fn fido2_keyslot_passes_when_token_present() {
        assert!(evaluate_fido2_keyslot(LUKSDUMP_003).passed);
    }

    #[test]
    fn fido2_keyslot_fails_before_manual_enroll() {
        assert!(!evaluate_fido2_keyslot("Tokens:\n  0: clevis\n  1: systemd-tpm2\n").passed);
    }

    #[test]
    fn shim_present_passes_with_shim_and_grub() {
        assert!(evaluate_shim_present(ESP_LISTING_003).passed);
    }

    #[test]
    fn shim_present_fails_without_shim() {
        // grub installed directly with no shim → Secure Boot can't be enabled.
        let r = evaluate_shim_present("grub.cfg\ngrubx64.efi\n");
        assert!(!r.passed);
        assert!(r.detail.contains("shimx64.efi"));
    }
}
