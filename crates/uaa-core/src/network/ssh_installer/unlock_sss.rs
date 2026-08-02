// file: crates/uaa-core/src/network/ssh_installer/unlock_sss.rs
// version: 1.1.0
// guid: 08434a81-e744-40ab-a281-e34e41973bac
// last-edited: 2026-08-02

//! Composable clevis SSS unlock policy — the shared wire type.
//!
//! # Why this exists: `"tang":[a,b,c]` is THREE shares, not one
//!
//! clevis's `sss` pin is Shamir threshold secret sharing over CHILD PINS, and
//! an array-valued pin contributes one share PER ELEMENT. So the flat config
//! the installer emits today,
//!
//! ```json
//! {"t":2,"pins":{"tang":[a,b,c],"tpm2":{"pcr_ids":"7"}}}
//! ```
//!
//! is 2-of-**4**, which means the three Tang servers ALONE satisfy it. It is
//! NOT "tang AND tpm2" — a fact measured empirically against the live fleet
//! Tang servers, not inferred.
//!
//! A true AND requires NESTING, so that the whole Tang group collapses to a
//! single share:
//!
//! ```json
//! {"t":2,"pins":{
//!   "tpm2":{"pcr_ids":"7","pcr_bank":"sha256"},
//!   "sss":[{"t":2,"pins":{"tang":[a,b,c]}}]
//! }}
//! ```
//!
//! Nested `sss` was verified end-to-end: it encrypts and decrypts correctly,
//! fails closed when the inner threshold is unmet, and succeeds when it is met
//! (2 of 3 Tang up, 1 dead). It was also verified that `sss` accepts an ARRAY
//! for ANY pin, not just `tang` — `{"t":2,"pins":{"null":[{},{}]}}` yields two
//! shares (proved by `t=3` over that array erroring with
//! `Invalid threshold (required: 1 <= 3 <= 2)`).
//!
//! # The one invariant this type enforces by construction
//!
//! **Each element of [`SssPolicy::pins`] is exactly ONE share.** That is the
//! whole point of modeling the policy as a flat `Vec<UnlockPin>` per level
//! instead of a kind-keyed map: the share arithmetic is `pins.len()`
//! ([`SssPolicy::share_count`]), never something the reader has to recompute
//! by remembering that one JSON key can expand to N shares. Three Tang servers
//! that must count as three shares are three [`UnlockPin::Tang`] elements;
//! three Tang servers that must count as ONE share are one
//! [`UnlockPin::Sss`] element wrapping them.
//!
//! # Emitting clevis JSON
//!
//! clevis's `pins` is a JSON OBJECT keyed by pin name, so an emitter MUST
//! group same-kind pins into one array-valued key rather than emitting a
//! duplicate key. [`SssPolicy::pins_by_kind`] does that grouping — with
//! [`UnlockPin::kind`] as the single source of truth for the clevis pin name,
//! the same `kind()`-is-authoritative idiom `ApplicationSpec` uses. This
//! module deliberately stops at the semantic tree and its grouping: the
//! JSON-string construction lives in `system_setup.rs`.

use super::config::TangServer;
use serde::{Deserialize, Serialize};

/// Default TPM2 PCR bank. clevis itself defaults to `sha1`, which Secure Boot
/// does not populate — binding there yields a policy that cannot be satisfied
/// on a Secure Boot host, so this type defaults to the bank the fleet uses.
fn default_pcr_bank() -> String {
    "sha256".to_string()
}

/// A single share in an [`SssPolicy`]. Closed-but-growing by design, mirroring
/// `ApplicationSpec`: adding a `yubikey`/`null` pin later is a new variant, not
/// a plugin framework. An unknown `kind` is a hard parse error — never a silent
/// skip, because a silently-dropped share changes the unlock threshold
/// arithmetic and can produce a machine that cannot unlock at boot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum UnlockPin {
    /// One Tang server = one share.
    Tang(TangPin),
    /// A TPM2 policy = one share.
    Tpm2(Tpm2Pin),
    /// One PKCS#11 token URI = one share.
    Pkcs11(Pkcs11Pin),
    /// A NESTED threshold group that collapses to exactly ONE share at this
    /// level — the construct that makes a true AND (or a grouped OR)
    /// expressible. See the module doc.
    Sss(SssPolicy),
}

impl UnlockPin {
    /// The clevis pin name for this variant — the SINGLE SOURCE OF TRUTH for
    /// both the serde tag and the JSON key an emitter writes. Adding a variant
    /// without extending this match is a compile error, which is exactly the
    /// property that keeps the emitter and the authored YAML from drifting.
    pub fn kind(&self) -> &'static str {
        match self {
            UnlockPin::Tang(_) => "tang",
            UnlockPin::Tpm2(_) => "tpm2",
            UnlockPin::Pkcs11(_) => "pkcs11",
            UnlockPin::Sss(_) => "sss",
        }
    }
}

/// One Tang server share.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TangPin {
    pub url: String,
}

/// One TPM2 share.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tpm2Pin {
    /// Comma-separated PCR indices, e.g. `"7"` (Secure Boot state).
    pub pcr_ids: String,
    /// PCR bank — defaults to `sha256`, see [`default_pcr_bank`].
    #[serde(default = "default_pcr_bank")]
    pub pcr_bank: String,
}

/// One PKCS#11 token share (e.g. a YubiKey PIV slot URI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pkcs11Pin {
    pub uri: String,
}

/// A clevis `sss` threshold group: `threshold`-of-`pins.len()`.
///
/// Usable as the whole policy (top level) or, wrapped in
/// [`UnlockPin::Sss`], as a single share inside a parent group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SssPolicy {
    /// clevis's `t`. Named `threshold` for authoring legibility; an emitter
    /// writes it as `"t"`.
    pub threshold: u8,
    /// The shares. Each element is EXACTLY one share — see the module doc.
    pub pins: Vec<UnlockPin>,
}

impl SssPolicy {
    /// Number of shares at THIS level: `pins.len()`, because each element is
    /// one share regardless of how deep its subtree goes. clevis requires
    /// `1 <= threshold <= share_count()`; enforcing that is the validation
    /// layer's job, not this type's (`lower` is pure and total).
    pub fn share_count(&self) -> usize {
        self.pins.len()
    }

    /// Groups this level's pins by [`UnlockPin::kind`] for JSON emission,
    /// since clevis's `pins` is an object and a duplicate key would be
    /// invalid JSON. Order is deterministic: kinds in first-appearance order,
    /// pins within a kind in authored order — so the emitted JSON is stable
    /// across runs and diffable.
    pub fn pins_by_kind(&self) -> Vec<(&'static str, Vec<&UnlockPin>)> {
        let mut grouped: Vec<(&'static str, Vec<&UnlockPin>)> = Vec::new();
        for pin in &self.pins {
            let kind = pin.kind();
            match grouped.iter_mut().find(|(k, _)| *k == kind) {
                Some((_, bucket)) => bucket.push(pin),
                None => grouped.push((kind, vec![pin])),
            }
        }
        grouped
    }

    /// Every Tang URL anywhere in the tree, in depth-first authored order.
    ///
    /// The installer must pre-fetch each Tang advertisement before binding
    /// (without a pre-fetched `adv`, `clevis luks bind` prompts on `/dev/tty`
    /// and fails non-interactively over SSH), and with nesting those URLs are
    /// no longer all at the top level — so an emitter MUST walk the tree with
    /// this rather than iterating `InstallationConfig::tang_servers`.
    pub fn tang_urls(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_tang_urls(&mut out);
        out
    }

    fn collect_tang_urls<'a>(&'a self, out: &mut Vec<&'a str>) {
        for pin in &self.pins {
            match pin {
                UnlockPin::Tang(t) => out.push(t.url.as_str()),
                UnlockPin::Sss(nested) => nested.collect_tang_urls(out),
                UnlockPin::Tpm2(_) | UnlockPin::Pkcs11(_) => {}
            }
        }
    }

    /// Does ANY level of the tree use the given [`UnlockPin::kind`]?
    ///
    /// The installer gates *packages* on this. A pin whose userspace is missing
    /// from the target does not fail loudly at install time — it fails at the
    /// next boot, in the initramfs, on a host that is by then encrypted and
    /// unreachable. `tpm2` is the live example: `clevis-decrypt-tpm2` ships in
    /// the separate `clevis-tpm2` package, and without it the tpm2 share is
    /// simply unsatisfiable. Nesting means the pin can be at any depth, so the
    /// check must recurse rather than scan `pins` at the top level.
    ///
    /// Note the deliberate asymmetry with [`Self::tang_urls`]: this reports
    /// `"sss"` for a level that *contains* a nested group, since `kind()` is the
    /// authority and a nested group really is an `sss` pin at its own level.
    pub fn contains_kind(&self, kind: &str) -> bool {
        self.pins.iter().any(|pin| {
            pin.kind() == kind
                || match pin {
                    UnlockPin::Sss(nested) => nested.contains_kind(kind),
                    _ => false,
                }
        })
    }

    /// The LEGACY flat policy: `threshold`-of-N over N Tang servers, one share
    /// each. Exactly what `{"t":N,"pins":{"tang":[...]}}` means today, so a
    /// host that authors no tree keeps byte-identical behavior.
    pub fn flat_tang(servers: &[TangServer], threshold: u8) -> Self {
        SssPolicy {
            threshold,
            pins: servers
                .iter()
                .map(|s| UnlockPin::Tang(TangPin { url: s.url.clone() }))
                .collect(),
        }
    }

    /// True AND of a TPM2 share and a nested `tang_threshold`-of-N Tang group:
    /// outer `t=2` over exactly two shares, so BOTH are required. This is the
    /// policy the flat shape only LOOKS like it expresses — see the module doc.
    pub fn tpm2_and_tang(pcr_ids: &str, servers: &[TangServer], tang_threshold: u8) -> Self {
        SssPolicy {
            threshold: 2,
            pins: vec![
                UnlockPin::Tpm2(Tpm2Pin {
                    pcr_ids: pcr_ids.to_string(),
                    pcr_bank: default_pcr_bank(),
                }),
                UnlockPin::Sss(SssPolicy::flat_tang(servers, tang_threshold)),
            ],
        }
    }

    /// An OR over PKCS#11 token URIs (`t=1`), shaped so it can be dropped into
    /// a parent group as ONE share — "any one of these YubiKeys" as a single
    /// factor of an AND.
    pub fn any_pkcs11(uris: &[&str]) -> Self {
        SssPolicy {
            threshold: 1,
            pins: uris
                .iter()
                .map(|u| {
                    UnlockPin::Pkcs11(Pkcs11Pin {
                        uri: (*u).to_string(),
                    })
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tang3() -> Vec<TangServer> {
        vec![
            TangServer {
                url: "http://tang1.example.internal".to_string(),
            },
            TangServer {
                url: "http://tang2.example.internal".to_string(),
            },
            TangServer {
                url: "http://tang3.example.internal".to_string(),
            },
        ]
    }

    #[test]
    fn test_kind_matches_serde_tag_for_every_variant() {
        // kind() is the single source of truth for the clevis pin name; if it
        // ever drifts from the serde tag the emitted JSON key stops matching
        // the authored YAML. Assert they agree, variant by variant.
        let cases: Vec<(UnlockPin, &str)> = vec![
            (
                UnlockPin::Tang(TangPin {
                    url: "http://t".to_string(),
                }),
                "tang",
            ),
            (
                UnlockPin::Tpm2(Tpm2Pin {
                    pcr_ids: "7".to_string(),
                    pcr_bank: "sha256".to_string(),
                }),
                "tpm2",
            ),
            (
                UnlockPin::Pkcs11(Pkcs11Pin {
                    uri: "pkcs11:token=a".to_string(),
                }),
                "pkcs11",
            ),
            (
                UnlockPin::Sss(SssPolicy {
                    threshold: 1,
                    pins: vec![],
                }),
                "sss",
            ),
        ];
        for (pin, expected) in cases {
            assert_eq!(pin.kind(), expected);
            let json = serde_json::to_value(&pin).unwrap();
            assert_eq!(
                json.get("kind").and_then(|k| k.as_str()),
                Some(expected),
                "serde tag must equal kind() for {expected}"
            );
        }
    }

    #[test]
    fn test_flat_tang_is_three_shares_not_one() {
        // The arithmetic that motivates this whole type: three Tang servers in
        // the flat shape are THREE shares, so t=2 is satisfiable by Tang alone.
        let flat = SssPolicy::flat_tang(&tang3(), 2);
        assert_eq!(flat.share_count(), 3);
        assert!(
            u8::try_from(flat.share_count()).unwrap() > flat.threshold,
            "flat 2-of-3 tang is satisfiable without any other factor"
        );
    }

    #[test]
    fn test_nested_and_policy_is_exactly_two_shares() {
        // AND(tpm2, 2-of-3 tang): the Tang group collapses to ONE share, so
        // the outer t=2 over 2 shares REQUIRES both factors.
        let and = SssPolicy::tpm2_and_tang("7", &tang3(), 2);
        assert_eq!(and.share_count(), 2, "AND policy must be 2-of-2");
        assert_eq!(and.threshold, 2);
        assert_eq!(and.pins[0].kind(), "tpm2");
        assert_eq!(and.pins[1].kind(), "sss");
        match &and.pins[1] {
            UnlockPin::Sss(inner) => {
                assert_eq!(inner.share_count(), 3, "inner tang group is 3 shares");
                assert_eq!(inner.threshold, 2);
            }
            other => panic!("expected a nested sss share, got {}", other.kind()),
        }
    }

    #[test]
    fn test_any_pkcs11_is_one_share_when_nested() {
        // An OR of two token URIs is t=1 of 2 on its own...
        let or = SssPolicy::any_pkcs11(&["pkcs11:token=a", "pkcs11:token=b"]);
        assert_eq!(or.threshold, 1);
        assert_eq!(or.share_count(), 2);

        // ...but exactly ONE share when used as a factor of an AND.
        let and = SssPolicy {
            threshold: 2,
            pins: vec![
                UnlockPin::Sss(or),
                UnlockPin::Sss(SssPolicy::flat_tang(&tang3(), 2)),
            ],
        };
        assert_eq!(and.share_count(), 2);
    }

    #[test]
    fn test_pins_by_kind_groups_and_never_repeats_a_kind() {
        // clevis `pins` is a JSON object: two tang shares must become ONE
        // "tang" key holding a 2-element array, never a duplicate key.
        let policy = SssPolicy {
            threshold: 2,
            pins: vec![
                UnlockPin::Tang(TangPin {
                    url: "http://a".to_string(),
                }),
                UnlockPin::Tpm2(Tpm2Pin {
                    pcr_ids: "7".to_string(),
                    pcr_bank: "sha256".to_string(),
                }),
                UnlockPin::Tang(TangPin {
                    url: "http://b".to_string(),
                }),
            ],
        };
        let grouped = policy.pins_by_kind();
        assert_eq!(grouped.len(), 2, "two distinct kinds");
        assert_eq!(grouped[0].0, "tang", "first-appearance order");
        assert_eq!(grouped[0].1.len(), 2, "both tang shares in one bucket");
        assert_eq!(grouped[1].0, "tpm2");
        assert_eq!(grouped[1].1.len(), 1);

        let total: usize = grouped.iter().map(|(_, v)| v.len()).sum();
        assert_eq!(
            total,
            policy.share_count(),
            "grouping must not lose or duplicate a share"
        );
    }

    #[test]
    fn test_tang_urls_walks_nested_groups() {
        // The adv pre-fetch loop must find Tang URLs buried under nesting.
        let and = SssPolicy::tpm2_and_tang("7", &tang3(), 2);
        assert_eq!(
            and.tang_urls(),
            vec![
                "http://tang1.example.internal",
                "http://tang2.example.internal",
                "http://tang3.example.internal",
            ],
            "nested tang URLs must be reachable in authored order"
        );
        assert!(
            SssPolicy::any_pkcs11(&["pkcs11:token=a"])
                .tang_urls()
                .is_empty(),
            "a tang-free policy has no advertisements to fetch"
        );
    }

    #[test]
    fn test_yaml_roundtrip_three_level_nested_policy() {
        // YAML is the authoring format, and recursive internally-tagged enums
        // are exactly where a YAML backend can diverge from JSON — so assert
        // the deepest shape we support parses and round-trips.
        let yaml = r#"
threshold: 2
pins:
  - kind: sss
    threshold: 1
    pins:
      - kind: pkcs11
        uri: "pkcs11:token=yubi-a"
      - kind: pkcs11
        uri: "pkcs11:token=yubi-b"
  - kind: sss
    threshold: 2
    pins:
      - kind: tang
        url: http://tang1.example.internal
      - kind: tang
        url: http://tang2.example.internal
      - kind: tang
        url: http://tang3.example.internal
"#;
        let parsed: SssPolicy = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.share_count(), 2, "AND of two grouped factors");
        assert_eq!(parsed.tang_urls().len(), 3);

        let reserialized = serde_yaml::to_string(&parsed).unwrap();
        let again: SssPolicy = serde_yaml::from_str(&reserialized).unwrap();
        assert_eq!(parsed, again, "YAML round-trip must be lossless");

        // and the same value survives JSON too (the registry's transport)
        let json = serde_json::to_string(&parsed).unwrap();
        let from_json: SssPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, from_json);
    }

    #[test]
    fn test_tpm2_pcr_bank_defaults_to_sha256() {
        // clevis defaults to sha1, which Secure Boot leaves unpopulated — an
        // omitted bank must NOT inherit that trap.
        let pin: UnlockPin = serde_yaml::from_str("kind: tpm2\npcr_ids: \"7\"\n").unwrap();
        match pin {
            UnlockPin::Tpm2(t) => assert_eq!(t.pcr_bank, "sha256"),
            other => panic!("expected tpm2, got {}", other.kind()),
        }
    }

    #[test]
    fn test_unknown_pin_kind_is_a_hard_parse_error() {
        // A silently-dropped share changes the threshold arithmetic, so an
        // unknown kind must fail loudly rather than be skipped.
        let err = serde_yaml::from_str::<UnlockPin>("kind: yubikey\nuri: x\n")
            .expect_err("unknown pin kind must not parse");
        assert!(
            err.to_string().contains("yubikey"),
            "error must name the offending kind, got: {err}"
        );

        let err = serde_yaml::from_str::<SssPolicy>("threshold: 1\npins: []\nt: 1\n")
            .expect_err("unknown field must not parse");
        assert!(
            err.to_string().contains('t'),
            "error must name the offending key, got: {err}"
        );
    }

    #[test]
    fn test_contains_kind_recurses_into_nested_groups() {
        // The package gate that consumes this must see a tpm2 pin buried two
        // levels down; a top-level-only scan is the bug it exists to prevent.
        let deep = SssPolicy {
            threshold: 2,
            pins: vec![
                UnlockPin::Sss(SssPolicy::flat_tang(&tang3(), 2)),
                UnlockPin::Sss(SssPolicy {
                    threshold: 1,
                    pins: vec![UnlockPin::Tpm2(Tpm2Pin {
                        pcr_ids: "7".to_string(),
                        pcr_bank: default_pcr_bank(),
                    })],
                }),
            ],
        };
        assert!(deep.contains_kind("tpm2"), "nested tpm2 must be found");
        assert!(deep.contains_kind("tang"), "nested tang must be found");
        assert!(
            deep.contains_kind("sss"),
            "an sss pin is present at level 0"
        );
        assert!(!deep.contains_kind("pkcs11"));

        // Flat trees still answer correctly, and a Tang-only tree must NOT
        // claim tpm2 — that direction is what suppresses a bogus clevis-tpm2.
        let flat = SssPolicy::flat_tang(&tang3(), 2);
        assert!(flat.contains_kind("tang"));
        assert!(!flat.contains_kind("tpm2"));
        assert!(!flat.contains_kind("sss"));
    }

    #[test]
    fn test_contains_kind_agrees_with_kind_for_every_variant() {
        // Same drift guard as kind(): a new variant that forgets to be
        // reachable here silently loses its package.
        for pin in [
            UnlockPin::Tang(TangPin {
                url: "http://t".to_string(),
            }),
            UnlockPin::Tpm2(Tpm2Pin {
                pcr_ids: "7".to_string(),
                pcr_bank: default_pcr_bank(),
            }),
            UnlockPin::Pkcs11(Pkcs11Pin {
                uri: "pkcs11:x".to_string(),
            }),
            UnlockPin::Sss(SssPolicy {
                threshold: 1,
                pins: vec![],
            }),
        ] {
            let kind = pin.kind();
            let wrapped = SssPolicy {
                threshold: 1,
                pins: vec![pin],
            };
            assert!(wrapped.contains_kind(kind), "contains_kind missed {kind}");
        }
    }
}
