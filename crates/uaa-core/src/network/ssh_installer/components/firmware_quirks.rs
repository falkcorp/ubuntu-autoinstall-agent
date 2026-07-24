// file: crates/uaa-core/src/network/ssh_installer/components/firmware_quirks.rs
// version: 1.0.0
// guid: 6fdd6905-8262-48c8-9799-5581219d7080
// last-edited: 2026-07-23

//! Firmware/board quirk components.
//!
//! `FirmwareQuirk` is a closed, variant-select (union-by-kind) component
//! carried as `Vec<FirmwareQuirk>` on a host/profile. It captures
//! per-board firmware workarounds that don't belong in the generic
//! install path.
//!
//! Deliberately NOT modeled here:
//! - serial-console: PS-SERIAL-18 makes this an arch-gated installer
//!   default, never a quirk.
//! - nvme-cant-boot: stays represented via `DiskRole::System`.

use serde::{Deserialize, Serialize};

/// A single firmware/board quirk to apply to a host.
///
/// Closed tagged enum: an unknown `kind` is a hard parse error, matching
/// the `ApplicationSpec`/`DiskLayout` pattern elsewhere in this module —
/// a silently-dropped quirk is a silent behavior regression on real
/// hardware.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FirmwareQuirk {
    /// GRUB must be installed to the removable-media fallback path
    /// (`/EFI/BOOT/BOOTX64.EFI`) because the board's firmware doesn't
    /// honor NVRAM boot entries reliably.
    GrubRemovableFallback,
    /// Force a specific NIC driver to be loaded/bound instead of the
    /// kernel's default autodetected driver.
    ForceNicDriver { driver: String },
    /// Stagger the hardware watchdog across a slot, spacing resets so
    /// multiple RPi Tang servers don't reboot in lockstep.
    ///
    /// TODO(PS-MIG-RPI-24): finalize staggered-watchdog params.
    WatchdogStaggered { slot: u8, interval_secs: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grub_removable_fallback_round_trips() {
        let quirk = FirmwareQuirk::GrubRemovableFallback;
        let json = serde_json::to_string(&quirk).expect("serialize");
        assert_eq!(json, r#"{"kind":"grub-removable-fallback"}"#);
        let back: FirmwareQuirk = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, quirk);
    }

    #[test]
    fn force_nic_driver_round_trips() {
        let quirk = FirmwareQuirk::ForceNicDriver {
            driver: "r8169".to_string(),
        };
        let json = serde_json::to_string(&quirk).expect("serialize");
        assert_eq!(json, r#"{"kind":"force-nic-driver","driver":"r8169"}"#);
        let back: FirmwareQuirk = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, quirk);
    }

    #[test]
    fn watchdog_staggered_round_trips() {
        let quirk = FirmwareQuirk::WatchdogStaggered {
            slot: 2,
            interval_secs: 30,
        };
        let json = serde_json::to_string(&quirk).expect("serialize");
        assert_eq!(
            json,
            r#"{"kind":"watchdog-staggered","slot":2,"interval_secs":30}"#
        );
        let back: FirmwareQuirk = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, quirk);
    }

    #[test]
    fn unknown_kind_fails_to_deserialize() {
        let json = r#"{"kind":"totally-unknown-quirk"}"#;
        let result: Result<FirmwareQuirk, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    /// Test-local wrapper exercising `skip_serializing_if` on an empty
    /// `Vec<FirmwareQuirk>`. There is no `InstallationConfig` field for
    /// this yet (no wiring in this brief), so the wrapper is local to
    /// the test module.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
    struct Holder {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        quirks: Vec<FirmwareQuirk>,
    }

    #[test]
    fn holder_omits_empty_quirks_key() {
        let holder = Holder::default();
        let json = serde_json::to_string(&holder).expect("serialize");
        assert_eq!(json, "{}");
        let back: Holder = serde_json::from_str("{}").expect("deserialize");
        assert_eq!(back, holder);
    }

    #[test]
    fn holder_serializes_non_empty_quirks() {
        let holder = Holder {
            quirks: vec![FirmwareQuirk::GrubRemovableFallback],
        };
        let json = serde_json::to_string(&holder).expect("serialize");
        assert_eq!(json, r#"{"quirks":[{"kind":"grub-removable-fallback"}]}"#);
        let back: Holder = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, holder);
    }
}
