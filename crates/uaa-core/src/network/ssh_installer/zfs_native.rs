// file: crates/uaa-core/src/network/ssh_installer/zfs_native.rs
// version: 2.3.0
// guid: bca3258c-2a81-4e7d-a50b-50128c83b2cc
// last-edited: 2026-08-03

//! ZFS **native-encryption** pool + keystore builder for
//! [`StorageMode::NativeKeystore`] (U1 / the future server profile) — Phase 3's
//! parallel to [`super::zfs_ops::ZfsManager`], selected by `config.storage_mode`.
//!
//! Builds, in the order proven on real U1 hardware:
//! 1. `bpool` = mirror of the two System (bootable SATA SSD) `p2`s —
//!    GRUB-compatible `/boot` on a disk the firmware can boot (the Optane can't).
//! 2. `rpool` = mirror(System `p3`s) `[data]` + mirror(Special/Optane `p1`s)
//!    `[special]`, **root unencrypted**, `special_small_blocks=0` (metadata only
//!    — the data pool is SSD, so no small-file offload is worthwhile).
//! 3. `rpool/keystore` zvol (`encryption=off`) → LUKS2 (opened with the
//!    `luks_key` recovery passphrase) → ext4 → `system.key` (32 raw bytes).
//! 4. `rpool/ROOT` + `rpool/USERDATA` as the **encryptionroots**
//!    (`encryption=on`, `keylocation=file://…/system.key`) + the stock Ubuntu
//!    dataset tree beneath them.
//!
//! The encryptionroot is `rpool/ROOT`/`rpool/USERDATA`, NOT the bare `rpool`
//! (ZFS inherits encryption downward, so the keystore zvol must hang off an
//! unencrypted parent — proven live, corrected in the design doc). The clevis
//! D2-B bind of the keystore LUKS happens later in Phase 5.
//!
//! The `self.variables["UUID"]` contract is preserved (same as
//! [`super::zfs_ops::ZfsManager`]) so Phase 4/5 keep resolving
//! `rpool/ROOT/ubuntu_<uuid>` and `bpool/BOOT/ubuntu_<uuid>`.
//!
//! [`StorageMode::NativeKeystore`]: super::config::StorageMode::NativeKeystore

use super::config::InstallationConfig;
use super::layout;
use crate::network::CommandExecutor;
use crate::security::luks::LUKS2_FORMAT_HEADER_FLAGS;
use crate::Result;
use std::collections::HashMap;
use tracing::info;

/// Where the ZFS master key lives once the keystore LUKS is unlocked + mounted.
const SYSTEM_KEY: &str = "/run/keystore/rpool/system.key";
/// `keylocation` for the encrypted datasets — the file inside the keystore LUKS.
const KEYLOCATION: &str = "file:///run/keystore/rpool/system.key";
/// dm-crypt mapper name for the opened keystore (matches the boot-time hook).
const KEYSTORE_MAPPER: &str = "keystore-rpool";
/// The keystore zvol device node (post-`zpool create` + udev).
const KEYSTORE_ZVOL: &str = "/dev/zvol/rpool/keystore";
/// Root-only 0600 tmpfs keyfile carrying the LUKS passphrase to `cryptsetup`.
const KEYSTORE_SETUP_KEY: &str = "/run/.uaa-keystore-setup.key";

pub struct ZfsNativeManager<'a> {
    runner: &'a mut dyn CommandExecutor,
    variables: &'a mut HashMap<String, String>,
}

impl<'a> ZfsNativeManager<'a> {
    pub fn new(
        runner: &'a mut dyn CommandExecutor,
        variables: &'a mut HashMap<String, String>,
    ) -> Self {
        Self { runner, variables }
    }

    /// Create the native pools, keystore, and encrypted dataset tree.
    /// Assumes Phase 2 ([`super::disk_native`]) already partitioned the disks.
    pub async fn create_native_pools(&mut self, config: &InstallationConfig) -> Result<()> {
        // Validate + resolve the roster into ordered member device paths.
        let plan = layout::plan_layout(&config.disks)
            .map_err(|e| crate::error::AutoInstallError::ConfigError(e.to_string()))?;
        let sys: Vec<&str> = plan.system_disks().map(|d| d.id.as_str()).collect();
        let spec: Vec<&str> = plan.special_disks().map(|d| d.id.as_str()).collect();
        // Topology follows the roster (plan_layout guarantees >=1 system and
        // never exactly 1 special): bpool + data live on the System disks
        // (p2/p3), the special metadata vdev on the Special disks' half-disk p1.
        // Two or more members mirror; a lone System disk builds a single vdev,
        // and an empty Special set omits the special vdev entirely.
        let bpool_vdev = vdev_spec(&sys, 2);
        let data_vdev = vdev_spec(&sys, 3);
        let special_vdev = if spec.is_empty() {
            String::new()
        } else {
            format!(" special {}", vdev_spec(&spec, 1))
        };

        self.log_and_execute("Ensure altroot", "mkdir -p /mnt/targetos")
            .await?;
        let uuid = self.installation_uuid().await?;
        self.variables.insert("UUID".to_string(), uuid.clone());
        info!("NativeKeystore pools: install uuid = {uuid}");

        self.create_bpool(&bpool_vdev).await?;
        self.create_rpool(&data_vdev, &special_vdev).await?;
        self.create_keystore(config).await?;
        // Load-bearing order (see zfs_ops): rpool ROOT datasets (mount `/`)
        // BEFORE bpool BOOT (mount `/boot`), so /boot lands on top of / and
        // grub-probe resolves /boot to the bpool vdev.
        self.create_rpool_datasets(&uuid).await?;
        self.create_bpool_datasets(&uuid).await?;

        info!("NativeKeystore pools + keystore + datasets created");
        Ok(())
    }

    /// bpool: GRUB-compatible pool across the System disks' `p2` — a mirror when
    /// the roster has two or more, a single vdev on a one-disk host (feature set
    /// mirrors `zfs_ops::build_bpool_create_command`).
    ///
    /// `vdev` is the already-rendered spec (e.g. `mirror A-part2 B-part2` or a
    /// bare `A-part2`), so the mirrored and single-disk paths share one command.
    async fn create_bpool(&mut self, vdev: &str) -> Result<()> {
        let cmd = format!(
            "zpool create -f -o ashift=12 -o autotrim=on -o cachefile=/etc/zfs/zpool.cache \
             -o compatibility=grub2 -o feature@livelist=enabled -o feature@zpool_checkpoint=enabled \
             -O devices=off -O acltype=posixacl -O xattr=sa -O compression=lz4 \
             -O normalization=formD -O relatime=on -O canmount=off -O mountpoint=none \
             -m none -R /mnt/targetos bpool {vdev}"
        );
        self.log_and_execute("Creating bpool (System disk p2)", &cmd)
            .await
    }

    /// rpool: data vdev (System p3) plus an optional special metadata vdev
    /// (Special p1), root UNENCRYPTED (encryption lives on rpool/ROOT +
    /// rpool/USERDATA).
    ///
    /// `data_vdev` is a rendered spec (`mirror A-part3 B-part3` or bare
    /// `A-part3`); `special_vdev` is either empty (single-disk / no Special
    /// disks) or a leading-space ` special mirror C-part1 D-part1`.
    async fn create_rpool(&mut self, data_vdev: &str, special_vdev: &str) -> Result<()> {
        // `cachefile=/etc/zfs/zpool.cache` is MANDATORY and was missing — the
        // single most important line in the whole install. Importing with
        // `-R /mnt/targetos` (altroot) defaults `cachefile=none`, so without an
        // explicit cachefile rpool is NEVER written to /etc/zfs/zpool.cache. That
        // cache is copied into the initramfs; at boot `zfs-import-cache` imports
        // only the pools it lists — so rpool (which holds the encrypted root AND
        // the keystore zvol) never imports, the keystore never appears, no key is
        // ever loaded, and `sysroot.mount` fails ("failed to mount sysroot").
        // bpool has it (above); rpool must too.
        let cmd = format!(
            "zpool create -f -o ashift=12 -o autotrim=on -o cachefile=/etc/zfs/zpool.cache \
             -O acltype=posixacl -O xattr=sa -O dnodesize=auto -O compression=lz4 \
             -O normalization=formD -O relatime=on -O special_small_blocks=0 \
             -O canmount=off -O mountpoint=none -m none -R /mnt/targetos \
             rpool {data_vdev}{special_vdev}"
        );
        self.log_and_execute("Creating rpool (data vdev + optional special)", &cmd)
            .await
    }

    /// The chicken-and-egg breaker: an unencrypted `rpool/keystore` zvol holding
    /// a LUKS container whose plaintext is the ZFS `system.key`. The zvol is
    /// readable on `zpool import` without rpool's key (it inherits
    /// `encryption=off` from the unencrypted rpool root); what import exposes is
    /// LUKS ciphertext, and clevis (Phase 5) unlocks it at boot.
    async fn create_keystore(&mut self, config: &InstallationConfig) -> Result<()> {
        self.log_and_execute(
            "Creating rpool/keystore zvol",
            "zfs create -V 100M -b 16k -o compression=off -o primarycache=metadata \
             -o secondarycache=none -o com.sun:auto-snapshot=false rpool/keystore",
        )
        .await?;
        self.log_and_execute("Settle keystore zvol", "udevadm settle")
            .await?;

        // Passphrase → 0600 tmpfs keyfile (never logged; shred'd after). This is
        // the break-glass recovery passphrase; clevis is the unattended path.
        let write_key = format!(
            "install -m 600 /dev/null {KEYSTORE_SETUP_KEY} && printf '%s' '{}' > {KEYSTORE_SETUP_KEY}",
            shell_single_quote_escape(&config.luks_key)
        );
        self.runner.execute(&write_key).await?;

        self.log_and_execute("LUKS2 format keystore", &build_keystore_luks_format_cmd())
            .await?;
        self.log_and_execute(
            "Open keystore LUKS",
            &format!(
                "cryptsetup open --key-file {KEYSTORE_SETUP_KEY} {KEYSTORE_ZVOL} {KEYSTORE_MAPPER}"
            ),
        )
        .await?;
        // Keyfile no longer needed — shred it (best-effort).
        let _ = self
            .runner
            .execute(&format!(
                "shred -u {KEYSTORE_SETUP_KEY} 2>/dev/null || rm -f {KEYSTORE_SETUP_KEY}"
            ))
            .await;

        self.log_and_execute(
            "ext4 on keystore + mount",
            &format!(
                "mkfs.ext4 -q -L {KEYSTORE_MAPPER} /dev/mapper/{KEYSTORE_MAPPER} && \
                 mkdir -p /run/keystore/rpool && \
                 mount /dev/mapper/{KEYSTORE_MAPPER} /run/keystore/rpool"
            ),
        )
        .await?;
        // Generate the 32-byte raw ZFS key INSIDE the LUKS container.
        self.log_and_execute(
            "Generate system.key",
            &format!("sh -c 'umask 077; head -c 32 /dev/urandom > {SYSTEM_KEY}' && chmod 400 {SYSTEM_KEY}"),
        )
        .await?;
        Ok(())
    }

    /// rpool datasets. Only `rpool/ROOT` and `rpool/USERDATA` differ from the
    /// PlainLuks tree (they carry `encryption=on` + the keystore keylocation);
    /// every child inherits encryption. Kept in sync with
    /// `zfs_ops::create_rpool_datasets`.
    async fn create_rpool_datasets(&mut self, uuid: &str) -> Result<()> {
        let enc = format!("-o encryption=on -o keyformat=raw -o keylocation={KEYLOCATION}");

        self.log_and_execute(
            "Creating rpool/ROOT (encryptionroot)",
            &format!("zfs create -o canmount=off -o mountpoint=none {enc} rpool/ROOT"),
        )
        .await?;
        self.log_and_execute(
            "Creating root filesystem",
            &format!(
                "zfs create -o mountpoint=/ -o com.ubuntu.zsys:bootfs=yes rpool/ROOT/ubuntu_{uuid}"
            ),
        )
        .await?;

        // Stock Ubuntu sub-datasets (inherit encryption from rpool/ROOT).
        let datasets: &[(&str, &str)] = &[
            ("-o com.ubuntu.zsys:bootfs=no -o canmount=off", "usr"),
            ("-o com.ubuntu.zsys:bootfs=no -o canmount=off", "var"),
            ("", "var/lib"),
            ("", "var/log"),
            ("", "var/spool"),
            ("", "var/cache"),
            ("", "var/lib/nfs"),
            ("", "var/tmp"),
            ("", "var/lib/apt"),
            ("", "var/lib/dpkg"),
            ("-o com.ubuntu.zsys:bootfs=no", "srv"),
            ("", "usr/local"),
            ("", "var/games"),
            ("", "var/lib/AccountsService"),
        ];
        for (opts, sub) in datasets {
            self.log_and_execute(
                &format!("Creating {sub}"),
                &format!("zfs create {opts} rpool/ROOT/ubuntu_{uuid}/{sub}"),
            )
            .await?;
        }

        self.log_and_execute("Ensure /var/tmp exists", "mkdir -p /mnt/targetos/var/tmp")
            .await?;
        self.log_and_execute(
            "Setting /var/tmp permissions",
            "chmod 1777 /mnt/targetos/var/tmp",
        )
        .await?;

        // USERDATA is a second encryptionroot (directly under the unencrypted
        // rpool root, so it needs its own encryption=on).
        self.log_and_execute(
            "Creating USERDATA (encryptionroot)",
            &format!("zfs create -o canmount=off -o mountpoint=/ {enc} rpool/USERDATA"),
        )
        .await?;
        self.log_and_execute(
            "Creating root user data",
            &format!(
                "zfs create -o com.ubuntu.zsys:bootfs-datasets=rpool/ROOT/ubuntu_{uuid} \
                 -o canmount=on -o mountpoint=/root rpool/USERDATA/root_{uuid}"
            ),
        )
        .await?;
        self.log_and_execute("Ensure /root exists", "mkdir -p /mnt/targetos/root")
            .await?;
        self.log_and_execute("Setting /root permissions", "chmod 700 /mnt/targetos/root")
            .await?;
        Ok(())
    }

    /// bpool datasets (identical to PlainLuks — bpool is unencrypted `/boot`).
    async fn create_bpool_datasets(&mut self, uuid: &str) -> Result<()> {
        self.log_and_execute("Ensure /boot mountpoint", "mkdir -p /mnt/targetos/boot")
            .await?;
        self.log_and_execute(
            "Creating bpool/BOOT",
            "zfs create -o canmount=off -o mountpoint=none bpool/BOOT",
        )
        .await?;
        self.log_and_execute(
            "Creating bpool boot dataset",
            &format!("zfs create -o mountpoint=/boot bpool/BOOT/ubuntu_{uuid}"),
        )
        .await
    }

    /// 6-hex-char install id (matches `zfs_ops`'s `ubuntu_<uuid>` convention).
    async fn installation_uuid(&mut self) -> Result<String> {
        let out = self
            .runner
            .execute_with_output("head -c3 /dev/urandom | od -An -tx1 | tr -d ' \\n'")
            .await?;
        let uuid = out.trim().to_string();
        if uuid.len() < 6 {
            return Err(crate::error::AutoInstallError::ConfigError(format!(
                "could not generate install uuid (got {uuid:?})"
            )));
        }
        Ok(uuid)
    }

    async fn log_and_execute(&mut self, description: &str, command: &str) -> Result<()> {
        info!("Executing: {} -> {}", description, command);
        self.runner.execute(command).await
    }
}

/// The `cryptsetup luksFormat` command for the `rpool/keystore` zvol.
///
/// Pure and argument-free (every operand is a module constant) so the emitted
/// flags are assertable without a command-recording harness.
///
/// **This is the volume the clevis policy binds to** on a `NativeKeystore`
/// host: the LUKS container inside the keystore zvol is what holds the ZFS
/// `system.key`, so its header is where the ~22 KB nested-`sss` JWE has to fit.
/// The metadata area can only be sized at format time — see
/// [`LUKS2_FORMAT_HEADER_FLAGS`].
fn build_keystore_luks_format_cmd() -> String {
    format!(
        "cryptsetup luksFormat {LUKS2_FORMAT_HEADER_FLAGS} --batch-mode --key-file {KEYSTORE_SETUP_KEY} {KEYSTORE_ZVOL}"
    )
}

/// Escape a value for embedding inside a single-quoted shell string
/// (`'…'`) — closes the quote, inserts an escaped quote, reopens. Used for the
/// LUKS passphrase so an arbitrary secret can't break the command.
fn shell_single_quote_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Render a `zpool create` vdev spec for `disks`, each addressed by partition
/// number `part` (`<by-id>-part<N>`).
///
/// Two or more disks become a `mirror`; a lone disk is a bare single vdev.
/// Emitting the `mirror` keyword for one device would be a `zpool` syntax
/// error, so the distinction is load-bearing, not cosmetic — it is what lets a
/// single-NVMe host use the same NativeKeystore path as the mirrored U1
/// roster. Empty input yields an empty string; callers omit the vdev entirely
/// in that case (see the optional `special` vdev).
fn vdev_spec(disks: &[&str], part: u32) -> String {
    if disks.is_empty() {
        return String::new();
    }
    let members = disks
        .iter()
        .map(|d| format!("{d}-part{part}"))
        .collect::<Vec<_>>()
        .join(" ");
    if disks.len() >= 2 {
        format!("mirror {members}")
    } else {
        members
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passphrase_escaping_is_single_quote_safe() {
        assert_eq!(shell_single_quote_escape("plain"), "plain");
        assert_eq!(shell_single_quote_escape("a'b"), "a'\\''b");
    }

    /// The U1 (2-disk) rendering must be EXACTLY what the pre-single-disk code
    /// emitted — `mirror A-partN B-partN` — or the hardware-validated install
    /// changes behaviour underneath us.
    #[test]
    fn vdev_spec_two_disks_renders_mirror_byte_identical() {
        let disks = [
            "/dev/disk/by-id/ata-CRUCIAL_0",
            "/dev/disk/by-id/ata-CRUCIAL_1",
        ];
        assert_eq!(
            vdev_spec(&disks, 2),
            "mirror /dev/disk/by-id/ata-CRUCIAL_0-part2 /dev/disk/by-id/ata-CRUCIAL_1-part2"
        );
        assert_eq!(
            vdev_spec(&disks, 3),
            "mirror /dev/disk/by-id/ata-CRUCIAL_0-part3 /dev/disk/by-id/ata-CRUCIAL_1-part3"
        );
    }

    /// A lone disk must NOT emit the `mirror` keyword — `zpool create rpool
    /// mirror /dev/x` is a syntax error, so this is correctness, not style.
    #[test]
    fn vdev_spec_single_disk_has_no_mirror_keyword() {
        let disks = ["/dev/disk/by-id/nvme-WDC_ONLY"];
        let spec = vdev_spec(&disks, 3);
        assert_eq!(spec, "/dev/disk/by-id/nvme-WDC_ONLY-part3");
        assert!(
            !spec.contains("mirror"),
            "single vdev must not say 'mirror': {spec}"
        );
    }

    /// No Special disks → empty spec, so the caller omits the `special` vdev
    /// entirely rather than emitting a dangling `special` keyword.
    #[test]
    fn vdev_spec_empty_roster_is_empty() {
        let none: [&str; 0] = [];
        assert_eq!(vdev_spec(&none, 1), "");
    }

    /// Three or more disks still mirror across every member.
    #[test]
    fn vdev_spec_three_disks_mirrors_all_members() {
        let disks = ["/dev/a", "/dev/b", "/dev/c"];
        assert_eq!(
            vdev_spec(&disks, 1),
            "mirror /dev/a-part1 /dev/b-part1 /dev/c-part1"
        );
    }

    /// The keystore LUKS header must reserve a 1 MiB metadata area.
    ///
    /// The keystore zvol IS the clevis-bound volume on a NativeKeystore host,
    /// and the metadata area is immutable after `luksFormat` — so a host built
    /// without this flag cannot be repaired in place. Measured on cryptsetup
    /// 2.8.4: a 22064-byte token import into a default (16 KiB) header fails
    /// with `Failed to import token from file.` and succeeds at `1m`, with the
    /// payload offset unchanged at 16 MiB.
    #[test]
    fn keystore_luks_format_reserves_a_1m_metadata_area() {
        let cmd = build_keystore_luks_format_cmd();
        assert!(
            cmd.contains("--luks2-metadata-size 1m"),
            "keystore luksFormat must reserve 1 MiB of metadata or the \
             nested-sss binding will not fit: {cmd}"
        );
        // Regression guards on the operands the flag insertion sits between.
        assert!(cmd.contains("--type luks2"), "{cmd}");
        assert!(cmd.contains(KEYSTORE_ZVOL), "{cmd}");
        assert!(cmd.contains(KEYSTORE_SETUP_KEY), "{cmd}");
        assert!(
            !cmd.contains("--luks2-keyslots-size"),
            "raising the keyslots area moves the data offset for no benefit: {cmd}"
        );
    }

    #[test]
    fn keystore_constants_agree_on_the_key_path() {
        // system.key path and the keylocation URI must point at the same file,
        // or the datasets can't find their key at boot.
        assert!(KEYLOCATION.ends_with(SYSTEM_KEY));
        assert_eq!(KEYLOCATION, format!("file://{SYSTEM_KEY}"));
    }
}
