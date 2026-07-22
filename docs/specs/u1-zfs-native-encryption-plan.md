<!-- file: docs/specs/u1-zfs-native-encryption-plan.md -->
<!-- version: 1.1.0 -->
<!-- guid: f3b8e0a7-6c14-4d92-8b53-1e9a7c2f4d60 -->
<!-- last-edited: 2026-07-22 -->

# U1 ZFS Native Encryption + Keystore — Implementation Plan

Phased, VM-gated build plan for the design in
[`u1-zfs-native-encryption-design.md`](u1-zfs-native-encryption-design.md). Read
that first — the decisions, topology, threat model, and invariants live there
and are not repeated.

> **Nothing here executes.** Each phase lands as its own gated PR; every phase
> boot-proves on the QEMU/swtpm VM gate (with **OVMF Secure Boot ON**) before
> the next begins. **U1 stays powered off** until §"U1 checkpoint" — a separate,
> explicit operator action, never triggered by this plan. The `len-serv-*`
> plain-LUKS path stays intact throughout (Design Decision 9).

## How the new path is selected (no Lenovo regression)

The single most important structural decision: **do not mutate the existing
install path.** Introduce a **storage-mode discriminator** on
`InstallationConfig`:

```
storage_mode: enum { PlainLuks (default), NativeKeystore }
```

- `PlainLuks` → the current `zfs_ops.rs` / `luks_keys.rs` path, byte-identical.
  Every `len-serv-*.yaml` omits the field and gets the default → zero change.
- `NativeKeystore` → the new partitioner + pool builder + keystore + D2-B bind.
  Only `unimatrixone.yaml` sets it.

All new code is reached **only** under `NativeKeystore`. A Lenovo-config test
suite runs in every phase's gate to prove the default path is unchanged.

### These modes are profiles, not per-host YAML (machine templates)

`storage_mode` and the fields below are **profile-level attributes** in the
deploy-system registry ([`deploy-system-design.md`](deploy-system-design.md)),
not one-off keys authored per host. The two machine types are two templates:

| Template | Registry shape | `storage_mode` | Members |
|---|---|---|---|
| **Lenovo** | HostGroupProfile `len-serv` | `PlainLuks` (group default) | `len-serv-001/002/003` (indexed, inherit) |
| **U1** | standalone HostProfile `unimatrixone` | `NativeKeystore` (host) | `unimatrixone` |

The profile merge (`crate::profile::merge`, group defaults + host overrides)
already produces `InstallationConfig`; adding these fields to the profile
partial types is all that's needed to make a template carry its storage mode.
Both profiles **already exist in the registry** (the 2026-07-21 backfill
reified `len-serv-001/002/003` + `unimatrixone`). Consequence: **`len-serv-003`
needs none of this work** — it resolves to `PlainLuks` and deploys via the
existing path today (`resolve profile → config place → PXE`); this migration
only adds the `NativeKeystore` branch the U1 profile selects.

Also add (design §3, §6) — as profile-mergeable fields:
- `TangServer { url, thp: Option<String> }` — thumbprint pinning (Decision 6).
  `thp` optional in the type, **required by validation** when
  `storage_mode = NativeKeystore`.
- `tpm2_sss_peer: bool` (default false) — the clevis `tpm2` peer share
  (Decision 4). Distinct from `enroll_tpm2` (the dropped systemd path).
- `disks: Vec<DiskRole>` for the multi-disk layout (data/special/boot roles,
  by-id) — the partitioner stops assuming a single `disk_device`.

## Phases

Each phase: **goal · files · gate**. "Gate" = the boot-proof/tests that must
pass before merge and before the next phase.

### Phase 1 — Multi-disk partitioner (drop single-disk / IMSM)

- **Goal:** partition N disks by role (ESP+bpool+special on Optane; whole-disk
  data on SSD), by-id, per the design table. Retire the single `disk_device`
  and the `mdadm`/IMSM `/dev/md/Volume0_0` special-casing
  (`system_setup.rs:403-411`, `818-835`) *for the NativeKeystore mode only*.
- **Files:** `network/ssh_installer/partitions.rs`, `config.rs` (the `disks`
  vec + `storage_mode`), a new `layout.rs` computing partition sizes.
- **Gate:** unit tests for the partition plan (both Optane symmetric, both SSD
  whole-disk, two ESPs); Lenovo-config tests still green; `cargo test/clippy`.
  No VM yet (no bootable artifact) — partition-plan assertions only.

### Phase 2 — Native pool + keystore zvol + load-key + D7.1 hook

- **Goal:** create `bpool` (mirror) and `rpool` (data mirror + special mirror,
  `encryption=on`); create `rpool/keystore` (`encryption=off`), LUKS-format it,
  write `system.key`, set `keylocation`; ship the **`91uaa-keystore-wait`**
  dracut module + the `network-online` drop-in (D7.1/D7.2).
- **Files:** new `zfs_native.rs` (parallel to `zfs_ops.rs`, selected by mode),
  `installer.rs` phase wiring, `dracut/91uaa-keystore-wait/*`, dracut/dropin
  install in `system_setup.rs`.
- **Gate:** **first VM boot-proof** — install into the QEMU 4-disk rig, reboot,
  confirm `zpool import` → keystore unlock (passphrase only, no Tang yet) →
  `zfs load-key` → root mounts, with Secure Boot ON and the signed module
  loading. Prove the D7.1 race is closed (delay the zvol; boot still succeeds).

### Phase 3 — D2-B unlock policy on the keystore LUKS

- **Goal:** bind the keystore LUKS with clevis SSS `t=2` over
  {3 Tang thp-pinned + tpm2 peer}; `enroll_tpm2:false`; make the Tang bind
  **fatal**; add the `verify` guard (D7.4).
- **Files:** `system_setup.rs` (`enroll_tang_clevis` → add the `tpm2` pin + `thp`
  + fatal error path), `luks_keys.rs` (verify guard), `config.rs` (`tpm2_sss_peer`,
  `thp`).
- **Gate:** VM boot-proof of **every §7 scenario** (all Tang up; 1 down; 2 down
  + TPM2; stolen/off-LAN stays locked; recovery key at "SOL"). Confirm the
  clevis `tpm2` token does **not** surface as a systemd token (no R4 hang) — the
  linchpin negative from the recommendation. `verify` fails a planted
  systemd-tpm2 token.

### Phase 4 — Secure Boot signed-module pinning + chain verify

- **Goal:** guarantee the signed ZFS module (`linux-modules-extra`, not dkms)
  and the signed shim/grub/kernel chain; `grub-install --uefi-secure-boot`;
  assert PCR 7 stability across a normal kernel update.
- **Files:** `system_setup.rs` (package pins + grub-install flags),
  `packages.rs`, a `verify` check that no MOK is enrolled.
- **Gate:** VM with SB ON: module loads, chain verifies, a simulated kernel
  update keeps PCR 7 stable so the TPM2 peer still unseals.

### Phase 5 — Two ESPs registered + synced (D7.3)

- **Goal:** install GRUB to both ESPs, register **both** in NVRAM, ship the
  `zz-uaa-esp-sync` unit (rsync ESP#1→ESP#2 on bootloader change).
- **Files:** `installer.rs` (dual `grub-install` + `efibootmgr`), a systemd
  unit + dpkg hook.
- **Gate:** VM: kill disk 0; firmware boots ESP #2 and the pool imports degraded
  but unlocks. ESP #2 matches #1 after a GRUB change.

### Phase 6 — Special vdev (Optane metadata mirror)

- **Goal:** add `special mirror(Optane0.p3, Optane1.p3)` with
  `special_small_blocks=0` at pool-create time (built from the start, per the
  design). Confirm metadata lands on it.
- **Files:** `zfs_native.rs` (the `zpool create` special clause).
- **Gate:** VM: `zpool status` shows the special vdev; `zpool iostat` shows
  metadata on Optane; pool faults if both Optane special members are pulled
  (proving the "loss = pool loss" invariant is understood, not a surprise).

### Phase 7 — `unimatrixone.yaml` rewrite

- **Goal:** author the 4-drive native config: `storage_mode: NativeKeystore`,
  the `disks` roles (by-id), Tang with `thp` pins, `tpm2_sss_peer: true`,
  `enroll_tpm2: false`, `expect_fido2: false`. Drop `disk_device: /dev/md/Volume0_0`.
- **Files:** `examples/configs/install/unimatrixone.yaml`.
- **Gate:** config parses; resolves through the NativeKeystore path in a dry run;
  no Lenovo config affected.

### Phase 8 — VM-gate re-validation matrix

- **Goal:** one consolidated harness run of the full matrix (below) on the exact
  stack, Secure Boot ON, before U1 is even considered.
- **Files:** `vm_validate.rs` / `scripts/vm-validate.sh` extensions; a
  `vm-test-u1.yaml` mirroring `unimatrixone.yaml` with throwaway secrets.
- **Gate:** green matrix = the software precondition for the U1 checkpoint.

## Dependency waves

```
Wave A: Phase 1
Wave B: Phase 2            (needs 1)
Wave C: Phase 3, Phase 4   (both need 2; independent of each other)
Wave D: Phase 5, Phase 6   (need 2; 6 wants 3's policy present to test faults)
Wave E: Phase 7            (needs 1,3 — the config shape)
Wave F: Phase 8            (needs all)
```

## VM-gate re-validation matrix (Secure Boot ON, swtpm)

| # | Scenario | Expect | Introduced |
|---|---|---|---|
| 1 | All 3 Tang up | unattended unlock + boot | P2/P3 |
| 2 | 1 Tang down | unattended | P3 |
| 3 | 2 Tang down, TPM2 present | unattended (TPM2 + 1 Tang) | P3 |
| 4 | Off-LAN (0 Tang), TPM2 only | **stays locked** | P3 |
| 5 | Recovery key at console (SOL sim) | attended unlock | P3 |
| 6 | clevis `tpm2` token vs systemd loop | **no R4 hang** | P3 |
| 7 | keystore zvol node delayed | D7.1 waits, boots | P2 |
| 8 | Secure Boot ON, signed zfs module | module loads | P4 |
| 9 | kernel update → PCR 7 stable | TPM2 peer still unseals | P4 |
| 10 | disk 0 dead | boots ESP #2, pool degraded-but-unlocks | P5 |
| 11 | planted systemd-tpm2 token | `verify` **fails** | P3 |
| 12 | Lenovo (`PlainLuks`) config | byte-identical to today | all |

## U1 checkpoint (out of plan scope — operator-gated)

**Only after the matrix is green**, and as a distinct operator decision:

1. BIOS: Optane bifurcation, Secure Boot on, TPM=2.0 (design §9.4).
2. Confirm U1 disk-by-id inventory matches `unimatrixone.yaml`.
3. Operator powers U1 on (quiet-hours — timing is the operator's call; fans wake
   the house). Not this plan, not autonomously.
4. Install → observe the SOL console through the D2-B unlock → verify.

## Risks & rollback

- **Native-encryption + special vdev is a new pool shape** we haven't booted on
  hardware. Mitigation: the VM gate boots the *exact* 4-disk shape first;
  Phase 6 fault-tests the special-vdev dependency explicitly.
- **Rollback is per phase** (revert the PR). Because everything new is behind
  `storage_mode = NativeKeystore`, a revert can never affect the Lenovo fleet.
- **Silent killers** (R4 hang, wiped keystore slot, systemd token drift) are
  each covered by a specific gate row (6, 11) and the `verify` guard, not left
  to reasoning.
- **Optane-as-special is permanent-ish** (`zpool remove` copies back; possible
  on mirror pools but slow). Accepted: the design commits to it from the start.
