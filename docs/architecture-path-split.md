<!-- file: docs/architecture-path-split.md -->
<!-- version: 1.0.0 -->
<!-- guid: 023a3d1b-689f-4ea7-8422-098b7416d681 -->
<!-- last-edited: 2026-07-10 -->

# Architecture: Path A vs Path B

The ubuntu-autoinstall-agent provides two installer code paths to support different deployment workflows and requirements. **Path A** (`src/autoinstall/`) renders cloud-init/subiquity autoinstall user-data from host specifications, places it on netboot servers, and verifies installed hosts post-deployment. **Path B** (`src/network/ssh_installer/`) is a proven 7-phase direct installer that handles partitioning, LUKS encryption, ZFS pool creation, debootstrap, and system configuration without relying on subiquity/curtin. This document explains the architecture of each path, their intended use cases, and the guardrails that govern future development.

## Path A — subiquity render pipeline (src/autoinstall/)

Path A consists of three main phases:

1. **Render** (`render.rs`): Generates cloud-init/subiquity autoinstall user-data from structured host specifications, transforming declarative host specs into user-data documents that subiquity and curtin consume.
2. **Place** (`place.rs`): Deploys the rendered user-data to netboot infrastructure so provisioning servers can serve it to installing hosts.
3. **Verify** (`verify.rs`): Post-installation checks that validate the resulting system configuration against expected specifications.

The actual OS installation—partitioning, storage management, package installation—is performed by subiquity and curtin (the Debian/Ubuntu installers) from the rendered user-data. Path A never directly executes cryptsetup, zpool, or related storage commands; it provides the configuration that curtin consumes.

### The LVM Defect and Regression Guard

Historically, subiquity/curtin's storage layout pipeline produced a LUKS+LVM+ext4 filesystem arrangement instead of the required ZFS-on-LUKS topology. The function `evaluate_no_lvm()` in `src/autoinstall/verify.rs` acts as a standing regression guard, detecting LVM presence and reporting failure when the expected ZFS-on-LUKS layout is not present. This defect is the primary motivation for the existence of Path B.

### Live Golden Tests

Path A remains live and load-bearing. The golden-fixture test suite (`GOLDEN_001`, `GOLDEN_002`, `GOLDEN_003` in `src/autoinstall/render.rs`) validates that the render pipeline produces correct user-data against real-world host specifications. These tests are part of the 237-test lib suite and run on every build. The render, place, and verify functionality remain in operational use.

## Path B — direct ZFS-on-LUKS installer (src/network/ssh_installer/)

Path B is a complete, 7-phase installer implemented entirely in Rust that handles end-to-end provisioning without relying on subiquity/curtin. It executes over SSH (`ssh-install` subcommand), locally (`local-install`), or through the unified `install` interface.

### Phases

0. **Variables & Validation**: Initialize environment, parse host specs, validate prerequisites.
1. **Packages**: Install base system packages required for subsequent phases.
2. **Disk Preparation**: Partition the target disk, create LUKS-encrypted container.
3. **ZFS Pools**: Create boot pool (bpool) and root pool (rpool) with appropriate compression and property settings.
4. **Debootstrap**: Bootstrap the base Ubuntu system into the ZFS root filesystem.
5. **System Configuration**: Install and configure GRUB bootloader, crypttab, dracut for initramfs generation, and Tang unlocking integration.
6. **Cleanup**: Finalize the installation, verify integrity, prepare system for first boot.

### Proven Status

Path B has successfully completed all 7 phases on unimatrixone (Supermicro X10DSC+) hardware as of 2026-07-09. It is the recommended and tested approach for new ZFS-on-LUKS deployments.

## When to Use Which

| Task | Path | Subcommand(s) |
|------|------|---|
| Render cloud-init/subiquity user-data from host specs | A | `render-user-data` |
| Place user-data on netboot infrastructure | A | `place` |
| Verify installed system post-deployment | A | `verify` |
| Install or reinstall a machine with ZFS-on-LUKS | B | `ssh-install`, `local-install`, `install` |

Path A is appropriate for workflows where you are seeding autoinstall via netboot and need to generate configuration from specifications; Path B is appropriate for direct machine provisioning that requires ZFS-on-LUKS storage.

## Guardrails

The following principles govern future development and prevent regressions:

1. **New install-execution logic goes to Path B only.** If adding storage management, system setup, or provisioning execution code, implement it in `src/network/ssh_installer/`. Do not add storage-execution code to Path A; it is a configuration generation and verification layer, not an executor.

2. **Path A remains for render, place, and verify.** Do not remove or significantly refactor Path A's render/place/verify pipeline. It is load-bearing for golden-fixture tests and operational verification workflows. Do not repurpose it for storage operations or system provisioning execution.

3. **Path A removal is NOT planned.** Its golden tests and `verify` command are live and required to remain. No timeline or plan for Path A removal exists. Both paths coexist indefinitely.

No curtin logic exists in the Rust codebase; curtin integration is a one-way relationship where Path A generates user-data that curtin consumes.

