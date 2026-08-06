// file: crates/uaa-core/src/network/ssh_installer/system_setup.rs
// version: 2.35.0
// guid: sshsys01-2345-6789-abcd-ef0123456789
// last-edited: 2026-08-06

//! System setup and configuration for SSH/local installation.
//!
//! Supports both initramfs-tools and dracut. When dracut is selected the GRUB
//! kernel command line receives `rd.neednet=1 ip=dhcp` so the Tang servers are
//! reachable during initramfs boot for clevis-based LUKS unlock.

use super::config::{Arch, DiskRole, InitramfsType, InstallationConfig, StorageMode, UserAccount};
use super::packages::{clevis23_apt_config_commands, target_pkcs11_package_suffix};
use super::partitions::partition_path;
use super::unlock_sss::{SssPolicy, UnlockPin, DEFAULT_PKCS11_MECHANISM};
use crate::network::CommandExecutor;
use crate::Result;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use tracing::{info, warn};

/// grub.d drop-in that gives every installed host a serial console.
///
/// The fleet is headless (Supermicro/Lenovo servers watched over IPMI SOL), so
/// the boot — including the LUKS/keystore unlock prompt — must land on `ttyS0`
/// or it is invisible remotely *and* to the VM-gate disk-boot serial capture.
///
/// Written as `/etc/default/grub.d/99-uaa-serial-console.cfg`, which
/// `grub-mkconfig` sources **after** `/etc/default/grub`, so
/// `GRUB_CMDLINE_LINUX="$GRUB_CMDLINE_LINUX …"` **appends** to whatever the
/// dracut+Tang step already set rather than clobbering it. `ttyS0` is listed
/// **last** so it is the primary console (kernel log + login getty) on a
/// headless box; `console=tty0` keeps a local VGA console too. Harmless on a
/// host with no physical UART (the kernel just never opens it).
const SERIAL_CONSOLE_DROPIN: &str = "# file: /etc/default/grub.d/99-uaa-serial-console.cfg\n# Installed by ubuntu-autoinstall-agent: expose boot + LUKS unlock + emergency\n# shell on serial for IPMI SOL. Supermicro X10 BMC SOL = COM2/ttyS1 (IPMI SOL\n# payload channel 1), NOT ttyS0/COM1 — emit to BOTH COM ports with ttyS1 LAST so\n# it wins /dev/console (interactive LUKS/maintenance prompts land where SOL reads).\n# Sourced after /etc/default/grub, so this APPENDS.\nGRUB_CMDLINE_LINUX=\"$GRUB_CMDLINE_LINUX console=tty0 console=ttyS0,115200n8 console=ttyS1,115200n8\"\nGRUB_TERMINAL=\"console serial\"\nGRUB_SERIAL_COMMAND=\"serial --speed=115200 --unit=1 --word=8 --parity=no --stop=1\"\n";

/// One Tang server plus the on-disk path of its **pre-fetched** advertisement.
///
/// `adv` is a path, not the advertisement body — clevis reads the file itself.
/// Field order is the serialized key order (`url` then `adv`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct TangAdv<'a> {
    pub url: &'a str,
    pub adv: &'a str,
}

/// The clevis `tpm2` pin parameters.
///
/// `pcr_bank` is explicit because clevis defaults to `sha1`, which Secure Boot
/// does not populate — a sha1-banked PCR7 seal unseals unconditionally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Tpm2Peer<'a> {
    pub pcr_ids: &'a str,
    pub pcr_bank: &'a str,
}

/// `{"tang":[…]}` — the inner `pins` object of the Tang group.
#[derive(serde::Serialize)]
struct TangPins<'a> {
    tang: &'a [TangAdv<'a>],
}

/// `{"t":N,"pins":{"tang":[…]}}` — the legacy flat policy, and also the inner
/// group of the nested policy.
#[derive(serde::Serialize)]
struct TangGroup<'a> {
    t: u8,
    pins: TangPins<'a>,
}

/// `{"tpm2":{…},"sss":[{…}]}` — outer pins of the nested AND policy.
///
/// `sss` is a one-element array so the whole Tang group counts as exactly one
/// outer share.
#[derive(serde::Serialize)]
struct NestedPins<'a> {
    tpm2: Tpm2Peer<'a>,
    sss: [TangGroup<'a>; 1],
}

/// `{"t":2,"pins":{"tpm2":{…},"sss":[…]}}` — tpm2 AND Tang.
#[derive(serde::Serialize)]
struct NestedPolicy<'a> {
    t: u8,
    pins: NestedPins<'a>,
}

/// The clevis `pkcs11` pin parameters (e.g. a YubiKey PIV slot URI).
///
/// `mechanism` is the only optional key clevis 23 reads besides `uri`, and it is
/// `skip_serializing_if` so an unset one does not appear in the binding at all —
/// `clevis-decrypt-pkcs11` passes `--mechanism` only when non-empty, and an
/// explicit `"mechanism":""` would be a gratuitous diff against every binding
/// authored before the field existed. Module path and slot are NOT keys here:
/// clevis derives both from the URI. See [`super::unlock_sss::Pkcs11Pin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
struct Pkcs11Peer<'a> {
    uri: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    mechanism: Option<&'a str>,
}

pub struct SystemConfigurator<'a> {
    runner: &'a mut dyn CommandExecutor,
}

impl<'a> SystemConfigurator<'a> {
    pub fn new(runner: &'a mut dyn CommandExecutor) -> Self {
        Self { runner }
    }

    /// Does this host unlock via Tang at all — flat roster **or** anywhere
    /// inside an authored [`SssPolicy`] tree?
    ///
    /// Tang is the only unlock factor that needs the *network* up inside the
    /// initramfs, so this (not `!tang_servers.is_empty()`) is the predicate for
    /// `rd.neednet=1`, the static `ip=` cmdline, and the forced NIC driver. A
    /// host that declares its Tang servers only in the tree has an empty
    /// `tang_servers`; gating on that roster ships an initramfs with no network
    /// and no NIC driver, so the Tang bind exists but can never be satisfied at
    /// boot — a bricked host from a green install.
    fn uses_tang(config: &InstallationConfig) -> bool {
        !config.tang_servers.is_empty()
            || config
                .unlock_sss
                .as_ref()
                .is_some_and(|p| !p.tang_urls().is_empty())
    }

    /// Build the command used to detect the ESP partition by GUID
    fn build_esp_detection_command(guid: &str) -> String {
        format!(
            // `-P` (pairs) WITHOUT `-r`: util-linux on Ubuntu 26.04 makes --raw
            // and --pairs mutually exclusive, so `lsblk -rP` errors out and the
            // detection silently returns nothing. `-P` alone still emits the
            // KEY=\"value\" pairs this sed parses.
            "bash -lc 'lsblk -P -o PATH,PARTTYPE | grep -i \"PARTTYPE=\\\"{0}\\\"\" | head -n1 | sed -n \"s/.*PATH=\\\"\\([^\\\" ]*\\)\\\".*/\\1/p\"'",
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

    /// Detect the ESP partition path.
    ///
    /// NativeKeystore: resolve directly to `<first System disk>-part1` (by-id).
    /// GUID detection can't be used here — the USB installer stick's ESP is also
    /// type EF00, so `head -n1` could pick it over the target disk — and there is
    /// no single `disk_device` to fall back on.
    ///
    /// PlainLuks: GUID PARTTYPE detection, fallback to partition 1 of the
    /// configured disk (suffix-aware: nvme0n1p1 / sda1) if not found.
    async fn detect_esp_partition_path(&mut self, config: &InstallationConfig) -> Result<String> {
        if config.storage_mode == StorageMode::NativeKeystore {
            if let Some(sys) = config.disks.iter().find(|d| d.role == DiskRole::System) {
                return Ok(format!("{}-part1", sys.id));
            }
        }
        let guid = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
        let cmd = Self::build_esp_detection_command(guid);
        let out = self
            .runner
            .execute_with_output(&cmd)
            .await
            .unwrap_or_default();
        Ok(Self::choose_esp_partition(&out, &config.disk_device))
    }

    /// Install the serial-console grub.d drop-in on the target ([`SERIAL_CONSOLE_DROPIN`]).
    ///
    /// base64-piped into the chroot so the `$`, quotes, and spaces in the config
    /// survive the SSH → `bash -lc` → file hops without fragile escaping. Fatal
    /// (`?`): a headless server that can't be watched over SOL is a failed
    /// deployment, and a `mkdir -p` + file write in the chroot does not
    /// legitimately fail. MUST be called before `update-grub`.
    ///
    /// Gated on `config.arch == Arch::Amd64` (skipped for `Arch::Arm64`): today's
    /// fleet is headless x86_64 servers watched over IPMI SOL on `ttyS0`; arm64
    /// targets are not assumed to have an equivalent serial path. `arch` is the
    /// real serialized field added by PS-WIRE-AXES-10 (`#[serde(skip_serializing_if
    /// = "Arch::is_amd64")]`), so every committed amd64 host omits the `arch:` key,
    /// deserializes back to `Arch::Amd64` on the target, and still gets the
    /// serial-console drop-in — the placed artifact stays byte-identical. A
    /// `#[serde(skip)]` flag would NOT survive `config place` -> installer-reads-
    /// serialized-YAML, since it would deserialize back to its default and could
    /// silently stop applying; gating on the real `arch` axis avoids that trap.
    async fn configure_serial_console(&mut self, config: &InstallationConfig) -> Result<()> {
        if config.arch != Arch::Amd64 {
            info!(
                "Skipping serial console (arch={:?}, amd64-only default)",
                config.arch
            );
            return Ok(());
        }
        let b64 = BASE64.encode(SERIAL_CONSOLE_DROPIN);
        let cmd = format!(
            "chroot /mnt/targetos bash -lc 'mkdir -p /etc/default/grub.d && printf %s {b64} | base64 -d > /etc/default/grub.d/99-uaa-serial-console.cfg'"
        );
        self.log_and_execute("Configuring serial console (grub.d drop-in)", &cmd)
            .await?;

        // Explicitly enable a login getty on BOTH COM ports.
        //
        // The GRUB drop-in above only sets the kernel `console=` args (boot +
        // emergency output). It does NOT guarantee an interactive login prompt on
        // real root: systemd-getty-generator auto-spawns serial-getty only on the
        // *last* `console=` (the one that becomes /dev/console), so on this box
        // that lands on ttyS1 alone and ttyS0 gets no prompt. The generator's
        // behavior has also shifted across releases — the reliable answer is to
        // statically enable serial-getty@ on both units so the login prompt shows
        // regardless of which COM port the operator's SOL is wired to (X10 BMC SOL
        // = COM2/ttyS1) and regardless of the generator heuristic. This is
        // real-root only; the dracut initramfs shell uses the kernel console arg,
        // not a getty.
        self.log_and_execute(
            "Enabling serial-getty on ttyS0 + ttyS1",
            "chroot /mnt/targetos bash -lc 'systemctl enable serial-getty@ttyS0.service serial-getty@ttyS1.service'",
        )
        .await?;
        Ok(())
    }

    /// Install base system using debootstrap
    pub async fn install_base_system(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Installing base system");

        self.log_and_execute(
            "Creating ESP mount point",
            "mkdir -p /mnt/targetos/boot/efi",
        )
        .await?;
        let esp_part = self.detect_esp_partition_path(config).await?;
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
        let _ = self
            .log_and_execute(
                "Make /dev private",
                "mount --make-private /mnt/targetos/dev || true",
            )
            .await;
        let _ = self.log_and_execute(
            "Ensuring /dev/pts",
            "[ -d /mnt/targetos/dev/pts ] || mkdir -p /mnt/targetos/dev/pts; mountpoint -q /mnt/targetos/dev/pts || mount -t devpts devpts /mnt/targetos/dev/pts || true"
        ).await;
        let _ = self.log_and_execute(
            "Bind /proc (rbind)",
            "[ -d /mnt/targetos/proc ] || mkdir -p /mnt/targetos/proc; mountpoint -q /mnt/targetos/proc || mount --rbind /proc /mnt/targetos/proc"
        ).await;
        let _ = self
            .log_and_execute(
                "Make /proc private",
                "mount --make-private /mnt/targetos/proc || true",
            )
            .await;
        let _ = self.log_and_execute(
            "Bind /sys (rbind)",
            "[ -d /mnt/targetos/sys ] || mkdir -p /mnt/targetos/sys; mountpoint -q /mnt/targetos/sys || mount --rbind /sys /mnt/targetos/sys"
        ).await;
        let _ = self
            .log_and_execute(
                "Make /sys private",
                "mount --make-private /mnt/targetos/sys || true",
            )
            .await;
        let _ = self.log_and_execute(
            "Bind /run (rbind)",
            "[ -d /mnt/targetos/run ] || mkdir -p /mnt/targetos/run; mountpoint -q /mnt/targetos/run || mount --rbind /run /mnt/targetos/run"
        ).await;
        let _ = self
            .log_and_execute(
                "Make /run private",
                "mount --make-private /mnt/targetos/run || true",
            )
            .await;

        // DNS in chroot
        let _ = self.log_and_execute(
            "Reset chroot resolv.conf",
            "[ -e /mnt/targetos/etc/resolv.conf ] && rm -f /mnt/targetos/etc/resolv.conf; echo 'nameserver 1.1.1.1' > /mnt/targetos/etc/resolv.conf"
        ).await;

        // ESP
        let _ = self
            .log_and_execute(
                "Ensure ESP mountpoint",
                "[ -d /mnt/targetos/boot/efi ] || mkdir -p /mnt/targetos/boot/efi",
            )
            .await;
        let esp_part = self.detect_esp_partition_path(config).await?;
        let _ = self
            .log_and_execute(
                "Mount ESP if not mounted",
                &format!(
                "mountpoint -q /mnt/targetos/boot/efi || mount {} /mnt/targetos/boot/efi || true",
                esp_part
            ),
            )
            .await;

        // fstab entry for ESP (UUID-based)
        let esp_part = self.detect_esp_partition_path(config).await?;
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

        // clevis for Tang. There is no separate clevis-tang package (the tang
        // pin is bundled in base `clevis`), but the TPM2 pin is NOT — its
        // `clevis-decrypt-tpm2` lives in the separate `clevis-tpm2` package.
        // NativeKeystore D2-B binds a clevis SSS *tpm2 pin*, so it MUST pull
        // clevis-tpm2 or the tpm2 share silently fails to unlock in the
        // initramfs (and the clevis-pin-tpm2 / tpm2-tss dracut modules refuse to
        // install for lack of clevis-decrypt-tpm2 + the tpm2 binary).
        // NativeKeystore ALWAYS needs clevis, even with no Tang servers: the
        // `91uaa-keystore-wait` dracut module is installed unconditionally for
        // that storage mode and its `depends()` lists clevis (+ the tang/tpm2/sss
        // pins). Gating clevis on `tang_servers` alone meant a Tang-less
        // native-keystore install wrote a module that could never be built, and
        // `dracut --regenerate-all` died mid-Phase-5 with
        //   "Module 'uaa-keystore-wait' depends on module 'clevis', which can't
        //    be installed"
        // after the pools were already created. Caught by the VM gate.
        // An authored `unlock_sss` tree is ALSO a clevis host, whatever the
        // storage mode and whatever the flat roster says. A host that declares
        // its Tang servers only inside the tree has an EMPTY `tang_servers`, so
        // gating on the roster alone installed no clevis at all, ran no
        // `clevis luks bind`, and still reported a successful install — leaving
        // a machine that boots to a LUKS prompt nobody can satisfy.
        //
        // `clevis_pkcs11_pin` also implies clevis: the pkcs11 pin IS a clevis
        // pin. Without this term a PlainLuks host with no Tang servers would
        // pin the 26.10 pocket and install opensc/pcscd but no clevis at all -
        // a silent success with no unlock factor.
        let needs_clevis = !config.tang_servers.is_empty()
            || config.storage_mode == StorageMode::NativeKeystore
            || config.unlock_sss.is_some()
            || config.clevis_pkcs11_pin;
        let clevis_pkgs = if needs_clevis {
            let base = match config.initramfs_type {
                InitramfsType::Dracut => " clevis clevis-luks clevis-dracut clevis-systemd",
                InitramfsType::InitramfsTools => " clevis clevis-luks clevis-initramfs",
            };
            // Same reasoning one level down: the tpm2 pin's decrypter lives in
            // `clevis-tpm2`, so a PlainLuks host whose TREE carries a tpm2 pin
            // needs it just as much as a NativeKeystore host does — and misses
            // it silently, in the initramfs, at first boot. `contains_kind`
            // recurses, because the pin may sit at any depth.
            let tree_uses_tpm2 = config
                .unlock_sss
                .as_ref()
                .is_some_and(|p| p.contains_kind("tpm2"));
            if config.storage_mode == StorageMode::NativeKeystore || tree_uses_tpm2 {
                format!("{base} clevis-tpm2")
            } else {
                base.to_string()
            }
        } else {
            String::new()
        };

        // TPM2+PIN and FIDO2 keyslots are unlocked by systemd-cryptsetup (its own
        // package, which ships the cryptsetup tpm2/fido2 token plugins). tpm2-tools
        // pulls the libtss2 stack; tpm-udev creates the TPM device nodes;
        // libfido2-1 backs FIDO2. Matches the 003 reference set.
        // NativeKeystore's clevis tpm2 pin needs the tpm2 userspace in the
        // target (tpm2-tools = the `tpm2` binary the clevis-pin-tpm2 / tpm2-tss
        // dracut modules require, + tpm-udev for the TPM device nodes) even when
        // enroll_tpm2 is off. The systemd-cryptsetup TPM2+PIN / FIDO2 keyslot
        // path additionally needs systemd-cryptsetup's token plugins + libfido2.
        let needs_tpm2_userspace = config.storage_mode == StorageMode::NativeKeystore
            || config.enroll_tpm2
            || config.expect_fido2;
        let sd_crypt = if config.enroll_tpm2 || config.expect_fido2 {
            " systemd-cryptsetup libfido2-1"
        } else {
            ""
        };
        let crypt_extra = if needs_tpm2_userspace {
            format!(" tpm2-tools tpm-udev{sd_crypt}")
        } else {
            String::new()
        };

        // OPT-IN, OFF BY DEFAULT: the clevis pkcs11 pin (YubiKey PIV) needs
        // clevis 23, which 26.04 does not ship. When enabled, the narrowly
        // pinned 26.10 pocket is written into the target BEFORE `apt update`
        // (ordering is load-bearing — after it, the pocket is unindexed and the
        // chroot silently resolves clevis 20 with no pkcs11 pin, producing a
        // keyslot that never unlocks). `opensc`/`pcscd` come from plain 26.04
        // universe; `clevis` only Recommends opensc and nothing pulls pcscd.
        let pkcs11_apt_setup: Vec<String> = if config.clevis_pkcs11_pin {
            clevis23_apt_config_commands("")
        } else {
            Vec::new()
        };
        let pkcs11_pkgs = target_pkcs11_package_suffix(config.clevis_pkcs11_pin);

        let chroot_commands: Vec<String> = pkcs11_apt_setup
            .into_iter()
            .chain([
            "apt update".to_string(),
            format!(
                "DEBIAN_FRONTEND=noninteractive apt install -y grub-efi-amd64 grub-efi-amd64-signed linux-image-generic shim-signed {} {} efibootmgr cryptsetup dosfstools{}{}{}",
                initramfs_pkg, zfs_pkg, clevis_pkgs, crypt_extra, pkcs11_pkgs
            ),
            "DEBIAN_FRONTEND=noninteractive apt install -y linux-headers-generic".to_string(),
            // `sudo` is load-bearing for provisioned operator accounts (they get
            // password-prompted sudo via the `sudo` group), so install it
            // explicitly rather than relying on it being pulled in transitively.
            "DEBIAN_FRONTEND=noninteractive apt install -y openssh-server sudo vim htop curl".to_string(),
            "DEBIAN_FRONTEND=noninteractive apt purge -y os-prober || true".to_string(),
            "addgroup --system lpadmin || true".to_string(),
            "addgroup --system lxd || true".to_string(),
            "addgroup --system sambashare || true".to_string(),
            ])
            .collect();

        for cmd in chroot_commands {
            let desc = format!("Chroot: {}", cmd);
            let wrapped = format!("chroot /mnt/targetos bash -lc '{}'", cmd);
            self.run_tolerating_zsys_errors(&desc, &wrapped).await?;
        }

        // /etc/hostid for ZFS — MUST match the hostid the pools were created
        // under (the LIVE environment's hostid), or the initramfs `zpool import`
        // sees the pool as "last used by another system" and (if the pool wasn't
        // cleanly exported) refuses to import → `zfs-import.target` never
        // activates → the boot hangs.
        //
        // The old code ran INSIDE the chroot: `zgenhostid -f /etc/hostid` is
        // malformed (treats the path as a *value*), and the fallback `hostid >
        // /etc/hostid` wrote the chroot's null hostid as the ASCII text
        // "00000000" — never a valid 4-byte binary hostid, and never matching the
        // pool. Fix: run on the LIVE host (where `hostid` returns the value the
        // pools were created with) and write a proper binary file with zgenhostid.
        // Must precede the initramfs regen so the correct hostid is baked in.
        self.log_and_execute(
            "Write /etc/hostid matching pool (live-env hostid)",
            "HID=$(hostid); zgenhostid -f -o /mnt/targetos/etc/hostid \"0x${HID}\"",
        )
        .await?;

        // Root password. Base64-encode the `root:password` pair in Rust and
        // decode inside the chroot (`base64 -d`), so no shell metacharacter in
        // the password can break out of the outer `bash -c` + inner `bash -lc`
        // nesting — mandatory now that the password may be a *generated random*
        // string (which contains arbitrary charset bytes) as well as a literal.
        // An empty root password locks the account (key/console-only) rather
        // than setting a passwordless root.
        if config.root_password.is_empty() {
            let _ = self
                .log_and_execute(
                    "Lock root password (empty config → key/console-only)",
                    "chroot /mnt/targetos bash -lc 'passwd -l root || true'",
                )
                .await;
        } else {
            let creds_b64 = BASE64.encode(format!("root:{}", config.root_password));
            let _ = self
                .log_and_execute(
                    "Setting root password",
                    &format!(
                        "chroot /mnt/targetos bash -lc 'echo {creds_b64} | base64 -d | chpasswd'"
                    ),
                )
                .await;
        }

        // SSH authorized keys for root
        if !config.ssh_authorized_keys.is_empty() {
            let _ = self
                .log_and_execute(
                    "Create root .ssh dir",
                    "chroot /mnt/targetos bash -lc 'mkdir -p /root/.ssh && chmod 700 /root/.ssh'",
                )
                .await;
            for key in &config.ssh_authorized_keys {
                let cmd = format!(
                    "chroot /mnt/targetos bash -lc \"echo '{}' >> /root/.ssh/authorized_keys\"",
                    key
                );
                let _ = self
                    .log_and_execute("Inject SSH authorized key", &cmd)
                    .await;
            }
            let _ = self
                .log_and_execute(
                    "Fix authorized_keys permissions",
                    "chroot /mnt/targetos bash -lc 'chmod 600 /root/.ssh/authorized_keys || true'",
                )
                .await;
        }

        // Operator user accounts. Empty `users` (every pre-existing config)
        // leaves this a no-op, preserving the root-only prior behavior.
        for user in &config.users {
            // Identifier fields are interpolated literally into shell commands,
            // so reject any that aren't safe bare tokens (the free-form password
            // and keys are base64'd, so they need no such guard). Fail-safe:
            // skip the whole user rather than run a tainted command.
            if !is_safe_ident(&user.name)
                || !is_safe_ident(&user.shell)
                || user.groups.iter().any(|g| !is_safe_ident(g))
            {
                warn!(
                    "Skipping operator user '{}': name/shell/group contains unsafe characters",
                    user.name
                );
                continue;
            }
            for (desc, cmd) in build_user_provision_cmds(user) {
                let _ = self.log_and_execute(&desc, &cmd).await;
            }
        }

        let _ = self
            .log_and_execute(
                "Enabling SSH",
                "chroot /mnt/targetos bash -lc 'systemctl enable ssh'",
            )
            .await;

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
            let _ = self
                .log_and_execute(
                    &format!("ZFS: {}", cmd),
                    &format!("chroot /mnt/targetos bash -lc '{}'", cmd),
                )
                .await;
        }

        // Seed ZFS cache
        let _ = self
            .log_and_execute(
                "Ensure /etc/zfs in target",
                "mkdir -p /mnt/targetos/etc/zfs",
            )
            .await;
        let _ = self
            .log_and_execute(
                "Copy zpool.cache",
                "cp -f /etc/zfs/zpool.cache /mnt/targetos/etc/zfs/ 2>/dev/null || true",
            )
            .await;
        let _ = self
            .log_and_execute(
                "Ensure zfs-list.cache dir",
                "mkdir -p /mnt/targetos/etc/zfs/zfs-list.cache",
            )
            .await;
        let _ = self.log_and_execute(
            "Touch zfs-list.cache files",
            "bash -lc 'touch /mnt/targetos/etc/zfs/zfs-list.cache/bpool /mnt/targetos/etc/zfs/zfs-list.cache/rpool'",
        ).await;
        // Populate zfs-list.cache DIRECTLY via `zfs list` (deterministic). The
        // prior `timeout 5 zed -F` was a NO-OP: zed only writes the cache in
        // response to a zpool history EVENT, and running it briefly (inside the
        // chroot, blind to the host-imported pools) with no event triggered left
        // the files EMPTY — so zfs-mount-generator produced no mount units and
        // every dataset (/var, /var/log, ...) failed to mount at boot, dropping
        // to the systemd MAINTENANCE shell even though the D2-B unlock succeeded.
        // Generate the exact tab-separated format the zed cacher emits, on the
        // HOST where rpool/bpool are imported (mountpoints carry the /mnt/targetos
        // altroot prefix here; the sed below strips it back to /).
        let zfs_list_props = "name,mountpoint,canmount,atime,relatime,devices,exec,readonly,setuid,nbmand,encroot,keylocation,org.openzfs.systemd:requires,org.openzfs.systemd:requires-mounts-for,org.openzfs.systemd:before,org.openzfs.systemd:after,org.openzfs.systemd:wanted-by,org.openzfs.systemd:required-by,org.openzfs.systemd:nofail,org.openzfs.systemd:ignore";
        for pool in ["rpool", "bpool"] {
            let _ = self.log_and_execute(
                &format!("Generate zfs-list.cache/{pool}"),
                &format!(
                    "zfs list -H -t filesystem -o {zfs_list_props} -r {pool} | sort > /mnt/targetos/etc/zfs/zfs-list.cache/{pool}"
                ),
            ).await;
        }
        // Fix mountpoint paths — run on host so sed can see the file directly
        let _ = self
            .log_and_execute(
                "Fix zfs-list paths",
                "sed -Ei 's|/mnt/targetos/?|/|' /mnt/targetos/etc/zfs/zfs-list.cache/* || true",
            )
            .await;

        // Regenerate initramfs (dracut or initramfs-tools)
        let regen_cmd = config.initramfs_type.regenerate_cmd();
        let _ = self
            .log_and_execute(
                "Regenerate initramfs (post-ZFS)",
                &format!("chroot /mnt/targetos bash -lc '{}'", regen_cmd),
            )
            .await;

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

        let _ = self
            .log_and_execute(
                "Ensure ESP mountpoint",
                "[ -d /mnt/targetos/boot/efi ] || mkdir -p /mnt/targetos/boot/efi",
            )
            .await;
        let esp_part = self.detect_esp_partition_path(config).await?;
        let _ = self
            .log_and_execute(
                "Mount ESP if not mounted",
                &format!(
                "mountpoint -q /mnt/targetos/boot/efi || mount {} /mnt/targetos/boot/efi || true",
                esp_part
            ),
            )
            .await;

        let _ = self.log_and_execute(
            "Ensure efivarfs",
            "chroot /mnt/targetos bash -lc '[ -d /sys/firmware/efi/efivars ] || mkdir -p /sys/firmware/efi/efivars; mountpoint -q /sys/firmware/efi/efivars || mount -t efivarfs efivarfs /sys/firmware/efi/efivars || true'"
        ).await;

        // For dracut + Tang: the initramfs needs the network up before Tang is
        // queried for the LUKS key, so pass `rd.neednet=1` + an `ip=` config.
        //
        // Prefer STATIC over `ip=dhcp`. On Ubuntu 26.04 the systemd-networkd
        // dracut module drops a default `/run/systemd/network/zzzz-dracut-default
        // .network` that forces `DHCP=yes` on every interface and never sets up
        // wait-for-network logic — so `ip=dhcp` yields a late/duplicate lease
        // (registers the host in DNS under a second address) and network-online
        // never settles. A static `ip=IP::GW:PREFIX::IFACE:none` on the KERNEL
        // cmdline is parsed by systemd-network-generator (NOT dracut's own
        // variables) and, because it generates a *.network file, overrides the
        // DHCP default. Falls back to `ip=dhcp` only when the host really is DHCP.
        if config.initramfs_type == InitramfsType::Dracut && Self::uses_tang(config) {
            let ip_arg = if config.network_address.eq_ignore_ascii_case("dhcp") {
                "ip=dhcp".to_string()
            } else {
                // network_address is "IP/PREFIX" (e.g. "172.16.2.35/23").
                let (ip, prefix) = config
                    .network_address
                    .split_once('/')
                    .unwrap_or((config.network_address.as_str(), "24"));
                format!(
                    "ip={ip}::{gw}:{prefix}::{iface}:none",
                    gw = config.network_gateway,
                    iface = config.network_interface
                )
            };
            info!("Dracut+Tang: adding rd.neednet=1 {ip_arg} to GRUB_CMDLINE_LINUX");
            let grub_extra = format!("rd.neednet=1 {ip_arg}");
            let set_cmdline = format!(
                r#"chroot /mnt/targetos bash -lc 'grep -q "rd.neednet" /etc/default/grub 2>/dev/null || sed -i "s|^GRUB_CMDLINE_LINUX=\\\"\\(.*\\)\\\"|GRUB_CMDLINE_LINUX=\\\"\\1 {}\\\"| " /etc/default/grub'"#,
                grub_extra
            );
            let _ = self
                .log_and_execute("Set GRUB_CMDLINE_LINUX for dracut+Tang", &set_cmdline)
                .await;
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

        // Serial console for the headless fleet — MUST run before update-grub so
        // grub-mkconfig folds it into grub.cfg. See SERIAL_CONSOLE_DROPIN.
        // Arch-gated (amd64-only default): see configure_serial_console doc.
        self.configure_serial_console(config).await?;

        self.log_and_execute(
            "Updating GRUB config",
            "chroot /mnt/targetos bash -lc 'update-grub'",
        )
        .await?;

        // Best-effort: order NVRAM entries network-first, ubuntu-second. Mirrors
        // set_boot_order() in uaa-usb-bootstrap.sh. MUST stay non-fatal (let _ =):
        // legacy-BIOS hosts have no efivars, and grub-install --no-nvram/--removable
        // fallbacks mean the "ubuntu" entry may not exist.
        let _ = self.set_uefi_boot_order().await;

        Ok(())
    }

    /// Configure LUKS crypttab and optionally enroll Tang via Clevis SSS.
    pub async fn setup_luks_key_in_chroot(&mut self, config: &InstallationConfig) -> Result<()> {
        info!(
            "Configuring LUKS crypttab in chroot (storage_mode = {:?})",
            config.storage_mode
        );

        // NativeKeystore (U1 / server profile) binds clevis + crypttab to the
        // keystore-zvol LUKS, not a root p4 — a wholly different device path.
        if config.storage_mode == StorageMode::NativeKeystore {
            return self.setup_keystore_luks_in_chroot(config).await;
        }

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
        // FATAL: no crypttab entry = no unlock unit = unbootable encrypted root.
        // Same silent-killer class as the keystore path — never swallow it.
        self.log_and_execute(
            "Write LUKS crypttab",
            &format!(
                "[ -d /mnt/targetos/etc ] || mkdir -p /mnt/targetos/etc; echo '{}' > /mnt/targetos/etc/crypttab",
                crypttab_entry
            ),
        )
        .await?;

        // Ensure the initramfs carries BOTH unlock subsystems before any regen:
        //   - clevis  → Tang (network) unlock
        //   - crypt/tpm2/fido2 → systemd-cryptenroll TPM2+PIN and YubiKey keyslots
        self.configure_dracut_crypt_modules(config).await?;

        // Enroll via Clevis SSS when configured (PlainLuks: tang-only unless a
        // tree is authored). An authored `unlock_sss` tree counts even with an
        // EMPTY flat roster — it may carry Tang, tpm2 and pkcs11 shares of its
        // own, and skipping the bind here is what produced a "successful"
        // install of a host with no unattended-unlock binding at all.
        if !config.tang_servers.is_empty() || config.unlock_sss.is_some() {
            self.enroll_tang_clevis(config, &part, false).await?;
        }

        // Stage TPM2+PIN enrollment for first boot (binds the *installed*
        // system's PCRs, which the live installer cannot produce). FIDO2/YubiKey
        // is enrolled manually post-install via register-fido2-luks.sh.
        if config.enroll_tpm2 && config.tpm2_pin.as_deref().is_some_and(|p| !p.is_empty()) {
            self.setup_tpm2_firstboot_enrollment(
                config,
                if uuid.is_empty() { None } else { Some(uuid) },
            )
            .await?;
        }

        // Regenerate initramfs after crypttab + Tang enrollment
        let regen_cmd = config.initramfs_type.regenerate_cmd();
        let _ = self
            .log_and_execute(
                "Regenerate initramfs (post-crypttab)",
                &format!("chroot /mnt/targetos bash -lc '{}'", regen_cmd),
            )
            .await;

        Ok(())
    }

    /// NativeKeystore Phase 5: crypttab + dracut + clevis D2-B for the
    /// keystore-zvol LUKS (`/dev/zvol/rpool/keystore`) — the device holding the
    /// ZFS `system.key`. Runs on the host (the zvol/LUKS is not visible in the
    /// chroot), mirroring the PlainLuks path but pointed at the keystore.
    async fn setup_keystore_luks_in_chroot(&mut self, config: &InstallationConfig) -> Result<()> {
        let keystore_dev = "/dev/zvol/rpool/keystore";
        info!("Configuring keystore-LUKS crypttab + clevis (device {keystore_dev})");

        // NO crypttab entry for the keystore — deliberately.
        //
        // The keystore is opened by the `91uaa-keystore-wait` dracut hook at
        // pre-mount 89 (D7.3): it runs `clevis luks unlock` directly, mounts the
        // keystore, and `zfs load-key`s the root key — the exact sequence that
        // works by hand. A `/etc/crypttab` entry would generate a COMPETING
        // `systemd-cryptsetup@keystore-rpool.service` that opens the same device
        // via the systemd ask-password + `clevis-luks-askpass` path. That path
        // does NOT answer reliably in this dracut initramfs (the boot hangs
        // silently waiting for a password nobody supplies) and, worse, if that
        // unit runs before the hook it wins the race and hangs first. Removing
        // the entry eliminates the fragile path entirely; the hook is the sole
        // opener, and stock `zfs-load-key.sh` (pre-mount 90) then no-ops because
        // the key is already loaded. Write an explicit empty crypttab so no unit
        // is generated. FATAL: a swallowed write could leave a stale entry behind.
        let _ = keystore_dev; // device path is used by the hook, not crypttab
        self.log_and_execute(
            "Write empty crypttab (keystore unlocked by 91uaa-keystore-wait hook)",
            "[ -d /mnt/targetos/etc ] || mkdir -p /mnt/targetos/etc; printf '# keystore unlocked by the 91uaa-keystore-wait dracut hook (D7.3), not crypttab\\n' > /mnt/targetos/etc/crypttab",
        )
        .await?;

        // dracut: clevis/network/tpm2/zfs modules + the D7.1 keystore-wait hook.
        self.configure_dracut_crypt_modules(config).await?;
        self.install_keystore_dracut_module().await?;

        // Clevis D2-B bind on the keystore LUKS (tang + tpm2 peer, sha256), or
        // the authored tree verbatim when one is present — see the PlainLuks
        // call site for why an empty flat roster must not skip the bind.
        if !config.tang_servers.is_empty() || config.unlock_sss.is_some() {
            self.enroll_tang_clevis(config, keystore_dev, true).await?;
        }

        // Regenerate the initramfs so crypttab + clevis + the keystore-wait
        // module are baked in. FATAL on failure: a keystore host whose initramfs
        // lacks the clevis/tpm2/zfs unlock modules cannot unlock at boot, so a
        // dracut failure here must abort the install rather than be swallowed.
        let regen_cmd = config.initramfs_type.regenerate_cmd();
        self.log_and_execute(
            "Regenerate initramfs (keystore)",
            &format!("chroot /mnt/targetos bash -lc '{regen_cmd}'"),
        )
        .await?;

        Ok(())
    }

    /// Embed + install the `91uaa-keystore-wait` dracut module into the target
    /// and enable it (D7.1). The two files are compiled into the binary via
    /// `include_str!`, so there is no runtime fetch; base64-piped in to survive
    /// the shell hops.
    async fn install_keystore_dracut_module(&mut self) -> Result<()> {
        const MODULE_SETUP: &str =
            include_str!("../../../../../dracut/91uaa-keystore-wait/module-setup.sh");
        const KEYSTORE_WAIT: &str =
            include_str!("../../../../../dracut/91uaa-keystore-wait/keystore-wait.sh");
        let dir = "/mnt/targetos/usr/lib/dracut/modules.d/91uaa-keystore-wait";
        self.runner.execute(&format!("mkdir -p {dir}")).await?;
        for (name, content) in [
            ("module-setup.sh", MODULE_SETUP),
            ("keystore-wait.sh", KEYSTORE_WAIT),
        ] {
            let b64 = BASE64.encode(content);
            self.runner
                .execute(&format!(
                    "printf %s {b64} | base64 -d > {dir}/{name} && chmod 0755 {dir}/{name}"
                ))
                .await?;
        }
        // Enable the module for this host's initramfs (module name = dir minus
        // the NN priority prefix).
        self.runner
            .execute(
                "mkdir -p /mnt/targetos/etc/dracut.conf.d && \
                 printf 'add_dracutmodules+=\" uaa-keystore-wait \"\\n' \
                 > /mnt/targetos/etc/dracut.conf.d/91-uaa-keystore.conf",
            )
            .await?;
        info!("Installed 91uaa-keystore-wait dracut module");
        Ok(())
    }

    /// Write the install CA's public cert into the target at
    /// `/etc/uaa/install-ca.crt`, the default `--ca` path `uaa enroll` pins
    /// (spec Decision 7). Runs in both the full install and in-target-only
    /// paths (phase 5 is shared by both — see `perform_in_target_configuration`).
    ///
    /// Best-effort but loud: a config placed before the CA was reachable
    /// still carries the literal `REPLACE_AT_PLACE_TIME` placeholder here, so
    /// this writes it as-is and warns — matching `uaa enroll`'s own
    /// fail-closed treatment of an unparseable CA (abort + retry), never
    /// silently granting trust.
    pub async fn install_ca_cert_in_chroot(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Installing CA trust anchor in chroot");
        if config
            .install_ca_cert
            .contains(crate::config_place::PLACEHOLDER)
        {
            warn!(
                "install_ca_cert still contains {} — uaa enroll on this host will fail closed \
                 until the config is re-placed with the install CA reachable",
                crate::config_place::PLACEHOLDER
            );
        }
        self.runner
            .execute("mkdir -p /mnt/targetos/etc/uaa && chmod 0755 /mnt/targetos/etc/uaa")
            .await?;
        self.runner
            .execute(&format!(
                "cat > /mnt/targetos/etc/uaa/install-ca.crt <<'UAA_INSTALL_CA_EOF'\n{}\nUAA_INSTALL_CA_EOF",
                config.install_ca_cert
            ))
            .await?;
        self.runner
            .execute("chmod 0644 /mnt/targetos/etc/uaa/install-ca.crt")
            .await?;
        Ok(())
    }

    /// Build the clevis SSS policy JSON.
    ///
    /// **Pure** — no I/O, no `self`. The Tang advertisements must already have
    /// been pre-fetched by the caller; this function only consumes the resulting
    /// `(url, adv_path)` pairs. Keeping it pure is what makes the policy shape
    /// exhaustively unit-testable, which matters because a wrong shape here
    /// silently produces a *weaker* binding that still installs and still boots.
    ///
    /// Two shapes are emitted:
    ///
    /// * `tpm2 == None` — **legacy flat, Tang-only**:
    ///   `{"t":N,"pins":{"tang":[…]}}`. This is byte-for-byte what
    ///   len-serv-001/002 were bound with in production; it must never drift.
    ///
    /// * `tpm2 == Some(_)` — **nested AND**:
    ///   `{"t":2,"pins":{"tpm2":{…},"sss":[{"t":N,"pins":{"tang":[…]}}]}}`.
    ///
    /// The nesting is the whole point. In clevis SSS an *array* pin contributes
    /// one share **per element**, so the old flat
    /// `{"t":2,"pins":{"tang":[a,b,c],"tpm2":{…}}}` was really 2-of-**4** and the
    /// three Tang servers alone met the threshold — tpm2 was decorative, not
    /// required. Wrapping the Tang group in an inner `sss` collapses it to a
    /// single outer share, so the outer `t` = 2 over exactly two shares
    /// (`tpm2`, inner-`sss`) means both are required: a true AND.
    ///
    /// The outer threshold is hardcoded to 2 for that reason — it is not a
    /// tunable, it is "there are two outer shares and both are mandatory".
    /// The inner threshold stays `tang_threshold`; an outer t=2 does **not**
    /// degrade it (verified against live Tang: 2-of-3 inner with 2 servers down
    /// still fails closed).
    ///
    /// Availability trade-off, deliberate: under the flat policy a host whose TPM
    /// failed or whose PCR7 changed (firmware update, Secure Boot key rotation)
    /// still unlocked from Tang alone. Under the nested policy it will not unlock
    /// at all. That is the intended semantics, not an oversight.
    ///
    /// No validation is performed on `tang` or `tang_threshold` (an empty slice
    /// emits `"tang":[]`, an over-large threshold is emitted verbatim) — this
    /// matches the previous behaviour exactly, and callers already guarantee a
    /// non-empty Tang list before reaching here.
    fn build_clevis_sss_config(
        tang: &[TangAdv<'_>],
        tang_threshold: u8,
        tpm2: Option<Tpm2Peer<'_>>,
    ) -> String {
        let tang_group = TangGroup {
            t: tang_threshold,
            pins: TangPins { tang },
        };
        // serde_json emits derived structs in field-declaration order (only
        // `Value`/`Map` sorts keys), so the byte layout below is deterministic
        // and matches the hand-rolled `format!` it replaced. clevis itself does
        // not care about key order; the golden tests do.
        let json = match tpm2 {
            None => serde_json::to_string(&tang_group),
            Some(tpm2) => serde_json::to_string(&NestedPolicy {
                t: 2,
                pins: NestedPins {
                    tpm2,
                    sss: [tang_group],
                },
            }),
        };
        // Every field is a plain string/integer, so serialization is infallible;
        // the fallback keeps the signature free of a Result the caller can't act on.
        json.unwrap_or_default()
    }

    /// Emit clevis SSS JSON for an AUTHORED [`SssPolicy`] tree.
    ///
    /// **Pure** — the sibling of [`Self::build_clevis_sss_config`], which stays
    /// untouched and keeps serving the flat, un-authored path byte-for-byte.
    /// This one exists because a tree can nest to any depth and mix pin kinds,
    /// which the fixed `(tang, threshold, tpm2)` signature cannot express: it
    /// would have to *flatten* the tree, and a flattened `sss` group is exactly
    /// the 2-of-4 weakening the tree type was introduced to make unrepresentable.
    ///
    /// # Shape
    ///
    /// Every level emits `{"t":<threshold>,"pins":{…}}`, with `t` before `pins`
    /// (declaration order, not `serde_json::Value`'s key sorting). Each pin kind
    /// becomes ONE key — clevis's `pins` is a JSON object, so a duplicate key
    /// would be invalid — grouped by `SssPolicy::pins_by_kind`, which orders
    /// kinds by first appearance and pins within a kind in authored order. The
    /// output is therefore deterministic and diffable.
    ///
    /// Every kind is emitted as an **array**, uniformly, including a lone
    /// `tpm2` or `pkcs11`. That is not cosmetic: `sss` accepts an array for any
    /// pin and an N-element array is N shares (measured, see `unlock_sss`'s
    /// module doc), so a 1-element array is exactly one share — identical
    /// semantics to the bare object, with no special case to get wrong as pins
    /// are added. `UnlockPin::kind()` supplies the key; the variant supplies the
    /// value shape, so a new variant cannot be added without deciding both.
    ///
    /// This means the nested output here is textually *different* from
    /// `build_clevis_sss_config(.., Some(tpm2))` (which emits a bare
    /// `"tpm2":{…}`) while being semantically identical. Both are correct; the
    /// tree path is only ever taken when a tree was authored.
    ///
    /// # `adv` lookup
    ///
    /// `advs` is `(tang_url, adv_path)` built by the caller from this same
    /// tree's `tang_urls()`, so every Tang pin resolves by construction. If one
    /// somehow did not, it emits `"adv":""` — clevis then fails the bind, which
    /// is fatal to the install. It fails CLOSED: no path here can produce a
    /// silently unbound host.
    fn build_clevis_policy_from_tree(policy: &SssPolicy, advs: &[(String, String)]) -> String {
        let mut out = String::new();
        Self::emit_sss_level(policy, advs, &mut out);
        out
    }

    fn emit_sss_level(policy: &SssPolicy, advs: &[(String, String)], out: &mut String) {
        use std::fmt::Write as _;
        // Writing into a String is infallible; `let _ =` keeps this total.
        let _ = write!(out, r#"{{"t":{},"pins":{{"#, policy.threshold);
        for (i, (kind, pins)) in policy.pins_by_kind().into_iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, r#""{kind}":["#);
            for (j, pin) in pins.into_iter().enumerate() {
                if j > 0 {
                    out.push(',');
                }
                match pin {
                    UnlockPin::Tang(t) => {
                        let adv = advs
                            .iter()
                            .find(|(url, _)| url == &t.url)
                            .map(|(_, adv)| adv.as_str())
                            .unwrap_or("");
                        out.push_str(
                            &serde_json::to_string(&TangAdv {
                                url: t.url.as_str(),
                                adv,
                            })
                            .unwrap_or_default(),
                        );
                    }
                    UnlockPin::Tpm2(p) => out.push_str(
                        &serde_json::to_string(&Tpm2Peer {
                            pcr_ids: p.pcr_ids.as_str(),
                            pcr_bank: p.pcr_bank.as_str(),
                        })
                        .unwrap_or_default(),
                    ),
                    UnlockPin::Pkcs11(p) => out.push_str(
                        &serde_json::to_string(&Pkcs11Peer {
                            uri: p.uri.as_str(),
                            // NEVER `None` in the emitted JSON. A binding with
                            // no mechanism succeeds and then dies at unlock
                            // with `Decrypt mechanism not supported`, so the
                            // default is applied here as well as at
                            // deserialization — this is the last point at which
                            // a `None` reaching the wire can be caught.
                            mechanism: Some(
                                p.mechanism.as_deref().unwrap_or(DEFAULT_PKCS11_MECHANISM),
                            ),
                        })
                        .unwrap_or_default(),
                    ),
                    UnlockPin::Sss(nested) => Self::emit_sss_level(nested, advs, out),
                }
            }
            out.push(']');
        }
        out.push_str("}}");
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
        include_tpm2_peer: bool,
    ) -> Result<()> {
        // LAST GATE, and the only one on this path that cannot be walked around.
        //
        // `validate_resolved` runs the same rules, but only in
        // `uaa-control`'s `resolve_from_registry` — so a hand-authored
        // `InstallationConfig` handed straight to the installer never sees them.
        // The rules this enforces (a PIN stored in the LUKS header, a
        // `slot-id=`-keyed binding that addresses the wrong token after the next
        // reboot, a threshold clevis will reject mid-bind, a duplicated share
        // that makes a group weaker than it reads) all describe policies that
        // are WORSE than not installing at all, because each of them produces a
        // host that looks bound and is not. So this fails CLOSED, immediately
        // before the bind, rather than trusting an upstream caller to have
        // checked.
        if let Some(policy) = &config.unlock_sss {
            match policy.validate() {
                Ok(warnings) => {
                    for warning in warnings {
                        warn!("unlock_sss policy: {warning}");
                    }
                }
                Err(errors) => {
                    return Err(crate::error::AutoInstallError::ConfigError(format!(
                        "refusing to bind an invalid unlock_sss policy: {}",
                        errors.join("; ")
                    )));
                }
            }
        }

        // Which Tang servers does this host actually use? An authored tree
        // carries its own — possibly NESTED, so not reachable by iterating the
        // flat roster, and possibly with the roster left entirely empty.
        // `tang_urls()` walks the tree depth-first; the flat roster is the
        // fallback for the un-authored path.
        let tang_urls: Vec<String> = match &config.unlock_sss {
            Some(policy) => policy.tang_urls().into_iter().map(str::to_string).collect(),
            None => config.tang_servers.iter().map(|s| s.url.clone()).collect(),
        };
        info!(
            "Enrolling {} Tang servers via Clevis SSS (authored_tree={}, threshold={}, tpm2_peer={})",
            tang_urls.len(),
            config.unlock_sss.is_some(),
            config.tang_threshold,
            include_tpm2_peer,
        );

        // Pre-fetch each Tang advertisement to a root-only file so it can be
        // referenced via the clevis `adv` key. WITHOUT a pre-fetched adv,
        // `clevis luks bind` prompts on /dev/tty to confirm trust of the Tang
        // signing keys, which fails non-interactively over SSH ("/dev/tty: No
        // such device or address") and would leave the keystore with NO
        // unattended-unlock binding. Embedding the adv makes the bind fully
        // non-interactive. This loop is the only I/O here; the JSON assembly
        // below is a pure, unit-tested function.
        //
        // Keyed by URL rather than by position: a tree may legitimately name the
        // same server at two places in the policy, and one advertisement serves
        // both. Numbering follows fetch order, so the un-authored path still
        // emits /run/uaa-tang-0..N-1 exactly as before.
        let mut advs: Vec<(String, String)> = Vec::with_capacity(tang_urls.len());
        for url in &tang_urls {
            if advs.iter().any(|(u, _)| u == url) {
                continue;
            }
            let adv_path = format!("/run/uaa-tang-{}.adv", advs.len());
            self.log_and_execute(
                &format!("Fetch Tang advertisement from {url}"),
                &format!("curl -sf --max-time 10 {url}/adv -o {adv_path}"),
            )
            .await?;
            advs.push((url.clone(), adv_path));
        }

        // THE TREE WINS, WHOLESALE. When `unlock_sss` is authored it supplies
        // its own threshold and its own tpm2/pkcs11 shares, so BOTH
        // `config.tang_threshold` and `include_tpm2_peer` are deliberately
        // ignored on that path. Honoring them would either overwrite the
        // authored `t` or graft an extra tpm2 share onto a policy that did not
        // ask for one — silently changing the share arithmetic the author
        // computed. The un-authored path below is untouched:
        //
        //   PlainLuks: Tang-only, flat — {"t":N,"pins":{"tang":[…]}}.
        //   NativeKeystore D2-B: nested AND — the tpm2 PEER share (PCR7 in the
        //   SHA-256 bank; clevis defaults to sha1, which Secure Boot doesn't
        //   populate) plus a one-share inner sss holding the Tang group, so BOTH
        //   are required. See build_clevis_sss_config for why nesting matters.
        let sss_config = match &config.unlock_sss {
            Some(policy) => {
                // A NativeKeystore tree with no tpm2 pin gives up the default
                // peer share. That is the author's call and is honored, but it
                // is exactly the kind of thing someone hunts for in the install
                // log after a host fails to unlock — so say it out loud.
                if include_tpm2_peer && !policy.contains_kind("tpm2") {
                    warn!(
                        "Authored unlock_sss tree has no tpm2 pin; the default NativeKeystore \
                         tpm2 peer share is NOT being added (tree wins). This host unlocks from \
                         the authored factors alone."
                    );
                }
                Self::build_clevis_policy_from_tree(policy, &advs)
            }
            None => {
                let tang: Vec<TangAdv<'_>> = advs
                    .iter()
                    .map(|(url, adv)| TangAdv {
                        url: url.as_str(),
                        adv: adv.as_str(),
                    })
                    .collect();
                let tpm2 = include_tpm2_peer.then_some(Tpm2Peer {
                    pcr_ids: config.tpm2_pcr_ids.as_str(),
                    pcr_bank: "sha256",
                });
                Self::build_clevis_sss_config(&tang, config.tang_threshold, tpm2)
            }
        };

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

        // Create empty 0600 file.
        //
        // FATAL, not "skip": this function is only reached when Tang enrollment is
        // required (a non-empty tang_servers roster OR an authored unlock_sss
        // tree). A silent `return Ok(())` here bypasses
        // the fatal bind below and lets the install report success on a keystore
        // with NO unattended-unlock binding — the exact silent-killer the bind's
        // fatal handling was added to prevent.
        let mk_tmp = format!("install -m 0600 /dev/null {}", tmp_key_path);
        if let Err(e) = self.runner.execute(&mk_tmp).await {
            return Err(crate::error::AutoInstallError::SystemError(format!(
                "Clevis enrollment: could not create key tempfile ({e}) — keystore would have no unattended-unlock binding"
            )));
        }

        // Write passphrase — NOT logged (no log_and_execute)
        let write_key = format!(
            "printf '%s' '{}' > {}",
            config.luks_key.replace('\'', r"'\''"),
            tmp_key_path
        );
        if let Err(e) = self.runner.execute(&write_key).await {
            let _ = self
                .runner
                .execute(&format!(
                    "shred -u {} 2>/dev/null || rm -f {}",
                    tmp_key_path, tmp_key_path
                ))
                .await;
            return Err(crate::error::AutoInstallError::SystemError(format!(
                "Clevis enrollment: could not write key to tempfile ({e}) — keystore would have no unattended-unlock binding"
            )));
        }

        // Run clevis bind — key path in command, not the key itself
        let bind_cmd = format!(
            "clevis luks bind -d {} -k {} sss '{}'",
            luks_part, tmp_key_path, sss_config
        );
        info!("Executing: Enroll Tang via clevis SSS (passphrase via tempfile, redacted)");
        let bind_result = self.runner.execute(&bind_cmd).await;

        // Always shred the tempfile regardless of outcome
        let _ = self
            .runner
            .execute(&format!(
                "shred -u {} 2>/dev/null || rm -f {}",
                tmp_key_path, tmp_key_path
            ))
            .await;

        // Clean up the pre-fetched advertisements regardless of outcome.
        let _ = self.runner.execute("rm -f /run/uaa-tang-*.adv").await;

        if let Err(e) = bind_result {
            // FATAL: a NativeKeystore/Tang host without this binding has NO
            // unattended unlock — the entire point of the encrypted install.
            // Never report success with a silently passphrase-only keystore
            // (the pre-fix behaviour that produced a "🎉 success" that couldn't
            // boot unattended).
            return Err(crate::error::AutoInstallError::SystemError(format!(
                "Clevis Tang+TPM2 SSS enrollment failed — keystore has no unattended-unlock binding: {e}"
            )));
        }
        info!("Tang/Clevis SSS enrollment complete");

        Ok(())
    }

    /// Build the `/etc/dracut.conf.d` fragment that pulls both LUKS unlock
    /// subsystems into the initramfs.
    ///
    /// - `clevis`  satisfies the Tang (network) keyslot.
    /// - `crypt` + `tpm2-tss` + the cryptsetup token plugins let
    ///   systemd-cryptsetup satisfy the TPM2+PIN and FIDO2/YubiKey keyslots at
    ///   the boot prompt.
    ///
    /// NOTE: the exact module/plugin set is confirmed on the QEMU+swtpm VM
    /// before any real host is installed (see PLAN test strategy).
    fn build_dracut_crypt_conf(include_clevis: bool, nic_driver: &str) -> String {
        // `network` is REQUIRED alongside clevis: Tang unlock happens over the net
        // in the initramfs, so the network stack must be present. Without it the
        // initramfs "fails to start the network", clevis can't reach Tang, LUKS
        // never opens, and the zfs import (rpool on /dev/mapper/luks) fails.
        let clevis = if include_clevis {
            " clevis network"
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
             #   zfs              -> import rpool/bpool\n\
             add_dracutmodules+=\" crypt tpm2-tss zfs{clevis} \"\n\
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
            warn!(
                "network_interface '{}' failed validation; not forcing a NIC driver into initramfs",
                iface
            );
            String::new()
        };
        let nic_driver = nic_driver.as_str();
        // `uses_tang`, not the flat roster: a tree-only Tang host still needs the
        // network + NIC driver baked into the initramfs or the bind is
        // unsatisfiable at boot.
        let uses_tang = Self::uses_tang(config);
        if uses_tang {
            info!(
                "Tang unlock: forcing NIC driver '{}' into initramfs",
                nic_driver
            );
        }
        // The `clevis` DRACUT MODULE must be gated on the same predicate as the
        // `clevis luks bind` call sites, not on `uses_tang`. Those two were the
        // identical expression before authored trees existed, so they could not
        // diverge; now they can. A Tang-LESS tree (tpm2-only, or the pkcs11 OR
        // that `SssPolicy::any_pkcs11` exists to build) gets clevis installed and
        // bound but would ship an initramfs with no clevis module — a green
        // install that boots to a LUKS prompt, the same bricking class this
        // change exists to close, in a new shape.
        //
        // This pulls the `network` module in for a Tang-less tree too. Wasteful,
        // not harmful, and much cheaper than splitting the helper's signature.
        let needs_clevis_module = !config.tang_servers.is_empty() || config.unlock_sss.is_some();
        let conf = Self::build_dracut_crypt_conf(needs_clevis_module, nic_driver);
        let cmd = format!(
            "mkdir -p /mnt/targetos/etc/dracut.conf.d && cat > /mnt/targetos/etc/dracut.conf.d/90-uaa-crypt.conf <<'UAA_DRACUT_EOF'\n{}UAA_DRACUT_EOF",
            conf
        );
        // FATAL: this fragment is what pulls the crypt/tpm2/zfs/network unlock
        // modules (and the forced NIC driver for Tang) INTO the initramfs. If it
        // silently fails to write, the regenerated initramfs ships without the
        // unlock stack and the box cannot decrypt at boot — another "silent
        // killer" that must abort the install, not be swallowed.
        self.log_and_execute("Write dracut crypt-module config", &cmd)
            .await?;
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
        let seed =
            Self::build_tpm2_enroll_seed(&config.luks_key, pin, &config.tpm2_pcr_ids, &luksdev);
        let write_seed = format!(
            "install -m 0600 /dev/null /mnt/targetos/etc/uaa-tpm2-enroll.env && cat > /mnt/targetos/etc/uaa-tpm2-enroll.env <<'UAA_TPM2_SEED_EOF'\n{}UAA_TPM2_SEED_EOF",
            seed
        );
        if let Err(e) = self.runner.execute(&write_seed).await {
            warn!(
                "TPM2 enrollment: could not write seed ({}); skipping TPM2 slot",
                e
            );
            return Ok(());
        }

        // Unit body has no secrets — safe to log.
        let unit = Self::build_tpm2_enroll_unit();
        let write_unit = format!(
            "cat > /mnt/targetos/etc/systemd/system/uaa-tpm2-enroll.service <<'UAA_TPM2_UNIT_EOF'\n{}UAA_TPM2_UNIT_EOF",
            unit
        );
        let _ = self
            .log_and_execute("Write first-boot TPM2 enrollment unit", &write_unit)
            .await;
        let _ = self
            .log_and_execute(
                "Enable first-boot TPM2 enrollment unit",
                "chroot /mnt/targetos bash -lc 'systemctl enable uaa-tpm2-enroll.service'",
            )
            .await;

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
            // NativeKeystore: the keystore-rpool mapper holds /dev/zvol/rpool/keystore
            // open, so it MUST be torn down before `zpool export rpool` (else the
            // export fails with the pool busy). No-op on PlainLuks (|| true).
            ("Unmounting keystore", "umount /run/keystore/rpool 2>/dev/null || true"),
            ("Closing keystore LUKS mapper", "cryptsetup status keystore-rpool >/dev/null 2>&1 && cryptsetup close keystore-rpool || true"),
            // Closing the mapper is asynchronous: udev still has the zvol node
            // open for a moment afterwards. Settle before exporting, or the
            // export blocks on the still-referenced /dev/zvol/rpool/keystore.
            ("Settle udev before pool export", "udevadm settle --timeout=30 || true"),
            // Every export is wrapped in `timeout` and falls back to `-f`.
            // `|| true` alone is NOT enough: a busy pool makes `zpool export`
            // BLOCK rather than fail, and an unbounded block here hangs the
            // whole install after every phase has already succeeded (observed
            // on the VM gate: Phase 6 stuck ~47min on `zpool export rpool`
            // until the harness timed out and failed an otherwise-good run).
            // Leaving a pool imported is harmless — /etc/hostid matches the
            // pool, so the next boot imports it from the cachefile normally.
            ("Exporting bpool", "timeout 120 zpool export bpool || timeout 60 zpool export -f bpool || true"),
            ("Exporting rpool", "timeout 120 zpool export rpool || timeout 60 zpool export -f rpool || true"),
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

/// Whether a value is a safe bare identifier to interpolate into a shell
/// command — a strict whitelist (letters, digits, and `._/@:-`), non-empty.
/// Used to gate a `UserAccount`'s `name`, `shell`, and group names, which are
/// interpolated literally (they name paths/accounts, so they can't be
/// base64-round-tripped like the free-form password/keys are). Anything with
/// whitespace, quotes, `$`, backtick, `;`, `|`, `&`, `<`, `>`, `(`, `)`, or `\`
/// is rejected, closing the shell-injection vector for identifier positions.
fn is_safe_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '@' | ':' | '-'))
}

/// Build the ordered chroot commands that provision one operator user account.
///
/// Pure (no I/O) so the command sequence is unit-testable; the caller runs each
/// `(description, command)` pair through `log_and_execute`. Ordering is
/// deliberate: create → password → groups → ssh keys. Every step is idempotent
/// (the `useradd` is guarded by an `id` check) and group additions are guarded
/// by `getent` so a group the target lacks (e.g. `docker`) is skipped rather
/// than failing the whole `usermod`.
///
/// Injection-safety: the free-form `password` and each SSH key are
/// **base64-encoded in Rust and decoded inside the chroot** (`base64 -d`), so
/// no shell metacharacter from those values ever reaches a shell — bulletproof
/// through the outer `bash -c` + inner `bash -lc` nesting. The identifier
/// fields (`name`, `shell`, groups) are interpolated literally and MUST be
/// pre-validated with [`is_safe_ident`] by the caller (which skips any user
/// that fails), so those positions are safe too.
fn build_user_provision_cmds(user: &UserAccount) -> Vec<(String, String)> {
    let name = &user.name;
    let mut cmds: Vec<(String, String)> = Vec::new();

    // 1. Create the account (idempotent) with home dir + login shell.
    cmds.push((
        format!("Create user {name}"),
        format!(
            "chroot /mnt/targetos bash -lc 'id {name} >/dev/null 2>&1 || useradd -m -s {shell} {name}'",
            shell = user.shell,
        ),
    ));

    // 2. Password via chpasswd, or lock it (key-only login) when empty.
    //    The `name:password` pair is base64'd so any character is safe.
    if user.password.is_empty() {
        cmds.push((
            format!("Lock password for {name} (SSH-key only)"),
            format!("chroot /mnt/targetos bash -lc 'passwd -l {name} || true'"),
        ));
    } else {
        let creds_b64 = BASE64.encode(format!("{name}:{pw}", pw = user.password));
        cmds.push((
            format!("Set password for {name}"),
            format!("chroot /mnt/targetos bash -lc 'echo {creds_b64} | base64 -d | chpasswd'"),
        ));
    }

    // 3. Supplementary groups — each guarded so a missing group is skipped.
    for group in &user.groups {
        cmds.push((
            format!("Add {name} to group {group}"),
            format!(
                "chroot /mnt/targetos bash -lc 'getent group {group} >/dev/null && usermod -aG {group} {name} || true'"
            ),
        ));
    }

    // 4. Per-user SSH authorized keys (+ ownership fix, since we write as root).
    //    Each key is base64'd so its content can't break out of the shell.
    if !user.ssh_authorized_keys.is_empty() {
        cmds.push((
            format!("Create {name} .ssh dir"),
            format!(
                "chroot /mnt/targetos bash -lc 'mkdir -p /home/{name}/.ssh && chmod 700 /home/{name}/.ssh'"
            ),
        ));
        for key in &user.ssh_authorized_keys {
            let key_b64 = BASE64.encode(key);
            cmds.push((
                format!("Inject SSH key for {name}"),
                format!(
                    "chroot /mnt/targetos bash -lc '{{ echo {key_b64} | base64 -d; echo; }} >> /home/{name}/.ssh/authorized_keys'"
                ),
            ));
        }
        cmds.push((
            format!("Fix {name} .ssh ownership/perms"),
            format!(
                "chroot /mnt/targetos bash -lc 'chmod 600 /home/{name}/.ssh/authorized_keys && chown -R {name}: /home/{name}/.ssh'"
            ),
        ));
    }

    cmds
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    /// Records every command routed through the executor into a shared log
    /// so tests can assert on the recorded-command count, not just
    /// `is_ok()`. Mirrors `applications.rs`/`installer.rs`'s `RecordingExecutor`.
    #[derive(Clone, Default)]
    struct RecordingExecutor {
        commands: Arc<Mutex<Vec<String>>>,
    }

    fn joined_cmds(user: &UserAccount) -> String {
        build_user_provision_cmds(user)
            .into_iter()
            .map(|(_, c)| c)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_build_user_provision_cmds_password_sudo_adm() {
        let user = UserAccount {
            name: "jdfalk".to_string(),
            password: "s3cret".to_string(),
            groups: vec!["adm".to_string(), "sudo".to_string(), "docker".to_string()],
            shell: "/bin/bash".to_string(),
            ssh_authorized_keys: vec!["ssh-ed25519 AAAAtest jdfalk@x".to_string()],
        };
        let joined = joined_cmds(&user);

        // Idempotent account creation with the requested shell.
        assert!(joined.contains("id jdfalk >/dev/null 2>&1 || useradd -m -s /bin/bash jdfalk"));
        // A real password is set (via base64→chpasswd) — NOT locked.
        assert!(joined.contains("base64 -d | chpasswd"));
        assert!(!joined.contains("passwd -l"));
        // Security: the raw password NEVER appears in cleartext in the command
        // stream — it is base64-encoded so no shell can interpret its contents.
        assert!(!joined.contains("s3cret"));
        assert!(joined.contains(&BASE64.encode("jdfalk:s3cret")));
        // sudo (escalate) + adm (read logs), each getent-guarded.
        assert!(joined.contains("getent group sudo >/dev/null && usermod -aG sudo jdfalk"));
        assert!(joined.contains("getent group adm >/dev/null && usermod -aG adm jdfalk"));
        // A group the target may lack is still emitted but guarded, not fatal.
        assert!(joined.contains("getent group docker >/dev/null && usermod -aG docker jdfalk"));
        // Key seeded (base64) and ownership handed back to the user.
        assert!(joined.contains("base64 -d; echo; } >> /home/jdfalk/.ssh/authorized_keys"));
        assert!(joined.contains("chown -R jdfalk: /home/jdfalk/.ssh"));

        // Ordering: create → password → groups → keys.
        let idx = |s: &str| joined.find(s).expect("substring present");
        assert!(idx("useradd") < idx("chpasswd"));
        assert!(idx("chpasswd") < idx("usermod -aG adm"));
        assert!(idx("usermod -aG docker") < idx("authorized_keys"));
    }

    #[test]
    fn test_build_user_provision_cmds_password_with_shell_metachars_is_base64_safe() {
        // A password full of shell metacharacters must not appear raw anywhere —
        // base64 encoding neutralizes the injection through both shell layers.
        let user = UserAccount {
            name: "jdfalk".to_string(),
            password: "p$(rm -rf /)`whoami`'\";".to_string(),
            groups: vec![],
            shell: "/bin/bash".to_string(),
            ssh_authorized_keys: vec![],
        };
        let joined = joined_cmds(&user);
        assert!(joined.contains("base64 -d | chpasswd"));
        // None of the dangerous fragments survive in the command string.
        assert!(!joined.contains("rm -rf"));
        assert!(!joined.contains("$("));
        assert!(!joined.contains('`'));
    }

    #[test]
    fn test_is_safe_ident_rejects_shell_metacharacters() {
        assert!(is_safe_ident("jdfalk"));
        assert!(is_safe_ident("/usr/bin/zsh"));
        assert!(is_safe_ident("lxd"));
        assert!(!is_safe_ident(""));
        assert!(!is_safe_ident("bad name")); // space
        assert!(!is_safe_ident("x;rm -rf /")); // semicolon
        assert!(!is_safe_ident("x$(id)")); // command sub
        assert!(!is_safe_ident("x'y")); // quote
    }

    #[test]
    fn test_build_user_provision_cmds_empty_password_locks_and_no_keys() {
        let user = UserAccount {
            name: "svc".to_string(),
            password: String::new(),
            groups: Vec::new(),
            shell: "/bin/bash".to_string(),
            ssh_authorized_keys: Vec::new(),
        };
        let joined = joined_cmds(&user);
        // Empty password locks the account (key-only), never runs chpasswd.
        assert!(joined.contains("passwd -l svc"));
        assert!(!joined.contains("chpasswd"));
        // No keys → no .ssh handling emitted.
        assert!(!joined.contains(".ssh"));
    }

    impl RecordingExecutor {
        fn recorded(&self) -> Vec<String> {
            self.commands.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CommandExecutor for RecordingExecutor {
        async fn connect(&mut self, _host: &str, _user: &str) -> Result<()> {
            Ok(())
        }
        async fn execute(&mut self, cmd: &str) -> Result<()> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok(())
        }
        async fn execute_with_output(&mut self, cmd: &str) -> Result<String> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok(String::new())
        }
        async fn execute_with_error_collection(
            &mut self,
            cmd: &str,
            _desc: &str,
        ) -> Result<(i32, String, String)> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok((0, String::new(), String::new()))
        }
        async fn check_silent(&mut self, cmd: &str) -> Result<bool> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok(true)
        }
        async fn collect_debug_info(&mut self) -> Result<String> {
            Ok(String::new())
        }
        async fn upload_file(&mut self, _local_path: &str, _remote_path: &str) -> Result<()> {
            Ok(())
        }
        async fn download_file(&mut self, _remote_path: &str, _local_path: &str) -> Result<()> {
            Ok(())
        }
        fn disconnect(&mut self) {}
    }

    #[tokio::test]
    async fn test_configure_serial_console_runs_on_amd64() {
        let mut cfg = sample_netplan_config("192.0.2.10/24", "networkd");
        cfg.arch = Arch::Amd64;
        let mut executor = RecordingExecutor::default();
        {
            let mut sysconf = SystemConfigurator::new(&mut executor);
            sysconf.configure_serial_console(&cfg).await.unwrap();
        }
        let recorded = executor.recorded();
        assert!(
            recorded
                .iter()
                .any(|c| c.contains("99-uaa-serial-console.cfg")),
            "amd64 config must write the serial-console drop-in, got: {recorded:?}"
        );
    }

    #[tokio::test]
    async fn test_configure_serial_console_skips_on_arm64() {
        let mut cfg = sample_netplan_config("192.0.2.10/24", "networkd");
        cfg.arch = Arch::Arm64;
        let mut executor = RecordingExecutor::default();
        {
            let mut sysconf = SystemConfigurator::new(&mut executor);
            sysconf.configure_serial_console(&cfg).await.unwrap();
        }
        let recorded = executor.recorded();
        assert!(
            recorded.is_empty(),
            "arm64 config must skip the serial-console drop-in, got: {recorded:?}"
        );
    }

    #[test]
    fn test_serial_console_dropin_appends_and_sets_serial_terminal() {
        // Must APPEND, not clobber, GRUB_CMDLINE_LINUX — otherwise the dracut+Tang
        // `rd.neednet=1 ip=dhcp` flags set earlier would be lost and network-unlock
        // would break.
        assert!(
            SERIAL_CONSOLE_DROPIN.contains("GRUB_CMDLINE_LINUX=\"$GRUB_CMDLINE_LINUX "),
            "drop-in must reference $GRUB_CMDLINE_LINUX so it appends"
        );
        // ttyS0 must come AFTER tty0 on the kernel cmdline: the last `console=`
        // is the primary console (kernel log + login getty) — that's the one SOL
        // watches on a headless box.
        let cmdline = SERIAL_CONSOLE_DROPIN
            .lines()
            .find(|l| l.starts_with("GRUB_CMDLINE_LINUX="))
            .expect("drop-in has a GRUB_CMDLINE_LINUX line");
        let tty0 = cmdline.find("console=tty0").expect("tty0 present");
        let ttys0 = cmdline
            .find("console=ttyS0,115200n8")
            .expect("ttyS0 present");
        let ttys1 = cmdline
            .find("console=ttyS1,115200n8")
            .expect("ttyS1 present");
        // ttyS1 (Supermicro X10 BMC SOL = COM2) must be LAST so it wins the
        // primary /dev/console — the one SOL watches on a headless box.
        assert!(
            tty0 < ttys0 && ttys0 < ttys1,
            "ttyS1 must be last so it is the primary (SOL) console"
        );
        // GRUB's own menu on serial too (COM2 / unit 1), at a matching baud.
        assert!(SERIAL_CONSOLE_DROPIN.contains("GRUB_TERMINAL=\"console serial\""));
        assert!(
            SERIAL_CONSOLE_DROPIN.contains("GRUB_SERIAL_COMMAND=\"serial --speed=115200 --unit=1")
        );
    }

    #[test]
    fn test_build_esp_detection_command_contains_expected_parts() {
        let guid = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
        let cmd = SystemConfigurator::build_esp_detection_command(guid);
        assert!(cmd.starts_with("bash -lc '"));
        // `-P` not `-rP`: --raw/--pairs are mutually exclusive on Ubuntu 26.04.
        assert!(cmd.contains("lsblk -P -o PATH,PARTTYPE"));
        assert!(
            !cmd.contains("lsblk -rP"),
            "must not use -rP (breaks on 26.04)"
        );
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

    // ---- clevis SSS policy emitter -------------------------------------

    fn tang(n: usize) -> Vec<(String, String)> {
        (0..n)
            .map(|i| {
                (
                    format!("http://172.16.2.4{i}"),
                    format!("/run/uaa-tang-{i}.adv"),
                )
            })
            .collect()
    }

    fn advs(pairs: &[(String, String)]) -> Vec<TangAdv<'_>> {
        pairs
            .iter()
            .map(|(url, adv)| TangAdv {
                url: url.as_str(),
                adv: adv.as_str(),
            })
            .collect()
    }

    /// The **pre-fix** string builder, preserved verbatim as a test oracle.
    ///
    /// This is the exact `format!` that bound len-serv-001/002 in production.
    /// It is deliberately duplicated here rather than shared: the point is that
    /// it can never be refactored in lockstep with the real emitter.
    fn legacy_flat_oracle(pairs: &[(String, String)], threshold: u8) -> String {
        let entries: Vec<String> = pairs
            .iter()
            .map(|(url, adv)| format!(r#"{{"url":"{}","adv":"{}"}}"#, url, adv))
            .collect();
        format!(
            r#"{{"t":{},"pins":{{"tang":[{}]}}}}"#,
            threshold,
            entries.join(","),
        )
    }

    /// Byte-identity proof against the original code, swept over server counts
    /// and thresholds. len-serv-001/002 are bound with this exact string.
    #[test]
    fn test_sss_tang_only_is_byte_identical_to_legacy_builder() {
        for n in 1..=4usize {
            for threshold in 1..=3u8 {
                let pairs = tang(n);
                let got =
                    SystemConfigurator::build_clevis_sss_config(&advs(&pairs), threshold, None);
                assert_eq!(
                    got,
                    legacy_flat_oracle(&pairs, threshold),
                    "drift for n={n} t={threshold}"
                );
            }
        }
    }

    /// Hardcoded golden, so refactoring emitter *and* oracle together still fails.
    #[test]
    fn test_sss_tang_only_golden_literal() {
        let pairs = tang(3);
        let got = SystemConfigurator::build_clevis_sss_config(&advs(&pairs), 2, None);
        assert_eq!(
            got,
            r#"{"t":2,"pins":{"tang":[{"url":"http://172.16.2.40","adv":"/run/uaa-tang-0.adv"},{"url":"http://172.16.2.41","adv":"/run/uaa-tang-1.adv"},{"url":"http://172.16.2.42","adv":"/run/uaa-tang-2.adv"}]}}"#
        );
    }

    /// Single-Tang edge case: still a flat one-element array, no trailing comma.
    #[test]
    fn test_sss_tang_only_single_server() {
        let pairs = tang(1);
        let got = SystemConfigurator::build_clevis_sss_config(&advs(&pairs), 1, None);
        assert_eq!(
            got,
            r#"{"t":1,"pins":{"tang":[{"url":"http://172.16.2.40","adv":"/run/uaa-tang-0.adv"}]}}"#
        );
    }

    /// Empty Tang list is emitted verbatim (`"tang":[]`), matching the previous
    /// behaviour — validation is the caller's job, not the emitter's.
    #[test]
    fn test_sss_tang_only_empty_matches_legacy() {
        let got = SystemConfigurator::build_clevis_sss_config(&[], 2, None);
        assert_eq!(got, r#"{"t":2,"pins":{"tang":[]}}"#);
        assert_eq!(got, legacy_flat_oracle(&[], 2));
    }

    /// Nested AND: outer t=2 over exactly two shares (tpm2, inner sss).
    #[test]
    fn test_sss_nested_and_with_tpm2_golden() {
        let pairs = tang(3);
        let got = SystemConfigurator::build_clevis_sss_config(
            &advs(&pairs),
            2,
            Some(Tpm2Peer {
                pcr_ids: "7",
                pcr_bank: "sha256",
            }),
        );
        assert_eq!(
            got,
            r#"{"t":2,"pins":{"tpm2":{"pcr_ids":"7","pcr_bank":"sha256"},"sss":[{"t":2,"pins":{"tang":[{"url":"http://172.16.2.40","adv":"/run/uaa-tang-0.adv"},{"url":"http://172.16.2.41","adv":"/run/uaa-tang-1.adv"},{"url":"http://172.16.2.42","adv":"/run/uaa-tang-2.adv"}]}}]}}"#
        );
    }

    /// The Tang array must live *inside* the nested group, never as an outer
    /// sibling of tpm2 — an outer `"tang":[a,b,c]` is the 2-of-4 bug.
    #[test]
    fn test_sss_nested_and_never_emits_tang_as_outer_pin() {
        let pairs = tang(3);
        let got = SystemConfigurator::build_clevis_sss_config(
            &advs(&pairs),
            2,
            Some(Tpm2Peer {
                pcr_ids: "7",
                pcr_bank: "sha256",
            }),
        );
        assert!(!got.starts_with(r#"{"t":2,"pins":{"tang":"#));
        assert!(got.contains(r#""sss":[{"t":2,"pins":{"tang":"#));
        // Exactly two outer shares.
        assert_eq!(got.matches(r#""pins":{"#).count(), 2);
    }

    /// The outer threshold is fixed at 2 (AND); only the inner one varies.
    #[test]
    fn test_sss_nested_inner_threshold_varies_outer_stays_two() {
        let pairs = tang(3);
        for inner in 1..=3u8 {
            let got = SystemConfigurator::build_clevis_sss_config(
                &advs(&pairs),
                inner,
                Some(Tpm2Peer {
                    pcr_ids: "7",
                    pcr_bank: "sha256",
                }),
            );
            assert!(
                got.starts_with(r#"{"t":2,"pins":{"tpm2":"#),
                "outer t for {inner}"
            );
            assert!(
                got.contains(&format!(r#""sss":[{{"t":{inner},"#)),
                "inner t for {inner}"
            );
        }
    }

    /// Multi-PCR / non-default bank flow through unmodified.
    #[test]
    fn test_sss_nested_multi_pcr_ids() {
        let pairs = tang(1);
        let got = SystemConfigurator::build_clevis_sss_config(
            &advs(&pairs),
            1,
            Some(Tpm2Peer {
                pcr_ids: "0,7",
                pcr_bank: "sha384",
            }),
        );
        assert_eq!(
            got,
            r#"{"t":2,"pins":{"tpm2":{"pcr_ids":"0,7","pcr_bank":"sha384"},"sss":[{"t":1,"pins":{"tang":[{"url":"http://172.16.2.40","adv":"/run/uaa-tang-0.adv"}]}}]}}"#
        );
    }

    /// Whatever the shape, the result must be valid JSON clevis can parse.
    #[test]
    fn test_sss_output_is_valid_json_both_shapes() {
        let pairs = tang(3);
        for tpm2 in [
            None,
            Some(Tpm2Peer {
                pcr_ids: "7",
                pcr_bank: "sha256",
            }),
        ] {
            let got = SystemConfigurator::build_clevis_sss_config(&advs(&pairs), 2, tpm2);
            let parsed: serde_json::Value =
                serde_json::from_str(&got).expect("emitted policy must be valid JSON");
            assert_eq!(parsed["t"], 2);
        }
    }

    // ---- authored-tree emitter -----------------------------------------

    use super::super::unlock_sss::{Pkcs11Pin, SssPolicy, TangPin, Tpm2Pin, UnlockPin};

    fn tang_pin(url: &str) -> UnlockPin {
        UnlockPin::Tang(TangPin {
            url: url.to_string(),
        })
    }

    fn tpm2_pin() -> UnlockPin {
        UnlockPin::Tpm2(Tpm2Pin {
            pcr_ids: "7".to_string(),
            pcr_bank: "sha256".to_string(),
        })
    }

    /// The tree shape used by the bricking-regression tests: Tang lives TWO
    /// levels down, so any emitter or pre-fetch that only looks at the top level
    /// misses it. A tree with top-level-only Tang would pass under both the
    /// correct walk and the naive one — it does not discriminate.
    fn nested_tree() -> SssPolicy {
        SssPolicy {
            threshold: 2,
            pins: vec![
                tpm2_pin(),
                UnlockPin::Sss(SssPolicy {
                    threshold: 2,
                    pins: vec![
                        tang_pin("http://172.16.2.40"),
                        tang_pin("http://172.16.2.41"),
                        tang_pin("http://172.16.2.42"),
                    ],
                }),
            ],
        }
    }

    fn adv_pairs(urls: &[&str]) -> Vec<(String, String)> {
        urls.iter()
            .enumerate()
            .map(|(i, u)| ((*u).to_string(), format!("/run/uaa-tang-{i}.adv")))
            .collect()
    }

    /// The cheap proof the two emitters agree where they overlap: a tree that is
    /// nothing but a flat Tang group must serialize byte-for-byte as the legacy
    /// flat builder — the string len-serv-001/002 are bound with.
    #[test]
    fn test_tree_emitter_flat_tang_matches_legacy_emitter_byte_for_byte() {
        for n in 1..=4usize {
            for threshold in 1..=3u8 {
                let pairs = tang(n);
                let urls: Vec<&str> = pairs.iter().map(|(u, _)| u.as_str()).collect();
                let tree = SssPolicy {
                    threshold,
                    pins: urls.iter().map(|u| tang_pin(u)).collect(),
                };
                let from_tree = SystemConfigurator::build_clevis_policy_from_tree(&tree, &pairs);
                let legacy =
                    SystemConfigurator::build_clevis_sss_config(&advs(&pairs), threshold, None);
                assert_eq!(from_tree, legacy, "drift for n={n} t={threshold}");
            }
        }
    }

    /// Hardcoded golden for the nested tree. Note `"tpm2":[{…}]` — every kind is
    /// emitted as an array, uniformly. An N-element array is N shares, so a
    /// 1-element array is exactly one share: same semantics as the bare object
    /// the flat emitter writes, with no per-kind special case to get wrong.
    #[test]
    fn test_tree_emitter_nested_and_golden() {
        let pairs = adv_pairs(&[
            "http://172.16.2.40",
            "http://172.16.2.41",
            "http://172.16.2.42",
        ]);
        let got = SystemConfigurator::build_clevis_policy_from_tree(&nested_tree(), &pairs);
        assert_eq!(
            got,
            r#"{"t":2,"pins":{"tpm2":[{"pcr_ids":"7","pcr_bank":"sha256"}],"sss":[{"t":2,"pins":{"tang":[{"url":"http://172.16.2.40","adv":"/run/uaa-tang-0.adv"},{"url":"http://172.16.2.41","adv":"/run/uaa-tang-1.adv"},{"url":"http://172.16.2.42","adv":"/run/uaa-tang-2.adv"}]}}]}}"#
        );
        // The 2-of-4 bug: Tang must never appear as an OUTER sibling of tpm2.
        assert!(!got.starts_with(r#"{"t":2,"pins":{"tang":"#));
        assert!(got.contains(r#""sss":[{"t":2,"pins":{"tang":"#));
        let parsed: serde_json::Value = serde_json::from_str(&got).expect("valid JSON");
        assert_eq!(parsed["t"], 2);
        // Exactly two outer shares — tpm2 and the collapsed Tang group.
        assert_eq!(parsed["pins"]["tpm2"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["pins"]["sss"].as_array().unwrap().len(), 1);
    }

    /// Every authored kind round-trips through `kind()` into its own array key,
    /// and same-kind pins at one level collapse into ONE key (clevis's `pins` is
    /// an object; a duplicate key would be invalid JSON).
    #[test]
    fn test_tree_emitter_groups_same_kind_and_carries_pkcs11() {
        let tree = SssPolicy {
            threshold: 2,
            pins: vec![
                UnlockPin::Pkcs11(Pkcs11Pin {
                    uri: "pkcs11:serial=YK0000001;token=TOKYK0000001".to_string(),
                    mechanism: None,
                }),
                UnlockPin::Pkcs11(Pkcs11Pin {
                    uri: "pkcs11:serial=YK0000002;token=TOKYK0000002".to_string(),
                    mechanism: None,
                }),
                tang_pin("http://172.16.2.40"),
            ],
        };
        let got = SystemConfigurator::build_clevis_policy_from_tree(
            &tree,
            &adv_pairs(&["http://172.16.2.40"]),
        );
        assert_eq!(
            got,
            r#"{"t":2,"pins":{"pkcs11":[{"uri":"pkcs11:serial=YK0000001;token=TOKYK0000001","mechanism":"RSA-PKCS"},{"uri":"pkcs11:serial=YK0000002;token=TOKYK0000002","mechanism":"RSA-PKCS"}],"tang":[{"url":"http://172.16.2.40","adv":"/run/uaa-tang-0.adv"}]}}"#
        );
        serde_json::from_str::<serde_json::Value>(&got).expect("valid JSON");
    }

    // ---- golden: the settled fleet policies ------------------------------

    const PEER_A: &str = "http://172.16.2.45";
    const PEER_B: &str = "http://172.16.2.46";
    const NANO: &str = "pkcs11:serial=NANO0001;token=TOKNANO0001";
    const CARRIED_A: &str = "pkcs11:serial=CARRIED0A;token=TOKCARRIED0A";
    const CARRIED_B: &str = "pkcs11:serial=CARRIED0B;token=TOKCARRIED0B";

    /// Emit the settled fleet policy, parsed to a `Value`.
    ///
    /// Deliberately parsed, never substring-matched: substring assertions on
    /// clevis JSON have produced three separate false-passes in this repo, and
    /// they will keep doing so, because every group in this policy is a
    /// *superstring* of a weaker group. `"tang":[peerA,peerB]` appears inside
    /// the correct policy AND inside a broken one that flattened group 3, so any
    /// `contains()` on it passes either way. Structure is the only assertion
    /// that discriminates.
    fn emit_fleet_policy(tpm2_pcr_ids: Option<&str>) -> serde_json::Value {
        let policy = SssPolicy::fleet_three_group(
            &[PEER_A, PEER_B],
            2,
            NANO,
            &[CARRIED_A, CARRIED_B],
            2,
            tpm2_pcr_ids,
        );
        // The policy must be emittable in the first place — a golden test on an
        // invalid policy documents a shape we would refuse to install.
        policy.validate().expect("fleet policy must validate");
        let json = SystemConfigurator::build_clevis_policy_from_tree(
            &policy,
            &adv_pairs(&[PEER_A, PEER_B]),
        );
        serde_json::from_str(&json).expect("emitter must produce valid JSON")
    }

    /// `{"url":…,"adv":…}` for one Tang peer, as `adv_pairs` numbers them.
    fn tang_entry(url: &str, index: usize) -> serde_json::Value {
        serde_json::json!({"url": url, "adv": format!("/run/uaa-tang-{index}.adv")})
    }

    /// `{"uri":…,"mechanism":…}` for one token share. The mechanism is NOT
    /// optional in a binding — omit it and the policy binds cleanly and then
    /// fails at unlock with `Decrypt mechanism not supported`.
    fn pkcs11_entry(uri: &str) -> serde_json::Value {
        serde_json::json!({"uri": uri, "mechanism": DEFAULT_PKCS11_MECHANISM})
    }

    /// The two peer Tang servers as a `t`-of-2 group.
    fn tang_group(t: u8) -> serde_json::Value {
        serde_json::json!({
            "t": t,
            "pins": {"tang": [tang_entry(PEER_A, 0), tang_entry(PEER_B, 1)]}
        })
    }

    /// Group 2 (any 2 of 3 tokens) and group 3 ((any Tang) AND (either CARRIED
    /// key)) are identical between the RPi and lenserv policies; only group 1
    /// differs, by ANDing tpm2.
    fn group_two() -> serde_json::Value {
        serde_json::json!({
            "t": 2,
            "pins": {"pkcs11": [
                pkcs11_entry(NANO),
                pkcs11_entry(CARRIED_A),
                pkcs11_entry(CARRIED_B),
            ]}
        })
    }

    fn group_three() -> serde_json::Value {
        serde_json::json!({
            "t": 2,
            "pins": {"sss": [
                tang_group(1),
                {"t": 1, "pins": {"pkcs11": [
                    pkcs11_entry(CARRIED_A),
                    pkcs11_entry(CARRIED_B),
                ]}},
            ]}
        })
    }

    /// GOLDEN — the RPi Tang-server policy, whole, asserted as one structure.
    #[test]
    fn test_golden_rpi_fleet_policy() {
        let got = emit_fleet_policy(None);
        let want = serde_json::json!({
            "t": 1,
            "pins": {"sss": [
                // group 1 — automatic unlock while both peers are up. NO tpm2:
                // the RPis have no TPM and deliberately get none anywhere.
                //
                // The chassis nano is ANDed in precisely BECAUSE there is no
                // TPM. This golden previously expected a bare `tang_group(2)`,
                // which was two shares but both of them Tang — so two Tang keys
                // decrypted the volume, and the outer t=1 OR propagated that to
                // the whole policy. The nano fills the structural role a TPM
                // plays on a lenserv: a factor that is always present at boot.
                {
                    "t": 2,
                    "pins": {
                        "pkcs11": [pkcs11_entry(NANO)],
                        "sss": [tang_group(2)],
                    }
                },
                // group 2 — any 2 of the 3 tokens.
                group_two(),
                // group 3 — (any one Tang) AND (either CARRIED key).
                group_three(),
            ]}
        });
        assert_eq!(got, want);

        // The RPis have no TPM: assert the ABSENCE of tpm2 anywhere in the tree,
        // not just at the top level, since a stray tpm2 share would make the
        // policy unsatisfiable on hardware that cannot provide it.
        assert!(
            !got.to_string().contains("pcr_ids"),
            "the RPi policy must carry no tpm2 pin at any depth: {got}"
        );
    }

    /// GOLDEN — the lenserv variant: identical, except group 1 ANDs tpm2.
    #[test]
    fn test_golden_lenserv_fleet_policy() {
        let got = emit_fleet_policy(Some("7"));
        let want = serde_json::json!({
            "t": 1,
            "pins": {"sss": [
                // group 1 — tpm2 AND (both peers). Outer t=2 over exactly TWO
                // shares (the tpm2 pin, and the whole Tang group collapsed to
                // one by nesting), so BOTH are required. Flattening this into
                // {"t":2,"pins":{"tpm2":…,"tang":[a,b]}} would be 2-of-3 and
                // satisfiable by Tang alone.
                {
                    "t": 2,
                    "pins": {
                        "tpm2": [{"pcr_ids": "7", "pcr_bank": "sha256"}],
                        "sss": [tang_group(2)],
                    }
                },
                group_two(),
                group_three(),
            ]}
        });
        assert_eq!(got, want);

        // The ONLY difference from the RPi policy is group 1 — assert it, so a
        // future edit cannot quietly diverge groups 2 and 3 between platforms.
        let rpi = emit_fleet_policy(None);
        assert_eq!(got["pins"]["sss"][1], rpi["pins"]["sss"][1]);
        assert_eq!(got["pins"]["sss"][2], rpi["pins"]["sss"][2]);
        assert_ne!(got["pins"]["sss"][0], rpi["pins"]["sss"][0]);
    }

    /// The fleet policy is the first tree that names the SAME Tang peer twice —
    /// once in group 1 and again inside group 3 — so `tang_urls()` returns
    /// `[peerA, peerB, peerA, peerB]`.
    ///
    /// Every other emitter test hands `build_clevis_policy_from_tree` a
    /// hand-written `adv_pairs(&[PEER_A, PEER_B])`, which quietly papers over
    /// what `enroll_tang_clevis` actually does with a repeating list. This test
    /// builds `advs` the way the installer builds them — by walking
    /// `tang_urls()` and skipping a URL already fetched — and asserts the
    /// emitted JSON is still the golden one. If the pre-fetch loop ever stops
    /// deduplicating (four fetches numbered 0..3, so group 3's peerA resolves to
    /// `/run/uaa-tang-2.adv` instead of `-0`), or starts erroring on a repeat,
    /// this fails instead of the bind failing on real hardware.
    #[test]
    fn test_adv_prefetch_survives_a_peer_named_in_two_groups() {
        let policy = SssPolicy::fleet_three_group(
            &[PEER_A, PEER_B],
            2,
            NANO,
            &[CARRIED_A, CARRIED_B],
            2,
            None,
        );

        let urls = policy.tang_urls();
        assert_eq!(
            urls,
            vec![PEER_A, PEER_B, PEER_A, PEER_B],
            "the fixture must actually repeat, or this test is vacuous"
        );

        // Mirror of the loop in `enroll_tang_clevis`: skip a URL already
        // fetched, number by fetch order.
        let mut advs: Vec<(String, String)> = Vec::new();
        for url in &urls {
            if advs.iter().any(|(u, _)| u == url) {
                continue;
            }
            let path = format!("/run/uaa-tang-{}.adv", advs.len());
            advs.push(((*url).to_string(), path));
        }
        assert_eq!(advs.len(), 2, "one advertisement per DISTINCT server");

        let got: serde_json::Value = serde_json::from_str(
            &SystemConfigurator::build_clevis_policy_from_tree(&policy, &advs),
        )
        .expect("valid JSON");
        assert_eq!(got, emit_fleet_policy(None));

        // No Tang share may carry an empty adv — that is how this fails on real
        // hardware, since clevis then prompts on /dev/tty and the bind dies.
        assert!(
            !got.to_string().contains(r#""adv":"""#),
            "every Tang share must resolve to a fetched advertisement: {got}"
        );
    }

    /// GOLDEN — `mechanism` is ALWAYS emitted, and is never `null`.
    ///
    /// The config shape clevis 23 reads is `{"uri":…,"mechanism":…}`, and
    /// `clevis-decrypt-pkcs11` passes `--mechanism` only when the value is
    /// non-empty. Measured: omit it and the policy binds cleanly and then never
    /// unlocks — `error: Decrypt mechanism not supported`, i.e. a green install
    /// and a dead host. So an unset mechanism is filled in here with
    /// [`DEFAULT_PKCS11_MECHANISM`] rather than being skipped.
    ///
    /// `skip_serializing_if` on the struct field stays, and stays load-bearing
    /// in the other direction: without it a `None` that somehow reached the
    /// wire would serialize as `"mechanism": null`, which clevis cannot hand to
    /// `pkcs11-tool` at all. The emitter's job is to make sure `None` never
    /// gets that far.
    #[test]
    fn test_golden_pkcs11_mechanism_is_always_emitted() {
        let tree = SssPolicy {
            threshold: 1,
            pins: vec![
                UnlockPin::Pkcs11(Pkcs11Pin {
                    uri: "pkcs11:serial=YK0000001;token=Yubi1".to_string(),
                    mechanism: None,
                }),
                UnlockPin::Pkcs11(Pkcs11Pin {
                    uri: "pkcs11:serial=YK0000002;token=Yubi2".to_string(),
                    mechanism: Some("RSA-PKCS-OAEP".to_string()),
                }),
            ],
        };
        tree.validate().expect("both shares must be valid");

        let raw = SystemConfigurator::build_clevis_policy_from_tree(&tree, &[]);
        let got: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(
            got,
            serde_json::json!({
                "t": 1,
                "pins": {"pkcs11": [
                    // Unset -> filled with the working default, NOT skipped.
                    {"uri": "pkcs11:serial=YK0000001;token=Yubi1", "mechanism": DEFAULT_PKCS11_MECHANISM},
                    {"uri": "pkcs11:serial=YK0000002;token=Yubi2", "mechanism": "RSA-PKCS-OAEP"},
                ]}
            })
        );
        assert!(
            !raw.contains("null"),
            "mechanism must never be emitted as null: {raw}"
        );

        // Module path and slot are URI attributes, never config keys — clevis
        // derives both from the uri. A struct field for either would serialize
        // into the binding and be silently ignored.
        let with_module = SssPolicy {
            threshold: 1,
            pins: vec![UnlockPin::Pkcs11(Pkcs11Pin {
                uri: "pkcs11:serial=YK0000001;token=Yubi1;module-path=/usr/lib/opensc-pkcs11.so"
                    .to_string(),
                mechanism: None,
            })],
        };
        with_module
            .validate()
            .expect("a module-path= attribute is a normal part of the URI");
        let got: serde_json::Value = serde_json::from_str(
            &SystemConfigurator::build_clevis_policy_from_tree(&with_module, &[]),
        )
        .expect("valid JSON");
        let share = &got["pins"]["pkcs11"][0];
        assert_eq!(
            share.as_object().unwrap().len(),
            2,
            "uri and mechanism are the only keys: {share}"
        );
        assert!(share["uri"].as_str().unwrap().contains("module-path="));
    }

    /// TOPOLOGY — EVERY pkcs11 share, at EVERY depth of the settled fleet tree,
    /// carries a non-empty `mechanism`.
    ///
    /// Walks the parsed JSON rather than scanning the string: a `contains()`
    /// check would pass as soon as ONE share had a mechanism, which is exactly
    /// the shape of the bug (one good share, four silently unusable ones).
    #[test]
    fn test_every_pkcs11_share_in_the_fleet_policy_has_a_mechanism() {
        fn walk(node: &serde_json::Value, found: &mut usize) {
            match node {
                serde_json::Value::Object(map) => {
                    for (key, val) in map {
                        if key == "pkcs11" {
                            let entries: Vec<&serde_json::Value> = match val {
                                serde_json::Value::Array(items) => items.iter().collect(),
                                other => vec![other],
                            };
                            for entry in entries {
                                let mech = entry.get("mechanism").unwrap_or_else(|| {
                                    panic!("pkcs11 share has no mechanism: {entry}")
                                });
                                let mech = mech.as_str().unwrap_or_else(|| {
                                    panic!("mechanism is not a string: {entry}")
                                });
                                assert!(!mech.is_empty(), "empty mechanism: {entry}");
                                *found += 1;
                            }
                        }
                        walk(val, found);
                    }
                }
                serde_json::Value::Array(items) => items.iter().for_each(|i| walk(i, found)),
                _ => {}
            }
        }

        for tpm2 in [None, Some("7")] {
            let got = emit_fleet_policy(tpm2);
            let mut found = 0usize;
            walk(&got, &mut found);
            // The absence assertion above is vacuous if the walk found nothing,
            // so pin the count. Group 2 holds nano + 2 carried and group 3 holds
            // the 2 carried again = 5. A TPM-less host adds a sixth: the nano
            // ANDed into group 1, standing in for the TPM it does not have.
            let expected = if tpm2.is_none() { 6 } else { 5 };
            assert_eq!(
                found, expected,
                "expected {expected} pkcs11 shares in the fleet tree (tpm2={tpm2:?}): {got}"
            );
        }
    }

    /// SECURITY PROPERTY — no EMITTED policy is satisfiable by one kind of
    /// factor, checked by the shipping verifier rather than by a second opinion.
    ///
    /// The emitter and `verify.rs`'s test fixtures are separate hand-written
    /// trees, so they can drift — and did: the fixture and the emitter both
    /// carried a bare Tang group 1 for TPM-less hosts, and each looked correct
    /// against the other. Running the REAL emitter output through the REAL
    /// verifier is the only assertion that cannot be satisfied by fixing one
    /// side alone.
    #[test]
    fn test_emitted_fleet_policy_is_never_satisfiable_by_one_factor_kind() {
        use crate::autoinstall::verify::satisfiable_with_only;

        for tpm2 in [None, Some("7")] {
            let got = emit_fleet_policy(tpm2);
            for kind in ["tang", "tpm2"] {
                assert!(
                    !satisfiable_with_only(&got, kind),
                    "the emitted policy (tpm2={tpm2:?}) must not be satisfiable by \
                     {kind} alone — multiple shares of one kind fall to a single \
                     compromise: {got}"
                );
            }
        }
    }

    /// The counterpart: `pkcs11` alone IS allowed to satisfy a policy, because
    /// group 2 is deliberately all-token with zero Tang — it is the cold-outage
    /// bootstrap. If this ever starts failing, someone has broadened the
    /// factor-diversity rule into rejecting the design it exists to protect.
    #[test]
    fn test_group_two_is_deliberately_reachable_with_tokens_alone() {
        use crate::autoinstall::verify::satisfiable_with_only;

        let got = emit_fleet_policy(Some("7"));
        assert!(
            satisfiable_with_only(&got, "pkcs11"),
            "group 2 (2-of-3 tokens, zero Tang) must remain reachable with tokens \
             alone — that is the whole point of a cold-outage bootstrap: {got}"
        );
    }

    /// SECURITY PROPERTY — the nano is not a group-3 factor, in the EMITTED
    /// JSON.
    ///
    /// The nano lives in the chassis, so a thief who steals the server already
    /// holds it. Group 3 is (any one Tang) AND (either carried key); if the nano
    /// counted, that thief would need only to reach ONE Tang — trivial while the
    /// box is still on the LAN — and the disk would open. This test fails if
    /// someone "helpfully" adds it.
    #[test]
    fn test_nano_is_excluded_from_group_three() {
        for tpm2 in [None, Some("7")] {
            let got = emit_fleet_policy(tpm2);
            let group_three = &got["pins"]["sss"][2];

            // Collect every pkcs11 uri ANYWHERE under group 3, so nesting the
            // nano one level deeper does not smuggle it past this test.
            let mut uris = Vec::new();
            collect_pkcs11_uris(group_three, &mut uris);

            // BOTH directions. An absence assertion on a path navigated wrong
            // passes vacuously — so first prove the carried keys ARE here.
            assert_eq!(
                uris,
                vec![CARRIED_A, CARRIED_B],
                "group 3's token factor must be exactly the two CARRIED keys"
            );
            assert!(
                !uris.contains(&NANO.to_string()),
                "the chassis-resident nano must NEVER be a group-3 factor"
            );

            // ...and it is present in group 2, so the above is a deliberate
            // exclusion rather than the nano being missing from the policy.
            let mut group_two_uris = Vec::new();
            collect_pkcs11_uris(&got["pins"]["sss"][1], &mut group_two_uris);
            assert!(
                group_two_uris.contains(&NANO.to_string()),
                "the nano must still be one of group 2's three tokens"
            );
        }
    }

    /// Every `pins.pkcs11[].uri` at any depth under `node`, in document order.
    fn collect_pkcs11_uris(node: &serde_json::Value, out: &mut Vec<String>) {
        let Some(pins) = node.get("pins") else {
            return;
        };
        if let Some(tokens) = pins.get("pkcs11").and_then(|p| p.as_array()) {
            for token in tokens {
                out.push(
                    token["uri"]
                        .as_str()
                        .expect("a pkcs11 share must carry a uri")
                        .to_string(),
                );
            }
        }
        if let Some(nested) = pins.get("sss").and_then(|p| p.as_array()) {
            for child in nested {
                collect_pkcs11_uris(child, out);
            }
        }
    }

    // ---- the bricking regression ---------------------------------------

    /// Config for a host that declares its Tang servers ONLY inside the tree —
    /// `tang_servers` is empty, exactly the shape that used to install clean and
    /// boot to an unsatisfiable LUKS prompt.
    fn tree_only_config() -> InstallationConfig {
        let mut cfg = sample_netplan_config("192.0.2.10/24", "networkd");
        cfg.tang_servers = vec![];
        cfg.unlock_sss = Some(nested_tree());
        // Distinctive: the shared fixture's `"key"` is a substring of the
        // `/run/.uaa-tang-enroll.key` tempfile path, which would make the
        // passphrase-leak assertion below fire on the path instead of the key.
        cfg.luks_key = "PASSPHRASE-must-never-appear".into();
        cfg
    }

    /// The unlock-policy rules must gate the BIND, not merely the registry.
    ///
    /// `validate_resolved` runs the same rules, but only inside `uaa-control`'s
    /// `resolve_from_registry` — so a hand-authored `InstallationConfig` handed
    /// straight to the installer would never see them. Each of these policies
    /// produces a host that looks bound and is not, which is strictly worse than
    /// a failed install, so `enroll_tang_clevis` fails CLOSED before touching
    /// the device.
    #[tokio::test]
    async fn test_enroll_refuses_an_invalid_policy_before_binding() {
        let bad_policies = [
            // PIN written into the LUKS header in the clear.
            (
                "pin-value=",
                SssPolicy {
                    threshold: 1,
                    pins: vec![UnlockPin::Pkcs11(Pkcs11Pin {
                        uri: "pkcs11:serial=YK1;pin-value=1234".to_string(),
                        mechanism: None,
                    })],
                },
            ),
            // Threshold clevis would reject part-way through the bind.
            (
                "threshold 4",
                SssPolicy {
                    threshold: 4,
                    pins: vec![tang_pin("http://172.16.2.40")],
                },
            ),
            // Same token twice: reads as 2-of-2, satisfiable by one token.
            (
                "duplicate pkcs11",
                SssPolicy {
                    threshold: 2,
                    pins: vec![
                        UnlockPin::Pkcs11(Pkcs11Pin {
                            uri: "pkcs11:serial=YK1;token=TOKYK1".to_string(),
                            mechanism: None,
                        }),
                        UnlockPin::Pkcs11(Pkcs11Pin {
                            uri: "pkcs11:serial=YK1;token=TOKYK1".to_string(),
                            mechanism: None,
                        }),
                    ],
                },
            ),
        ];

        for (needle, policy) in bad_policies {
            let mut runner = RecordingExecutor::default();
            let log = runner.commands.clone();
            let mut cfg = tree_only_config();
            cfg.unlock_sss = Some(policy);

            let err = SystemConfigurator::new(&mut runner)
                .enroll_tang_clevis(&cfg, "/dev/zvol/rpool/keystore", true)
                .await
                .expect_err("an invalid policy must not be bound");
            assert!(
                err.to_string().contains(needle),
                "error must name the violation `{needle}`: {err}"
            );

            // Fails CLOSED and EARLY: nothing was fetched, nothing was bound.
            // A refusal that happens after `clevis luks bind` is not a refusal.
            assert!(
                log.lock().unwrap().is_empty(),
                "no command may run before the policy is accepted: {:?}",
                log.lock().unwrap()
            );
        }

        // Positive control: the settled fleet policy binds. Without this the
        // assertions above would also pass if `enroll_tang_clevis` simply
        // refused every tree.
        let mut runner = RecordingExecutor::default();
        let mut cfg = tree_only_config();
        cfg.unlock_sss = Some(SssPolicy::fleet_three_group(
            &[PEER_A, PEER_B],
            2,
            NANO,
            &[CARRIED_A, CARRIED_B],
            2,
            None,
        ));
        SystemConfigurator::new(&mut runner)
            .enroll_tang_clevis(&cfg, "/dev/zvol/rpool/keystore", true)
            .await
            .expect("the settled fleet policy must bind");
    }

    /// THE regression. A tree-only host must still install clevis, still
    /// pre-fetch EVERY nested Tang advertisement, and still run `clevis luks
    /// bind` with the nested policy. Against the un-integrated code this test
    /// fails on the very first assertion: the bind never happens at all.
    #[tokio::test]
    async fn test_tree_only_host_with_empty_tang_roster_still_prefetches_and_binds() {
        let mut runner = RecordingExecutor::default();
        let log = runner.commands.clone();
        let cfg = tree_only_config();
        assert!(
            cfg.tang_servers.is_empty(),
            "fixture must exercise the EMPTY-roster path"
        );

        SystemConfigurator::new(&mut runner)
            .enroll_tang_clevis(&cfg, "/dev/zvol/rpool/keystore", true)
            .await
            .expect("tree-only enrollment must succeed");

        let cmds = log.lock().unwrap().clone();
        let joined = cmds.join("\n");

        // Every NESTED Tang URL got its advertisement pre-fetched. Without a
        // pre-fetched adv, `clevis luks bind` prompts on /dev/tty and dies
        // non-interactively over SSH — no unattended-unlock binding.
        for (i, url) in [
            "http://172.16.2.40",
            "http://172.16.2.41",
            "http://172.16.2.42",
        ]
        .iter()
        .enumerate()
        {
            let expected = format!("curl -sf --max-time 10 {url}/adv -o /run/uaa-tang-{i}.adv");
            assert!(
                cmds.iter().any(|c| c == &expected),
                "missing nested adv pre-fetch: {expected}\n--- got ---\n{joined}"
            );
        }

        // The bind actually ran, against the keystore, with the AUTHORED nested
        // policy rather than a flattened one.
        let bind = cmds
            .iter()
            .find(|c| c.starts_with("clevis luks bind"))
            .unwrap_or_else(|| panic!("no clevis luks bind issued\n--- got ---\n{joined}"));
        assert!(bind.contains("-d /dev/zvol/rpool/keystore"), "bind: {bind}");
        assert!(
            bind.contains(r#""sss":[{"t":2,"pins":{"tang":"#),
            "Tang group must stay NESTED (an outer tang array is the 2-of-4 bug): {bind}"
        );
        assert!(
            !bind.contains(r#"{"t":2,"pins":{"tang":[{"url":"http://172.16.2.40","adv":"/run/uaa-tang-0.adv"},{"url":"http://172.16.2.41""#)
                || bind.contains(r#""sss":["#),
            "policy must not be flattened: {bind}"
        );
        // The passphrase never reaches the bind command line.
        assert!(!bind.contains(&cfg.luks_key), "passphrase leaked: {bind}");
    }

    /// The tree wins wholesale: `tang_threshold` and `include_tpm2_peer` are
    /// ignored when a tree is authored. Grafting the default peer share on top
    /// of an authored policy silently changes the share arithmetic its author
    /// computed.
    #[tokio::test]
    async fn test_authored_tree_ignores_tang_threshold_and_forced_tpm2_peer() {
        let mut cfg = tree_only_config();
        // A tree with NO tpm2 pin, on the NativeKeystore path that would
        // otherwise force one in.
        cfg.unlock_sss = Some(SssPolicy {
            threshold: 3,
            pins: vec![
                tang_pin("http://172.16.2.40"),
                tang_pin("http://172.16.2.41"),
                tang_pin("http://172.16.2.42"),
            ],
        });
        // Deliberately absurd, so a leak into the output is unmistakable.
        cfg.tang_threshold = 9;
        cfg.tpm2_pcr_ids = "0,1,2,3".into();

        let mut runner = RecordingExecutor::default();
        let log = runner.commands.clone();
        SystemConfigurator::new(&mut runner)
            .enroll_tang_clevis(&cfg, "/dev/zvol/rpool/keystore", true)
            .await
            .expect("enrollment must succeed");

        let cmds = log.lock().unwrap().clone();
        let bind = cmds
            .iter()
            .find(|c| c.starts_with("clevis luks bind"))
            .expect("bind issued");
        assert!(
            bind.contains(r#"{"t":3,"pins":{"tang":["#),
            "authored threshold must win over config.tang_threshold: {bind}"
        );
        assert!(
            !bind.contains("\"t\":9"),
            "config.tang_threshold must not leak into an authored tree: {bind}"
        );
        assert!(
            !bind.contains("tpm2"),
            "include_tpm2_peer must not graft a share onto an authored tree: {bind}"
        );
    }

    /// A tree naming the same Tang server twice fetches ONE advertisement and
    /// references it from both pins — the url-keyed lookup, not a positional one.
    #[tokio::test]
    async fn test_repeated_tang_url_in_tree_fetches_one_adv() {
        let mut cfg = tree_only_config();
        cfg.unlock_sss = Some(SssPolicy {
            threshold: 1,
            pins: vec![
                tang_pin("http://172.16.2.40"),
                UnlockPin::Sss(SssPolicy {
                    threshold: 1,
                    pins: vec![tang_pin("http://172.16.2.40")],
                }),
            ],
        });

        let mut runner = RecordingExecutor::default();
        let log = runner.commands.clone();
        SystemConfigurator::new(&mut runner)
            .enroll_tang_clevis(&cfg, "/dev/sda4", false)
            .await
            .expect("enrollment must succeed");

        let cmds = log.lock().unwrap().clone();
        assert_eq!(
            cmds.iter().filter(|c| c.starts_with("curl -sf")).count(),
            1,
            "same server twice = one advertisement"
        );
        let bind = cmds
            .iter()
            .find(|c| c.starts_with("clevis luks bind"))
            .expect("bind issued");
        // Both pins resolve to the same adv path — and neither is empty, which
        // is what an unresolved lookup would emit.
        assert_eq!(bind.matches(r#""adv":"/run/uaa-tang-0.adv""#).count(), 2);
        assert!(!bind.contains(r#""adv":"""#), "unresolved adv: {bind}");
    }

    /// The package gate, same bricking class one layer up: a tree-only host with
    /// an EMPTY roster must still get clevis installed — and `clevis-tpm2` when
    /// its tree carries a tpm2 pin, even on PlainLuks, since the tpm2 pin's
    /// decrypter ships in that separate package and its absence only surfaces in
    /// the initramfs at first boot.
    #[tokio::test]
    async fn test_tree_only_host_still_installs_clevis_packages() {
        for (mode, tree, want_tpm2) in [
            (StorageMode::PlainLuks, nested_tree(), true),
            (
                StorageMode::PlainLuks,
                SssPolicy {
                    threshold: 2,
                    pins: vec![
                        tang_pin("http://172.16.2.40"),
                        tang_pin("http://172.16.2.41"),
                    ],
                },
                false,
            ),
        ] {
            let mut cfg = tree_only_config();
            cfg.storage_mode = mode;
            cfg.unlock_sss = Some(tree);

            let mut runner = RecordingExecutor::default();
            let log = runner.commands.clone();
            SystemConfigurator::new(&mut runner)
                .configure_system_in_chroot(&cfg)
                .await
                .expect("chroot configuration must succeed");

            let cmds = log.lock().unwrap().clone();
            let apt = cmds
                .iter()
                .find(|c| c.contains("apt install -y grub-efi-amd64"))
                .expect("the real apt install line");
            assert!(
                apt.contains(" clevis clevis-luks"),
                "tree-only host must still install clevis: {apt}"
            );
            assert_eq!(
                apt.contains("clevis-tpm2"),
                want_tpm2,
                "clevis-tpm2 must follow the tree's tpm2 pin: {apt}"
            );
        }
    }

    /// The CALL-SITE guard, which the direct `enroll_tang_clevis` tests above
    /// deliberately bypass. This is the literal bricking path: NativeKeystore
    /// Phase 5 used to skip enrollment entirely on an empty `tang_servers`, so a
    /// tree-only host finished Phase 5 with no binding and the install reported
    /// success.
    #[tokio::test]
    async fn test_keystore_phase5_reaches_the_bind_for_a_tree_only_host() {
        let mut runner = RecordingExecutor::default();
        let log = runner.commands.clone();
        let mut cfg = tree_only_config();
        cfg.storage_mode = StorageMode::NativeKeystore;

        SystemConfigurator::new(&mut runner)
            .setup_keystore_luks_in_chroot(&cfg)
            .await
            .expect("keystore phase 5 must succeed");

        let cmds = log.lock().unwrap().clone();
        assert!(
            cmds.iter()
                .any(|c| c.starts_with("clevis luks bind")
                    && c.contains("-d /dev/zvol/rpool/keystore")),
            "an empty tang_servers roster must NOT skip the keystore bind\n{}",
            cmds.join("\n")
        );
    }

    /// Same guard on the PlainLuks path.
    #[tokio::test]
    async fn test_plainluks_reaches_the_bind_for_a_tree_only_host() {
        let mut runner = RecordingExecutor::default();
        let log = runner.commands.clone();
        let mut cfg = tree_only_config();
        cfg.storage_mode = StorageMode::PlainLuks;
        // Not a real TPM2+PIN host; keep first-boot enrollment out of the way.
        cfg.enroll_tpm2 = false;

        SystemConfigurator::new(&mut runner)
            .setup_luks_key_in_chroot(&cfg)
            .await
            .expect("plainluks crypttab phase must succeed");

        let cmds = log.lock().unwrap().clone();
        assert!(
            cmds.iter().any(|c| c.starts_with("clevis luks bind")),
            "an empty tang_servers roster must NOT skip the PlainLuks bind\n{}",
            cmds.join("\n")
        );
    }

    /// The initramfs network guards are the same class: a tree-only Tang host
    /// with no `rd.neednet=1` / no forced NIC driver has a bind it can never
    /// satisfy at boot.
    #[tokio::test]
    async fn test_tree_only_tang_host_gets_initramfs_network() {
        let mut runner = RecordingExecutor::default();
        let log = runner.commands.clone();
        let cfg = tree_only_config();
        SystemConfigurator::new(&mut runner)
            .configure_dracut_crypt_modules(&cfg)
            .await
            .expect("dracut config must succeed");

        // Assert on the add_dracutmodules+= LINE, never on the whole command:
        // the conf's static comment block mentions both "clevis" and "network",
        // so a substring search over the full text passes even when the modules
        // are absent.
        let modules = dracut_modules_line(&log.lock().unwrap().join("\n"));
        assert!(
            modules.contains("network"),
            "tree-only Tang host needs the dracut network module: {modules}"
        );
        assert!(
            modules.contains("clevis"),
            "tree-only Tang host needs the dracut clevis module: {modules}"
        );
    }

    /// A TANG-LESS tree still gets the clevis dracut module. The bind happens
    /// for any authored tree, so the module must too — gating the module on
    /// "uses Tang" while gating the bind on "has a tree" lets the two diverge
    /// into an installed-and-bound host whose initramfs cannot run clevis.
    #[tokio::test]
    async fn test_tang_less_tree_still_gets_the_clevis_dracut_module() {
        for tree in [
            // tpm2 only.
            SssPolicy {
                threshold: 1,
                pins: vec![tpm2_pin()],
            },
            // pkcs11 OR — the shape `SssPolicy::any_pkcs11` builds.
            SssPolicy {
                threshold: 1,
                pins: vec![
                    UnlockPin::Pkcs11(Pkcs11Pin {
                        uri: "pkcs11:serial=YK0000001;token=TOKYK0000001".to_string(),
                        mechanism: None,
                    }),
                    UnlockPin::Pkcs11(Pkcs11Pin {
                        uri: "pkcs11:serial=YK0000002;token=TOKYK0000002".to_string(),
                        mechanism: None,
                    }),
                ],
            },
        ] {
            let mut cfg = tree_only_config();
            cfg.unlock_sss = Some(tree);
            assert!(
                cfg.unlock_sss.as_ref().unwrap().tang_urls().is_empty(),
                "fixture must be Tang-LESS"
            );

            let mut runner = RecordingExecutor::default();
            let log = runner.commands.clone();
            SystemConfigurator::new(&mut runner)
                .configure_dracut_crypt_modules(&cfg)
                .await
                .expect("dracut config must succeed");

            let modules = dracut_modules_line(&log.lock().unwrap().join("\n"));
            assert!(
                modules.contains("clevis"),
                "a Tang-less tree is still bound with clevis, so the module is \
                 required: {modules}"
            );
        }
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
            users: Vec::new(),
            enroll_tpm2: true,
            tpm2_pin: None,
            tpm2_pcr_ids: "7".into(),
            expect_fido2: true,
            clevis_pkcs11_pin: false,
            install_ca_cert: "test-ca-pem".into(),
            applications: vec![],
            cockroach_members: Vec::new(),
            storage_mode: Default::default(),
            disks: Vec::new(),
            arch: Default::default(),
            role: Default::default(),
            firmware_quirks: Vec::new(),
            hooks: Default::default(),
            unlock_sss: None,
        }
    }

    /// The chroot apt line as it stood before clevis-23 support existed, for
    /// `sample_netplan_config` (Dracut, PlainLuks, no Tang, TPM2+FIDO2 on).
    /// This is the line that actually installs len-serv-001/002 — the
    /// `build_next_commands_after_storage` list in installer.rs is only the
    /// pause-after-storage manual transcript.
    const BASELINE_CHROOT_APT_LINE: &str = "chroot /mnt/targetos bash -lc \'DEBIAN_FRONTEND=noninteractive apt install -y grub-efi-amd64 grub-efi-amd64-signed linux-image-generic shim-signed dracut dracut-network zfs-dracut zfsutils-linux zfs-zed efibootmgr cryptsetup dosfstools tpm2-tools tpm-udev systemd-cryptsetup libfido2-1\'";

    async fn chroot_commands_for(cfg: &InstallationConfig) -> Vec<String> {
        let mut executor = RecordingExecutor::default();
        {
            let mut sysconf = SystemConfigurator::new(&mut executor);
            sysconf.configure_system_in_chroot(cfg).await.unwrap();
        }
        executor.recorded()
    }

    #[tokio::test]
    async fn clevis_pkcs11_pin_off_leaves_the_real_apt_line_untouched() {
        let cfg = sample_netplan_config("192.0.2.10/24", "networkd");
        assert!(!cfg.clevis_pkcs11_pin, "default MUST be off");
        let recorded = chroot_commands_for(&cfg).await;

        let apt: Vec<&String> = recorded
            .iter()
            .filter(|c| c.contains("apt install -y grub-efi-amd64"))
            .collect();
        assert_eq!(apt.len(), 1, "one base apt line: {recorded:#?}");
        // Byte equality, not contains() — this is the byte-identity guarantee
        // for the hosts already in service.
        assert_eq!(apt[0], BASELINE_CHROOT_APT_LINE);

        // And the apt plumbing must be absent from the stream entirely.
        for c in &recorded {
            assert!(!c.contains("uaa-clevis23.sources"), "{c}");
            assert!(!c.contains("99-uaa-clevis23-pkcs11"), "{c}");
            assert!(!c.contains("opensc"), "{c}");
            assert!(!c.contains("pcscd"), "{c}");
        }
    }

    #[tokio::test]
    async fn clevis_pkcs11_pin_on_pins_the_pocket_before_apt_update() {
        let mut cfg = sample_netplan_config("192.0.2.10/24", "networkd");
        cfg.clevis_pkcs11_pin = true;
        let recorded = chroot_commands_for(&cfg).await;

        let sources = recorded
            .iter()
            .position(|c| c.contains("uaa-clevis23.sources"))
            .expect("sources file must be written");
        let prefs = recorded
            .iter()
            .position(|c| c.contains("99-uaa-clevis23-pkcs11"))
            .expect("preferences file must be written");
        let update = recorded
            .iter()
            .position(|c| c.contains("bash -lc \'apt update\'"))
            .expect("apt update");
        assert!(
            sources < update && prefs < update,
            "plumbing must precede apt update or the pocket is never indexed: {recorded:#?}"
        );

        let apt = recorded
            .iter()
            .find(|c| c.contains("apt install -y grub-efi-amd64"))
            .expect("base apt line");
        assert!(apt.ends_with("opensc pcscd\'"), "{apt}");
        // The pkcs11 pin IS a clevis pin: a Tang-less PlainLuks host must still
        // get clevis, or the flag installs a token stack with nothing to use it.
        assert!(
            apt.contains(" clevis clevis-luks clevis-dracut clevis-systemd"),
            "{apt}"
        );
        assert_ne!(apt.as_str(), BASELINE_CHROOT_APT_LINE);
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
    fn test_dracut_crypt_conf_includes_clevis_subsystem() {
        let conf = SystemConfigurator::build_dracut_crypt_conf(true, "ixgbe");
        let modules = dracut_modules_line(&conf);
        // The clevis (Tang) and crypt/tpm2 unlock subsystems must both be
        // enabled in the initramfs module list.
        assert!(
            modules.contains("clevis"),
            "Tang unlock (clevis) missing: {modules}"
        );
        assert!(
            modules.contains("crypt"),
            "systemd-cryptsetup (crypt) missing: {modules}"
        );
        assert!(
            modules.contains("tpm2-tss"),
            "TPM2 support missing: {modules}"
        );
        // zfs module must be present so rpool/bpool import in the initramfs.
        assert!(modules.contains("zfs"), "zfs module missing: {modules}");
        // Tang unlock needs the network stack + the NIC driver in the initramfs.
        assert!(
            modules.contains("network"),
            "network module missing for Tang: {modules}"
        );
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
    fn test_dracut_crypt_conf_omits_clevis_when_not_needed() {
        let conf = SystemConfigurator::build_dracut_crypt_conf(false, "");
        let modules = dracut_modules_line(&conf);
        assert!(
            !modules.contains("clevis"),
            "clevis module should be absent with no Tang servers: {modules}"
        );
        assert!(
            !modules.contains("network"),
            "network module should be absent with no Tang servers: {modules}"
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
