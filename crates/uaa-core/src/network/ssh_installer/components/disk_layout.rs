// file: crates/uaa-core/src/network/ssh_installer/components/disk_layout.rs
// version: 1.0.0
// guid: c6f34af5-c97a-43be-8839-3b60bdfe2b5a
// last-edited: 2026-07-23

//! Disk-layout component types (PS-DISK-01).
//!
//! Authoring-time types for expressing a host's on-disk partition/pool
//! layout as a tagged variant, mirroring the
//! [`ApplicationSpec`](crate::network::ssh_installer::config::ApplicationSpec)
//! newtype pattern in `config.rs`. These types are NOT wired onto
//! [`InstallationConfig`](crate::network::ssh_installer::config::InstallationConfig)
//! or [`InstallationConfigPartial`](crate::profile::InstallationConfigPartial) —
//! that wiring, and the appliers that make the sizes here actually take
//! effect in `disk_ops.rs`, are deferred to PS-INSTALLER-29. Until then these
//! fields are authoring-expressibility only: `lower()` drops them.

use crate::network::ssh_installer::config::DiskSpec;
use serde::{Deserialize, Serialize};

/// A host's disk layout. Closed-but-growing like `ApplicationSpec` (spec
/// Decision 15): an unknown `kind` is a hard parse error, never a silent
/// skip, because a silently-dropped layout deploys a machine with no
/// partition plan at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DiskLayout {
    SingleLuks(SingleLuksSpec),
    ZfsNativeKeystore(NativeKeystoreSpec),
}

/// Single-disk ESP + RESET + BPOOL + LUKS-encrypted RPOOL layout — the live
/// Lenovo (`len-serv*`) fleet's `PlainLuks` path. The size defaults are
/// sgdisk suffix literals and MUST match the hardcoded strings in
/// `disk_ops.rs`'s `sgdisk -n ...:+SIZE` calls exactly, or the two diverge
/// silently the day this component is wired in (PS-INSTALLER-29).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SingleLuksSpec {
    /// EFI System Partition size. Matches `disk_ops.rs`'s ESP (p1) literal.
    #[serde(default = "default_esp_size")]
    pub esp_size: String,
    /// RESET partition size. Matches `disk_ops.rs`'s RESET (p2) literal.
    #[serde(default = "default_reset_size")]
    pub reset_size: String,
    /// ZFS boot pool partition size. Matches `disk_ops.rs`'s BPOOL (p3)
    /// literal.
    #[serde(default = "default_bpool_size")]
    pub bpool_size: String,
    pub disk_device: Option<String>,
    #[serde(default = "default_reset_enabled")]
    pub reset_enabled: bool,
}

/// Multi-disk ZFS-native-encryption layout (Supermicro `unimatrix*` fleet).
/// Reuses [`DiskSpec`]/[`DiskRole`](crate::network::ssh_installer::config::DiskRole)
/// verbatim from `config.rs` — this is the same roster type
/// `InstallationConfig::disks` already carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeKeystoreSpec {
    pub disks: Vec<DiskSpec>,
}

fn default_esp_size() -> String {
    "512M".to_string()
}

fn default_reset_size() -> String {
    "4G".to_string()
}

fn default_bpool_size() -> String {
    "2G".to_string()
}

fn default_reset_enabled() -> bool {
    true
}

/// Every field of [`SingleLuksSpec`], all optional — modeled literally on
/// `CockroachSpecPartial` (`profile/mod.rs:175`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SingleLuksSpecPartial {
    pub esp_size: Option<String>,
    pub reset_size: Option<String>,
    pub bpool_size: Option<String>,
    pub disk_device: Option<String>,
    pub reset_enabled: Option<bool>,
}

/// Every field of [`NativeKeystoreSpec`], all optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NativeKeystoreSpecPartial {
    pub disks: Option<Vec<DiskSpec>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::ssh_installer::config::DiskRole;

    #[test]
    fn test_single_luks_defaults_match_disk_ops_literals() {
        let layout: DiskLayout = serde_json::from_str(r#"{"kind":"single-luks"}"#).unwrap();
        match layout {
            DiskLayout::SingleLuks(spec) => {
                assert_eq!(spec.esp_size, "512M");
                assert_eq!(spec.reset_size, "4G");
                assert_eq!(spec.bpool_size, "2G");
                assert_eq!(spec.disk_device, None);
                assert!(spec.reset_enabled);
            }
            DiskLayout::ZfsNativeKeystore(_) => panic!("expected SingleLuks variant"),
        }
    }

    #[test]
    fn test_disk_layout_single_luks_roundtrip() {
        let layout = DiskLayout::SingleLuks(SingleLuksSpec {
            esp_size: "512M".to_string(),
            reset_size: "4G".to_string(),
            bpool_size: "2G".to_string(),
            disk_device: Some("/dev/sda".to_string()),
            reset_enabled: false,
        });

        let json = serde_json::to_string(&layout).unwrap();
        let roundtripped: DiskLayout = serde_json::from_str(&json).unwrap();
        assert_eq!(layout, roundtripped);
    }

    #[test]
    fn test_disk_layout_zfs_native_keystore_roundtrip() {
        let layout = DiskLayout::ZfsNativeKeystore(NativeKeystoreSpec {
            disks: vec![
                DiskSpec {
                    id: "/dev/disk/by-id/nvme-eui.001".to_string(),
                    role: DiskRole::System,
                },
                DiskSpec {
                    id: "/dev/disk/by-id/nvme-eui.002".to_string(),
                    role: DiskRole::Special,
                },
            ],
        });

        let json = serde_json::to_string(&layout).unwrap();
        let roundtripped: DiskLayout = serde_json::from_str(&json).unwrap();
        assert_eq!(layout, roundtripped);
    }

    #[test]
    fn test_disk_layout_unknown_kind_errors() {
        let result: Result<DiskLayout, _> = serde_json::from_str(r#"{"kind":"bogus"}"#);
        assert!(result.is_err(), "unknown kind must be a hard parse error");
    }

    #[test]
    fn test_disk_layout_rejects_unknown_field() {
        let result: Result<DiskLayout, _> =
            serde_json::from_str(r#"{"kind":"single-luks","typo_field":true}"#);
        let err = result.expect_err("unknown field must fail to parse");
        assert!(
            err.to_string().contains("typo_field"),
            "error must name the offending key, got: {err}"
        );
    }

    #[test]
    fn test_single_luks_spec_partial_all_none_is_legal() {
        let partial = SingleLuksSpecPartial::default();
        assert_eq!(
            partial,
            SingleLuksSpecPartial {
                esp_size: None,
                reset_size: None,
                bpool_size: None,
                disk_device: None,
                reset_enabled: None,
            }
        );
    }

    #[test]
    fn test_single_luks_spec_partial_roundtrip() {
        let partial = SingleLuksSpecPartial {
            esp_size: Some("1G".to_string()),
            reset_size: None,
            bpool_size: Some("3G".to_string()),
            disk_device: Some("/dev/sda".to_string()),
            reset_enabled: Some(false),
        };

        let json = serde_json::to_string(&partial).unwrap();
        let roundtripped: SingleLuksSpecPartial = serde_json::from_str(&json).unwrap();
        assert_eq!(partial, roundtripped);
    }

    #[test]
    fn test_native_keystore_spec_partial_all_none_is_legal() {
        let partial = NativeKeystoreSpecPartial::default();
        assert_eq!(partial, NativeKeystoreSpecPartial { disks: None });
    }

    #[test]
    fn test_native_keystore_spec_partial_roundtrip() {
        let partial = NativeKeystoreSpecPartial {
            disks: Some(vec![DiskSpec {
                id: "/dev/disk/by-id/nvme-eui.003".to_string(),
                role: DiskRole::System,
            }]),
        };

        let json = serde_json::to_string(&partial).unwrap();
        let roundtripped: NativeKeystoreSpecPartial = serde_json::from_str(&json).unwrap();
        assert_eq!(partial, roundtripped);
    }
}
