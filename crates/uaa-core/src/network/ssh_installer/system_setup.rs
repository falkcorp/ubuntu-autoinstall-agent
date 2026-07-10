// file: crates/uaa-core/src/network/ssh_installer/system_setup.rs
// version: 2.11.1
// guid: sshsys01-2345-6789-abcd-ef0123456789
// last-edited: 2026-07-10

//! System setup and configuration for SSH/local installation.
//!
//! Supports both initramfs-tools and dracut. When dracut is selected the GRUB
//! kernel command line receives `rd.neednet=1 ip=dhcp` so the Tang servers are
//! reachable during initramfs boot for clevis-based LUKS unlock.

use super::config::{InitramfsType, InstallationConfig};
use super::partitions::partition_path;
use crate::network::CommandExecutor;
use crate::Result;
use tracing::{info, warn};

pub struct SystemConfigurator<'a> {
    runner: &'a mut dyn CommandExecutor,
}

impl<'a> SystemConfigurator<'a> {
    pub fn new(runner: &'a mut dyn CommandExecutor) -> Self {
        Self { runner }
    }

    /// Build the command used to detect the ESP partition by GUID
    fn build_esp_detection_command(guid: &str) -> String {
        format!(
            "bash -lc 'lsblk -rP -o PATH,PARTTYPE | grep -i \"PARTTYPE=\\\"{0}\\\"\" | head -n1 | sed -n \"s/.*PATH=\\\"\\([^\\\" ]*\\)\\\".*/\\1/p\"'",
            guid
        )
    }

    /// Build Deb822-style Ubuntu apt sources content for the given release
    fn build_apt_deb822_sources(release: &str) -> String {
        format!(
            "Types: deb\nURIs: http://archive.ubuntu.com/ubuntu/\nSuites: {rel}\nComponents: main restricted universe multiverse\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n\nTypes: deb\nURIs: http://security.ubuntu.com/ubuntu\nSuites: {rel}-security\nComponents: main restricted universe multiverse\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n",
            rel = release
        )
    }

    /// Build a crypttab entry for the LUKS partition using either a UUID or the raw device
    fn build_crypttab_entry(disk_device: &str, uuid_opt: Option<&str>) -> String {
        let dev = if let Some(uuid) = uuid_opt {
            if uuid.trim().is_empty() {
                partition_path(disk_device, 4)
            } else {
                format!("/dev/disk/by-uuid/{}", uuid.trim())
            }
        } else {
            partition_path(disk_device, 4)
        };
        format!("luks {} none luks,discard,initramfs", dev)
    }

    /// Build the netplan YAML for `/etc/netplan/01-netcfg.yaml`.
    ///
    /// Validates `config.network_renderer` first (must be exactly `"networkd"`
    /// or `"NetworkManager"` — no case-insensitive aliasing). When
    /// `config.network_address` is the literal `dhcp` (case-insensitive) —
    /// the marker `detect_network_config` emits for DHCP-assigned interfaces —
    /// renders a `dhcp4: true` ethernet stanza with no `addresses:`,
    /// `routes:`, or `nameservers:` blocks. Otherwise renders the static
    /// template byte-identical to before, apart from the renderer
    /// substitution.
    fn build_netplan_yaml(config: &InstallationConfig) -> Result<String> {
        let renderer = config.network_renderer.as_str();
        match renderer {
            "networkd" | "NetworkManager" => {}
            other => {
                return Err(crate::error::AutoInstallError::ConfigError(format!(
                    "unsupported network_renderer '{other}' (expected \"networkd\" or \"NetworkManager\")"
                )))
            }
        }

        if config.network_address.eq_ignore_ascii_case("dhcp") {
            return Ok(format!(
                r#"network:
  version: 2
  renderer: {renderer}
  ethernets:
    {interface}:
      dhcp4: true"#,
                renderer = renderer,
                interface = config.network_interface,
            ));
        }

        Ok(format!(
            r#"network:
  version: 2
  renderer: {renderer}
  ethernets:
    {}:
      addresses:
        - {}
      routes:
        - to: default
          via: {}
      nameservers:
        search:
          - {}
        addresses:
{}"#,
            config.network_interface,
            config.network_address,
            config.network_gateway,
            config.network_search,
            config
                .network_nameservers
                .iter()
                .map(|ns| format!("          - {}", ns))
                .collect::<Vec<_>>()
                .join("\n"),
            renderer = renderer,
        ))
    }

    /// Decide which ESP partition path to use based on detection output
    fn choose_esp_partition(detected_output: &str, default_disk: &str) -> String {
        let part = detected_output.trim();
        if part.is_empty() {
            partition_path(default_disk, 1)
        } else {
            part.to_string()
        }
    }

    /// Detect the ESP partition path by GUID PARTTYPE; fallback to partition 1 of the
    /// configured disk (suffix-aware: nvme0n1p1 / sda1) if not found
    async fn detect_esp_partition_path(&mut self, default_disk: &str) -> Result<String> {
        let guid = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
        let cmd = Self::build_esp_detection_command(guid);
        let out = self
            .runner
            .execute_with_output(&cmd)
            .await
            .unwrap_or_default();
        Ok(Self::choose_esp_partition(&out, default_disk))
    }

    /// Install base system using debootstrap
    pub async fn install_base_system(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Installing base system");

        self.log_and_execute(
            "Creating ESP mount point",
            "mkdir -p /mnt/targetos/boot/efi",
        )
        .await?;
        let esp_part = self.detect_esp_partition_path(&config.disk_device).await?;
        self.log_and_execute(
            "Mounting ESP",
            &format!("mount {} /mnt/targetos/boot/efi", esp_part),
        )
        .await?;

        let release = config.debootstrap_release.as_deref().unwrap_or("resolute");
        let mirror = config
            .debootstrap_mirror
            .as_deref()
            .unwrap_or("http://archive.ubuntu.com/ubuntu/");
        // Use a persistent debootstrap base tarball if one is present on the
        // `uaacache`-labelled device (e.g. the box's spare NVMe): mount it and
        // pass `--unpack-tarball` so the base packages are NOT re-downloaded —
        // debootstrap over WAN is the slow phase. Falls back to a full
        // debootstrap when no cache is available. Build the cache once with:
        //   debootstrap --make-tarball=/mnt/uaacache/<release>-<arch>-base.tar.gz \
        //               <release> /tmp/scratch <mirror>
        let primary_cmd = format!(
            "mkdir -p /mnt/uaacache; \
             mountpoint -q /mnt/uaacache || mount -o ro /dev/disk/by-label/uaacache /mnt/uaacache 2>/dev/null || true; \
             CACHE=/mnt/uaacache/{release}-$(dpkg --print-architecture)-base.tar.gz; \
             if [ -f \"$CACHE\" ]; then \
               echo \"debootstrap: using cached base $CACHE\"; \
               debootstrap --unpack-tarball=\"$CACHE\" {release} /mnt/targetos {mirror}; \
             else \
               echo \"debootstrap: no cache, full download\"; \
               debootstrap {release} /mnt/targetos {mirror}; \
             fi",
            release = release,
            mirror = mirror
        );
        if let Err(_e) = self
            .log_and_execute("Running debootstrap", &primary_cmd)
            .await
        {
            let fallback_mirror = "http://old-releases.ubuntu.com/ubuntu/";
            if mirror != fallback_mirror {
                let fallback_cmd =
                    format!("debootstrap {} /mnt/targetos {}", release, fallback_mirror);
                self.log_and_execute("Running debootstrap (fallback old-releases)", &fallback_cmd)
                    .await?;
            } else {
                return Err(_e);
            }
        }

        self.setup_basic_system_files(config).await?;
        self.configure_system_in_chroot(config).await?;

        info!("Base system installation completed");
        Ok(())
    }

    /// Setup basic system files
    async fn setup_basic_system_files(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Setting up basic system files");

        self.runner
            .execute(&format!(
                "echo '{}' > /mnt/targetos/etc/hostname",
                config.hostname
            ))
            .await?;

        let hosts_content = format!(
            "127.0.0.1 localhost\n127.0.1.1 {}\n::1 localhost ip6-localhost ip6-loopback\nff02::1 ip6-allnodes\nff02::2 ip6-allrouters",
            config.hostname
        );
        self.runner
            .execute(&format!(
                "cat > /mnt/targetos/etc/hosts << 'EOF'\n{}\nEOF",
                hosts_content
            ))
            .await?;

        self.setup_network_configuration(config).await?;

        self.runner
            .execute(&format!(
                "ln -sf /usr/share/zoneinfo/{} /mnt/targetos/etc/localtime",
                config.timezone
            ))
            .await?;

        let release = config.debootstrap_release.as_deref().unwrap_or("resolute");
        let ubuntu_sources = Self::build_apt_deb822_sources(release);
        self.runner
            .execute("mkdir -p /mnt/targetos/etc/apt/sources.list.d")
            .await?;
        self.runner
            .execute(&format!(
                "cat > /mnt/targetos/etc/apt/sources.list.d/ubuntu.sources << 'EOF'\n{}\nEOF",
                ubuntu_sources
            ))
            .await?;
        let _ = self
            .runner
            .execute("rm -f /mnt/targetos/etc/apt/sources.list || true")
            .await;

        Ok(())
    }

    /// Setup network configuration
    async fn setup_network_configuration(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Setting up network configuration");

        let netplan_config = Self::build_netplan_yaml(config)?;

        self.runner
            .execute("mkdir -p /mnt/targetos/etc/netplan")
            .await?;
        self.runner
            .execute(&format!(
                "cat > /mnt/targetos/etc/netplan/01-netcfg.yaml << 'EOF'\n{}\nEOF",
                netplan_config
            ))
            .await?;

        Ok(())
    }

    /// Configure system in chroot environment
    async fn configure_system_in_chroot(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Configuring system in chroot");

        // Bind mounts (idempotent)
        let _ = self.log_and_execute(
            "Bind /dev (rbind)",
            "[ -d /mnt/targetos/dev ] || mkdir -p /mnt/targetos/dev; mountpoint -q /mnt/targetos/dev || mount --rbind /dev /mnt/targetos/dev"
        ).await;
        let _ = self.log_and_execute(
            "Make /dev private",
            "mount --make-private /mnt/targetos/dev || true",
        ).await;
        let _ = self.log_and_execute(
            "Ensuring /dev/pts",
            "[ -d /mnt/targetos/dev/pts ] || mkdir -p /mnt/targetos/dev/pts; mountpoint -q /mnt/targetos/dev/pts || mount -t devpts devpts /mnt/targetos/dev/pts || true"
        ).await;
        let _ = self.log_and_execute(
            "Bind /proc (rbind)",
            "[ -d /mnt/targetos/proc ] || mkdir -p /mnt/targetos/proc; mountpoint -q /mnt/targetos/proc || mount --rbind /proc /mnt/targetos/proc"
        ).await;
        let _ = self.log_and_execute(
            "Make /proc private",
            "mount --make-private /mnt/targetos/proc || true",
        ).await;
        let _ = self.log_and_execute(
            "Bind /sys (rbind)",
            "[ -d /mnt/targetos/sys ] || mkdir -p /mnt/targetos/sys; mountpoint -q /mnt/targetos/sys || mount --rbind /sys /mnt/targetos/sys"
        ).await;
        let _ = self.log_and_execute(
            "Make /sys private",
            "mount --make-private /mnt/targetos/sys || true",
        ).await;
        let _ = self.log_and_execute(
            "Bind /run (rbind)",
            "[ -d /mnt/targetos/run ] || mkdir -p /mnt/targetos/run; mountpoint -q /mnt/targetos/run || mount --rbind /run /mnt/targetos/run"
        ).await;
        let _ = self.log_and_execute(
            "Make /run private",
            "mount --make-private /mnt/targetos/run || true",
        ).await;

        // DNS in chroot
        let _ = self.log_and_execute(
            "Reset chroot resolv.conf",
            "[ -e /mnt/targetos/etc/resolv.conf ] && rm -f /mnt/targetos/etc/resolv.conf; echo 'nameserver 1.1.1.1' > /mnt/targetos/etc/resolv.conf"
        ).await;

        // ESP
        let _ = self.log_and_execute(
            "Ensure ESP mountpoint",
            "[ -d /mnt/targetos/boot/efi ] || mkdir -p /mnt/targetos/boot/efi",
        ).await;
        let esp_part = self.detect_esp_partition_path(&config.disk_device).await?;
        let _ = self.log_and_execute(
            "Mount ESP if not mounted",
            &format!(
                "mountpoint -q /mnt/targetos/boot/efi || mount {} /mnt/targetos/boot/efi || true",
                esp_part
            ),
        ).await;

        // fstab entry for ESP (UUID-based)
        let esp_part = self.detect_esp_partition_path(&config.disk_device).await?;
        let esp_uuid_out = self
            .runner
            .execute_with_output(&format!(
                "blkid -s UUID -o value {} 2>/dev/null || true",
                esp_part
            ))
            .await?;
        let esp_uuid = esp_uuid_out.trim();
        if !esp_uuid.is_empty() {
            let fstab_line = format!("UUID={} /boot/efi vfat umask=0077 0 1", esp_uuid);
            let cmd = format!(
                "bash -lc \"grep -q '^UUID=.* /boot/efi ' /mnt/targetos/etc/fstab 2>/dev/null || echo '{0}' >> /mnt/targetos/etc/fstab\"",
                fstab_line
            );
            let _ = self.runner.execute(&cmd).await;
        }

        // efivarfs
        let _ = self.log_and_execute(
            "Ensure efivarfs in chroot",
            "chroot /mnt/targetos bash -lc '[ -d /sys/firmware/efi/efivars ] || mkdir -p /sys/firmware/efi/efivars; mountpoint -q /sys/firmware/efi/efivars || mount -t efivarfs efivarfs /sys/firmware/efi/efivars || true'"
        ).await;

        // Package set matched to the clean direct 26.04 install on len-serv-003
        // (the reference host). That install uses **dracut**, never initramfs-tools.
        let initramfs_pkg = match config.initramfs_type {
            InitramfsType::Dracut => "dracut dracut-network",
            InitramfsType::InitramfsTools => "initramfs-tools cryptsetup-initramfs",
        };

        // ZFS support MUST match the generator. On dracut it's zfs-dracut (+ the
        // signed linux-main-modules-zfs-* module pulled via the kernel, which is
        // what lets ZFS root load under Secure Boot). NOT zfs-initramfs — that is
        // the initramfs-tools hook and depends on initramfs-tools, which both
        // fails to import rpool under dracut and drags the second generator back
        // in (the dual-generator mess seen on len-serv-002).
        let zfs_pkg = match config.initramfs_type {
            InitramfsType::Dracut => "zfs-dracut zfsutils-linux zfs-zed",
            InitramfsType::InitramfsTools => "zfs-initramfs zfsutils-linux",
        };

        // clevis for Tang. 26.04 bundles the tang/tpm2 PINS into base `clevis`;
        // there is no separate clevis-tang package (installing it fails).
        let clevis_pkgs = if !config.tang_servers.is_empty() {
            match config.initramfs_type {
                InitramfsType::Dracut => " clevis clevis-luks clevis-dracut clevis-systemd",
                InitramfsType::InitramfsTools => " clevis clevis-luks clevis-initramfs",
            }
        } else {
            ""
        };

        // TPM2+PIN and FIDO2 keyslots are unlocked by systemd-cryptsetup (its own
        // package, which ships the cryptsetup tpm2/fido2 token plugins). tpm2-tools
        // pulls the libtss2 stack; tpm-udev creates the TPM device nodes;
        // libfido2-1 backs FIDO2. Matches the 003 reference set.
        let crypt_extra = if config.enroll_tpm2 || config.expect_fido2 {
            " systemd-cryptsetup tpm2-tools tpm-udev libfido2-1"
        } else {
            ""
        };

        // mdadm in the TARGET so its initramfs can re-assemble a BIOS/IMSM
        // fake-RAID root (e.g. unimatrixone's /dev/md126) before LUKS/ZFS unlock.
        // Without it the installed system boots to an initramfs that can't find
        // the array. Harmless on hosts with a plain (non-md) disk.
        let mdadm_pkg = if config.disk_device.starts_with("/dev/md") {
            " mdadm"
        } else {
            ""
        };

        let chroot_commands = vec![
            "apt update".to_string(),
            format!(
                "DEBIAN_FRONTEND=noninteractive apt install -y grub-efi-amd64 grub-efi-amd64-signed linux-image-generic shim-signed {} {} efibootmgr cryptsetup dosfstools{}{}{}",
                initramfs_pkg, zfs_pkg, clevis_pkgs, crypt_extra, mdadm_pkg
            ),
            "DEBIAN_FRONTEND=noninteractive apt install -y linux-headers-generic".to_string(),
            "DEBIAN_FRONTEND=noninteractive apt install -y openssh-server vim htop curl".to_string(),
            "DEBIAN_FRONTEND=noninteractive apt purge -y os-prober || true".to_string(),
            "addgroup --system lpadmin || true".to_string(),
            "addgroup --system lxd || true".to_string(),
            "addgroup --system sambashare || true".to_string(),
        ];

        for cmd in chroot_commands {
            let desc = format!("Chroot: {}", cmd);
            let wrapped = format!("chroot /mnt/targetos bash -lc '{}'", cmd);
            self.run_tolerating_zsys_errors(&desc, &wrapped).await?;
        }

        // hostid for ZFS
        let _ = self.log_and_execute(
            "Generate /etc/hostid",
            "chroot /mnt/targetos bash -lc 'command -v zgenhostid >/dev/null 2>&1 && zgenhostid -f /etc/hostid || (command -v hostid >/dev/null 2>&1 && hostid > /etc/hostid) || true'"
        ).await;

        // Root password
        let _ = self.log_and_execute(
            "Setting root password",
            &format!(
                "chroot /mnt/targetos bash -lc \"echo 'root:{}' | chpasswd\"",
                config.root_password
            ),
        ).await;

        // SSH authorized keys for root
        if !config.ssh_authorized_keys.is_empty() {
            let _ = self.log_and_execute(
                "Create root .ssh dir",
                "chroot /mnt/targetos bash -lc 'mkdir -p /root/.ssh && chmod 700 /root/.ssh'",
            ).await;
            for key in &config.ssh_authorized_keys {
                let cmd = format!(
                    "chroot /mnt/targetos bash -lc \"echo '{}' >> /root/.ssh/authorized_keys\"",
                    key
                );
                let _ = self.log_and_execute("Inject SSH authorized key", &cmd).await;
            }
            let _ = self.log_and_execute(
                "Fix authorized_keys permissions",
                "chroot /mnt/targetos bash -lc 'chmod 600 /root/.ssh/authorized_keys || true'",
            ).await;
        }

        let _ = self.log_and_execute(
            "Enabling SSH",
            "chroot /mnt/targetos bash -lc 'systemctl enable ssh'",
        ).await;

        Ok(())
    }

    /// Configure ZFS in chroot
    pub async fn configure_zfs_in_chroot(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Configuring ZFS in chroot");

        let zfs_commands = vec![
            "systemctl enable zfs-import-cache",
            "systemctl enable zfs-mount",
            "systemctl enable zfs-import.target",
        ];

        for cmd in zfs_commands {
            let _ = self.log_and_execute(
                &format!("ZFS: {}", cmd),
                &format!("chroot /mnt/targetos bash -lc '{}'", cmd),
            ).await;
        }

        // Seed ZFS cache
        let _ = self.log_and_execute(
            "Ensure /etc/zfs in target",
            "mkdir -p /mnt/targetos/etc/zfs",
        ).await;
        let _ = self.log_and_execute(
            "Copy zpool.cache",
            "cp -f /etc/zfs/zpool.cache /mnt/targetos/etc/zfs/ 2>/dev/null || true",
        ).await;
        let _ = self.log_and_execute(
            "Ensure zfs-list.cache dir",
            "mkdir -p /mnt/targetos/etc/zfs/zfs-list.cache",
        ).await;
        let _ = self.log_and_execute(
            "Touch zfs-list.cache files",
            "bash -lc 'touch /mnt/targetos/etc/zfs/zfs-list.cache/bpool /mnt/targetos/etc/zfs/zfs-list.cache/rpool'",
        ).await;
        let _ = self.log_and_execute(
            "Populate zfs-list via zed",
            "chroot /mnt/targetos bash -lc 'timeout 5 zed -F || true'",
        ).await;
        // Fix mountpoint paths — run on host so sed can see the file directly
        let _ = self.log_and_execute(
            "Fix zfs-list paths",
            "sed -Ei 's|/mnt/targetos/?|/|' /mnt/targetos/etc/zfs/zfs-list.cache/* || true",
        ).await;

        // Regenerate initramfs (dracut or initramfs-tools)
        let regen_cmd = config.initramfs_type.regenerate_cmd();
        let _ = self.log_and_execute(
            "Regenerate initramfs (post-ZFS)",
            &format!("chroot /mnt/targetos bash -lc '{}'", regen_cmd),
        ).await;

        Ok(())
    }

    /// BootOrder script: network entries first, ubuntu second, rest after.
    /// Regexes are copied VERBATIM from set_boot_order() in
    /// installer-image/nocloud/uaa-usb-bootstrap.sh so USB and chroot behave
    /// identically. Every failure path exits 0 (non-fatal by design).
    fn build_boot_order_cmd() -> String {
        let script = r#"command -v efibootmgr >/dev/null 2>&1 || { echo "uaa: efibootmgr not present; skipping boot order"; exit 0; }; entries="$(efibootmgr 2>/dev/null)" || { echo "uaa: efibootmgr unreadable (legacy BIOS?); skipping boot order"; exit 0; }; net="$(echo "$entries" | sed -n "s/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\}[[:space:]].*\(PXE\|[Nn]etwork\|IPv[46]\).*/\1/p" | tr "\n" ",")"; ubuntu="$(echo "$entries" | sed -n "s/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\}[[:space:]][Uu]buntu.*/\1/p" | tr "\n" ",")"; rest="$(echo "$entries" | sed -n "s/^Boot\([0-9A-Fa-f]\{4\}\)\*\{0,1\}[[:space:]].*/\1/p" | tr "\n" ",")"; order="$(echo "${net}${ubuntu}${rest}" | tr "," "\n" | grep -v "^$" | awk "!seen[\$0]++" | paste -sd, -)"; [ -n "$order" ] || { echo "uaa: no EFI boot entries found; skipping boot order"; exit 0; }; efibootmgr -o "$order" && echo "uaa: BootOrder set: $order" || echo "uaa: efibootmgr -o failed (non-fatal)"; exit 0"#;
        format!("chroot /mnt/targetos bash -lc '{}'", script)
    }

    /// Best-effort UEFI boot order (network first, ubuntu second). Non-fatal:
    /// legacy-BIOS / no-efivars hosts log and continue; Phase 5 still completes.
    async fn set_uefi_boot_order(&mut self) -> Result<()> {
        self.log_and_execute(
            "Set UEFI BootOrder (network first, ubuntu second)",
            &Self::build_boot_order_cmd(),
        )
        .await
    }

    /// Configure GRUB in chroot — adds Tang network parameters when using dracut.
    pub async fn configure_grub_in_chroot(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Configuring GRUB in chroot");

        // Re-ensure bind mounts
        for (desc, cmd) in [
            ("Rebind /dev", "[ -d /mnt/targetos/dev ] || mkdir -p /mnt/targetos/dev; mountpoint -q /mnt/targetos/dev || mount --rbind /dev /mnt/targetos/dev"),
            ("Re-ensure /dev/pts", "[ -d /mnt/targetos/dev/pts ] || mkdir -p /mnt/targetos/dev/pts; mountpoint -q /mnt/targetos/dev/pts || mount -t devpts devpts /mnt/targetos/dev/pts || true"),
            ("Rebind /proc", "[ -d /mnt/targetos/proc ] || mkdir -p /mnt/targetos/proc; mountpoint -q /mnt/targetos/proc || mount --rbind /proc /mnt/targetos/proc"),
            ("Rebind /sys", "[ -d /mnt/targetos/sys ] || mkdir -p /mnt/targetos/sys; mountpoint -q /mnt/targetos/sys || mount --rbind /sys /mnt/targetos/sys"),
            ("Rebind /run", "[ -d /mnt/targetos/run ] || mkdir -p /mnt/targetos/run; mountpoint -q /mnt/targetos/run || mount --rbind /run /mnt/targetos/run"),
        ] {
            let _ = self.log_and_execute(desc, cmd).await;
        }

        let _ = self.log_and_execute(
            "Check udev presence",
            "bash -lc '[ -d /mnt/targetos/run/udev ] && [ -d /mnt/targetos/dev/disk/by-id ] && echo udev-ok || echo udev-missing'",
        ).await;

        let _ = self.log_and_execute(
            "Ensure ESP mountpoint",
            "[ -d /mnt/targetos/boot/efi ] || mkdir -p /mnt/targetos/boot/efi",
        ).await;
        let esp_part = self.detect_esp_partition_path(&config.disk_device).await?;
        let _ = self.log_and_execute(
            "Mount ESP if not mounted",
            &format!(
                "mountpoint -q /mnt/targetos/boot/efi || mount {} /mnt/targetos/boot/efi || true",
                esp_part
            ),
        ).await;

        let _ = self.log_and_execute(
            "Ensure efivarfs",
            "chroot /mnt/targetos bash -lc '[ -d /sys/firmware/efi/efivars ] || mkdir -p /sys/firmware/efi/efivars; mountpoint -q /sys/firmware/efi/efivars || mount -t efivarfs efivarfs /sys/firmware/efi/efivars || true'"
        ).await;

        // For dracut + Tang: GRUB must pass rd.neednet=1 ip=dhcp so the network
        // is available in the initramfs before Tang is queried for the LUKS key.
        if config.initramfs_type == InitramfsType::Dracut && !config.tang_servers.is_empty() {
            info!("Dracut+Tang: adding rd.neednet=1 ip=dhcp to GRUB_CMDLINE_LINUX");
            let grub_extra = "rd.neednet=1 ip=dhcp";
            let set_cmdline = format!(
                r#"chroot /mnt/targetos bash -lc 'grep -q "rd.neednet" /etc/default/grub 2>/dev/null || sed -i "s|^GRUB_CMDLINE_LINUX=\\\"\\(.*\\)\\\"|GRUB_CMDLINE_LINUX=\\\"\\1 {}\\\"| " /etc/default/grub'"#,
                grub_extra
            );
            let _ = self.log_and_execute("Set GRUB_CMDLINE_LINUX for dracut+Tang", &set_cmdline).await;
        }

        // GRUB install with fallbacks.
        //
        // `--uefi-secure-boot` (the Ubuntu default, made explicit here) lays down
        // the SIGNED shim chain: shimx64.efi as the first-stage loader chainloading
        // the signed grubx64.efi. Secure Boot can then be turned on in firmware
        // without reinstalling. NOTE: the generic kernel's zfs.ko is Canonical-signed,
        // so ZFS root still loads under enforced Secure Boot.
        if let Err(_e) = self.log_and_execute(
            "Installing GRUB+shim to ESP (Secure Boot ready)",
            "chroot /mnt/targetos bash -lc 'grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=ubuntu --uefi-secure-boot --recheck'",
        ).await {
            if let Err(_e2) = self.log_and_execute(
                "Installing GRUB+shim to ESP (no-nvram fallback)",
                "chroot /mnt/targetos bash -lc 'grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=ubuntu --uefi-secure-boot --recheck --no-nvram'",
            ).await {
                self.log_and_execute(
                    "Installing GRUB+shim to ESP (removable fallback)",
                    "chroot /mnt/targetos bash -lc 'grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=ubuntu --uefi-secure-boot --recheck --removable'",
                ).await?;
            }
        }

        self.log_and_execute(
            "Updating GRUB config",
            "chroot /mnt/targetos bash -lc 'update-grub'",
        ).await?;

        // Best-effort: order NVRAM entries network-first, ubuntu-second. Mirrors
        // set_boot_order() in uaa-usb-bootstrap.sh. MUST stay non-fatal (let _ =):
        // legacy-BIOS hosts have no efivars, and grub-install --no-nvram/--removable
        // fallbacks mean the "ubuntu" entry may not exist.
        let _ = self.set_uefi_boot_order().await;

        Ok(())
    }

    /// Configure LUKS crypttab and optionally enroll Tang via Clevis SSS.
    pub async fn setup_luks_key_in_chroot(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Configuring LUKS crypttab in chroot");

        let part = partition_path(&config.disk_device, 4);
        let uuid_out = self
            .runner
            .execute_with_output(&format!(
                "blkid -s UUID -o value {} 2>/dev/null || true",
                part
            ))
            .await?;
        let uuid = uuid_out.trim();
        let crypttab_entry = Self::build_crypttab_entry(
            &config.disk_device,
            if uuid.is_empty() { None } else { Some(uuid) },
        );
        let _ = self.runner.execute(&format!(
            "[ -d /mnt/targetos/etc ] || mkdir -p /mnt/targetos/etc; echo '{}' > /mnt/targetos/etc/crypttab",
            crypttab_entry
        )).await;

        // Ensure the initramfs carries BOTH unlock subsystems before any regen:
        //   - clevis  → Tang (network) unlock
        //   - crypt/tpm2/fido2 → systemd-cryptenroll TPM2+PIN and YubiKey keyslots
        self.configure_dracut_crypt_modules(config).await?;

        // Enroll Tang servers via Clevis SSS when configured
        if !config.tang_servers.is_empty() {
            self.enroll_tang_clevis(config, &part).await?;
        }

        // Stage TPM2+PIN enrollment for first boot (binds the *installed*
        // system's PCRs, which the live installer cannot produce). FIDO2/YubiKey
        // is enrolled manually post-install via register-fido2-luks.sh.
        if config.enroll_tpm2 && config.tpm2_pin.as_deref().is_some_and(|p| !p.is_empty()) {
            self.setup_tpm2_firstboot_enrollment(config, if uuid.is_empty() { None } else { Some(uuid) }).await?;
        }

        // Regenerate initramfs after crypttab + Tang enrollment
        let regen_cmd = config.initramfs_type.regenerate_cmd();
        let _ = self.log_and_execute(
            "Regenerate initramfs (post-crypttab)",
            &format!("chroot /mnt/targetos bash -lc '{}'", regen_cmd),
        ).await;

        Ok(())
    }

    /// Enroll Tang servers via Clevis SSS (t-of-n) on the LUKS partition.
    ///
    /// The clevis binary runs on the *host* (live environment) because the LUKS
    /// device is not visible inside the chroot. The clevis-dracut/clevis-initramfs
    /// package inside the chroot handles unlock at boot time.
    async fn enroll_tang_clevis(
        &mut self,
        config: &InstallationConfig,
        luks_part: &str,
    ) -> Result<()> {
        info!(
            "Enrolling {} Tang servers via Clevis SSS (threshold={})",
            config.tang_servers.len(),
            config.tang_threshold
        );

        // Build the SSS pin JSON for clevis
        // {"t":2,"pins":{"tang":[{"url":"http://172.16.2.45"},...]}}
        let tang_entries: Vec<String> = config
            .tang_servers
            .iter()
            .map(|s| format!(r#"{{"url":"{}"}}"#, s.url))
            .collect();
        let sss_config = format!(
            r#"{{"t":{},"pins":{{"tang":[{}]}}}}"#,
            config.tang_threshold,
            tang_entries.join(",")
        );

        // Write the LUKS passphrase to a root-only tempfile so it never appears
        // in the clevis bind command line (visible in /proc/<pid>/cmdline) or in
        // any log message.  `shred -u` is called in a finally-style block so the
        // file is removed even when the bind step fails.
        //
        // Security notes:
        //   - The key is written via a separate command; it still travels over the
        //     SSH channel but is NOT logged (we explicitly skip log_and_execute).
        //   - install(1) creates the file with 0600 atomically before content is
        //     written, so there is no race window where another process could read
        //     a world-readable file containing the passphrase.
        //   - `shred` overwrites the file before unlinking; `rm -f` is a fallback
        //     for filesystems where shred is not installed.
        let tmp_key_path = "/run/.uaa-tang-enroll.key";

        // Create empty 0600 file
        let mk_tmp = format!("install -m 0600 /dev/null {}", tmp_key_path);
        if let Err(e) = self.runner.execute(&mk_tmp).await {
            warn!("Clevis enrollment: could not create key tempfile ({}); skipping", e);
            return Ok(());
        }

        // Write passphrase — NOT logged (no log_and_execute)
        let write_key = format!(
            "printf '%s' '{}' > {}",
            config.luks_key.replace('\'', r"'\''"),
            tmp_key_path
        );
        if let Err(e) = self.runner.execute(&write_key).await {
            let _ = self.runner.execute(&format!("shred -u {} 2>/dev/null || rm -f {}", tmp_key_path, tmp_key_path)).await;
            warn!("Clevis enrollment: could not write key to tempfile ({}); skipping", e);
            return Ok(());
        }

        // Run clevis bind — key path in command, not the key itself
        let bind_cmd = format!(
            "clevis luks bind -d {} -k {} sss '{}'",
            luks_part, tmp_key_path, sss_config
        );
        info!("Executing: Enroll Tang via clevis SSS (passphrase via tempfile, redacted)");
        let bind_result = self.runner.execute(&bind_cmd).await;

        // Always shred the tempfile regardless of outcome
        let _ = self.runner.execute(
            &format!("shred -u {} 2>/dev/null || rm -f {}", tmp_key_path, tmp_key_path)
        ).await;

        if let Err(e) = bind_result {
            warn!(
                "Clevis Tang enrollment failed (non-fatal — passphrase fallback remains): {}",
                e
            );
        } else {
            info!("Tang/Clevis SSS enrollment complete");
        }

        Ok(())
    }

    /// Build the `/etc/dracut.conf.d` fragment that pulls both LUKS unlock
    /// subsystems into the initramfs.
    ///
    /// - `clevis`  satisfies the Tang (network) keyslot.
    /// - `crypt` + `tpm2-tss` + the cryptsetup token plugins let
    ///   systemd-cryptsetup satisfy the TPM2+PIN and FIDO2/YubiKey keyslots at
    ///   the boot prompt.
    /// - `mdraid` assembles a BIOS/IMSM fake-RAID root (e.g. /dev/md126) in the
    ///   initramfs BEFORE LUKS/ZFS, so the array exists when unlock runs. Only
    ///   added for md-backed targets.
    ///
    /// NOTE: the exact module/plugin set is confirmed on the QEMU+swtpm VM
    /// before any real host is installed (see PLAN test strategy).
    fn build_dracut_crypt_conf(include_clevis: bool, include_mdraid: bool, nic_driver: &str) -> String {
        // `network` is REQUIRED alongside clevis: Tang unlock happens over the net
        // in the initramfs, so the network stack must be present. Without it the
        // initramfs "fails to start the network", clevis can't reach Tang, LUKS
        // never opens, and the zfs import (rpool on /dev/mapper/luks) fails.
        let clevis = if include_clevis { " clevis network" } else { "" };
        let mdraid = if include_mdraid { " mdraid" } else { "" };
        // For an md/IMSM root, also bake the array config (/etc/mdadm/mdadm.conf,
        // the ARRAY UUID lines) into the initramfs and force the raid1 driver, so
        // the fake-RAID volume assembles deterministically at boot before LUKS.
        // NOTE: the dracut module is `mdraid` (dir 90mdraid); there is no `mdadm`
        // dracut module — `mdadmconf` is the directive that includes mdadm.conf.
        let mdadm_extra = if include_mdraid {
            "mdadmconf=\"yes\"\n\
             add_drivers+=\" md_mod raid1 \"\n"
        } else {
            ""
        };
        // Force the boot NIC's kernel driver into the initramfs for Tang unlock —
        // dracut hostonly omits it because the NIC isn't needed to reach a LOCAL
        // root, so `rd.neednet=1 ip=dhcp` has no device to bring up otherwise.
        let nic = if include_clevis && !nic_driver.is_empty() {
            format!("add_drivers+=\" {} \"\n", nic_driver)
        } else {
            String::new()
        };
        format!(
            "# Managed by ubuntu-autoinstall-agent — do not edit by hand.\n\
             # Unlock subsystems + ZFS import must live in the initramfs:\n\
             #   crypt/tpm2/fido2 -> systemd-cryptsetup for TPM2+PIN and YubiKey\n\
             #   clevis+network   -> Tang (network) unlock (needs NIC driver, below)\n\
             #   mdraid           -> assemble BIOS/IMSM fake-RAID (md) root before unlock\n\
             #   zfs              -> import rpool/bpool\n\
             add_dracutmodules+=\" crypt tpm2-tss zfs{clevis}{mdraid} \"\n\
             {mdadm_extra}\
             {nic}\
             # cryptsetup token plugins + libfido2 so TPM2/FIDO2 slots resolve in initrd\n\
             install_optional_items+=\" /usr/lib/*/cryptsetup/libcryptsetup-token-systemd-tpm2.so /usr/lib/*/cryptsetup/libcryptsetup-token-systemd-fido2.so /usr/lib/*/libfido2.so* \"\n"
        )
    }

    /// Write the dracut crypt-module config into the target.
    async fn configure_dracut_crypt_modules(&mut self, config: &InstallationConfig) -> Result<()> {
        if config.initramfs_type != InitramfsType::Dracut {
            return Ok(());
        }
        info!("Configuring dracut modules for clevis + systemd-cryptsetup (TPM2/FIDO2)");
        let is_md = config.disk_device.starts_with("/dev/md");
        // Detect the boot NIC's kernel driver (for Tang network unlock) from the
        // live env — the config interface name matches (predictable naming).
        // SECURITY: network_interface may come from a server-fetched config in the
        // USB/netboot flow, so validate it as a real iface name (no shell
        // metacharacters) before interpolating, and validate the returned driver
        // before it lands in the dracut conf. Otherwise skip forcing a driver.
        let iface = config.network_interface.as_str();
        let iface_ok = !iface.is_empty()
            && iface.len() <= 15
            && iface
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
        let nic_driver = if iface_ok {
            let drv = self
                .runner
                .execute_with_output(&format!(
                    "basename \"$(readlink -f /sys/class/net/{}/device/driver 2>/dev/null)\" 2>/dev/null || true",
                    iface
                ))
                .await
                .unwrap_or_default();
            let drv = drv.trim().to_string();
            if !drv.is_empty()
                && drv
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            {
                drv
            } else {
                String::new()
            }
        } else {
            warn!("network_interface '{}' failed validation; not forcing a NIC driver into initramfs", iface);
            String::new()
        };
        let nic_driver = nic_driver.as_str();
        if !config.tang_servers.is_empty() {
            info!("Tang unlock: forcing NIC driver '{}' into initramfs", nic_driver);
        }
        let conf =
            Self::build_dracut_crypt_conf(!config.tang_servers.is_empty(), is_md, nic_driver);
        let cmd = format!(
            "mkdir -p /mnt/targetos/etc/dracut.conf.d && cat > /mnt/targetos/etc/dracut.conf.d/90-uaa-crypt.conf <<'UAA_DRACUT_EOF'\n{}UAA_DRACUT_EOF",
            conf
        );
        let _ = self.log_and_execute("Write dracut crypt-module config", &cmd).await;

        // For an md-backed target, the initramfs also needs the array definition
        // so it can assemble it at boot. `mdadm --detail --scan` runs in the LIVE
        // env (where the array is already assembled) and its output — the ARRAY
        // lines for the IMSM container + volume — is written into the target's
        // /etc/mdadm/mdadm.conf, which the dracut `mdraid` module reads.
        if is_md {
            let mdadm_conf_cmd =
                "mkdir -p /mnt/targetos/etc/mdadm && mdadm --detail --scan > /mnt/targetos/etc/mdadm/mdadm.conf && cat /mnt/targetos/etc/mdadm/mdadm.conf";
            let _ = self
                .log_and_execute("Write target /etc/mdadm/mdadm.conf from live array scan", mdadm_conf_cmd)
                .await;
        }
        Ok(())
    }

    /// Build the EnvironmentFile seed consumed by the first-boot TPM2 unit.
    /// `systemd-cryptenroll` reads `$PASSWORD` (existing) and `$NEWPIN` (new PIN)
    /// from the environment automatically.
    fn build_tpm2_enroll_seed(password: &str, pin: &str, pcr_ids: &str, luksdev: &str) -> String {
        // Quoted-heredoc delivery means no shell interpolation, so raw values are
        // safe here. systemd EnvironmentFile treats the rest of the line as the
        // value; wrap in double quotes so a value with spaces is preserved.
        format!(
            "# Managed by ubuntu-autoinstall-agent — first-boot TPM2 enrollment.\n\
             # 0600, shredded by the unit after a successful enrollment.\n\
             PASSWORD=\"{password}\"\n\
             NEWPIN=\"{pin}\"\n\
             PCRS=\"{pcr_ids}\"\n\
             LUKSDEV=\"{luksdev}\"\n"
        )
    }

    /// Build the one-shot, self-removing systemd unit that enrolls the TPM2+PIN
    /// keyslot on first boot (binding the *installed* system's real PCRs).
    fn build_tpm2_enroll_unit() -> String {
        "# Managed by ubuntu-autoinstall-agent — one-shot, self-removing.\n\
         [Unit]\n\
         Description=First-boot TPM2+PIN LUKS enrollment\n\
         After=local-fs.target\n\
         ConditionPathExists=/etc/uaa-tpm2-enroll.env\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         EnvironmentFile=/etc/uaa-tpm2-enroll.env\n\
         ExecStart=/usr/bin/systemd-cryptenroll --tpm2-device=auto --tpm2-with-pin=yes --tpm2-pcrs=${PCRS} ${LUKSDEV}\n\
         ExecStartPost=/usr/bin/systemctl disable uaa-tpm2-enroll.service\n\
         ExecStartPost=-/bin/sh -c 'command -v shred >/dev/null && shred -u /etc/uaa-tpm2-enroll.env || rm -f /etc/uaa-tpm2-enroll.env'\n\
         ExecStartPost=-/bin/rm -f /etc/systemd/system/uaa-tpm2-enroll.service\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
            .to_string()
    }

    /// Stage first-boot TPM2+PIN enrollment: write the secret seed (0600) and the
    /// one-shot unit into the target, then enable it. The unit shreds the seed
    /// and deletes itself after the first successful run.
    async fn setup_tpm2_firstboot_enrollment(
        &mut self,
        config: &InstallationConfig,
        uuid_opt: Option<&str>,
    ) -> Result<()> {
        let pin = match config.tpm2_pin.as_deref() {
            Some(p) if !p.is_empty() => p,
            _ => return Ok(()),
        };
        info!("Staging first-boot TPM2+PIN LUKS enrollment (self-removing unit)");

        let luksdev = match uuid_opt {
            Some(u) if !u.trim().is_empty() => format!("/dev/disk/by-uuid/{}", u.trim()),
            _ => partition_path(&config.disk_device, 4),
        };

        // Seed contains the passphrase + PIN — write via unlogged execute + a
        // quoted heredoc so the secrets are neither logged nor interpolated.
        let seed = Self::build_tpm2_enroll_seed(&config.luks_key, pin, &config.tpm2_pcr_ids, &luksdev);
        let write_seed = format!(
            "install -m 0600 /dev/null /mnt/targetos/etc/uaa-tpm2-enroll.env && cat > /mnt/targetos/etc/uaa-tpm2-enroll.env <<'UAA_TPM2_SEED_EOF'\n{}UAA_TPM2_SEED_EOF",
            seed
        );
        if let Err(e) = self.runner.execute(&write_seed).await {
            warn!("TPM2 enrollment: could not write seed ({}); skipping TPM2 slot", e);
            return Ok(());
        }

        // Unit body has no secrets — safe to log.
        let unit = Self::build_tpm2_enroll_unit();
        let write_unit = format!(
            "cat > /mnt/targetos/etc/systemd/system/uaa-tpm2-enroll.service <<'UAA_TPM2_UNIT_EOF'\n{}UAA_TPM2_UNIT_EOF",
            unit
        );
        let _ = self.log_and_execute("Write first-boot TPM2 enrollment unit", &write_unit).await;
        let _ = self.log_and_execute(
            "Enable first-boot TPM2 enrollment unit",
            "chroot /mnt/targetos bash -lc 'systemctl enable uaa-tpm2-enroll.service'",
        ).await;

        Ok(())
    }

    /// Final cleanup and unmounting
    pub async fn final_cleanup(&mut self, _config: &InstallationConfig) -> Result<()> {
        info!("Performing final cleanup");

        for (desc, cmd) in [
            ("Unmounting /sys (recursive)", "umount -R /mnt/targetos/sys || true"),
            ("Unmounting /proc (recursive)", "umount -R /mnt/targetos/proc || true"),
            ("Unmounting /dev (recursive)", "umount -R /mnt/targetos/dev || true"),
            ("Unmounting /run (recursive)", "umount -R /mnt/targetos/run || true"),
            ("Unmounting ESP", "umount /mnt/targetos/boot/efi || true"),
            ("Exporting bpool", "zpool export bpool || true"),
            ("Exporting rpool", "zpool export rpool || true"),
            ("Unmounting /mnt/luks if mounted", "mountpoint -q /mnt/luks && umount -lf /mnt/luks || true"),
            ("Closing LUKS mapper if open", "cryptsetup status luks >/dev/null 2>&1 && cryptsetup close luks || true"),
        ] {
            self.log_and_execute(desc, cmd).await?;
        }

        info!("Final cleanup completed");
        Ok(())
    }

    /// Helper method to log and execute commands
    async fn log_and_execute(&mut self, description: &str, command: &str) -> Result<()> {
        info!("Executing: {} -> {}", description, command);
        self.runner.execute(command).await
    }

    /// Execute a command but tolerate known benign zsys errors in chroot contexts.
    async fn run_tolerating_zsys_errors(&mut self, description: &str, command: &str) -> Result<()> {
        match self.runner.execute(command).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let (code, _stdout, stderr) = self
                    .runner
                    .execute_with_error_collection(command, description)
                    .await?;

                if code == 0 {
                    Ok(())
                } else {
                    let s = stderr.to_lowercase();
                    let has_zsys = (s.contains("zsys") && s.contains("daemon"))
                        || s.contains("/run/zsysd.sock")
                        || s.contains("couldn't connect to zsys daemon");

                    if has_zsys {
                        warn!(
                            "Ignoring benign zsys error for '{}': exit={} stderr={}",
                            description, code, stderr
                        );
                        Ok(())
                    } else {
                        Err(e)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_esp_detection_command_contains_expected_parts() {
        let guid = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
        let cmd = SystemConfigurator::build_esp_detection_command(guid);
        assert!(cmd.starts_with("bash -lc '"));
        assert!(cmd.contains("lsblk -rP -o PATH,PARTTYPE"));
        assert!(cmd.contains("grep -i \"PARTTYPE=\\\""));
        assert!(cmd.contains(guid));
        assert!(cmd.ends_with("'"));
    }

    #[test]
    fn test_boot_order_cmd_matches_usb_script_regexes() {
        let cmd = SystemConfigurator::build_boot_order_cmd();
        assert!(cmd.contains("PXE"));
        assert!(cmd.contains("[Nn]etwork"));
        assert!(cmd.contains("IPv[46]"));
        assert!(cmd.contains("[Uu]buntu"));
        assert!(cmd.contains("[0-9A-Fa-f]\\{4\\}"));
        assert!(cmd.contains("efibootmgr -o"));
    }

    #[test]
    fn test_boot_order_cmd_is_chrooted_and_nonfatal() {
        let cmd = SystemConfigurator::build_boot_order_cmd();
        assert!(cmd.starts_with("chroot /mnt/targetos bash -lc"));

        // Extract the payload between the outer single quotes of `bash -lc '...'`
        // and verify no interior single quote breaks out of that argument.
        let marker = "bash -lc '";
        let start = cmd.find(marker).expect("bash -lc marker present") + marker.len();
        assert!(cmd.ends_with('\''));
        let inner = &cmd[start..cmd.len() - 1];
        assert!(!inner.contains('\''));

        // Every skip path (efibootmgr missing, unreadable, no entries) plus the
        // final trailing statement exits 0 — non-fatal by design.
        assert!(cmd.matches("exit 0").count() >= 4);
    }

    #[test]
    fn test_boot_order_cmd_attempts_order_when_entries_exist() {
        let cmd = SystemConfigurator::build_boot_order_cmd();
        // The `efibootmgr -o` invocation is guarded ONLY by `[ -n "$order" ]`,
        // never by the ubuntu entry existing — anti-over-suppression: an
        // absent-ubuntu order (net,rest) must still reach efibootmgr -o.
        assert!(cmd.contains(
            "[ -n \"$order\" ] || { echo \"uaa: no EFI boot entries found; skipping boot order\"; exit 0; }; efibootmgr -o \"$order\""
        ));
    }

    #[test]
    fn test_choose_esp_partition_uses_detected_when_present() {
        let detected = "/dev/nvme0n1p1\n";
        let chosen = SystemConfigurator::choose_esp_partition(detected, "/dev/nvme0n1");
        assert_eq!(chosen, "/dev/nvme0n1p1");
    }

    #[test]
    fn test_choose_esp_partition_falls_back_when_empty() {
        let detected = "  \n\t";
        let chosen = SystemConfigurator::choose_esp_partition(detected, "/dev/sda");
        assert_eq!(chosen, "/dev/sda1");
    }

    #[test]
    fn test_build_apt_deb822_sources_plucky() {
        let s = SystemConfigurator::build_apt_deb822_sources("plucky");
        assert!(s.contains("Types: deb"));
        assert!(s.contains("URIs: http://archive.ubuntu.com/ubuntu/"));
        assert!(s.contains("Suites: plucky"));
        assert!(s.contains("Suites: plucky-security"));
        assert!(s.contains("Components: main restricted universe multiverse"));
    }

    #[test]
    fn test_build_crypttab_entry_with_uuid() {
        let e = SystemConfigurator::build_crypttab_entry("/dev/nvme0n1", Some("abcd-1234"));
        assert_eq!(
            e,
            "luks /dev/disk/by-uuid/abcd-1234 none luks,discard,initramfs"
        );
    }

    #[test]
    fn test_build_crypttab_entry_without_uuid() {
        let e = SystemConfigurator::build_crypttab_entry("/dev/sda", None);
        assert_eq!(e, "luks /dev/sda4 none luks,discard,initramfs");
    }

    #[test]
    fn test_build_crypttab_entry_with_empty_uuid() {
        let e = SystemConfigurator::build_crypttab_entry("/dev/sda", Some("  "));
        assert_eq!(e, "luks /dev/sda4 none luks,discard,initramfs");
    }

    fn sample_netplan_config(network_address: &str, network_renderer: &str) -> InstallationConfig {
        InstallationConfig {
            hostname: "test-host".into(),
            disk_device: "/dev/nvme0n1".into(),
            timezone: "UTC".into(),
            luks_key: "key".into(),
            root_password: "root".into(),
            network_interface: "eth0".into(),
            network_address: network_address.into(),
            network_gateway: "192.0.2.1".into(),
            network_search: "example.test".into(),
            network_nameservers: vec!["1.1.1.1".into()],
            network_renderer: network_renderer.into(),
            debootstrap_release: None,
            debootstrap_mirror: None,
            initramfs_type: InitramfsType::Dracut,
            tang_servers: vec![],
            tang_threshold: 2,
            ssh_authorized_keys: vec![],
            enroll_tpm2: true,
            tpm2_pin: None,
            tpm2_pcr_ids: "7".into(),
            expect_fido2: true,
        }
    }

    #[test]
    fn test_build_netplan_yaml_default_renderer_static() {
        let cfg = sample_netplan_config("192.0.2.10/24", "networkd");
        let yaml = SystemConfigurator::build_netplan_yaml(&cfg).unwrap();
        assert!(yaml.contains("renderer: networkd"));
        assert!(yaml.contains("addresses:"));
        assert!(yaml.contains("192.0.2.10/24"));
    }

    #[test]
    fn test_build_netplan_yaml_networkmanager() {
        let cfg = sample_netplan_config("192.0.2.10/24", "NetworkManager");
        let yaml = SystemConfigurator::build_netplan_yaml(&cfg).unwrap();
        assert!(yaml.contains("renderer: NetworkManager"));
    }

    #[test]
    fn test_build_netplan_yaml_rejects_unknown_renderer() {
        let cfg = sample_netplan_config("192.0.2.10/24", "netword");
        assert!(SystemConfigurator::build_netplan_yaml(&cfg).is_err());
    }

    #[test]
    fn test_build_netplan_yaml_dhcp() {
        let cfg = sample_netplan_config("dhcp", "networkd");
        let yaml = SystemConfigurator::build_netplan_yaml(&cfg).unwrap();
        assert!(yaml.contains("dhcp4: true"));
        assert!(!yaml.contains("addresses:"));
        assert!(!yaml.contains("- dhcp"));
    }

    #[test]
    fn test_build_netplan_yaml_dhcp_uppercase() {
        let cfg = sample_netplan_config("DHCP", "networkd");
        let yaml = SystemConfigurator::build_netplan_yaml(&cfg).unwrap();
        assert!(yaml.contains("dhcp4: true"));
        assert!(!yaml.contains("addresses:"));
    }

    /// Extract the enabled dracut module list (the value of `add_dracutmodules+=`).
    fn dracut_modules_line(conf: &str) -> String {
        conf.lines()
            .find(|l| l.trim_start().starts_with("add_dracutmodules+="))
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn test_dracut_crypt_conf_includes_both_subsystems() {
        let conf = SystemConfigurator::build_dracut_crypt_conf(true, true, "ixgbe");
        let modules = dracut_modules_line(&conf);
        // Both unlock subsystems must be enabled in the initramfs module list.
        assert!(modules.contains("clevis"), "Tang unlock (clevis) missing: {modules}");
        assert!(modules.contains("crypt"), "systemd-cryptsetup (crypt) missing: {modules}");
        assert!(modules.contains("tpm2-tss"), "TPM2 support missing: {modules}");
        // md-backed target -> mdraid must be present to assemble the array.
        assert!(modules.contains("mdraid"), "mdraid module missing for md target: {modules}");
        // zfs module must be present so rpool/bpool import in the initramfs.
        assert!(modules.contains("zfs"), "zfs module missing: {modules}");
        // Tang unlock needs the network stack + the NIC driver in the initramfs.
        assert!(modules.contains("network"), "network module missing for Tang: {modules}");
        assert!(
            conf.contains("add_drivers+=\" ixgbe \""),
            "NIC driver not forced into initramfs for Tang: {conf}"
        );
        // FIDO2 token plugin is pulled via install_optional_items, not a module.
        assert!(
            conf.contains("libcryptsetup-token-systemd-fido2.so"),
            "FIDO2 token plugin missing"
        );
    }

    #[test]
    fn test_dracut_crypt_conf_omits_clevis_and_mdraid_when_not_needed() {
        let conf = SystemConfigurator::build_dracut_crypt_conf(false, false, "");
        let modules = dracut_modules_line(&conf);
        assert!(
            !modules.contains("clevis"),
            "clevis module should be absent with no Tang servers: {modules}"
        );
        assert!(
            !modules.contains("network"),
            "network module should be absent with no Tang servers: {modules}"
        );
        assert!(
            !modules.contains("mdraid"),
            "mdraid module should be absent for a non-md disk: {modules}"
        );
        // TPM2/FIDO2 support still present for the non-Tang keyslots.
        assert!(modules.contains("crypt"));
        assert!(modules.contains("tpm2-tss"));
    }

    #[test]
    fn test_tpm2_enroll_seed_carries_password_pin_and_device() {
        let seed = SystemConfigurator::build_tpm2_enroll_seed(
            "s3cret pass",
            "1234",
            "7",
            "/dev/disk/by-uuid/abcd-1234",
        );
        // systemd-cryptenroll reads $PASSWORD (existing) and $NEWPIN (new pin).
        assert!(seed.contains("PASSWORD=\"s3cret pass\""));
        assert!(seed.contains("NEWPIN=\"1234\""));
        assert!(seed.contains("PCRS=\"7\""));
        assert!(seed.contains("LUKSDEV=\"/dev/disk/by-uuid/abcd-1234\""));
    }

    #[test]
    fn test_tpm2_enroll_unit_is_oneshot_and_self_removing() {
        let unit = SystemConfigurator::build_tpm2_enroll_unit();
        assert!(unit.contains("Type=oneshot"));
        assert!(unit.contains("--tpm2-with-pin=yes"));
        assert!(unit.contains("--tpm2-pcrs=${PCRS}"));
        assert!(unit.contains("ConditionPathExists=/etc/uaa-tpm2-enroll.env"));
        // Must disable itself and shred the secret seed after first run.
        assert!(unit.contains("systemctl disable uaa-tpm2-enroll.service"));
        assert!(unit.contains("shred -u /etc/uaa-tpm2-enroll.env"));
        assert!(unit.contains("rm -f /etc/systemd/system/uaa-tpm2-enroll.service"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }
}
