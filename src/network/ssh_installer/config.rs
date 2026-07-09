// file: src/network/ssh_installer/config.rs
// version: 2.2.0
// guid: sshcfg01-2345-6789-abcd-ef0123456789
// last-edited: 2026-07-09

//! Configuration structures for SSH/local installation

use serde::{Deserialize, Serialize};

/// Which initramfs generator is in use on the target.
///
/// Dracut is used on the actual servers (Lenovo M715q) and requires different
/// regeneration commands + GRUB kernel parameters for Tang network unlock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InitramfsType {
    /// dracut — used on the Lenovo servers. Enables rd.neednet + Tang unlock at boot.
    Dracut,
    /// initramfs-tools — Ubuntu default (cloud images, live ISOs).
    InitramfsTools,
}

impl Default for InitramfsType {
    fn default() -> Self {
        Self::Dracut
    }
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

/// Complete configuration for a machine installation.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}
