// file: crates/uaa-core/src/network/ssh_installer/config.rs
// version: 2.10.1
// guid: sshcfg01-2345-6789-abcd-ef0123456789
// last-edited: 2026-07-22

//! Configuration structures for SSH/local installation

use serde::{Deserialize, Serialize};

/// Which initramfs generator is in use on the target.
///
/// Dracut is used on the actual servers (Lenovo M715q) and requires different
/// regeneration commands + GRUB kernel parameters for Tang network unlock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum InitramfsType {
    /// dracut — used on the Lenovo servers. Enables rd.neednet + Tang unlock at boot.
    #[default]
    Dracut,
    /// initramfs-tools — Ubuntu default (cloud images, live ISOs).
    InitramfsTools,
}

impl InitramfsType {
    /// Shell command to regenerate the initramfs inside a chroot at `/mnt/targetos`.
    pub fn regenerate_cmd(&self) -> &'static str {
        match self {
            Self::Dracut => "dracut --regenerate-all --force",
            Self::InitramfsTools => "update-initramfs -u -k all",
        }
    }
}

/// Tang server entry for Clevis SSS binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TangServer {
    pub url: String,
}

/// A workload assignable to a host. Closed-but-growing by design (spec
/// Decision 15): adding HAProxy/Keepalived later is a new variant, not a
/// plugin framework. An unknown `kind` is a hard parse error — never a
/// silent skip, because a silently-dropped application deploys a machine
/// missing its workload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ApplicationSpec {
    Cockroach(CockroachSpec),
}

/// CockroachDB node parameters. `advertise`/`join` are NOT here: they are
/// DERIVED per host from the group's sibling list (profiles/TASK-04), never
/// authored. Defaults are the live fleet's values (verified 2026-07-16).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CockroachSpec {
    #[serde(default = "default_cockroach_version")]
    pub version: String,
    #[serde(default = "default_cockroach_port")]
    pub port: u16,
    #[serde(default = "default_cockroach_sql_port")]
    pub sql_port: u16,
    #[serde(default = "default_cockroach_http_addr")]
    pub http_addr: String,
    /// Cluster seed, always first in the join string.
    pub seed_ip: String,
    #[serde(default = "default_cockroach_cache")]
    pub cache: String,
    #[serde(default = "default_cockroach_max_sql")]
    pub max_sql_memory: String,
    #[serde(default = "default_cockroach_locality")]
    pub locality: String,
}

/// Complete configuration for a machine installation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallationConfig {
    pub hostname: String,
    pub disk_device: String,
    pub timezone: String,
    pub luks_key: String,
    pub root_password: String,
    pub network_interface: String,
    pub network_address: String,
    pub network_gateway: String,
    pub network_search: String,
    pub network_nameservers: Vec<String>,
    /// Netplan renderer for the installed system: "networkd" (default) or
    /// "NetworkManager". Validated at render time.
    #[serde(default = "default_network_renderer")]
    pub network_renderer: String,
    pub debootstrap_release: Option<String>,
    pub debootstrap_mirror: Option<String>,
    /// Initramfs generator — defaults to Dracut.
    #[serde(default)]
    pub initramfs_type: InitramfsType,
    /// Tang servers for Clevis SSS binding. Empty = no Tang enrollment.
    #[serde(default)]
    pub tang_servers: Vec<TangServer>,
    /// SSS threshold (how many Tang servers must respond). Default 2.
    #[serde(default = "default_tang_threshold")]
    pub tang_threshold: u8,
    /// SSH public keys to install for root.
    #[serde(default)]
    pub ssh_authorized_keys: Vec<String>,
    /// Enroll a TPM2 + PIN LUKS keyslot on first boot of the installed target.
    ///
    /// TPM2 must bind to the *installed* system's PCR values (not the live
    /// installer's), so enrollment happens via a oneshot systemd unit on first
    /// boot rather than during the unattended install. clevis's tpm2 pin has no
    /// PIN support, so this uses `systemd-cryptenroll --tpm2-with-pin` (unlocked
    /// at boot by the sd-cryptsetup dracut module, alongside clevis for Tang).
    #[serde(default = "default_true")]
    pub enroll_tpm2: bool,
    /// PIN required at boot for the TPM2 keyslot. Empty/None disables TPM2+PIN
    /// enrollment even when `enroll_tpm2` is true (no PIN = no anti-theft value).
    #[serde(default)]
    pub tpm2_pin: Option<String>,
    /// PCR indices the TPM2 policy binds to (comma-separated). Default "7"
    /// (secure-boot state). Kept minimal so routine kernel updates don't
    /// invalidate the binding; the PIN is the real anti-theft factor.
    #[serde(default = "default_tpm2_pcr_ids")]
    pub tpm2_pcr_ids: String,
    /// FIDO2 (YubiKey) unlock is enrolled MANUALLY post-install via
    /// `register-fido2-luks.sh` (needs the physical key + touch), so it is not
    /// part of the unattended install config. This flag only records intent /
    /// drives `verify` to check that at least one fido2 keyslot exists.
    #[serde(default = "default_true")]
    pub expect_fido2: bool,
    /// Install CA public cert (PEM), written to `/etc/uaa/install-ca.crt` on
    /// the target in Phase 5 so `uaa enroll`'s default `--ca` path finds it
    /// (spec Decision 7). NOT a per-host secret — the same cert for every
    /// host — so `uaa config place` fills this slot unconditionally from the
    /// server's `/var/lib/uaa/ca/ca.crt`, regardless of `--inject-from`. A
    /// config placed before the CA existed keeps the literal
    /// `REPLACE_AT_PLACE_TIME` placeholder here; Phase 5 writes it to the
    /// target as-is (fail-closed — `uaa enroll` treats an unparseable CA as
    /// the missing-CA case, never falling back to system roots).
    #[serde(default = "default_install_ca_cert")]
    pub install_ca_cert: String,
    /// Applications to install into the target during Phase 5. Empty = none,
    /// which is exactly today's behavior for every committed host config.
    ///
    /// `skip_serializing_if` omits the key entirely for an app-free host so a
    /// serialized (registry-resolved) config is byte-safe across a control
    /// rollback: a placed config that never gained an `applications:` key can
    /// still be parsed by an older `uaa install` binary whose
    /// `InstallationConfig` (deny_unknown_fields) predates the field. Without
    /// this, an app-free host would serialize `applications: []` and trip a
    /// fail-closed parse on every PXE after a rollback (DS-OPS-03).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applications: Vec<ApplicationSpec>,
    /// Storage layout the installer builds for this host. Defaults to
    /// [`StorageMode::PlainLuks`] — the single-disk ZFS-on-LUKS path every
    /// Lenovo (`len-serv-*`) uses today — so a config that omits the key is
    /// byte-for-byte unchanged. Only `unimatrixone` sets `NativeKeystore`.
    /// See `docs/specs/u1-zfs-native-encryption-{design,plan}.md`.
    ///
    /// `skip_serializing_if` omits the key for a `PlainLuks` host, so a
    /// registry-resolved Lenovo config serializes exactly as before — no new
    /// `storage-mode:` key to trip an older `deny_unknown_fields` binary on a
    /// control rollback (same rationale as `applications`).
    #[serde(default, skip_serializing_if = "StorageMode::is_default")]
    pub storage_mode: StorageMode,
    /// Multi-disk roster for [`StorageMode::NativeKeystore`] (by-id device
    /// paths + roles). Ignored under `PlainLuks`, which uses `disk_device`.
    /// `skip_serializing_if` keeps a `PlainLuks` config's serialization free of
    /// an empty `disks: []` key (same cross-version-rollback safety as
    /// `applications`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<DiskSpec>,
}

/// Which encryption/storage layout the installer builds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StorageMode {
    /// Single disk → LUKS → ZFS `rpool` on the mapper. The proven Lenovo path
    /// (`disk_ops::prepare_disk`); `disks` is ignored, `disk_device` drives it.
    #[default]
    PlainLuks,
    /// ZFS **native** encryption on the Ubuntu keystore-zvol layout across the
    /// multi-disk `disks` roster: bulk data mirror + Optane `special` metadata
    /// mirror, a `rpool/keystore` zvol, clevis SSS unlock. U1 only.
    NativeKeystore,
}

impl StorageMode {
    /// `true` for the default (`PlainLuks`) — the serde `skip_serializing_if`
    /// predicate that keeps a Lenovo config's serialization key-for-key unchanged.
    pub fn is_default(&self) -> bool {
        matches!(self, StorageMode::PlainLuks)
    }
}

/// Role a physical disk plays in the [`StorageMode::NativeKeystore`] layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiskRole {
    /// Fast/small device (Optane): carries ESP + bpool member + the `special`
    /// (metadata) vdev member.
    System,
    /// Bulk device (SSD): a whole-disk `rpool` data-vdev member.
    Data,
}

/// One disk in the [`StorageMode::NativeKeystore`] roster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskSpec {
    /// Stable device path — `/dev/disk/by-id/...`, **never** `sdX`/`nvmeXnY`
    /// (enumeration order is not stable across boots on a 4-drive box).
    pub id: String,
    /// What this disk is for.
    pub role: DiskRole,
}

fn default_tang_threshold() -> u8 {
    2
}

fn default_true() -> bool {
    true
}

fn default_tpm2_pcr_ids() -> String {
    "7".to_string()
}

pub fn default_install_ca_cert() -> String {
    crate::config_place::PLACEHOLDER.to_string()
}

pub fn default_network_renderer() -> String {
    "networkd".to_string()
}

fn default_cockroach_version() -> String {
    "v25.3.0".to_string()
}

fn default_cockroach_port() -> u16 {
    36357
}

fn default_cockroach_sql_port() -> u16 {
    36257
}

fn default_cockroach_http_addr() -> String {
    ":38080".to_string()
}

fn default_cockroach_cache() -> String {
    ".25".to_string()
}

fn default_cockroach_max_sql() -> String {
    ".25".to_string()
}

fn default_cockroach_locality() -> String {
    "region=us,cluster-unit=lenovo".to_string()
}

impl InstallationConfig {
    /// Load configuration from a YAML file.
    pub fn from_yaml_file(path: &str) -> crate::Result<Self> {
        let content =
            std::fs::read_to_string(path).map_err(crate::error::AutoInstallError::IoError)?;
        serde_yaml::from_str(&content).map_err(crate::error::AutoInstallError::SerdeError)
    }

    /// Create the production config for len-serv-003 (172.16.3.96).
    pub fn for_len_serv_003() -> Self {
        Self {
            hostname: "len-serv-003".to_string(),
            disk_device: "/dev/nvme0n1".to_string(),
            timezone: "America/New_York".to_string(),
            luks_key: "changeme123!@#".to_string(),
            root_password: "changeme123!@#".to_string(),
            network_interface: "enp1s0f0".to_string(),
            network_address: "172.16.3.96/23".to_string(),
            network_gateway: "172.16.2.1".to_string(),
            network_search: "jf.local".to_string(),
            network_nameservers: vec![
                "172.16.2.1".to_string(),
                "1.1.1.1".to_string(),
                "8.8.8.8".to_string(),
            ],
            network_renderer: default_network_renderer(),
            debootstrap_release: Some("resolute".to_string()),
            debootstrap_mirror: Some("http://archive.ubuntu.com/ubuntu/".to_string()),
            initramfs_type: InitramfsType::Dracut,
            tang_servers: vec![
                TangServer {
                    url: "http://172.16.2.45".to_string(),
                },
                TangServer {
                    url: "http://172.16.2.46".to_string(),
                },
                TangServer {
                    url: "http://172.16.2.47".to_string(),
                },
            ],
            tang_threshold: 2,
            ssh_authorized_keys: vec![
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOq0x6/0fA+vn0EdNJvBuadOo4rZ1IwkCWbBOWCwvId5 jdfalk@Norn.lan".to_string(),
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIP4PPvBh1cCMdh8S5Uqz/1cONHxhc78TfWLt0fx76B/G jdfalk@JohnathsMacBook.jf.local".to_string(),
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPghsb0DAzQX5LfLgb1Q11LJJhppTM1r093TWCTjxjdb eddsa-key-20220820".to_string(),
            ],
            enroll_tpm2: true,
            // Placeholder — the real PIN is injected per-host from a secret at
            // seed-render time, never committed. None here disables TPM2 in the
            // hardcoded fallback config.
            tpm2_pin: None,
            tpm2_pcr_ids: default_tpm2_pcr_ids(),
            expect_fido2: true,
            install_ca_cert: default_install_ca_cert(),
            applications: Vec::new(),
            storage_mode: StorageMode::PlainLuks,
            disks: Vec::new(),
        }
    }
}

/// Collected information about the target system.
#[derive(Debug, Default)]
pub struct SystemInfo {
    pub hostname: String,
    pub kernel_version: String,
    pub os_release: String,
    pub disk_info: String,
    pub network_info: String,
    pub available_tools: Vec<String>,
    pub memory_info: String,
    pub cpu_info: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initramfs_type_default_is_dracut() {
        assert_eq!(InitramfsType::default(), InitramfsType::Dracut);
    }

    #[test]
    fn test_dracut_regenerate_cmd() {
        assert_eq!(
            InitramfsType::Dracut.regenerate_cmd(),
            "dracut --regenerate-all --force"
        );
    }

    #[test]
    fn test_initramfs_tools_regenerate_cmd() {
        assert_eq!(
            InitramfsType::InitramfsTools.regenerate_cmd(),
            "update-initramfs -u -k all"
        );
    }

    #[test]
    fn test_for_len_serv_003_has_tang_servers() {
        let cfg = InstallationConfig::for_len_serv_003();
        assert_eq!(cfg.tang_servers.len(), 3);
        assert_eq!(cfg.tang_threshold, 2);
        assert_eq!(cfg.initramfs_type, InitramfsType::Dracut);
    }

    #[test]
    fn test_for_len_serv_003_network() {
        let cfg = InstallationConfig::for_len_serv_003();
        assert_eq!(cfg.network_address, "172.16.3.96/23");
        assert_eq!(cfg.network_interface, "enp1s0f0");
    }

    #[test]
    fn test_for_len_serv_003_multikey_defaults() {
        let cfg = InstallationConfig::for_len_serv_003();
        // TPM2+PIN and FIDO2 expectations default on; the PIN itself is injected
        // per-host from a secret (None in the hardcoded fallback).
        assert!(cfg.enroll_tpm2);
        assert_eq!(cfg.tpm2_pin, None);
        assert_eq!(cfg.tpm2_pcr_ids, "7");
        assert!(cfg.expect_fido2);
    }

    #[test]
    fn test_install_example_configs_round_trip() {
        // The committed per-host example configs under examples/configs/install/
        // must deserialize into InstallationConfig with the multi-key features
        // explicitly enabled (they must NOT rely on serde defaults for tang/tpm2).
        // Scoped to these four files only — the legacy examples/configs/*.yaml use
        // an older, incompatible schema and are intentionally not loaded here.
        let load = |host: &str| -> InstallationConfig {
            let path = format!(
                "{}/../../examples/configs/install/{}.yaml",
                env!("CARGO_MANIFEST_DIR"),
                host
            );
            InstallationConfig::from_yaml_file(&path)
                .unwrap_or_else(|e| panic!("{host} config must parse: {e}"))
        };

        // The len-servs are the PlainLuks (legacy single-disk) path.
        let plain = [
            ("len-serv-001", "/dev/nvme0n1", "172.16.3.92/23"),
            ("len-serv-002", "/dev/nvme0n1", "172.16.3.94/23"),
            ("len-serv-003", "/dev/nvme0n1", "172.16.3.96/23"),
        ];
        for (host, disk, addr) in plain {
            let cfg = load(host);
            assert_eq!(cfg.storage_mode, StorageMode::PlainLuks, "{host}: PlainLuks");
            assert_eq!(cfg.hostname, host, "{host}: hostname");
            assert_eq!(cfg.disk_device, disk, "{host}: disk_device");
            assert_eq!(cfg.network_address, addr, "{host}: network_address");
            assert_eq!(cfg.initramfs_type, InitramfsType::Dracut, "{host}: dracut");
            assert_eq!(cfg.tang_servers.len(), 3, "{host}: 3 tang servers");
            assert_eq!(cfg.tang_threshold, 2, "{host}: tang threshold");
            assert!(cfg.enroll_tpm2, "{host}: enroll_tpm2");
            assert!(cfg.expect_fido2, "{host}: expect_fido2");
            assert_eq!(
                cfg.tpm2_pin.as_deref(),
                Some("REPLACE_AT_PLACE_TIME"),
                "{host}: tpm2_pin placeholder"
            );
            assert_eq!(cfg.luks_key, "REPLACE_AT_PLACE_TIME", "{host}: luks_key placeholder");
            assert_eq!(cfg.root_password, "REPLACE_AT_PLACE_TIME", "{host}: root_password");
        }

        // unimatrixone is the NativeKeystore (ZFS native-encryption) path — the
        // future server profile. Different unlock policy: enroll_tpm2/expect_fido2
        // OFF (D2-B uses a clevis tpm2 pin, not the hanging systemd-tpm2 token).
        let u1 = load("unimatrixone");
        assert_eq!(u1.storage_mode, StorageMode::NativeKeystore, "u1: NativeKeystore");
        assert_eq!(u1.network_address, "172.16.2.35/23", "u1: network_address");
        assert_eq!(u1.initramfs_type, InitramfsType::Dracut, "u1: dracut");
        // 4-disk roster: 2 system (Optane) + 2 data (SSD), all by-id.
        assert_eq!(u1.disks.len(), 4, "u1: 4-disk roster");
        assert_eq!(
            u1.disks.iter().filter(|d| d.role == DiskRole::System).count(),
            2,
            "u1: 2 system disks"
        );
        assert_eq!(
            u1.disks.iter().filter(|d| d.role == DiskRole::Data).count(),
            2,
            "u1: 2 data disks"
        );
        assert!(
            u1.disks.iter().all(|d| d.id.starts_with("/dev/disk/by-id/")),
            "u1: disks must be by-id"
        );
        assert_eq!(u1.tang_servers.len(), 3, "u1: 3 tang servers");
        assert_eq!(u1.tang_threshold, 2, "u1: tang threshold (D2-B t=2)");
        assert!(!u1.enroll_tpm2, "u1: enroll_tpm2 OFF (clevis tpm2 pin instead)");
        assert!(!u1.expect_fido2, "u1: expect_fido2 OFF");
        assert_eq!(u1.luks_key, "REPLACE_AT_PLACE_TIME", "u1: luks_key placeholder");
    }

    #[test]
    fn test_multikey_serde_defaults_when_absent() {
        // A minimal YAML with none of the new fields must deserialize with the
        // secure defaults (TPM2 on, PCR 7, FIDO2 expected) rather than failing.
        let yaml = r#"
hostname: test
disk_device: /dev/sda
timezone: UTC
luks_key: k
root_password: p
network_interface: eth0
network_address: 10.0.0.2/24
network_gateway: 10.0.0.1
network_search: local
network_nameservers: ["10.0.0.1"]
"#;
        let cfg: InstallationConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.enroll_tpm2);
        assert_eq!(cfg.tpm2_pcr_ids, "7");
        assert!(cfg.expect_fido2);
        assert_eq!(cfg.tpm2_pin, None);
    }

    #[test]
    fn test_unknown_yaml_key_rejected() {
        // deny_unknown_fields: a typo'd key must fail parsing loudly, not be
        // silently dropped (this installer LUKS-formats disks off this config).
        let yaml = r#"
hostname: test
disk_devise: /dev/sda
disk_device: /dev/sda
timezone: UTC
luks_key: k
root_password: p
network_interface: eth0
network_address: 10.0.0.2/24
network_gateway: 10.0.0.1
network_search: local
network_nameservers: ["10.0.0.1"]
"#;
        let err = serde_yaml::from_str::<InstallationConfig>(yaml).unwrap_err();
        assert!(err.to_string().contains("disk_devise"), "error must name the unknown key: {err}");
    }

    #[test]
    fn test_network_renderer_defaults_when_absent() {
        // Old committed YAML has no `network_renderer` key; the serde default
        // must keep it parsing (and defaulting to "networkd") unchanged.
        let cfg = InstallationConfig::for_len_serv_003();
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        let yaml_without_renderer: String = yaml
            .lines()
            .filter(|l| !l.contains("network_renderer"))
            .collect::<Vec<_>>()
            .join("\n");
        let back: InstallationConfig = serde_yaml::from_str(&yaml_without_renderer).unwrap();
        assert_eq!(back.network_renderer, "networkd");
    }

    #[test]
    fn test_applications_defaults_to_empty_when_absent() {
        // A minimal YAML with no `applications:` key must deserialize with an
        // empty applications list, not fail — this is every committed host
        // config today.
        let yaml = r#"
hostname: test
disk_device: /dev/sda
timezone: UTC
luks_key: k
root_password: p
network_interface: eth0
network_address: 10.0.0.2/24
network_gateway: 10.0.0.1
network_search: local
network_nameservers: ["10.0.0.1"]
"#;
        let cfg: InstallationConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.applications.is_empty());
    }

    #[test]
    fn test_applications_empty_is_todays_behavior() {
        assert!(InstallationConfig::for_len_serv_003().applications.is_empty());
    }

    #[test]
    fn test_app_free_host_omits_applications_key() {
        // Cross-version rollback safety (DS-OPS-03): a host with no
        // applications must serialize WITHOUT an `applications:` key at all, so
        // a rolled-back `uaa install` binary (whose deny_unknown_fields
        // InstallationConfig predates the field) still parses the placed file
        // instead of hitting a fail-closed parse on every PXE.
        let cfg = InstallationConfig::for_len_serv_003();
        assert!(cfg.applications.is_empty(), "fixture must be app-free");
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("applications"),
            "an app-free host must omit the applications key entirely, got:\n{yaml}"
        );
    }

    #[test]
    fn test_plain_luks_host_omits_storage_keys() {
        // Cross-version rollback safety (U1 Phase 1): a stock PlainLuks host —
        // every len-serv — must serialize WITHOUT `storage-mode:` or `disks:`,
        // so a rolled-back control binary (whose deny_unknown_fields
        // InstallationConfig predates the U1 keystore fields) still parses the
        // placed file byte-for-byte as it did before U1. Only unimatrixone,
        // which sets NativeKeystore, emits these keys.
        let cfg = InstallationConfig::for_len_serv_003();
        assert_eq!(cfg.storage_mode, StorageMode::PlainLuks, "fixture is PlainLuks");
        assert!(cfg.disks.is_empty(), "fixture has no multi-disk roster");
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("storage-mode") && !yaml.contains("storage_mode"),
            "a PlainLuks host must omit the storage-mode key entirely, got:\n{yaml}"
        );
        assert!(
            !yaml.contains("disks"),
            "a PlainLuks host must omit the disks key entirely, got:\n{yaml}"
        );
    }

    #[test]
    fn test_native_keystore_host_emits_storage_mode() {
        // The inverse guard: when a host IS NativeKeystore the discriminator must
        // actually appear (the field key is snake_case like every other
        // InstallationConfig field; only the enum *value* is kebab-cased), else
        // the installer would silently fall back to the PlainLuks path on U1.
        let mut cfg = InstallationConfig::for_len_serv_003();
        cfg.storage_mode = StorageMode::NativeKeystore;
        cfg.disks = vec![DiskSpec {
            id: "/dev/disk/by-id/nvme-optane".to_string(),
            role: DiskRole::System,
        }];
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        assert!(
            yaml.contains("storage_mode: native-keystore"),
            "NativeKeystore must serialize the discriminator (kebab-case value), got:\n{yaml}"
        );
        assert!(yaml.contains("disks"), "a NativeKeystore host must emit its disks roster");
    }

    #[test]
    fn test_cockroach_spec_defaults() {
        let yaml = r#"
kind: cockroach
seed_ip: 172.16.3.92
"#;
        let spec: ApplicationSpec = serde_yaml::from_str(yaml).unwrap();
        let ApplicationSpec::Cockroach(cockroach) = spec;
        assert_eq!(cockroach.version, "v25.3.0");
        assert_eq!(cockroach.port, 36357);
        assert_eq!(cockroach.sql_port, 36257);
        assert_eq!(cockroach.cache, ".25");
        assert_eq!(cockroach.locality, "region=us,cluster-unit=lenovo");
    }

    #[test]
    fn test_unknown_application_kind_rejected() {
        // The enum is closed by design (spec Decision 15): an unknown kind
        // must be a hard parse error naming the unknown kind, never a silent
        // skip.
        let yaml = r#"
kind: redis
"#;
        let err = serde_yaml::from_str::<ApplicationSpec>(yaml).unwrap_err();
        assert!(err.to_string().contains("redis"), "error must name the unknown kind: {err}");
    }

    #[test]
    fn test_cockroach_spec_unknown_field_rejected() {
        let yaml = r#"
kind: cockroach
seed_ip: 172.16.3.92
typo_field: oops
"#;
        let err = serde_yaml::from_str::<ApplicationSpec>(yaml).unwrap_err();
        assert!(
            err.to_string().contains("typo_field"),
            "error must name the unknown field: {err}"
        );
    }
}
