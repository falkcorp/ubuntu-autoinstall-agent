// file: crates/uaa-core/src/profile/components/unlock_policy.rs
// version: 1.1.0
// guid: c0ff669c-a00b-4be8-b4f2-9caf7b51a86e
// last-edited: 2026-08-02

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
//! | `sss`                                        | `unlock_sss`                                    |
//! | `tpm2_clevis_peer`                           | *(none — see below)*                            |
//!
//! `tpm2_clevis_peer` is authoring/validate-ONLY: it never lowers to a wire
//! field. The D2-B clevis TPM2 peer share is derived by the installer from
//! `storage_mode == NativeKeystore`
//! (`network/ssh_installer/system_setup.rs:722` / `:772`), not from any
//! profile input.

use crate::network::ssh_installer::config::TangServer;
use crate::network::ssh_installer::unlock_sss::SssPolicy;
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
    /// EXPLICIT clevis SSS policy tree, lowering to `InstallationConfig::unlock_sss`.
    ///
    /// The concrete wire type is embedded directly rather than mirrored into a
    /// `*Partial` — the `applications: Option<Vec<ApplicationSpec>>` precedent
    /// on `InstallationConfigPartial`. Leaf-by-leaf merging a recursive tree is
    /// meaningless (there is no sensible "leaf" of a shape mismatch), so this
    /// resolves WHOLE-VALUE, host-tier-wins, like any other component leaf.
    ///
    /// ## Precedence: the tree WINS over `tang`
    ///
    /// When a host authors both `sss` and `tang`, the tree is what lowers to
    /// `unlock_sss` and therefore what the installer binds; `tang` still lowers
    /// to `tang_servers`/`tang_threshold` (which other things read — the
    /// initramfs clevis package gate among them). `lower` is documented pure
    /// and total, so this is a precedence rule, NOT an error. Cross-checking
    /// that a co-authored `tang` agrees with the tree belongs to the validate
    /// layer (PS-VALIDATE-14), as does clevis's `1 <= t <= share_count`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sss: Option<SssPolicy>,
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
            sss: Some(SssPolicy::tpm2_and_tang(
                "7",
                &[TangServer {
                    url: "http://tang1.example.internal".to_string(),
                }],
                1,
            )),
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
                sss: None,
            }
        );
    }

    /// The three policies the nested representation must express, authored the
    /// way a profile author actually writes them: YAML.
    #[test]
    fn test_yaml_authors_the_three_required_policies() {
        // 1. LEGACY: flat N-of-M Tang, authored with no `sss` at all. This is
        //    len-serv-001/002's shape and it must stay expressible untouched.
        let legacy: UnlockPolicyPartial = serde_yaml::from_str(
            r#"
tang:
  servers:
    - url: http://tang1.example.internal
    - url: http://tang2.example.internal
    - url: http://tang3.example.internal
  threshold: 2
"#,
        )
        .unwrap();
        assert!(
            legacy.sss.is_none(),
            "a flat-authored host must carry no policy tree"
        );
        assert_eq!(legacy.tang.as_ref().unwrap().threshold, Some(2));

        // 2. AND(tpm2, 2-of-3 tang): outer t=2 over exactly TWO shares,
        //    because the tang group is nested and so collapses to one.
        let and: UnlockPolicyPartial = serde_yaml::from_str(
            r#"
sss:
  threshold: 2
  pins:
    - kind: tpm2
      pcr_ids: "7"
      pcr_bank: sha256
    - kind: sss
      threshold: 2
      pins:
        - kind: tang
          url: http://tang1.example.internal
        - kind: tang
          url: http://tang2.example.internal
        - kind: tang
          url: http://tang3.example.internal
"#,
        )
        .unwrap();
        let tree = and.sss.as_ref().expect("tree must parse");
        assert_eq!(tree.share_count(), 2, "AND must be 2-of-2, not 2-of-4");
        assert_eq!(tree.threshold, 2);
        assert_eq!(tree.tang_urls().len(), 3);
        assert_eq!(
            and,
            UnlockPolicyPartial {
                sss: Some(SssPolicy::tpm2_and_tang(
                    "7",
                    &[
                        TangServer {
                            url: "http://tang1.example.internal".to_string()
                        },
                        TangServer {
                            url: "http://tang2.example.internal".to_string()
                        },
                        TangServer {
                            url: "http://tang3.example.internal".to_string()
                        },
                    ],
                    2,
                )),
                ..Default::default()
            },
            "the authored YAML must equal the constructor's tree"
        );

        // 3. An OR of several pkcs11 URIs, usable as ONE share inside an AND.
        let or_group: UnlockPolicyPartial = serde_yaml::from_str(
            r#"
sss:
  threshold: 2
  pins:
    - kind: sss
      threshold: 1
      pins:
        - kind: pkcs11
          uri: "pkcs11:token=yubi-a"
        - kind: pkcs11
          uri: "pkcs11:token=yubi-b"
    - kind: tang
      url: http://tang1.example.internal
"#,
        )
        .unwrap();
        let tree = or_group.sss.as_ref().unwrap();
        assert_eq!(tree.share_count(), 2, "the pkcs11 OR counts as one share");
        match &tree.pins[0] {
            crate::network::ssh_installer::unlock_sss::UnlockPin::Sss(inner) => {
                assert_eq!(inner.threshold, 1, "any-one-of");
                assert_eq!(inner.share_count(), 2);
            }
            other => panic!("expected a nested pkcs11 group, got {}", other.kind()),
        }

        // and every one of the three survives a YAML round-trip
        for policy in [legacy, and, or_group] {
            let yaml = serde_yaml::to_string(&policy).unwrap();
            let back: UnlockPolicyPartial = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(policy, back, "YAML round-trip must be lossless");
        }
    }

    #[test]
    fn test_absent_sss_key_is_omitted_from_serialization() {
        // Same cross-version-rollback contract as `applications`: a host that
        // authors no tree must not gain an `sss:` key.
        let flat = UnlockPolicyPartial {
            tang: Some(TangSssPartial {
                servers: None,
                threshold: Some(2),
            }),
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&flat).unwrap();
        assert!(
            !yaml.contains("sss"),
            "a tree-free policy must omit the key entirely, got:\n{yaml}"
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
