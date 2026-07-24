// file: crates/uaa-core/src/profile/components/unlock_policy.rs
// version: 1.0.0
// guid: c0ff669c-a00b-4be8-b4f2-9caf7b51a86e
// last-edited: 2026-07-23

//! Authoring-time unlock-policy sub-struct (PS-UNLOCK-02).
//!
//! [`UnlockPolicyPartial`] groups every disk-unlock-related authoring field
//! that today lives flat on
//! [`InstallationConfigPartial`](super::super::InstallationConfigPartial)
//! (`tang_servers`, `tang_threshold`, `tpm2_pin`, `tpm2_pcr_ids`,
//! `enroll_tpm2`, `expect_fido2`) into one nested, self-documenting shape
//! for profile authors. This module defines TYPES ONLY — no wiring onto
//! `InstallationConfigPartial` and no merge/lower logic. A future brief
//! (PS-WIRE-PARTIAL-11) adds `unlock_policy: Option<UnlockPolicyPartial>`
//! to `InstallationConfigPartial`; a later brief lowers it to the flat wire
//! fields consumed by merge/validate.
//!
//! ## Authoring -> flat-wire field mapping
//!
//! | `UnlockPolicyPartial` field                | flat wire field (`InstallationConfigPartial`) |
//! |---------------------------------------------|------------------------------------------------|
//! | `tang.servers`                               | `tang_servers`                                  |
//! | `tang.threshold`                             | `tang_threshold`                                |
//! | `tpm2_pin.pin` (double-option preserved)     | `tpm2_pin`                                      |
//! | `tpm2_pin.pcr_ids`                           | `tpm2_pcr_ids`                                  |
//! | `tpm2_pin.enroll`                            | `enroll_tpm2`                                   |
//! | `fido2_expected`                             | `expect_fido2`                                  |
//! | `tpm2_clevis_peer`                           | *(none — see below)*                            |
//!
//! `tpm2_clevis_peer` is authoring/validate-ONLY: it never lowers to a wire
//! field. The D2-B clevis TPM2 peer share is derived by the installer from
//! `storage_mode == NativeKeystore`
//! (`network/ssh_installer/system_setup.rs:722` / `:772`), not from any
//! profile input.

use crate::network::ssh_installer::config::TangServer;
use serde::{Deserialize, Serialize};

/// Nested Tang/SSS authoring group — see the module-level mapping table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TangSssPartial {
    pub servers: Option<Vec<TangServer>>,
    pub threshold: Option<u8>,
}

/// Nested TPM2+PIN authoring group — see the module-level mapping table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Tpm2PinPartial {
    /// Double Option: `None` = inherit, `Some(None)` = explicitly no PIN,
    /// `Some(Some(p))` = this PIN. Same trap as
    /// `InstallationConfigPartial::tpm2_pin` — see that field's doc
    /// comment for why a plain `Option<String>` would be wrong here.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::super::deserialize_double_option"
    )]
    pub pin: Option<Option<String>>,
    pub pcr_ids: Option<String>,
    pub enroll: Option<bool>,
}

/// Authoring-time unlock-policy group — see the module-level mapping table
/// for how each field lowers onto `InstallationConfigPartial`'s flat wire
/// fields (a future brief).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UnlockPolicyPartial {
    pub tang: Option<TangSssPartial>,
    pub tpm2_pin: Option<Tpm2PinPartial>,
    /// Authoring/validate-ONLY — never lowers to a wire field. See the
    /// module doc comment.
    pub tpm2_clevis_peer: Option<bool>,
    pub fido2_expected: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tpm2_pin_partial_distinguishes_absent_null_and_present() {
        let absent: Tpm2PinPartial = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.pin, None, "absent key must mean 'inherit'");

        let explicit_null: Tpm2PinPartial = serde_json::from_str(r#"{"pin":null}"#).unwrap();
        assert_eq!(
            explicit_null.pin,
            Some(None),
            "explicit null must mean 'explicitly no PIN'"
        );

        let present: Tpm2PinPartial = serde_json::from_str(r#"{"pin":"x"}"#).unwrap();
        assert_eq!(
            present.pin,
            Some(Some("x".to_string())),
            "present string must carry the PIN value"
        );

        assert_ne!(absent.pin, explicit_null.pin);
        assert_ne!(explicit_null.pin, present.pin);
        assert_ne!(absent.pin, present.pin);
    }

    #[test]
    fn test_unlock_policy_partial_roundtrip_fully_populated() {
        let partial = UnlockPolicyPartial {
            tang: Some(TangSssPartial {
                servers: Some(vec![
                    TangServer {
                        url: "http://tang1.example.internal".to_string(),
                    },
                    TangServer {
                        url: "http://tang2.example.internal".to_string(),
                    },
                ]),
                threshold: Some(1),
            }),
            tpm2_pin: Some(Tpm2PinPartial {
                pin: Some(Some("1234".to_string())),
                pcr_ids: Some("0,7".to_string()),
                enroll: Some(true),
            }),
            tpm2_clevis_peer: Some(true),
            fido2_expected: Some(false),
        };

        let json = serde_json::to_string(&partial).unwrap();
        let roundtripped: UnlockPolicyPartial = serde_json::from_str(&json).unwrap();
        assert_eq!(partial, roundtripped);
    }

    #[test]
    fn test_unlock_policy_partial_default_is_all_none() {
        let partial = UnlockPolicyPartial::default();
        assert_eq!(
            partial,
            UnlockPolicyPartial {
                tang: None,
                tpm2_pin: None,
                tpm2_clevis_peer: None,
                fido2_expected: None,
            }
        );
    }

    #[test]
    fn test_unlock_policy_partial_rejects_unknown_field() {
        let result: Result<UnlockPolicyPartial, _> =
            serde_json::from_str(r#"{"tang_clevis_peer":true}"#);
        let err = result.expect_err("typo'd key must fail to parse");
        assert!(
            err.to_string().contains("tang_clevis_peer"),
            "error must name the offending key, got: {err}"
        );
    }
}
