<!-- file: changelog.d/vm-gate-marker-72-81-resolution.md -->
<!-- version: 1.0.0 -->
<!-- guid: 5e8c2a17-4d63-4f92-b1a7-8c3d0f6e2b54 -->
<!-- last-edited: 2026-07-30 -->

### Fixed

#### VM-gate markers :72 and :81 reported false gaps and hid a real finding

`scripts/vm-validate.sh` stage 3 resolves two VERIFY-ON-VM markers from
`build-installer-image.sh`. Both were misreporting.

**Marker :72 (stock-installer autostart unit).** Three defects, all in the
verdict loop:

1. The verdict was *assigned* rather than accumulated, so only the **last**
   offending unit survived. A run observing both
   `snap.subiquity.subiquity-service.service` and `subiquity_config.mount`
   reported only the mount — the real finding was silently discarded by the
   noise one.
2. Every observed unit was treated as maskable, including `.mount` and
   `.device` units. Only unit types that can *start* something are relevant;
   masking a snap `.mount` would in fact break the snap.
3. Presence was treated as autostart. `systemctl list-units --all` includes
   inactive units and `list-unit-files` lists units that merely exist on
   disk, so a name match answered "does a subiquity unit exist?" when the
   marker asks "does one **autostart the installer**?"

Stage 3 now queries `is-enabled`/`is-active` per candidate service and flags
only autostart-capable, unmasked services, listing **all** of them. The
report gained a `service-states (enabled/active)` line so the evidence for
the verdict is visible rather than implied.

**Marker :81 (live-rootfs tools).** The snapshot was taken before the
install, but the agent apt-installs `clevis`/`clevis-luks`/`clevis-tpm2` into
the live environment *during* it
(`crates/uaa-core/src/network/ssh_installer/packages.rs:34-55`), so
`debootstrap`/`zpool`/`clevis` always reported MISSING on a perfectly healthy
run. The tools are now re-interrogated after stage 4 while the live
environment is still up, and reported as `pre -> post`; a tool the agent
provisions reads `MISSING -> present (provisioned by agent during install)`.
Only a tool still missing after the install is a genuine gap.

Report-only in both cases — neither changes the PASS/FAIL gate result.
