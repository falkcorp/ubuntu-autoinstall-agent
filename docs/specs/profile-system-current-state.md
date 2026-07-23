<!-- file: docs/specs/profile-system-current-state.md -->
<!-- version: 1.0.0 -->
<!-- guid: b48b4cbc-ab5b-4d63-bfa8-a9d68bf73d30 -->
<!-- last-edited: 2026-07-23 -->

# Current State: Provisioning, Profiles, and the Installer

## 1. End-to-end flow today

A host gets provisioned through one of two independent pipelines that share almost nothing:

**Live/production pipeline (SSH installer):**
1. An ISO or netboot image is built from stock Ubuntu Server (`scripts/make-ssh-ready-iso.sh`, Rust port `crates/uaa-core/src/iso/remaster.rs:126-160`). It patches GRUB to add `ds=nocloud;s=/cdrom/nocloud/`, pointing at a fixed seed dir (`installer-image/nocloud/`). No per-host config is baked into the image.
2. On boot, `installer-image/nocloud/uaa-usb-bootstrap.sh:40-182` (USB) or `installer-image/uaa-autoinstall.sh:23-125` (netboot) fetches a static agent binary and resolves the per-host `InstallationConfig` YAML — either via an explicit `uaa.config=` cmdline URL or a MAC-resolved endpoint served by the machine plane (`crates/uaa-control/src/machine_plane/seeds.rs:83-99`, pure filesystem lookup by hex MAC, no registry awareness).
3. `uaa install --config` runs the 7-phase installer (`crates/uaa-core/src/network/ssh_installer/installer.rs`), which does disk prep, LUKS/ZFS setup, unlock enrollment, base system install, and application install, driven entirely by the flat `InstallationConfig` struct.

**Config placement (the bridge from profile to served file):** `uaa config place --from-registry` (`crates/uaa/src/cli/config.rs:251-285`) resolves a host through the group/profile registry (`uaa_control::resolve_from_registry`) into an `InstallationConfig`, then writes it via `uaa_core::config_place::place_configs` to the same `<webroot>/<hexmac>/uaa.yaml` destination the machine plane serves. This is the one genuine link between "profile" and "what actually gets installed" today — but it's gated by a hardcoded 4-entry `KNOWN_HOSTS`/`mac_for_host` table (`crates/uaa-core/src/config_place.rs:51-79`), duplicated a third time in `scripts/deploy-usb-configs.sh:84-93`. None of these tables include any `rpi-serv` host.

**Legacy/parallel pipeline (dead-ish but still wired):** `crates/uaa-core/src/autoinstall/templates/len-serv.user-data.tmpl` is a raw cloud-init/subiquity template with exactly 4 substitution tokens (`HOSTNAME`, `NET_ADDRESS`, `COCKROACH_ADVERTISE`, `COCKROACH_JOIN` — `crates/uaa-core/src/autoinstall/render.rs:41-46`); everything else (packages, users, dracut/clevis logic, Tang binding, Cockroach setup) is hardcoded prose in the bash heredoc, golden-fixture tested (`tests/fixtures/golden/len-serv-00{1,2,3}.user-data`). It's driven by `HostSpec::for_lenserv()` (`crates/uaa-core/src/autoinstall/host_spec.rs:60`) via the `uaa place`/`uaa render-user-data`/`uaa verify` CLI trio (`crates/uaa/src/cli/commands.rs:1036-1163`) — a completely separate config shape from `InstallationConfig`, hardcoded to the len-serv host family only, with no unimatrixone or rpi equivalent. It's unclear whether this path is still load-bearing.

A **third, apparently dead** pipeline (`uaa create-image`/`uaa deploy`, `ImageBuilder`/`ImageDeployer`/`ImageCustomizer`, `crates/uaa-core/src/image/deployer.rs`, `crates/uaa-core/src/config/target.rs`) still exists in the CLI (`crates/uaa/src/cli/args.rs:29-91`) but uses a fourth config shape (`TargetConfig`), has no ZFS/clevis/Tang/dracut support, and `ImageCustomizer::customize_image` is an explicit no-op stub (`crates/uaa-core/src/image/customizer.rs:20-35`).

## 2. The existing partial profile/registry layer

There is a real, working (if partial) profile system in `uaa-control`:

- `HostGroup` (defaults) + `HostProfile` (per-host overrides) + hostname allocations + drift review, in `uaa-control/src/profiles/{store,resolve,reify,convert,alloc,drift}.rs`.
- `InstallationConfigPartial` + `merge.rs` (`crates/uaa-core/src/profile/{mod,merge}.rs`) implements group-default → host-override merging, producing a full `InstallationConfig`. This partial type mirrors **every** `InstallationConfig` field 1:1 (`crates/uaa-core/src/profile/mod.rs:41-78`) — concrete evidence the merge layer has no notion of components; it's the same flatness one level up.
- `HostProfileRow`/`HostGroupProfileRow` (`crates/uaa-control/src/db/mod.rs:367-378`) persist this as opaque overrides/applications JSON blobs, with no architecture or role field.
- `register_from_config` (`uaa-control/src/profiles/reify.rs`) reifies an `InstallationConfig` back into the registry (DS-OPS-05 work), and drift-scan/review routes (`DS-REG-05`, `DS-OPS-02`, PRs merged as of `38f87be`) compare deployed state against registry state.

This layer is real and actively being extended (see git log: drift scan, staleness checks, operator routes). It is the natural home for new component types — it should not be duplicated by a third config-layering mechanism.

## 3. The installer: standalone action-modules over one monolithic config

`InstallationConfig` (`crates/uaa-core/src/network/ssh_installer/config.rs:78-173`) is a single flat, `deny_unknown_fields` struct covering every provisioning axis: storage mode, disk roster, unlock policy (Tang/TPM2/FIDO2), network, base image, initramfs type, applications, and secrets — all as sibling scalar/vec fields.

The installer itself is a **fixed 7-phase orchestrator** (`installer.rs`) with no plugin/trait abstraction. Each phase dispatches to a self-contained "action-module" struct that all share the same shape — `struct { runner: &mut dyn CommandExecutor }` + `async fn(&mut self, config: &InstallationConfig)` (e.g. `disk_ops.rs:15-27`, `applications.rs:46-54`, `reset_partition.rs:111-119`):

| Module | File | Responsibility |
|---|---|---|
| `DiskManager` | `disk_ops.rs` | PlainLuks single-disk layout |
| `DiskNativeManager` + `layout.rs` | `disk_native.rs` | NativeKeystore multi-disk ZFS layout (pure planner → applier) |
| `ZfsManager` / `ZfsNativeManager` | `zfs_ops.rs` / `zfs_native.rs` | pool creation |
| `SystemConfigurator` | `system_setup.rs` | dracut/clevis/Tang/TPM2/serial-console/GRUB |
| `ApplicationInstaller` | `applications.rs` | workload install (Cockroach only) |
| `PackageManager` | `packages.rs` | base package set |
| `ResetPartitionStager` | `reset_partition.rs` | recovery ISO/tarball staging (always runs, no opt-out) |

Branching between the two storage strategies is a hardcoded `match config.storage_mode` in `installer.rs:891-919`. Below that, dozens of scattered field checks drive behavior: `if !config.tang_servers.is_empty()`, `if config.enroll_tpm2`, `if config.initramfs_type == Dracut`, and — critically — a firmware quirk (IMSM/mdraid) is inferred by **string-prefix sniffing** `config.disk_device.starts_with("/dev/md")` at three call sites (`disk_ops.rs:465-474`, `system_setup.rs:452-460,1017`) rather than being a declared field.

`ApplicationSpec` is the one part of the schema already built as a real, extensible component: a closed, tagged, `deny_unknown_fields` enum (currently just `Cockroach`), explicitly designed as closed-but-growing per spec Decision 15 (`config.rs:40-49`). It's the template other axes (unlock policy, disk layout, firmware quirks) should imitate.

There is **no pre/post-exec hook mechanism anywhere**. Any host-specific behavior not already one of the fixed phases must be hardcoded into a phase function or added as a new `StorageMode`/`ApplicationSpec` variant — requiring a Rust change and recompile for what should be a config change.

## 4. len-serv / U1 / rpi: what actually differs

**len-serv (amd64, CockroachDB, PlainLuks)** — `examples/configs/install/len-serv-00{1,2,3}.yaml` are byte-identical except hostname/IP/guid/date. `DiskManager::create_partitions` (`disk_ops.rs:293-360`) always builds the same fixed 4-partition GPT (ESP 512M / RESET 4G / BPOOL 2G / LUKS remainder) with literal sgdisk offsets — no config surface at all. Unlock policy is Tang-only (2-of-3, `172.16.2.45/46/47`) + TPM2+PIN, identical across all three hosts. CockroachDB cluster membership is derived from a hardcoded Rust constant `LENSERV_MEMBER_IPS` (`applications.rs:190-198`), explicitly flagged in-code as a stopgap pending the profile system. Notably, **no committed len-serv YAML actually sets `applications: [cockroach]`** today (`config.rs:325`, host YAMLs all omit it) — Cockroach in production is deployed through the *other*, legacy template pipeline, not through `ApplicationSpec` at all.

**U1 / unimatrixone (amd64, Supermicro X10DSC+, NativeKeystore)** — `unimatrixone.yaml:26-38` uses a 4-disk by-id roster (2 SSD `role: system`, 2 Optane `role: special`) via `StorageMode::NativeKeystore` + `DiskSpec`/`DiskRole` (`config.rs:198-221`), applied by a genuinely separate code path (`layout.rs` pure planner → `disk_native.rs` applier — the one component that already has a real planner/applier split). Its unlock policy is D2-B: Tang SSS (2-of-3, same servers as len-serv) plus a TPM2 peer share baked directly into the clevis SSS JSON via a bool param `include_tpm2_peer` threaded from `storage_mode` (`system_setup.rs:862-896`) — i.e. "NativeKeystore implies tpm2 peer" is hardcoded, not an independent selection. `enroll_tpm2`/`expect_fido2` are both off (`unimatrixone.yaml:81-83`) since the TPM2 factor is a clevis pin, not `systemd-cryptenroll`. The board's firmware quirk — X10DSC+ cannot boot from NVMe — is expressed only as YAML comments and a design-doc link (`unimatrixone.yaml:16-21`) plus a hardcoded disk-role assignment in `layout.rs:298-314`; there is no queryable "firmware quirk" field anywhere.

**rpi-serv (arm64, Tang servers)** — Zero representation. `InstallationConfig` has no architecture field at all; the live installer's chroot commands (dracut, GRUB, TPM2) are implicitly x86-oriented throughout `system_setup.rs`. A separate, largely vestigial `Architecture` enum exists (`crates/uaa-core/src/config/mod.rs:20-27`, `config/target.rs:15-16`) but only feeds the legacy `TargetConfig`/ISO-builder/VM-test-harness path (arm64 ISO URLs, `qemu-system-aarch64` selection) — it is not wired to `InstallationConfig` or the profile system. There is no `ApplicationSpec::TangServer` variant, no watchdog code/docs/scripts anywhere in the repo (verified by full-repo grep), and RPi provisioning (Tang cold-start, backup/restore, register-len-server.sh) happens entirely outside this repo via hand-run scripts on the server. Prior planning docs (`docs/deploy-system/00-ROADMAP.md:60`, `docs/specs/deploy-system-design.md:57`, `docs/agent-tasks/BREAKDOWN-2026-07-16.md:132`) all explicitly deferred RPi as out of scope — "covering rpi" is greenfield design work, not an extraction from existing code.

## 5. ISO / deploy / VM-gate pipeline

- **ISO build**: `iso::remaster` (NoCloud seed injection) and `iso::image_build` (netboot squashfs overlay — unsquash, inject static agent + `uaa-autoinstall.service`, mask subiquity units via a hardcoded `MASK_UNITS` const array, `image_build.rs:37-51`) are the two live image builders. Neither bakes per-host config; base OS release/mirror for the *target* rootfs is chosen later, inside `InstallationConfig.debootstrap_release/mirror` (`config.rs:93-97`), completely decoupled from which ISO/squashfs was built. Live-session SSH credentials (one operator key + throwaway password) are hardcoded in `installer-image/nocloud/user-data:24-41` for every host/arch.
- **Deploy**: `scripts/server-deploy.sh` handles the control plane's own git-pull/build/restart cycle (not fleet provisioning). `scripts/arp-discovery-scan.sh` is a fully host-agnostic passive discovery scanner.
- **VM gate**: `scripts/vm-validate.sh` is a strong, mostly-generic pass/fail harness (LUKS unlock, rpool/bpool import, multi-user boot) driven by whatever `InstallationConfig` is passed via `--config`. But stage 6's final readiness probe is hardcoded to CockroachDB (`cockroach sql ... SELECT 1`, `vm-validate.sh:534-556`) — it cannot gate a non-Cockroach profile (a bare Tang/rpi image, or any future application type) without editing the script.

## 6. Candid list of one-off pain points motivating componentization

1. **Disk layout has no schema for PlainLuks.** The len-serv 4-partition GPT is Rust-hardcoded literals (`disk_ops.rs:293-360`); only NativeKeystore has a real planner (`layout.rs`). Changing a partition size for one host means editing Rust.
2. **Unlock policy is 6 independent flat fields**, not a composed type (`config.rs:98-130`). Tang/TPM2/FIDO2 combinations that should be orthogonal are actually coupled through `storage_mode` (the `include_tpm2_peer` bool derived from `NativeKeystore`, `system_setup.rs:862-896`).
3. **Firmware/hardware quirks are inferred, not declared.** IMSM/mdraid detection via `disk_device.starts_with("/dev/md")` (3 call sites); NIC driver forced into initramfs by live-probing `/sys/class/net/.../driver` on the *installer* environment (`system_setup.rs:1030-1052`), not the target hardware; serial console and UEFI boot-order rewriting applied unconditionally to every host with no opt-out (`system_setup.rs:19-32,582-595`).
4. **CockroachDB cluster membership is a hardcoded constant** (`LENSERV_MEMBER_IPS`, `applications.rs:190-198`) instead of derived from group/profile membership — explicitly flagged in-code as a stopgap.
5. **No pre/post-exec hook mechanism exists.** Every host-specific command becomes new hardcoded phase logic or a new enum variant, requiring a recompile.
6. **RESET-partition recovery staging is unconditional** for every host, no config gate (`installer.rs:939-948`).
7. **Base image selection is scattered literals**: debootstrap release/mirror have in-code defaults, the old-releases fallback URL and `uaacache` tarball convention are fully hardcoded strings (`system_setup.rs:209-248`), not one addressable component.
8. **Three independently-maintained host/MAC tables** must agree by hand: `KNOWN_HOSTS`/`mac_for_host` in `config_place.rs:51-79`, the case statement in `deploy-usb-configs.sh:84-93`, and the profile registry's own allocation state. None include rpi hosts — adding any new host requires editing two hardcoded lists in two languages, exactly the "rebuild one-offs per change" pattern to eliminate. Even registry-resolved hosts still get refused by `place_configs` unless they're *also* in the flat MAC table (`config_place.rs:433-441`), a self-documented gap.
9. **Power/BMC mechanism selection lives in a totally separate, disconnected table** (`PowerMechanism` enum + `lookup_host`, `crates/uaa-core/src/power/mod.rs:53-107`) — per-host hardware metadata that belongs in the same profile/component model as disk layout and unlock policy but currently isn't merged into it at all.
10. **Two live "profile" systems encode overlapping facts independently** — `InstallationConfig`/ssh_installer YAML vs. the legacy autoinstall template — with no shared source of truth for SSH keys, Tang servers, hostname, or Cockroach params; a change in one doesn't propagate to the other.
11. **VM-gate readiness assertion isn't pluggable** per application type (hardcoded to Cockroach SQL), so it can't validate a Tang/rpi image or any future workload without a script edit.
12. **Secrets are one flat placeholder convention** (`REPLACE_AT_PLACE_TIME`, keyed by hostname in a single `SecretsFile`, `config_place.rs:91-134`) with no per-component secret typing — any new component with its own secret falls back to ad hoc placeholder naming rather than a typed slot.
