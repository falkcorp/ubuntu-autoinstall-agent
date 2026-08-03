// file: crates/uaa-core/src/autoinstall/verify.rs
// version: 1.5.0
// guid: c2d3e4f5-a6b7-8c9d-0e1f-2a3b4c5d6e7f
// last-edited: 2026-08-03

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
//! let spec = HostSpec::for_lenserv(
//!     "len-serv-003",
//!     "172.16.3.96/23",
//!     &["172.16.3.92", "172.16.3.94", "172.16.3.96"],
//! );
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
///
/// This is the DEFAULT sourced by `fleet::FleetConfig::luks_partition` — the
/// live value used at runtime (`verify_host` and the pure evaluators below)
/// is read through `crate::fleet::fleet()`, not this const directly.
pub const LUKS_PARTITION: &str = "/dev/nvme0n1p4";

/// The NIC used on all Lenovo fleet hosts.
///
/// DEFAULT sourced by `fleet::FleetConfig::lenserv_nic` — see [`LUKS_PARTITION`].
pub const LENSERV_NIC: &str = "enp1s0f0";

/// Tang servers that must all appear in the clevis SSS binding.
///
/// DEFAULT sourced by `fleet::FleetConfig::tang_urls` — see [`LUKS_PARTITION`].
///
/// This is an INVENTORY of the Tang servers the fleet runs, not a per-host
/// requirement: the settled policy pairs TWO peers per host, so
/// [`evaluate_clevis_binding`] requires every bound Tang URL to be one of ours
/// rather than requiring all of ours to be bound.
pub const TANG_URLS: &[&str] = &[
    "http://172.16.2.45",
    "http://172.16.2.46",
    "http://172.16.2.47",
];

/// The smallest number of shares that may open a volume.
///
/// The fleet security requirement is that **no single share can open the
/// volume, on any path through the policy tree** — see
/// [`min_satisfying_shares`]. This is deliberately NOT a threshold on any one
/// `"t"` field: the settled policy has an outer `t=1`, and thresholds only mean
/// anything relative to the cost of the branches they range over.
const MIN_SATISFYING_SHARES: usize = 2;

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
        let luks_partition = &crate::fleet::fleet().luks_partition;
        CheckResult::pass("luks_partition", format!("{luks_partition} is LUKS"))
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

/// One `slot: pin '<json>'` line from `clevis luks list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClevisBindingLine {
    /// LUKS keyslot number, e.g. `1`.
    pub slot: String,
    /// Top-level clevis pin name, e.g. `sss` or `tang`.
    pub pin: String,
    /// The single-quoted JSON payload, unquoted.
    pub json: String,
}

/// Parse `clevis luks list` output into one entry per bound keyslot.
///
/// Real-world format is one line per slot:
///
/// ```text
/// 1: sss '{"t":2,"pins":{...}}'
/// ```
///
/// The pin name is taken from the parsed token, never from a substring of the
/// whole line — that conflation is what let a vulnerable policy masquerade as a
/// valid one. Lines that do not match the shape are skipped.
pub fn parse_clevis_bindings(clevis_output: &str) -> Vec<ClevisBindingLine> {
    clevis_output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (slot, rest) = line.split_once(':')?;
            if slot.is_empty() || !slot.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            // JSON never contains a single quote, so first/last delimit it.
            let open = rest.find('\'')?;
            let close = rest.rfind('\'')?;
            if close <= open {
                return None;
            }
            let pin = rest[..open].trim();
            if pin.is_empty() {
                return None;
            }
            Some(ClevisBindingLine {
                slot: slot.to_string(),
                pin: pin.to_string(),
                json: rest[open + 1..close].to_string(),
            })
        })
        .collect()
}

/// Number of independent SSS **shares** contributed by a `pins` object.
///
/// This is the arithmetic the old substring-based verifier got wrong. In
/// clevis's `sss` pin, an **array** value contributes one share PER ELEMENT —
/// so `"tang":[a,b,c]` is three shares, not one. A non-array value is one
/// share. An empty `pins` object contributes zero shares (and is therefore
/// unsatisfiable at any `t >= 1`).
pub fn count_shares(pins: &serde_json::Map<String, serde_json::Value>) -> usize {
    pins.values()
        .map(|v| match v {
            serde_json::Value::Array(items) => items.len(),
            _ => 1,
        })
        .sum()
}

/// The **smallest number of leaf factors** that can satisfy a clevis policy
/// tree — i.e. the size of the cheapest satisfying set.
///
/// This is the generalisation of the fleet rule. The security requirement is
/// *no single share can open the volume, on any path through the tree*, which
/// is exactly `min_satisfying_shares(policy) >= 2`. Expressing it as a number
/// rather than as a shape check is what lets an outer `t=1` be legal: an OR is
/// only as strong as its cheapest branch, so an outer `t=1` passes if and only
/// if EVERY branch itself costs 2+.
///
/// Cost model, which follows clevis's own share arithmetic:
///
/// - A leaf pin (`"tpm2":{…}`) is one share, cost 1.
/// - A pin **array** is one share PER ELEMENT, each cost 1 — `"tang":[a,b,c]`
///   is three shares, not one.
/// - A nested `"sss"` policy is ONE share whose cost is its own
///   `min_satisfying_shares` — nesting collapses a group to a single share for
///   the *parent's* threshold, but does NOT degrade the group's own threshold.
/// - A node with threshold `t` costs the sum of its `t` CHEAPEST children.
///
/// Returns `Err` when the tree is malformed or unsatisfiable (`t` of 0, `t`
/// greater than the number of shares, missing `t`/`pins`) — such a policy can
/// never unlock unattended, so it fails closed rather than scoring 0.
pub fn min_satisfying_shares(policy: &serde_json::Value) -> std::result::Result<usize, String> {
    let obj = policy
        .as_object()
        .ok_or_else(|| "sss policy is not a JSON object".to_string())?;
    let t = obj
        .get("t")
        .and_then(|t| t.as_u64())
        .ok_or_else(|| "sss policy has no numeric \"t\" threshold".to_string())? as usize;
    let pins = obj
        .get("pins")
        .and_then(|p| p.as_object())
        .ok_or_else(|| "sss policy has no \"pins\" object".to_string())?;
    if t == 0 {
        return Err("threshold t=0 — a policy that needs no shares is not a policy".to_string());
    }

    // One cost per SHARE, in clevis's own accounting.
    let mut costs: Vec<usize> = Vec::new();
    for (key, value) in pins {
        let entries: Vec<&serde_json::Value> = match value {
            serde_json::Value::Array(items) => items.iter().collect(),
            other => vec![other],
        };
        for entry in entries {
            if key == "sss" {
                costs.push(min_satisfying_shares(entry)?);
            } else {
                costs.push(1);
            }
        }
    }

    if costs.len() < t {
        return Err(format!(
            "unsatisfiable policy: t={t} but pins provide only {} share(s) — \
             this volume can never unlock unattended",
            costs.len()
        ));
    }
    costs.sort_unstable();
    Ok(costs.iter().take(t).sum())
}

/// Collect every Tang `url` appearing anywhere in a clevis policy tree.
fn collect_tang_urls(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                if key == "tang" {
                    let entries: Vec<&serde_json::Value> = match val {
                        serde_json::Value::Array(items) => items.iter().collect(),
                        other => vec![other],
                    };
                    for entry in entries {
                        if let Some(url) = entry.get("url").and_then(|u| u.as_str()) {
                            out.push(url.to_string());
                        }
                    }
                }
                collect_tang_urls(val, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_tang_urls(item, out);
            }
        }
        _ => {}
    }
}

/// Reject the **flattening bug** anywhere in the tree.
///
/// A pin array is one share PER ELEMENT, so a MULTI-element array sitting as a
/// direct sibling of another pin is almost always a group that was meant to be
/// ANDed and got flattened instead:
///
/// ```text
/// {"t":2,"pins":{"tang":[a,b,c],"tpm2":{…}}}   // 2-of-FOUR, tang alone opens it
/// ```
///
/// The author meant `AND(tpm2, tang-group)`; clevis reads 2-of-4. The fix is to
/// nest the group under an inner `sss` so it collapses to ONE share.
///
/// Boundaries this rule deliberately does NOT cross:
///
/// - A multi-element array that is the level's ONLY pin is a plain threshold
///   over one kind (`{"t":2,"pins":{"tang":[a,b,c]}}`) — legal, and the legacy
///   len-serv-001/002 policy.
/// - A ONE-element array beside a sibling (`"tpm2":[{…}]` next to `"sss"`) is a
///   single share — legal, and exactly how the emitter writes the lenserv
///   variant of group 1.
/// - `"sss"` arrays are never flattened: each element is a real group whose cost
///   is computed on its own.
fn lint_flattened_groups(policy: &serde_json::Value, path: &str) -> std::result::Result<(), String> {
    let Some(pins) = policy.get("pins").and_then(|p| p.as_object()) else {
        return Ok(());
    };
    for (key, value) in pins {
        if key == "sss" {
            let children: Vec<&serde_json::Value> = match value {
                serde_json::Value::Array(items) => items.iter().collect(),
                other => vec![other],
            };
            for (i, child) in children.iter().enumerate() {
                lint_flattened_groups(child, &format!("{path}/sss[{i}]"))?;
            }
            continue;
        }
        let elements = match value {
            serde_json::Value::Array(items) => items.len(),
            _ => 1,
        };
        if elements >= 2 && pins.len() > 1 {
            let siblings: Vec<&str> = pins
                .keys()
                .filter(|k| k.as_str() != key)
                .map(|k| k.as_str())
                .collect();
            return Err(format!(
                "VULNERABLE: {path}: \"{key}\" is a MULTI-element array sitting directly \
                 beside [{siblings}], so its {elements} entries are {elements} SEPARATE \
                 shares — not one factor. The threshold at this level is therefore met by \
                 the \"{key}\" entries ALONE, and [{siblings}] add availability only, NOT \
                 security. Fix: nest the group under an inner sss so it collapses to ONE \
                 share, e.g. {{\"t\":2,\"pins\":{{\"tpm2\":[…],\"sss\":[{{\"t\":2,\"pins\":\
                 {{\"{key}\":[…]}}}}]}}}}",
                siblings = siblings.join(", ")
            ));
        }
    }
    Ok(())
}

/// Structural validation of ONE `sss` policy tree. `Ok(detail)` on success.
///
/// # The property, and why it is not a shape check
///
/// The security requirement is: **no single share can open the volume, on any
/// path through the tree.** That is precisely
/// [`min_satisfying_shares`]` >= 2`, and it is the ONLY threshold rule here.
///
/// The previous version hardcoded an outer `t == 2`, which rejected the settled
/// fleet policy — an outer `t=1` OR over three groups, each of which is itself a
/// real AND. An outer `t=1` is not weak in itself; it is weak exactly when some
/// branch is weak, and the cheapest-satisfying-set number says so directly.
///
/// The trap a local rule falls into: group 3 of the settled policy is
/// `{"t":2,"pins":{"sss":[{"t":1,"tang":…},{"t":1,"pkcs11":…}]}}`. Both inner
/// groups are `t=1` over a single kind — single-share-satisfiable in isolation —
/// yet the policy is sound because the enclosing group requires BOTH. Any rule
/// that rejects a `t=1` single-kind node locally rejects the settled design.
/// The question is only ever about satisfying-set size at the TOP.
///
/// Alongside that, [`lint_flattened_groups`] rejects the share-arithmetic
/// mis-encoding (a multi-element pin array beside a sibling), which the
/// satisfying-set count cannot see: clevis really does read `2-of-4` there, so
/// the number is 2 even though the author wrote an AND.
fn validate_sss_policy(policy: &serde_json::Value) -> std::result::Result<String, String> {
    lint_flattened_groups(policy, "policy")?;

    let min = min_satisfying_shares(policy)?;
    if min < MIN_SATISFYING_SHARES {
        return Err(format!(
            "VULNERABLE: this policy is satisfiable by {min} share — one factor \
             opens the volume. Every path through the tree must require at least \
             {MIN_SATISFYING_SHARES} shares; an outer t=1 is an OR, so it is only \
             as strong as its CHEAPEST branch. Fix the branch that costs {min}: \
             raise its threshold, or AND it with a second factor."
        ));
    }

    let obj = policy
        .as_object()
        .ok_or_else(|| "sss policy is not a JSON object".to_string())?;
    let t = obj.get("t").and_then(|t| t.as_u64()).unwrap_or_default();
    let shares = obj
        .get("pins")
        .and_then(|p| p.as_object())
        .map(count_shares)
        .unwrap_or_default();
    Ok(format!(
        "outer t={t} of {shares} share(s); cheapest satisfying set = {min} factors"
    ))
}

/// Clevis SSS binding is present, is bound only to Tang servers we run, and has
/// a **share topology** no single factor can open.
///
/// Expects output of `clevis luks list <dev>`.
///
/// # Why this is structural and not a substring match
///
/// In clevis's `sss` pin an array value contributes one share PER ELEMENT, so
/// `{"t":2,"pins":{"tang":[a,b,c],"tpm2":{…}}}` is **2-of-4** and the three Tang
/// servers alone satisfy the threshold — it is NOT `AND(tpm2, tang)`. The
/// correct policy nests the Tang group under an inner `sss` so it collapses to a
/// single share. Both strings contain `sss`, `"t":2` and every Tang URL, so the
/// old substring checks passed the vulnerable one. See [`count_shares`].
///
/// # Why there is no required `"t"` value
///
/// The settled fleet policy has an outer `t=1` — an OR over three groups, each
/// of which is itself a real AND. A verifier that demands `t == 2` at the top
/// rejects it. What is actually required is
/// [`min_satisfying_shares`]` >= `[`MIN_SATISFYING_SHARES`]: no single share may
/// open the volume on ANY path through the tree. See [`validate_sss_policy`].
///
/// Validation is against **invariants of the JSON itself**, not against a
/// declared spec: the policy is self-describing (a second pin alongside `tang`
/// means an AND was intended), so a mis-declared or absent spec cannot make a
/// Tang-satisfiable volume verify clean.
///
/// Fails closed: empty output, unparseable JSON, or no `sss` binding all FAIL.
/// With multiple keyslots, ANY vulnerable binding fails the check — every slot
/// can unlock the volume, so a good slot does not redeem a bad one.
pub fn evaluate_clevis_binding(clevis_output: &str) -> CheckResult {
    const NAME: &str = "clevis_binding";
    let tang_urls = &crate::fleet::fleet().tang_urls;

    let bindings = parse_clevis_bindings(clevis_output);
    if bindings.is_empty() {
        return CheckResult::fail(
            NAME,
            "no clevis binding found (empty or unparseable `clevis luks list` output)",
        );
    }

    let sss_bindings: Vec<&ClevisBindingLine> =
        bindings.iter().filter(|b| b.pin == "sss").collect();
    if sss_bindings.is_empty() {
        let pins: Vec<&str> = bindings.iter().map(|b| b.pin.as_str()).collect();
        return CheckResult::fail(
            NAME,
            format!(
                "missing 'sss' pin: bound pin(s) are [{}] — a bare pin has no threshold",
                pins.join(", ")
            ),
        );
    }

    // A non-sss keyslot has NO threshold at all — e.g. a bare `tang` pin unlocks
    // the volume from a single Tang server. It can open the volume just like the
    // sss slot can, so it is exactly the vulnerability class above in its
    // maximal form and must fail the check.
    if let Some(bare) = bindings.iter().find(|b| b.pin != "sss") {
        return CheckResult::fail(
            NAME,
            format!(
                "VULNERABLE: slot {} is bound with a bare '{}' pin alongside the sss \
                 policy. A bare pin has no threshold, so that slot ALONE unlocks the \
                 volume — it defeats the sss policy entirely. Remove the extra \
                 binding (`clevis luks unbind -d <dev> -s {}`).",
                bare.slot, bare.pin, bare.slot
            ),
        );
    }

    let mut details = Vec::new();
    for binding in &sss_bindings {
        let policy: serde_json::Value = match serde_json::from_str(&binding.json) {
            Ok(v) => v,
            Err(e) => {
                return CheckResult::fail(
                    NAME,
                    format!("slot {}: unparseable clevis JSON: {e}", binding.slot),
                )
            }
        };

        match validate_sss_policy(&policy) {
            Ok(detail) => {
                // Tang URLs are checked as a SUBSET of the fleet inventory, not
                // as a per-host requirement to bind all of them. The settled
                // policy pairs two peers per host; demanding all three would
                // reject it for a reason that has nothing to do with topology.
                // The check keeps its teeth the other way round: a URL we do not
                // run is an unknown key server and fails.
                let mut found = Vec::new();
                collect_tang_urls(&policy, &mut found);
                if found.is_empty() {
                    return CheckResult::fail(
                        NAME,
                        format!(
                            "slot {}: no Tang server anywhere in the policy — nothing \
                             unlocks this volume unattended",
                            binding.slot
                        ),
                    );
                }
                let unknown: Vec<&str> = found
                    .iter()
                    .filter(|f| !tang_urls.iter().any(|u| u == *f))
                    .map(|f| f.as_str())
                    .collect();
                if !unknown.is_empty() {
                    return CheckResult::fail(
                        NAME,
                        format!(
                            "slot {}: bound to Tang server(s) that are not ours: {} \
                             (fleet inventory: {})",
                            binding.slot,
                            unknown.join(", "),
                            tang_urls.join(", ")
                        ),
                    );
                }
                details.push(format!(
                    "slot {}: {detail}; {} Tang peer(s)",
                    binding.slot,
                    found.len()
                ));
            }
            Err(reason) => {
                return CheckResult::fail(NAME, format!("slot {}: {reason}", binding.slot))
            }
        }
    }

    CheckResult::pass(NAME, details.join(" | "))
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
    let lenserv_nic = &crate::fleet::fleet().lenserv_nic;
    if ip_br_output.contains(&spec.network_address) {
        CheckResult::pass("ip_matches", format!("{} on {lenserv_nic}", spec.network_address))
    } else {
        CheckResult::fail(
            "ip_matches",
            format!(
                "expected {} on {lenserv_nic}, got: {ip_br_output:?}",
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
    let luks_partition = crate::fleet::fleet().luks_partition.clone();
    let lenserv_nic = crate::fleet::fleet().lenserv_nic.clone();
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
        .execute_with_output(&format!("sudo -n clevis luks list -d {luks_partition}"))
        .await
        .unwrap_or_default();
    checks.push(evaluate_clevis_binding(&clevis));

    // 2b. TPM2 (first-boot) + FIDO2 (manual) keyslots from luksDump
    let luksdump = runner
        .execute_with_output(&format!("sudo -n cryptsetup luksDump {luks_partition}"))
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
        .execute_with_output(&format!("ip -br addr show {lenserv_nic}"))
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

    /// The live Lenovo fleet's member IPs. `for_lenserv` previously defaulted
    /// this internally via a hardcoded module-level constant; tests now pass
    /// it in explicitly (PS-COCKROACH-16). Unused by the checks below (they
    /// only compare hostname/IP/LUKS/etc against fixture output), so only
    /// `for_lenserv`'s signature needs it.
    const TEST_LENSERV_MEMBERS: &[&str] = &["172.16.3.92", "172.16.3.94", "172.16.3.96"];

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

    /// The VULNERABLE shape: `tang` is a DIRECT child of the outer `pins`, so
    /// its three entries are three separate shares. With `t:2` this is 2-of-4
    /// and the three Tang servers ALONE satisfy the threshold — the tpm2 pin
    /// contributes nothing to the threat model.
    const CLEVIS_VULNERABLE_2_OF_4: &str =
        "1: sss '{\"t\":2,\"pins\":{\"tang\":[{\"url\":\"http://172.16.2.45\"},{\"url\":\"http://172.16.2.46\"},{\"url\":\"http://172.16.2.47\"}],\"tpm2\":{\"pcr_ids\":\"7\",\"pcr_bank\":\"sha256\"}}}'";

    /// The CORRECT shape: the Tang group is nested under an inner `sss`, so it
    /// collapses to ONE share. Outer is 2-of-2 = AND(tpm2, 2-of-3 tang).
    const CLEVIS_NESTED_CORRECT: &str =
        "1: sss '{\"t\":2,\"pins\":{\"tpm2\":{\"pcr_ids\":\"7\",\"pcr_bank\":\"sha256\"},\"sss\":[{\"t\":2,\"pins\":{\"tang\":[{\"url\":\"http://172.16.2.45\"},{\"url\":\"http://172.16.2.46\"},{\"url\":\"http://172.16.2.47\"}]}}]}}'";

    #[test]
    fn clevis_binding_rejects_tang_satisfiable_2_of_4() {
        let r = evaluate_clevis_binding(CLEVIS_VULNERABLE_2_OF_4);
        assert!(
            !r.passed,
            "2-of-4 config is Tang-satisfiable and MUST be rejected: {}",
            r.detail
        );
        assert!(
            r.detail.to_lowercase().contains("tang"),
            "operator must be told it is Tang-satisfiable, got: {}",
            r.detail
        );
    }

    #[test]
    fn clevis_binding_passes_on_nested_tpm2_and_tang() {
        let r = evaluate_clevis_binding(CLEVIS_NESTED_CORRECT);
        assert!(
            r.passed,
            "correct nested policy must verify clean: {}",
            r.detail
        );
    }

    // ── count_shares: the arithmetic that was wrong ──────────────────────────

    fn pins_of(json: &str) -> serde_json::Map<String, serde_json::Value> {
        serde_json::from_str(json).expect("test fixture is valid JSON")
    }

    #[test]
    fn count_shares_counts_each_array_element_separately() {
        // THE bug: "tang":[a,b,c] is THREE shares, not one.
        assert_eq!(
            count_shares(&pins_of(
                r#"{"tang":[{"url":"a"},{"url":"b"},{"url":"c"}]}"#
            )),
            3
        );
    }

    #[test]
    fn count_shares_counts_non_array_pin_as_one() {
        assert_eq!(count_shares(&pins_of(r#"{"tpm2":{"pcr_ids":"7"}}"#)), 1);
    }

    #[test]
    fn count_shares_sums_mixed_pins() {
        // 3 tang + 1 tpm2 = 4 shares → t=2 is 2-of-4, NOT an AND.
        assert_eq!(
            count_shares(&pins_of(
                r#"{"tang":[{"url":"a"},{"url":"b"},{"url":"c"}],"tpm2":{"pcr_ids":"7"}}"#
            )),
            4
        );
        // Nested: the tang group collapses into ONE sss share → 2 total.
        assert_eq!(
            count_shares(&pins_of(
                r#"{"tpm2":{"pcr_ids":"7"},"sss":[{"t":2,"pins":{"tang":[{"url":"a"},{"url":"b"}]}}]}"#
            )),
            2
        );
    }

    #[test]
    fn count_shares_of_empty_pins_is_zero() {
        assert_eq!(count_shares(&pins_of("{}")), 0);
    }

    // ── parse_clevis_bindings ────────────────────────────────────────────────

    #[test]
    fn parse_clevis_bindings_reads_slot_pin_and_json() {
        let parsed = parse_clevis_bindings(CLEVIS_003);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].slot, "1");
        assert_eq!(parsed[0].pin, "sss");
        assert!(parsed[0].json.starts_with('{') && parsed[0].json.ends_with('}'));
    }

    #[test]
    fn parse_clevis_bindings_handles_multiple_slots_and_junk() {
        let out =
            format!("{CLEVIS_003}\n\n2: tang '{{\"url\":\"http://x\"}}'\nnot a binding line\n");
        let parsed = parse_clevis_bindings(&out);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].pin, "tang");
    }

    // ── evaluate_clevis_binding: topology ────────────────────────────────────

    #[test]
    fn clevis_binding_fails_closed_on_empty_output() {
        // verify_host uses unwrap_or_default(), so an SSH failure lands here.
        assert!(!evaluate_clevis_binding("").passed);
        assert!(!evaluate_clevis_binding("1: sss 'not json'").passed);
    }

    #[test]
    fn clevis_binding_rejects_vulnerable_slot_even_if_another_slot_is_good() {
        let out = format!("{CLEVIS_NESTED_CORRECT}\n{CLEVIS_VULNERABLE_2_OF_4}");
        assert!(!evaluate_clevis_binding(&out).passed);
    }

    #[test]
    fn clevis_binding_rejects_bare_tang_slot_beside_a_good_sss_slot() {
        // Slot 2 has no threshold at all: one Tang server unlocks the volume,
        // which defeats the correct sss policy in slot 1.
        let out = format!("{CLEVIS_NESTED_CORRECT}\n2: tang '{{\"url\":\"http://172.16.2.45\"}}'");
        let r = evaluate_clevis_binding(&out);
        assert!(!r.passed, "a bare pin slot must fail the check: {}", r.detail);
        assert!(r.detail.contains("VULNERABLE"), "{}", r.detail);
    }

    /// SEMANTIC CHANGE, deliberate: `AND(tpm2, any-one-Tang)` is ACCEPTED.
    ///
    /// The predecessor rule demanded `t >= 2` on the nested Tang group, which
    /// rejected this. That rule cannot survive the generalisation, because the
    /// settled policy's own group 3 is the same shape — `AND(any-one-Tang,
    /// either-carried-key)`, two inner `t=1` groups — and any local rule that
    /// rejects one rejects the other. Under the stated property both are sound:
    /// the cheapest satisfying set is 2 factors, so no single share opens the
    /// volume.
    #[test]
    fn clevis_binding_accepts_and_of_tpm2_and_a_single_tang_group() {
        let policy = "1: sss '{\"t\":2,\"pins\":{\"tpm2\":[{\"pcr_ids\":\"7\"}],\"sss\":[{\"t\":1,\"pins\":{\"tang\":[{\"url\":\"http://172.16.2.45\"},{\"url\":\"http://172.16.2.46\"},{\"url\":\"http://172.16.2.47\"}]}}]}}'";
        assert_eq!(
            min_satisfying_shares(&policy_of(
                &policy[policy.find('{').unwrap()..policy.rfind('}').unwrap() + 1]
            )),
            Ok(2),
            "tpm2 + one Tang = two factors"
        );
        let r = evaluate_clevis_binding(policy);
        assert!(r.passed, "{}", r.detail);
    }

    #[test]
    fn clevis_binding_rejects_unsatisfiable_nested_threshold() {
        // inner t=4 of 3 Tang servers: can never unlock.
        let r = evaluate_clevis_binding(
            "1: sss '{\"t\":2,\"pins\":{\"tpm2\":{\"pcr_ids\":\"7\"},\"sss\":[{\"t\":4,\"pins\":{\"tang\":[{\"url\":\"http://172.16.2.45\"},{\"url\":\"http://172.16.2.46\"},{\"url\":\"http://172.16.2.47\"}]}}]}}'",
        );
        assert!(!r.passed);
        assert!(r.detail.contains("unsatisfiable"), "{}", r.detail);
    }

    /// Replaces `clevis_binding_rejects_extra_outer_share`, whose `shares == t`
    /// rule was a proxy for "no single share opens it" and is subsumed by the
    /// satisfying-set count (it also rejects the settled policy's outer `t=1` of
    /// 3). The shape it guarded against is only dangerous when the extra share
    /// makes some path cost 1 — asserted here directly.
    #[test]
    fn clevis_binding_rejects_an_extra_outer_share_that_makes_one_factor_enough() {
        // t=1 over [tpm2, sshd, nested 2-of-3 Tang]: the tpm2 share ALONE opens
        // the volume.
        let r = evaluate_clevis_binding(
            "1: sss '{\"t\":1,\"pins\":{\"tpm2\":[{\"pcr_ids\":\"7\"}],\"sshd\":[{\"host\":\"h\"}],\"sss\":[{\"t\":2,\"pins\":{\"tang\":[{\"url\":\"http://172.16.2.45\"},{\"url\":\"http://172.16.2.46\"},{\"url\":\"http://172.16.2.47\"}]}}]}}'",
        );
        assert!(!r.passed);
        assert!(r.detail.contains("VULNERABLE"), "{}", r.detail);
    }

    // ── The SETTLED fleet policy (nested tree, outer t=1) ────────────────────
    //
    // These fixtures are the EMITTED policy, copied structurally from the
    // emitter's golden tests (`SssPolicy::fleet_three_group`): two Tang peers
    // with `adv` paths, `tpm2` as a ONE-element array, three groups under an
    // outer `t=1`. Built with `json!` rather than hand-written strings so a
    // divergence from the emitter is a structural difference, not a typo.

    const PEER_A: &str = "http://172.16.2.45";
    const PEER_B: &str = "http://172.16.2.46";
    const NANO: &str = "pkcs11:serial=NANO0001";
    const CARRIED_A: &str = "pkcs11:serial=CARRIED0A";
    const CARRIED_B: &str = "pkcs11:serial=CARRIED0B";

    fn tang_entry(url: &str, index: usize) -> serde_json::Value {
        serde_json::json!({"url": url, "adv": format!("/run/uaa-tang-{index}.adv")})
    }

    /// The two peer Tang servers as a `t`-of-2 group.
    fn tang_group(t: u64) -> serde_json::Value {
        serde_json::json!({
            "t": t,
            "pins": {"tang": [tang_entry(PEER_A, 0), tang_entry(PEER_B, 1)]}
        })
    }

    /// Group 2 — any 2 of the 3 tokens.
    fn group_two() -> serde_json::Value {
        serde_json::json!({
            "t": 2,
            "pins": {"pkcs11": [{"uri": NANO}, {"uri": CARRIED_A}, {"uri": CARRIED_B}]}
        })
    }

    /// Group 3 — (any ONE Tang) AND (either carried key). Both inner groups are
    /// `t=1` over a single pin kind, which is legal ONLY because the enclosing
    /// group requires BOTH of them: this is the shape a local "t=1 over one kind
    /// is weak" rule would wrongly reject.
    fn group_three() -> serde_json::Value {
        serde_json::json!({
            "t": 2,
            "pins": {"sss": [
                tang_group(1),
                {"t": 1, "pins": {"pkcs11": [{"uri": CARRIED_A}, {"uri": CARRIED_B}]}},
            ]}
        })
    }

    /// The settled policy. `tpm2_pcr_ids` = `None` for the RPi variant (no TPM
    /// anywhere), `Some(..)` for the lenserv variant, which ANDs tpm2 into
    /// group 1.
    fn settled_fleet_policy(tpm2_pcr_ids: Option<&str>) -> serde_json::Value {
        let group_one = match tpm2_pcr_ids {
            None => tang_group(2),
            Some(pcr_ids) => serde_json::json!({
                "t": 2,
                "pins": {
                    // ONE-element array: one share. Rule B must not mistake it
                    // for a flattened multi-share group.
                    "tpm2": [{"pcr_ids": pcr_ids, "pcr_bank": "sha256"}],
                    "sss": [tang_group(2)],
                }
            }),
        };
        serde_json::json!({
            "t": 1,
            "pins": {"sss": [group_one, group_two(), group_three()]}
        })
    }

    /// Render a policy as one `clevis luks list` line.
    fn clevis_line(policy: &serde_json::Value) -> String {
        format!("1: sss '{policy}'")
    }

    #[test]
    fn clevis_binding_passes_on_settled_rpi_fleet_policy() {
        let r = evaluate_clevis_binding(&clevis_line(&settled_fleet_policy(None)));
        assert!(
            r.passed,
            "the settled fleet policy is the design this check exists to protect \
             and MUST verify clean: {}",
            r.detail
        );
    }

    #[test]
    fn clevis_binding_passes_on_settled_lenserv_fleet_policy() {
        let r = evaluate_clevis_binding(&clevis_line(&settled_fleet_policy(Some("7"))));
        assert!(
            r.passed,
            "the lenserv variant (tpm2 ANDed into group 1) MUST verify clean: {}",
            r.detail
        );
    }

    /// The generalisation to outer `t=1` must not open a hole: an OR is only as
    /// strong as its WEAKEST branch, so one group satisfiable by a single share
    /// makes the whole policy single-share-satisfiable.
    #[test]
    fn clevis_binding_rejects_outer_or_with_a_single_share_group() {
        let mut policy = settled_fleet_policy(Some("7"));
        // Degrade group 1 to `t=1` over the two Tang peers: ONE Tang server now
        // opens the volume via that branch, even though groups 2 and 3 are fine.
        policy["pins"]["sss"][0] = tang_group(1);
        let r = evaluate_clevis_binding(&clevis_line(&policy));
        assert!(
            !r.passed,
            "an outer t=1 OR with a single-share branch is opened by one share \
             and MUST be rejected: {}",
            r.detail
        );
        assert!(r.detail.contains("VULNERABLE"), "{}", r.detail);
    }

    /// A `t=1` OR whose branches are all bare pins is the degenerate case: any
    /// one factor opens it.
    #[test]
    fn clevis_binding_rejects_outer_or_over_bare_pins() {
        let r = evaluate_clevis_binding(
            "1: sss '{\"t\":1,\"pins\":{\"tpm2\":[{\"pcr_ids\":\"7\"}],\"sss\":[{\"t\":2,\"pins\":{\"tang\":[{\"url\":\"http://172.16.2.45\"},{\"url\":\"http://172.16.2.46\"}]}}]}}'",
        );
        assert!(!r.passed, "{}", r.detail);
        assert!(r.detail.contains("VULNERABLE"), "{}", r.detail);
    }

    /// Rule B boundary: a ONE-element non-`sss` array beside a sibling is a
    /// single share and is legal. Only a MULTI-element array beside a sibling is
    /// the flattening bug.
    #[test]
    fn clevis_binding_accepts_single_element_pin_array_beside_a_sibling() {
        let r = evaluate_clevis_binding(&clevis_line(&serde_json::json!({
            "t": 2,
            "pins": {
                "tpm2": [{"pcr_ids": "7", "pcr_bank": "sha256"}],
                "sss": [tang_group(2)],
            }
        })));
        assert!(
            r.passed,
            "tpm2 as a one-element array is ONE share, not a flattened group: {}",
            r.detail
        );
    }

    /// Rule B, at depth: the flattening bug is rejected wherever it appears, not
    /// only at the top level.
    #[test]
    fn clevis_binding_rejects_flattened_tang_group_nested_inside_an_or() {
        let mut policy = settled_fleet_policy(Some("7"));
        // Flatten group 1 into the 2-of-3 shape: tang[a,b] beside tpm2 means the
        // two Tang peers ALONE meet t=2.
        policy["pins"]["sss"][0] = serde_json::json!({
            "t": 2,
            "pins": {
                "tpm2": [{"pcr_ids": "7", "pcr_bank": "sha256"}],
                "tang": [tang_entry(PEER_A, 0), tang_entry(PEER_B, 1)],
            }
        });
        let r = evaluate_clevis_binding(&clevis_line(&policy));
        assert!(
            !r.passed,
            "a flattened Tang group inside an OR branch is still Tang-satisfiable: {}",
            r.detail
        );
    }

    /// The Tang URLs a host binds are a SUBSET of the fleet's Tang servers (the
    /// settled policy uses two peers, the fleet runs three), but every URL bound
    /// must be a Tang server we actually run.
    #[test]
    fn clevis_binding_rejects_an_unknown_tang_url() {
        let mut policy = settled_fleet_policy(None);
        policy["pins"]["sss"][0] = serde_json::json!({
            "t": 2,
            "pins": {"tang": [tang_entry(PEER_A, 0), tang_entry("http://10.0.0.9", 1)]}
        });
        let r = evaluate_clevis_binding(&clevis_line(&policy));
        assert!(!r.passed, "an unknown Tang server must fail: {}", r.detail);
        assert!(r.detail.contains("10.0.0.9"), "{}", r.detail);
    }

    #[test]
    fn clevis_binding_rejects_a_policy_with_no_tang_server_at_all() {
        // pkcs11-only: nothing unlocks unattended, so this is not the fleet
        // policy and must not verify clean.
        let r = evaluate_clevis_binding(&clevis_line(&serde_json::json!({
            "t": 1, "pins": {"sss": [group_two(), group_two()]}
        })));
        assert!(!r.passed, "{}", r.detail);
        assert!(r.detail.to_lowercase().contains("tang"), "{}", r.detail);
    }

    // ── min_satisfying_shares: the generalised arithmetic ────────────────────

    fn policy_of(json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("test fixture is valid JSON")
    }

    #[test]
    fn min_satisfying_shares_sums_the_t_cheapest_branches() {
        // t=2 over [tang, tang, tang] → two shares.
        assert_eq!(
            min_satisfying_shares(&policy_of(
                r#"{"t":2,"pins":{"tang":[{"url":"a"},{"url":"b"},{"url":"c"}]}}"#
            )),
            Ok(2)
        );
        // Nesting does NOT degrade the inner threshold: the nested group costs
        // 2, so the outer AND costs 3.
        assert_eq!(
            min_satisfying_shares(&policy_of(
                r#"{"t":2,"pins":{"tpm2":[{"pcr_ids":"7"}],"sss":[{"t":2,"pins":{"tang":[{"url":"a"},{"url":"b"}]}}]}}"#
            )),
            Ok(3)
        );
        // An OR takes the CHEAPEST branch, not the average.
        assert_eq!(
            min_satisfying_shares(&policy_of(
                r#"{"t":1,"pins":{"sss":[{"t":2,"pins":{"tang":[{"url":"a"},{"url":"b"}]}},{"t":1,"pins":{"tang":[{"url":"a"},{"url":"b"}]}}]}}"#
            )),
            Ok(1)
        );
    }

    #[test]
    fn min_satisfying_shares_of_the_settled_policy_is_two() {
        // Every branch of the settled policy costs at least 2 — the property
        // the whole check is about.
        assert_eq!(min_satisfying_shares(&settled_fleet_policy(None)), Ok(2));
        assert_eq!(min_satisfying_shares(&settled_fleet_policy(Some("7"))), Ok(2));
    }

    #[test]
    fn min_satisfying_shares_reports_unsatisfiable_thresholds() {
        assert!(min_satisfying_shares(&policy_of(
            r#"{"t":4,"pins":{"tang":[{"url":"a"},{"url":"b"}]}}"#
        ))
        .is_err());
        assert!(min_satisfying_shares(&policy_of(r#"{"t":0,"pins":{"tpm2":{}}}"#)).is_err());
    }

    #[test]
    fn clevis_binding_fails_without_sss() {
        let r = evaluate_clevis_binding("1: tang '{\"url\":\"http://172.16.2.45\"}'");
        assert!(!r.passed);
        assert!(r.detail.contains("sss"));
    }

    /// Was `clevis_binding_fails_without_threshold`, which asserted on the
    /// literal `t=2` message. The reason it fails is not the number in the `"t"`
    /// field — an outer `t=1` is legal in the settled policy — but that ONE Tang
    /// server satisfies this flat group.
    #[test]
    fn clevis_binding_fails_on_a_flat_policy_one_share_can_open() {
        let r = evaluate_clevis_binding(
            "1: sss '{\"t\":1,\"pins\":{\"tang\":[{\"url\":\"http://172.16.2.45\"},{\"url\":\"http://172.16.2.46\"},{\"url\":\"http://172.16.2.47\"}]}}'",
        );
        assert!(!r.passed);
        assert!(r.detail.contains("VULNERABLE"), "{}", r.detail);
        assert!(r.detail.contains("1 share"), "{}", r.detail);
    }

    /// Replaces `clevis_binding_fails_on_missing_tang_url`: binding a SUBSET of
    /// the fleet's Tang servers is the settled design (two peers per host), so
    /// this shape must now PASS. The teeth moved to
    /// `clevis_binding_rejects_an_unknown_tang_url`.
    #[test]
    fn clevis_binding_accepts_a_subset_of_the_fleet_tang_servers() {
        let r = evaluate_clevis_binding(
            "1: sss '{\"t\":2,\"pins\":{\"tang\":[{\"url\":\"http://172.16.2.45\"},{\"url\":\"http://172.16.2.46\"}]}}'",
        );
        assert!(r.passed, "{}", r.detail);
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
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23", TEST_LENSERV_MEMBERS);
        assert!(evaluate_hostname("len-serv-003\n", &spec).passed);
    }

    #[test]
    fn hostname_fails_on_mismatch() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23", TEST_LENSERV_MEMBERS);
        let r = evaluate_hostname("len-serv-001\n", &spec);
        assert!(!r.passed);
        assert!(r.detail.contains("len-serv-001"));
    }

    #[test]
    fn ip_address_passes_when_present() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23", TEST_LENSERV_MEMBERS);
        let out = "enp1s0f0 UP 172.16.3.96/23 fe80::1/64";
        assert!(evaluate_ip_address(out, &spec).passed);
    }

    #[test]
    fn ip_address_fails_when_absent() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23", TEST_LENSERV_MEMBERS);
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
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23", TEST_LENSERV_MEMBERS);

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
        let spec = HostSpec::for_lenserv("len-serv-001", "172.16.3.92/23", TEST_LENSERV_MEMBERS);

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
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23", TEST_LENSERV_MEMBERS);

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
