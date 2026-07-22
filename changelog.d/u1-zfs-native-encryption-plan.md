### Added

#### Plan: U1 ZFS native encryption + keystore-zvol migration (design + phased build plan)

Adds `docs/specs/u1-zfs-native-encryption-design.md` and
`u1-zfs-native-encryption-plan.md` — the planning package (no implementation)
for migrating unimatrixone from plain ZFS-on-LUKS to ZFS **native encryption**
on the Ubuntu stock keystore-zvol layout, across a 4-drive topology (2×16 GB
Optane as boot + a mirrored metadata `special` vdev; 2 large SSDs as the bulk
data mirror). Encodes the settled decisions: the VM-gate-validated D2-B unlock
policy (clevis SSS `t=2` over 3 thumbprint-pinned Tang + a TPM2 peer-share,
`enroll_tpm2:false`, fatal Tang bind, verify guard), Secure Boot with the
Canonically-signed ZFS module (no DKMS/MOK, PCR 7 binding), the D7 hardening
(keystore-wait dracut hook, two synced ESPs), and a `storage_mode`
discriminator that keeps the existing Lenovo plain-LUKS path byte-identical.
The plan is gated: every phase boot-proves on the QEMU/swtpm VM gate with
Secure Boot on before the next, and U1 power-on is a separate operator
checkpoint.
