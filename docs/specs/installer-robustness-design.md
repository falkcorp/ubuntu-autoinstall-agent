<!-- file: docs/specs/installer-robustness-design.md -->
<!-- version: 1.0.0 -->
<!-- guid: 09140138-5010-4d24-932d-584411d96b38 -->
<!-- last-edited: 2026-07-09 -->

# Installer Robustness (Path B ssh_installer) — Design Spec

**Status:** Approved — decisions operator-locked; ready for implementation via `docs/specs/installer-robustness-plan.md`
**Scope:** Rust only — `src/network/ssh_installer/` (new `partitions.rs`, plus `mod.rs`, `disk_ops.rs`, `zfs_ops.rs`, `system_setup.rs`, `installer.rs`, `config.rs`), `src/cli/commands.rs`, and two docs files (`docs/architecture-path-split.md`, `README.md`). Server-side scripts, phase-selective re-run, boot-order, and the QEMU harness are separate workstreams.
**Workstream:** installer-robustness (8 tasks, `docs/agent-tasks/installer-robustness/`)

---

## Motivation

Path B (`src/network/ssh_installer/`) is the PROVEN installer — 7/7 phases passed on unimatrixone hardware on 2026-07-09 — but that hardware run succeeded partly because the target device (`/dev/md126`) happens to end in a digit. The code carries seven distinct robustness/security defects, each grep-verified by the evidence scouts:

1. **`/dev/sdapN` partition-path bug.** Partition paths are built by inline `format!("{}pN", disk)` at 11 production call sites plus 2 `#[cfg(test)]` builders across 4 installer files — there is NO shared helper (`grep -rn 'fn partition\|partition_path\|part_suffix' src --include='*.rs'` finds only the ESP-specific `detect_esp_partition_path`). The unconditional `p` is correct for `nvme0n1`/`md126` but wrong for `sda`/`vda`: on a QEMU virtio `/dev/vda` target, Phase 2 `mkfs` hits nonexistent `/dev/vdap1`/`vdap2`, `cryptsetup` hits `/dev/vdap4`, and Phase 3 `zpool create bpool vdap3` fails. Four existing tests *assert the bug* (`zfs_ops.rs:381` expects `/dev/sdap3`; `system_setup.rs:1001/1026/1032` expect `/dev/sdap1` and `luks /dev/sdap4`). This blocks the QEMU virtio VM gate (`testing-gates/TASK-01` depends on it).
2. **LUKS passphrase leaks.** `setup_luks_encryption` (`disk_ops.rs:340/348`) interpolates the passphrase into `echo '{}' | cryptsetup luksFormat/open` command lines (visible via `ps`), and Phase 0 `setup_installation_variables` exports it as env var `LUKS_KEY` (`installer.rs:483/493`, visible in `/proc/<pid>/environ`) — an export that is *inert* anyway, since each `runner.execute()` is a one-shot command and downstream code reads `config.luks_key` directly.
3. **`detect_primary_disk` is fragile text matching.** `cli/commands.rs:635` string-matches `lsblk` text and only recognizes `nvme*`/`sd*` — it misses `/dev/md*` entirely. Hard rule: on unimatrixone, `/dev/sda` is an IMSM RAID *member*; the real volume is `/dev/md126`. Guessing `/dev/sd*` targets the wrong device.
4. **`detect_network_config` is a stub.** `cli/commands.rs:654` ignores its input and returns hardcoded `("eth0", "dhcp", "auto")`.
5. **Netplan renderer hardcoded + `dhcp` renders invalid YAML.** `setup_network_configuration` (`system_setup.rs:~199`) always emits `renderer: networkd` and always emits the address as a static `addresses:` entry — a literal `"dhcp"` network_address (exactly what defect 4 produces) is not a CIDR and renders invalid netplan.
6. **Config schema accepts typos silently.** `InstallationConfig` (`config.rs:47-96`) has no `#[serde(deny_unknown_fields)]`, so a misspelled YAML key is silently dropped and the field silently defaults. The struct's 20 fields match `examples/configs/install/unimatrixone.yaml` keys 1:1 (scout-verified), so hardening is safe today.
7. **No curtin in-target compatibility.** `grep -rn 'curtin' --include='*.rs' src/` → 0 hits. Running the installer from inside an already-prepared target chroot re-runs mounts and debootstrap.

Separately, Path A (`src/autoinstall/`, the older subiquity render pipeline) coexists with Path B and the split is undocumented, which invites accidental "cleanup" of live code.

**Goal:** Make Path B correct on any Linux disk-naming scheme, keep the LUKS secret off argv/env, replace both autodetect stubs with real JSON parsing, render valid netplan for both renderers and for DHCP, fail loudly on config typos, tolerate curtin in-target invocation, and document the Path A/B split.

## Goals

- One shared, unit-tested partition-path helper used by every ssh_installer suffix site; the 4 wrong test assertions corrected in the same change.
- LUKS passphrase reaches `cryptsetup` only via a 0600 tempfile keyfile (`--key-file`), for both `luksFormat` and `open`; the inert `LUKS_KEY` env export is deleted.
- `detect_primary_disk` parses `lsblk --json`, includes md devices, excludes loop/rom, prefers the largest disk.
- `detect_network_config` parses `ip -j addr` / `ip -j route` for the real default-route interface, CIDR, and gateway.
- Netplan renderer selectable per config (`networkd` default, `NetworkManager` optional); `network_address == "dhcp"` renders `dhcp4: true`.
- `InstallationConfig` rejects unknown YAML keys; YAML round-trip tests pin the schema.
- Already-inside-target-chroot detection skips mounts + debootstrap and runs post-install configuration only (additive; default flow unchanged).
- A docs page defining Path A vs Path B, with guardrails against removing either.

## Non-goals (v1)

- **Removing Path A (`src/autoinstall/`).** It is STILL LIVE (render/place/verify + golden tests). TASK-08 is a split-*documentation* doc, not removal.
- Phase-selective re-run / non-destructive mount-existing-target — workstream `phase-rerun`.
- efibootmgr boot order, RESET partition population — workstream `boot-prod`.
- Server-side `scripts/autoinstall-agent.py` changes — workstream `install-server` (repo mirrors; humans deploy).
- The QEMU+swtpm VM harness itself — workstream `testing-gates` (it *consumes* TASK-01 here).
- `src/security/luks.rs` `create_luks_partition` — takes the device verbatim (deployer passes whole `config.disk_device`), no suffix logic; out of scope.
- Touching 172.16.2.30 ("the server") or len-serv-003 in any way. Code/docs only, validated in VM/QEMU.

## Decisions (LOCKED — operator-approved; do not reopen)

1. **One suffix-aware helper `partition_path(disk, n)` in NEW `src/network/ssh_installer/partitions.rs`.** Appends `"p"` only when the device *basename* ends in an ASCII digit (`nvme0n1`→`nvme0n1p3`, `md126`→`md126p3`, `sda`→`sda3`, `vda`→`vda3`). ALL 11 production call sites AND the 2 `#[cfg(test)]` builders (`build_mkfs_esp`/`build_mkfs_reset`) route through it; the 4 tests asserting `sdapN` are corrected to `sdaN` in the same task. The `src/utils/qemu.rs:118` loop-device site is EXCLUDED — `loopN` ends in a digit, so its unconditional `p` is already correct; "fixing" it would introduce a bug. *Rejected:* per-file helpers (drift between copies); a method on `InstallationConfig` (couples config to device-naming policy).
2. **LUKS secret via 0600 tempfile keyfile.** The keyfile is created with mode 0600, passed to `cryptsetup --key-file <path>` for BOTH `luksFormat` and `open`, and shredded (`shred -u`) afterward — mirroring the proven Tang enrollment pattern (`uaa-tang-enroll.key`, `system_setup.rs:~660`). The inert `LUKS_KEY` env export in `setup_installation_variables` is removed outright. *Rejected:* keeping the env export and merely hiding the passphrase via stdin — the `echo '<pass>' |` variant still leaks through `ps` on the remote host.
3. **JSON-based autodetection.** `detect_primary_disk` → `lsblk --json` (include md devices, exclude loop/rom, prefer largest); `detect_network_config` → `ip -j addr` / `ip -j route` parsing via `serde_json` (already a dependency). *Rejected:* patching more patterns into the text matcher (stays fragile, misses the md case that hard rule 2 exists for); scraping `/sys/block` directly (reimplements lsblk).
4. **Configurable netplan renderer + real DHCP rendering.** New config field (serde default = `networkd`) selecting `networkd` | `NetworkManager`; when `network_address == "dhcp"`, render `dhcp4: true` instead of an `addresses:` list (today a literal `"dhcp"` string renders invalid netplan). Byte-identical output when the field is absent and the address is static. *Rejected:* hardcoding NetworkManager (wrong for servers); auto-detecting the target's renderer (unknowable before the target OS exists).
5. **Schema hardening via serde.** `#[serde(deny_unknown_fields)]` on `InstallationConfig` + YAML round-trip tests (example YAML → struct → YAML → struct, equal) + an unknown-key rejection test. Safe because the struct already matches the example YAMLs 1:1 (scout-verified). *Rejected:* a hand-rolled validation pass (duplicates what serde does declaratively).
6. **curtin in-target: additive detection.** When the process is already inside the target chroot (curtin `in-target` invocation), skip mounts + debootstrap and run post-install configuration only. Detection is additive — a bare (non-chroot) run behaves exactly as today. *Rejected:* a separate curtin-specific subcommand/binary (duplicates phase logic and drifts).
7. **Path A is documented, not removed.** `docs/architecture-path-split.md` defines Path A (`src/autoinstall/`, subiquity render/place/verify + golden tests — STILL LIVE) vs Path B (`src/network/ssh_installer/`, the proven 7-phase installer) and the guardrails (which task types touch which path; neither is deleted without an operator decision). *Rejected:* deleting Path A (it is live) or leaving the split undocumented (invites accidental removal).

## Data model

```rust
// src/network/ssh_installer/partitions.rs  (NEW — TASK-01)

/// Build the partition device path for `disk`, partition number `n`,
/// following the Linux kernel naming rule: insert a "p" separator only
/// when the device BASENAME ends in an ASCII digit.
///
///   /dev/nvme0n1 -> /dev/nvme0n1p3     /dev/md126 -> /dev/md126p3
///   /dev/sda     -> /dev/sda3          /dev/vda   -> /dev/vda3
pub fn partition_path(disk: &str, n: u32) -> String {
    let base = disk.rsplit('/').next().unwrap_or(disk);
    if base.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        format!("{disk}p{n}")
    } else {
        format!("{disk}{n}")
    }
}
```

```rust
// src/network/ssh_installer/config.rs  (TASK-04 addition; field is optional in YAML)

/// Which netplan renderer the installed system's /etc/netplan/01-netcfg.yaml names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NetplanRenderer {
    #[default]
    Networkd,        // YAML value: networkd  -> renders `renderer: networkd`
    NetworkManager,  // YAML value: network-manager -> renders `renderer: NetworkManager`
}

// On InstallationConfig (must carry #[serde(default)] so existing YAMLs —
// and deny_unknown_fields from TASK-06 — keep parsing):
//   #[serde(default)]
//   pub netplan_renderer: NetplanRenderer,
```

```rust
// src/cli/commands.rs  (TASK-02/03 — private deserialization targets, serde_json)

#[derive(Debug, Deserialize)]
struct LsblkOutput { blockdevices: Vec<LsblkDevice> }

#[derive(Debug, Deserialize)]
struct LsblkDevice {
    name: String,                       // "nvme0n1", "md126", "sda", "loop0"
    #[serde(rename = "type")]
    dev_type: String,                   // "disk" | "raid1"/"raid0"/... (md) | "loop" | "rom" | "part"
    size: u64,                          // bytes (lsblk --json --bytes)
    #[serde(default)]
    children: Option<Vec<LsblkDevice>>, // partitions / raid members
}
// Selection rule (TASK-02): candidates = top-level devices whose dev_type is
// "disk" or starts with "raid" (md); EXCLUDE loop/rom AND any disk that has a
// raid child (it is a RAID *member* — the md device is the real volume, per
// hard rule 2 / the unimatrixone /dev/sda-vs-/dev/md126 case); pick largest size.
```

### Persistence

None. No new on-disk state in this workstream; the only schema change is the optional `netplan_renderer` YAML key (defaults preserve all existing config files).

## Components

### C1. Partition-path helper sweep (`src/network/ssh_installer/partitions.rs` + 4 caller files + `mod.rs`) — TASK-01

`partition_path` (above) is the single source of truth. `mod.rs` gains `pub mod partitions;`. Every `format!("{}pN", ...)` suffix site in ssh_installer routes through it — the scout inventory (verify with `grep -rn '}p[1-4]' src --include='*.rs'`, expect 15 code hits + 1 doc comment before the change):

| File | Sites (production) |
|---|---|
| `disk_ops.rs` | `format_partitions` p1 ESP (~320), p2 RESET (~325); `setup_luks_encryption` p4 luksFormat (~340), p4 open (~348) — plus `#[cfg(test)]` `build_mkfs_esp`/`build_mkfs_reset` (~390/~395), which MUST also route through the helper or they silently diverge from production |
| `zfs_ops.rs` | `build_bpool_create_command` `bpool {}p3` (~200) |
| `system_setup.rs` | `build_crypttab_entry` p4 fallback, both branches (~46/~51); `choose_esp_partition` p1 fallback (~60); `setup_luks_key_in_chroot` p4 (~573); `setup_tpm2_firstboot_enrollment` p4 fallback (~884); doc comment `${DISK}p1` (~66) updated alongside |
| `installer.rs` | `build_next_commands_after_storage` p1 esp (~566); the crypttab shell line at ~605 embeds `{d}p4` TWICE in one string — a naive per-`format!` sweep misses the second occurrence |

**Excluded:** `src/utils/qemu.rs:118` (`format!("{}p1", loop_device)`) — `/dev/loopN` ends in a digit; already correct. **Fail-closed semantics:** the helper is a pure function; on an empty basename it degrades to the no-`p` branch (`format!("{disk}{n}")`), and unit tests pin `nvme0n1`/`md126`/`sda`/`vda`/`loop0` plus a bare (non-`/dev/`) name. The 4 tests asserting `sdapN` flip to `sdaN` in the same commit and become the regression tests. End state: `grep -rn '}p[1-4]' src --include='*.rs'` matches ONLY `src/utils/qemu.rs`.

### C2. Disk autodetection (`src/cli/commands.rs::detect_primary_disk`, ~635) — TASK-02

Runs `lsblk --json --bytes -o NAME,TYPE,SIZE` (via the existing command-execution seam so tests can inject fixture JSON), deserializes into `LsblkOutput`, applies the selection rule in the data-model comment, and returns `/dev/<name>`. **Default on zero candidates: hard error**, not a guessed `/dev/sda` — hard rule 2 says `disk_device` is read from the live target, never guessed. Fixture tests: nvme-only host, md-over-sda host (must pick `/dev/md126`, not `/dev/sda`), loop/rom-only host (error).

### C3. Network autodetection (`src/cli/commands.rs::detect_network_config`, ~654) — TASK-03

Replaces the hardcoded `("eth0", "dhcp", "auto")` tuple: parse `ip -j route` to find the default route's `dev` and `gateway`, then `ip -j addr` for that interface's global IPv4 `local`/`prefixlen` → returns (iface, `addr/prefix` CIDR, gateway). **Fail-open to `dhcp`:** if no global address or no default route is found, return the detected (or first non-loopback) iface with address `"dhcp"` and empty gateway — which C4 renders as valid `dhcp4: true` netplan. This only affects the legacy no-config path; the `--config` path is untouched. Fixture tests on captured `ip -j` output.

### C4. Netplan renderer + DHCP rendering (`system_setup.rs::setup_network_configuration` ~193, `config.rs`) — TASK-04

`renderer:` line comes from `config.netplan_renderer` (**default `networkd`** — output byte-identical to today for existing configs). When `config.network_address == "dhcp"` (case-insensitive), emit `dhcp4: true` and omit `addresses:`/`routes:`/`nameservers:` static blocks; otherwise render exactly as today. Unit tests assert both renderer strings and the dhcp4 branch.

### C5. LUKS keyfile (`disk_ops.rs::setup_luks_encryption` ~333, `installer.rs::setup_installation_variables` ~470) — TASK-05

Replace both `echo '{passphrase}' | cryptsetup ...` pipelines (~340 luksFormat, ~348 open) with: create keyfile at a tmp path with mode 0600 *before* content lands (the `install -m 0600` + write pattern proven by Tang enrollment at `system_setup.rs:~660`), run `cryptsetup luksFormat --batch-mode --key-file <path> <part4>` and `cryptsetup open --key-file <path> <part4> luks`, then `shred -u <path>` — including on the failure path (shred before returning the error). Delete the `"LUKS_KEY"` tuple (~483) from `setup_installation_variables` — the scout confirmed the `export` runs as a separate one-shot `runner.execute()` and never persists to later commands; downstream reads `config.luks_key` directly, so removal is behavior-neutral. **Residual (accepted):** the command string transits the SSH channel like every other command — identical exposure class to the existing Tang enrollment; the fix removes the `ps`-visible cryptsetup pipeline and the `/proc/<pid>/environ` copy. Hard rule 4 holds: no real secret ever lands in git; committed configs carry `REPLACE_AT_PLACE_TIME`.

### C6. Config schema hardening (`config.rs`) — TASK-06

`#[serde(deny_unknown_fields)]` on `InstallationConfig` + three tests: round-trip `examples/configs/install/unimatrixone.yaml` (deserialize → serialize → deserialize → equal), a second example YAML if present, and an unknown-key YAML that MUST fail to parse. Ordering note: TASK-06 lands in wave 1, TASK-04's new field lands in wave 2 *with* `#[serde(default)]`, so `deny_unknown_fields` never breaks an existing YAML (they contain no extra keys — scout-verified 1:1 match).

### C7. curtin in-target compatibility (`installer.rs`, `cli/commands.rs`) — TASK-07

Additive detection: a helper determines "already inside the target chroot" (curtin `in-target` execution) — e.g. root-device comparison à la `ischroot` and/or curtin's own environment markers; the brief pins the exact check. When true: skip disk/mount/debootstrap work and run post-install configuration only (the `configure_*_in_chroot` family already operates on an existing tree). When false: behavior is bit-for-bit today's. **Anti-over-suppression:** a test must prove the bare path does NOT trigger the skip.

### C8. Path split documentation (`docs/architecture-path-split.md` NEW, `README.md` pointer) — TASK-08

Defines Path A (`src/autoinstall/` — subiquity render/place/verify pipeline, golden tests, STILL LIVE) vs Path B (`src/network/ssh_installer/` — the proven 7-phase installer), which flows use which, and the guardrail: neither path is removed without an explicit operator decision recorded in this doc.

## Migration / integration

Mechanical Before/After for the C1 sweep (every site follows this shape):

```rust
// Before (zfs_ops.rs ~200):
format!("... bpool {}p3 ...", config.disk_device)
// After:
let p3 = partition_path(&config.disk_device, 3);
format!("... bpool {p3} ...")
```

```rust
// Before (disk_ops.rs ~340, TASK-05 + TASK-01 combined shape):
format!("echo '{}' | cryptsetup luksFormat --batch-mode {}p4", config.luks_key, config.disk_device)
// After:
format!("cryptsetup luksFormat --batch-mode --key-file {keyfile} {p4}")
```

Test assertions migrate `sdapN` → `sdaN` (`zfs_ops.rs:381`, `system_setup.rs:1001/1026/1032`). Wave-2/3 tasks (TASK-03/04/05/07) rebase onto the merged wave-1 helper and MUST call `partition_path` for any partition path they touch. Exact line numbers get re-verified by each brief's anchor greps — never trusted blind.

## Milestones

- **M1 — Correctness + schema base (wave 1).** TASK-01 (helper sweep + test corrections), TASK-02 (lsblk json), TASK-06 (deny_unknown_fields), TASK-08 (docs). Additive/transform with tests; no phase-order changes.
- **M2 — Security + network (wave 2).** TASK-05 (LUKS keyfile — wipe-adjacent, Opus-class, review-critical), TASK-03 (ip -j parsing), TASK-04 (renderer + dhcp4). TASK-04/05 rebase onto M1's helper.
- **M3 — curtin compatibility (wave 3).** TASK-07, the ONE task adding a skip path to phase sequencing — gated by the in-chroot check defaulting to **false** (bare runs unchanged), with an anti-over-suppression test.

Each milestone is independently shippable; nothing changes default behavior except the intended bug fixes (suffix, dhcp4-instead-of-invalid-YAML, keyfile-instead-of-echo).

## Files modified

| File | Change | Task |
|---|---|---|
| `src/network/ssh_installer/partitions.rs` | NEW — `partition_path` + unit tests | 01 |
| `src/network/ssh_installer/mod.rs` | `pub mod partitions;` | 01 |
| `src/network/ssh_installer/disk_ops.rs` | route 4 prod + 2 test-builder sites through helper | 01, 05 |
| `src/network/ssh_installer/zfs_ops.rs` | bpool p3 site + fix sdap3 test | 01 |
| `src/network/ssh_installer/system_setup.rs` | 5 sites + doc comment + 3 sdapN tests; renderer/dhcp4 branch | 01, 04 |
| `src/network/ssh_installer/installer.rs` | 2 sites (605 has p4 ×2); drop LUKS_KEY export; in-chroot skip | 01, 05, 07 |
| `src/network/ssh_installer/config.rs` | `deny_unknown_fields` + tests; `NetplanRenderer` field | 06, 04 |
| `src/cli/commands.rs` | `detect_primary_disk` json; `detect_network_config` parse; in-target wiring | 02, 03, 07 |
| `docs/architecture-path-split.md` | NEW — Path A/B split + guardrails | 08 |
| `README.md` | pointer to the split doc | 08 |

## Testing

Gate for every task: `cargo test --lib --offline` (baseline **237 passed**, expect 237+ after) + `cargo build --offline` + `cargo clippy --offline` for code tasks.

| Test | Asserts |
|---|---|
| `partition_path` unit tests (new) | nvme0n1→p3, md126→p3, sda→3, vda→3, loop0→p1, bare name; empty-basename fallback |
| `test_build_bpool_create_command_has_expected_flags` (fixed) | ends_with `/dev/sda3` (was `sdap3`) |
| `test_choose_esp_partition_falls_back_when_empty` + 2 crypttab tests (fixed) | `/dev/sda1` / `luks /dev/sda4` (was `sdapN`) |
| lsblk fixture tests (new) | md-over-sda picks `/dev/md126`; nvme-only picks nvme; loop/rom-only errors |
| `ip -j` fixture tests (new) | default-route iface + CIDR + gateway; no-default-route → dhcp fallback |
| netplan render tests (new) | `renderer: networkd` default; `renderer: NetworkManager` when configured; `dhcp4: true` branch valid |
| LUKS command-builder tests (new/updated) | no passphrase substring in any built command; `--key-file` present in luksFormat AND open; shred on failure path |
| YAML round-trip + unknown-key tests (new) | example YAML round-trips equal; unknown key FAILS parse |
| in-chroot anti-over-suppression (new) | bare (non-chroot) run does NOT skip mounts/debootstrap |

## Failure modes

- **Helper inverts a case** (e.g. treats `md126` as no-`p`): mkfs/cryptsetup hit a nonexistent node → loud ENOENT in Phase 2, no silent wrong-device write; the five-device unit-test matrix pins every naming family. Note: `detect_esp_partition_path`'s GUID detection self-heals ESP paths in phases 4–6 and can MASK the bug — only Phase 2/3 on a virtio VM proves it end-to-end (that proof is `testing-gates/TASK-01`).
- **`deny_unknown_fields` rejects a real deployed YAML:** examples are 1:1 today (scout-verified); TASK-04's field carries `#[serde(default)]`; failure is a loud parse error at load, never mid-install.
- **Keyfile left behind on error:** C5 shreds on both success and failure paths; the keyfile lives on the target being installed (about to be wiped in the normal flow) — residual risk bounded.
- **dhcp4 branch misfires on a static config:** the branch triggers only on the exact string `dhcp`; static CIDRs render byte-identical to today (asserted by tests).
- **In-chroot detection false positive** (containers, mount namespaces): would skip debootstrap on a bare run — the anti-over-suppression test plus a conservative check (must ALSO see curtin/target markers, pinned in the brief) guard this; default is always "not in chroot".
- **Autodetect misreads lsblk/ip JSON:** zero-candidate → hard error (disk) / dhcp fallback (network); both stubs are only on the legacy no-`--config` path — the proven `--config` flow never calls them.

## Rollback

Every task is a single conventional commit on its own `agent/installer-robustness-<slug>` branch; `git revert <sha>` restores the prior state cleanly (the helper is a pure additive module; reverting TASK-01 restores the old inline strings AND the old sdapN assertions together, keeping tests green either way). TASK-04's renderer field and TASK-07's skip path are dormant-by-default (serde default / false-by-default detection) — reverting or simply not setting them yields today's behavior. TASK-05 has no config surface; revert restores the echo pipeline (accepted only as an emergency rollback — the security fix is the point). No live-server rollback exists because no task touches a live server (hard rule 1).

## Open questions (resolved — recorded for the plan)

1. ~~Where does the helper live?~~ → NEW `src/network/ssh_installer/partitions.rs` (LOCKED; per-file helpers and a config method rejected).
2. ~~Do the test builders also switch?~~ → Yes — `build_mkfs_esp`/`build_mkfs_reset` route through the helper, or they silently diverge from production.
3. ~~Is `utils/qemu.rs:118` in the sweep?~~ → No — loop devices end in a digit; excluded (LOCKED).
4. ~~Keep the LUKS_KEY env export but sanitize?~~ → No — export is inert (one-shot execute) and a leak channel; removed (LOCKED).
5. ~~Is removing Path A in scope?~~ → No — Path A is live; TASK-08 documents the split (LOCKED).
