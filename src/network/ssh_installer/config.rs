// file: src/network/ssh_installer/config.rs
// version: 2.1.0
// guid: sshcfg01-2345-6789-abcd-ef0123456789
// last-edited: 2026-06-20

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
}

fn default_tang_threshold() -> u8 {
    2
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
}
