// file: crates/uaa-core/src/profile/components/base_image.rs
// version: 1.0.0
// guid: c8f3b2a1-7e9c-4d1a-b5f2-e8c1a9d3f4b6
// last-edited: 2026-07-23

//! Base image authoring sub-struct (BaseImagePartial) for profile system.
//!
//! Defines the BaseImagePartial type used for authoring Ubuntu base image
//! configuration in host/group profiles. Fields map to lower-level installer
//! configuration per PS-LOWER-12:
//! - `release` → `debootstrap_release`
//! - `mirror` → `debootstrap_mirror`
//! - `initramfs` → `initramfs_type`
//! - `fallback_mirror` → (authoring-only, inert until installer reads it)

use crate::network::ssh_installer::config::InitramfsType;
use serde::{Deserialize, Serialize};

/// Partial base image configuration for host/group profiles.
///
/// Uses double-Option for `release` and `mirror` to distinguish:
/// - `None` = inherit from parent (group/defaults)
/// - `Some(None)` = explicitly no override
/// - `Some(Some(value))` = this value
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BaseImagePartial {
    /// Ubuntu release codename (e.g., "jammy", "focal").
    /// Double Option: `None` = inherit, `Some(None)` = explicitly no release
    /// override, `Some(Some(r))` = this release.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "super::super::deserialize_double_option")]
    pub release: Option<Option<String>>,

    /// Debian mirror URL.
    /// Double Option — see `release`.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "super::super::deserialize_double_option")]
    pub mirror: Option<Option<String>>,

    /// Initramfs type (dracut or initramfs-tools).
    pub initramfs: Option<InitramfsType>,

    /// Fallback mirror URL (e.g., http://old-releases.ubuntu.com/ubuntu/).
    /// Authoring-only; inert until installer brief reads it (same category as disk sizes).
    pub fallback_mirror: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_image_partial_serde_round_trip() {
        let partial = BaseImagePartial {
            release: Some(Some("jammy".to_string())),
            mirror: Some(Some("http://archive.ubuntu.com/ubuntu/".to_string())),
            initramfs: Some(InitramfsType::Dracut),
            fallback_mirror: Some("http://old-releases.ubuntu.com/ubuntu/".to_string()),
        };

        let json = serde_json::to_string(&partial).expect("serialize failed");
        let deserialized: BaseImagePartial =
            serde_json::from_str(&json).expect("deserialize failed");

        assert_eq!(partial, deserialized);
    }

    #[test]
    fn test_release_double_option_distinctness() {
        // Case 1: absent (None)
        let absent_json = "{}";
        let absent: BaseImagePartial =
            serde_json::from_str(absent_json).expect("deserialize absent failed");
        assert_eq!(absent.release, None, "absent should be None");

        // Case 2: explicit null (Some(None))
        let null_json = r#"{"release":null}"#;
        let null: BaseImagePartial =
            serde_json::from_str(null_json).expect("deserialize null failed");
        assert_eq!(null.release, Some(None), "explicit null should be Some(None)");

        // Case 3: explicit value (Some(Some(value)))
        let value_json = r#"{"release":"jammy"}"#;
        let value: BaseImagePartial =
            serde_json::from_str(value_json).expect("deserialize value failed");
        assert_eq!(
            value.release,
            Some(Some("jammy".to_string())),
            "explicit value should be Some(Some(value))"
        );

        // Verify all three are distinct
        assert_ne!(absent.release, null.release);
        assert_ne!(null.release, value.release);
        assert_ne!(absent.release, value.release);
    }

    #[test]
    fn test_base_image_partial_default() {
        let partial = BaseImagePartial::default();
        assert_eq!(partial.release, None);
        assert_eq!(partial.mirror, None);
        assert_eq!(partial.initramfs, None);
        assert_eq!(partial.fallback_mirror, None);
    }

    #[test]
    fn test_base_image_partial_deny_unknown_fields() {
        let bad_json = r#"{"unknown_field":"value"}"#;
        let result: Result<BaseImagePartial, _> = serde_json::from_str(bad_json);
        assert!(result.is_err(), "should reject unknown fields");
    }

    #[test]
    fn test_initramfs_type_regenerate_cmd() {
        // Verify InitramfsType is reused correctly and has regenerate_cmd()
        let dracut = InitramfsType::Dracut;
        assert_eq!(
            dracut.regenerate_cmd(),
            "dracut --regenerate-all --force"
        );

        let initramfs_tools = InitramfsType::InitramfsTools;
        assert_eq!(
            initramfs_tools.regenerate_cmd(),
            "update-initramfs -u -k all"
        );
    }
}
