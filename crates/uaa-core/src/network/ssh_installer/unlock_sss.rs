// file: crates/uaa-core/src/network/ssh_installer/unlock_sss.rs
// version: 1.3.0
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

    /// The settled fleet policy: an outer `t=1` OR over three groups, so ANY
    /// ONE group unlocks the host.
    ///
    /// ```json
    /// {"t":1,"pins":{"sss":[
    ///   {"t":<peer_threshold>,"pins":{"tang":[peers…]}},
    ///   {"t":<token_threshold>,"pins":{"pkcs11":[nano, carried…]}},
    ///   {"t":2,"pins":{"sss":[
    ///       {"t":1,"pins":{"tang":[peers…]}},
    ///       {"t":1,"pins":{"pkcs11":[carried…]}}
    ///   ]}}
    /// ]}}
    /// ```
    ///
    /// * **group 1** — automatic, hands-off unlock while the peer Tang servers
    ///   are up. With `tpm2_pcr_ids = Some(..)` (the lenserv variant) it
    ///   additionally ANDs a TPM2 share, so the disk only unlocks unattended in
    ///   *this* chassis with *this* Secure Boot state. The RPis have no TPM and
    ///   deliberately get no `tpm2` pin anywhere.
    /// * **group 2** — `token_threshold` of the three PKCS#11 tokens: the nano
    ///   that lives permanently in the chassis, plus the two carried keys (main
    ///   and offsite spare). The break-glass path when Tang is down.
    /// * **group 3** — (any one Tang) AND (either CARRIED key).
    ///
    /// # The nano is deliberately EXCLUDED from group 3
    ///
    /// This is a security property, not an oversight, and it is load-bearing.
    /// The nano lives in the chassis; anyone who steals the server is already
    /// holding it. If the nano were a member of group 3, that thief would only
    /// need to reach ONE Tang server — trivially satisfied while the box is
    /// still on the LAN, or by any single compromised/rehosted Tang — and the
    /// disk would open. Restricting group 3 to the CARRIED keys means physical
    /// possession of the chassis is never, by itself, one of the two factors.
    /// `test_nano_is_excluded_from_group_three` in `system_setup.rs` fails if
    /// someone "helpfully" adds it back.
    ///
    /// The caller is expected to run [`Self::validate`] on the result; this
    /// constructor is total and performs no checking of its own.
    pub fn fleet_three_group(
        peers: &[&str],
        peer_threshold: u8,
        nano_uri: &str,
        carried_uris: &[&str],
        token_threshold: u8,
        tpm2_pcr_ids: Option<&str>,
    ) -> Self {
        let tang_group = |threshold: u8| SssPolicy {
            threshold,
            pins: peers
                .iter()
                .map(|u| {
                    UnlockPin::Tang(TangPin {
                        url: (*u).to_string(),
                    })
                })
                .collect(),
        };

        // Group 1: peers, optionally ANDed with tpm2 (lenserv only).
        let group_one = match tpm2_pcr_ids {
            None => tang_group(peer_threshold),
            Some(pcr_ids) => SssPolicy {
                threshold: 2,
                pins: vec![
                    UnlockPin::Tpm2(Tpm2Pin {
                        pcr_ids: pcr_ids.to_string(),
                        pcr_bank: default_pcr_bank(),
                    }),
                    UnlockPin::Sss(tang_group(peer_threshold)),
                ],
            },
        };

        // Group 2: nano + both carried keys.
        let mut token_uris: Vec<&str> = vec![nano_uri];
        token_uris.extend_from_slice(carried_uris);
        let group_two = SssPolicy::any_pkcs11(&token_uris);
        let group_two = SssPolicy {
            threshold: token_threshold,
            ..group_two
        };

        // Group 3: (any one Tang) AND (either CARRIED key). NO NANO — see above.
        let group_three = SssPolicy {
            threshold: 2,
            pins: vec![
                UnlockPin::Sss(tang_group(1)),
                UnlockPin::Sss(SssPolicy::any_pkcs11(carried_uris)),
            ],
        };

        SssPolicy {
            threshold: 1,
            pins: vec![
                UnlockPin::Sss(group_one),
                UnlockPin::Sss(group_two),
                UnlockPin::Sss(group_three),
            ],
        }
    }

    /// Validate the whole tree, recursively.
    ///
    /// `Ok(warnings)` — the policy is emittable; any strings returned are
    /// non-fatal smells worth logging. `Err(messages)` — the policy MUST NOT be
    /// bound; every rule it broke is reported at once (not just the first), so
    /// an author fixes one round of YAML rather than N.
    ///
    /// See [`PolicyLint`] for the rules and why each one exists.
    pub fn validate(&self) -> std::result::Result<Vec<String>, Vec<String>> {
        let lint = self.lint();
        if lint.errors.is_empty() {
            Ok(lint.warnings)
        } else {
            Err(lint.errors)
        }
    }

    /// Collect every rule violation in the tree without deciding fatality.
    pub fn lint(&self) -> PolicyLint {
        let mut lint = PolicyLint::default();
        self.lint_level("policy", &mut lint);
        lint
    }

    fn lint_level(&self, path: &str, lint: &mut PolicyLint) {
        // --- rule: threshold must be legal AT THIS LEVEL --------------------
        // clevis enforces `1 <= t <= <shares>` per level and errors out with
        // `Invalid threshold (required: 1 <= t <= n)`. Catching it here means
        // the failure lands on the author's terminal instead of half-way
        // through a bind, on a host whose pools are already created.
        let shares = self.share_count();
        if shares == 0 {
            lint.errors.push(format!(
                "{path}: has no shares — an empty `pins` can never be satisfied"
            ));
        } else if self.threshold < 1 || usize::from(self.threshold) > shares {
            lint.errors.push(format!(
                "{path}: threshold {} is illegal for {shares} share(s) — clevis \
                 requires 1 <= t <= {shares} at EVERY level",
                self.threshold
            ));
        }

        // --- rule: no duplicate share identity WITHIN one level -------------
        // Two shares that resolve to the same secret-holder are one share
        // wearing two hats: a `t=2` group holding the same token twice reads as
        // 2-of-3 and is satisfiable by one token. This is the static analogue of
        // the `head -1` bind hazard (see
        // docs/research/2026-08-02-pkcs11-share-binding-hazard.md).
        //
        // Deliberately PER LEVEL, never global: the settled fleet policy names
        // the same Tang peer in group 1 and again inside group 3, and the same
        // carried key in group 2 and again inside group 3. Those repeats are the
        // whole point of an OR over groups. Only a repeat inside ONE `pins`
        // array corrupts that level's arithmetic.
        let mut seen: Vec<(&str, &str)> = Vec::new();
        for pin in &self.pins {
            let identity = match pin {
                UnlockPin::Tang(t) => Some(("tang", t.url.as_str())),
                UnlockPin::Pkcs11(p) => Some(("pkcs11", p.uri.as_str())),
                // A tpm2 pin has no identity beyond the host's own TPM, and a
                // nested group's identity is its subtree — neither is compared.
                UnlockPin::Tpm2(_) | UnlockPin::Sss(_) => None,
            };
            if let Some(id) = identity {
                if seen.contains(&id) {
                    lint.errors.push(format!(
                        "{path}: duplicate {} share `{}` at the same level — it \
                         counts twice toward t but can only be satisfied once, \
                         so this group is weaker than it looks",
                        id.0, id.1
                    ));
                } else {
                    seen.push(id);
                }
            }
        }

        // --- per-pin rules and recursion ------------------------------------
        for (i, pin) in self.pins.iter().enumerate() {
            let child = format!("{path}.pins[{i}]");
            match pin {
                UnlockPin::Pkcs11(p) => lint_pkcs11_uri(&child, &p.uri, lint),
                UnlockPin::Sss(nested) => {
                    // --- rule (warn): a 1-of-1 wrapper is a no-op nesting.
                    // Harmless to clevis, but it is almost always a half-edited
                    // group — the author deleted a share and left the wrapper,
                    // or meant to add one and did not.
                    if nested.share_count() == 1 {
                        lint.warnings.push(format!(
                            "{child}: nested group wraps a single share, which \
                             collapses to that share — either add the shares \
                             this group was meant to hold, or drop the wrapper"
                        ));
                    }
                    nested.lint_level(&child, lint);
                }
                UnlockPin::Tang(_) | UnlockPin::Tpm2(_) => {}
            }
        }
    }
}

/// The result of [`SssPolicy::lint`]: fatal violations and non-fatal smells.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyLint {
    /// Rules whose violation makes the policy unbindable or silently weaker
    /// than authored. Refuse the install.
    pub errors: Vec<String>,
    /// Shapes that are legal but suspicious. Log loudly; do not block.
    pub warnings: Vec<String>,
}

/// Rules that apply to a single PKCS#11 token URI.
fn lint_pkcs11_uri(path: &str, uri: &str, lint: &mut PolicyLint) {
    let lowered = uri.to_ascii_lowercase();

    if !lowered.starts_with("pkcs11:") {
        lint.errors.push(format!(
            "{path}: pkcs11 uri `{uri}` is not an RFC 7512 URI — it must begin \
             with `pkcs11:`"
        ));
    }

    // --- rule: never store the PIN in the binding -------------------------
    // clevis's pkcs11 pin takes the PIN at UNLOCK time, from
    // /run/systemd/clevis-pkcs11.pin (fed by clevis-luks-pkcs11-askpass.socket)
    // — or, if the URI carries `pin-value=`, straight out of the URI. But the
    // URI is stored IN THE LUKS HEADER, in the clear, on the disk the token is
    // supposed to protect. A `pin-value=` binding therefore reduces a
    // something-you-have-AND-something-you-know factor to something-you-have,
    // and does it invisibly: the policy still unlocks, so nothing ever fails to
    // reveal it. This is the single most damaging thing an author can type here.
    if lowered.contains("pin-value=") {
        lint.errors.push(format!(
            "{path}: pkcs11 uri carries `pin-value=` — the PIN would be written \
             into the LUKS header in the clear, defeating the entire factor. \
             Let clevis-luks-pkcs11-askpass supply the PIN at unlock time."
        ));
    }

    // --- rule: key on `serial=`, not `slot-id=` ---------------------------
    // PKCS#11 slot IDs are assigned by the module at enumeration time and shift
    // between insertions, reboots, and whenever another token is plugged in
    // first. A binding keyed on `slot-id=` resolves to a DIFFERENT token — or
    // to nothing — the next time the host boots, which surfaces as an unlock
    // failure in the initramfs on an encrypted, unreachable machine. `serial=`
    // is a property of the token itself and is stable.
    if lowered.contains("slot-id=") && !lowered.contains("serial=") {
        lint.errors.push(format!(
            "{path}: pkcs11 uri `{uri}` is keyed on `slot-id=` with no \
             `serial=` — slot IDs are reassigned between insertions, so this \
             binding will address the wrong token (or none) at the next boot. \
             Key on `serial=`."
        ));
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

    // ---- validation ----------------------------------------------------

    /// The settled RPi fleet policy, built by the production constructor.
    fn rpi_policy() -> SssPolicy {
        SssPolicy::fleet_three_group(
            &["http://172.16.2.45", "http://172.16.2.46"],
            2,
            "pkcs11:serial=NANO0001",
            &["pkcs11:serial=CARRIED0A", "pkcs11:serial=CARRIED0B"],
            2,
            None,
        )
    }

    fn errors_of(policy: &SssPolicy) -> Vec<String> {
        policy.validate().expect_err("policy must be rejected")
    }

    #[test]
    fn test_settled_fleet_policies_validate_clean() {
        // The deliverable itself must pass every rule — including the ones that
        // exist to catch shapes that superficially resemble it. A validator that
        // rejects the policy it was written for is worse than none.
        let warnings = rpi_policy()
            .validate()
            .expect("the settled RPi policy must be valid");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let lenserv = SssPolicy::fleet_three_group(
            &["http://172.16.2.45", "http://172.16.2.46"],
            2,
            "pkcs11:serial=NANO0001",
            &["pkcs11:serial=CARRIED0A", "pkcs11:serial=CARRIED0B"],
            2,
            Some("7"),
        );
        let warnings = lenserv
            .validate()
            .expect("the settled lenserv policy must be valid");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn test_reject_pkcs11_uri_with_pin_value() {
        // Storing the PIN in the LUKS header reduces the factor to
        // something-you-have and never fails visibly.
        let policy = SssPolicy {
            threshold: 1,
            pins: vec![UnlockPin::Pkcs11(Pkcs11Pin {
                uri: "pkcs11:serial=YK0000001;pin-value=123456".to_string(),
            })],
        };
        let errors = errors_of(&policy);
        assert_eq!(errors.len(), 1, "exactly one rule broken: {errors:?}");
        assert!(
            errors[0].contains("pin-value="),
            "error must name the offending parameter: {errors:?}"
        );

        // Case must not be an escape hatch — RFC 7512 attribute names are
        // matched case-insensitively by the tooling that parses them.
        let shouty = SssPolicy {
            threshold: 1,
            pins: vec![UnlockPin::Pkcs11(Pkcs11Pin {
                uri: "PKCS11:SERIAL=YK1;PIN-VALUE=123456".to_string(),
            })],
        };
        assert!(
            errors_of(&shouty).iter().any(|e| e.contains("pin-value=")),
            "uppercase pin-value must be rejected too"
        );
    }

    #[test]
    fn test_reject_pkcs11_uri_keyed_on_slot_id_without_serial() {
        // slot IDs are reassigned between insertions; the binding addresses the
        // wrong token at the next boot.
        let policy = SssPolicy {
            threshold: 1,
            pins: vec![UnlockPin::Pkcs11(Pkcs11Pin {
                uri: "pkcs11:slot-id=0".to_string(),
            })],
        };
        let errors = errors_of(&policy);
        assert_eq!(errors.len(), 1, "exactly one rule broken: {errors:?}");
        assert!(
            errors[0].contains("slot-id="),
            "error must name the offending parameter: {errors:?}"
        );

        // A slot-id used only as a HINT alongside a stable serial is fine —
        // that is how a multi-slot token is addressed, and rejecting it would
        // push authors back to serial-less URIs.
        let hinted = SssPolicy {
            threshold: 1,
            pins: vec![UnlockPin::Pkcs11(Pkcs11Pin {
                uri: "pkcs11:serial=YK0000001;slot-id=0".to_string(),
            })],
        };
        assert!(
            hinted.validate().is_ok(),
            "serial= plus a slot-id hint must be accepted"
        );
    }

    #[test]
    fn test_reject_pkcs11_uri_that_is_not_an_rfc7512_uri() {
        // A bare serial or a mistyped separator is not a PKCS#11 URI. clevis
        // hands it to pkcs11-tool as a token filter, where it silently matches
        // nothing — and a filter that matches nothing is exactly the state that
        // makes `pkcs11-tool -O` enumerate EVERY slot and encrypt every share to
        // whichever token happens to be first. See
        // docs/research/2026-08-02-pkcs11-share-binding-hazard.md.
        for bad in ["YK0000001", "pkcs11-serial=YK0000001", ""] {
            let policy = SssPolicy {
                threshold: 1,
                pins: vec![UnlockPin::Pkcs11(Pkcs11Pin {
                    uri: bad.to_string(),
                })],
            };
            assert!(
                errors_of(&policy).iter().any(|e| e.contains("RFC 7512")),
                "`{bad}` must be rejected as a non-URI"
            );
        }

        // Case is not an escape hatch, and a well-formed URI still passes.
        for good in ["pkcs11:serial=YK0000001", "PKCS11:serial=YK0000001"] {
            let policy = SssPolicy {
                threshold: 1,
                pins: vec![UnlockPin::Pkcs11(Pkcs11Pin {
                    uri: good.to_string(),
                })],
            };
            assert!(policy.validate().is_ok(), "`{good}` must be accepted");
        }
    }

    #[test]
    fn test_reject_illegal_threshold_at_every_level() {
        // t > shares at the ROOT.
        let too_high = SssPolicy {
            threshold: 3,
            pins: vec![
                UnlockPin::Tang(TangPin {
                    url: "http://a".to_string(),
                }),
                UnlockPin::Tang(TangPin {
                    url: "http://b".to_string(),
                }),
            ],
        };
        assert!(
            errors_of(&too_high)[0].contains("threshold 3"),
            "root t=3 of 2 must be rejected"
        );

        // t = 0 anywhere.
        let zero = SssPolicy {
            threshold: 0,
            pins: vec![UnlockPin::Tang(TangPin {
                url: "http://a".to_string(),
            })],
        };
        assert!(
            errors_of(&zero)[0].contains("threshold 0"),
            "t=0 must be rejected"
        );

        // And — the point of the recursion — t > shares THREE LEVELS DOWN,
        // under two legal levels. A root-only check passes this happily and
        // clevis then dies mid-bind.
        let deep = SssPolicy {
            threshold: 1,
            pins: vec![UnlockPin::Sss(SssPolicy {
                threshold: 1,
                pins: vec![
                    UnlockPin::Sss(SssPolicy {
                        threshold: 9,
                        pins: vec![UnlockPin::Tang(TangPin {
                            url: "http://a".to_string(),
                        })],
                    }),
                    UnlockPin::Tang(TangPin {
                        url: "http://b".to_string(),
                    }),
                ],
            })],
        };
        let errors = errors_of(&deep);
        assert!(
            errors.iter().any(|e| e.contains("threshold 9")),
            "a bad threshold at depth 2 must still be caught: {errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.contains("policy.pins[0].pins[0]")),
            "the error must locate the offending level: {errors:?}"
        );

        // An empty group is unsatisfiable at any threshold.
        let empty = SssPolicy {
            threshold: 1,
            pins: vec![],
        };
        assert!(
            errors_of(&empty)[0].contains("no shares"),
            "an empty group must be rejected"
        );
    }

    #[test]
    fn test_reject_duplicate_share_identity_within_one_level() {
        // A t=2 group holding the same token twice reads as 2-of-3 and is
        // satisfiable by ONE token — the static analogue of the head -1 bind
        // hazard.
        let dup_token = SssPolicy {
            threshold: 2,
            pins: vec![
                UnlockPin::Pkcs11(Pkcs11Pin {
                    uri: "pkcs11:serial=YK0000001".to_string(),
                }),
                UnlockPin::Pkcs11(Pkcs11Pin {
                    uri: "pkcs11:serial=YK0000001".to_string(),
                }),
                UnlockPin::Pkcs11(Pkcs11Pin {
                    uri: "pkcs11:serial=YK0000002".to_string(),
                }),
            ],
        };
        let errors = errors_of(&dup_token);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("duplicate pkcs11") && e.contains("YK0000001")),
            "the repeated token must be named: {errors:?}"
        );

        // Same for Tang.
        let dup_tang = SssPolicy {
            threshold: 2,
            pins: vec![
                UnlockPin::Tang(TangPin {
                    url: "http://172.16.2.45".to_string(),
                }),
                UnlockPin::Tang(TangPin {
                    url: "http://172.16.2.45".to_string(),
                }),
            ],
        };
        assert!(
            errors_of(&dup_tang)
                .iter()
                .any(|e| e.contains("duplicate tang")),
            "a repeated Tang URL must be rejected"
        );
    }

    #[test]
    fn test_duplicate_check_is_per_level_not_global() {
        // THE constraint that keeps rule 4 from rejecting its own deliverable:
        // the settled policy names peerA in group 1 AND again inside group 3,
        // and carriedA in group 2 AND again inside group 3. A global scan fails
        // the exact JSON we are required to emit.
        let policy = rpi_policy();
        assert!(
            policy.validate().is_ok(),
            "cross-group repeats are the point of an OR over groups"
        );

        // Prove the fixture really does repeat, so the assertion above is not
        // vacuous.
        let tang = policy.tang_urls();
        assert_eq!(
            tang.iter().filter(|u| **u == "http://172.16.2.45").count(),
            2,
            "peerA must appear in two different groups for this test to mean \
             anything: {tang:?}"
        );
    }

    #[test]
    fn test_warn_on_redundant_single_share_wrapper() {
        // Legal, but almost always a half-edited group. Warn, do not block.
        let policy = SssPolicy {
            threshold: 1,
            pins: vec![UnlockPin::Sss(SssPolicy {
                threshold: 1,
                pins: vec![UnlockPin::Tang(TangPin {
                    url: "http://a".to_string(),
                })],
            })],
        };
        let warnings = policy
            .validate()
            .expect("a no-op wrapper is legal clevis, not an error");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("single share"),
            "warning must say what is wrong: {warnings:?}"
        );
    }

    #[test]
    fn test_validate_reports_every_violation_not_just_the_first() {
        // An author fixing YAML should get one round of errors, not N rounds.
        let policy = SssPolicy {
            threshold: 5,
            pins: vec![
                UnlockPin::Pkcs11(Pkcs11Pin {
                    uri: "pkcs11:slot-id=0".to_string(),
                }),
                UnlockPin::Pkcs11(Pkcs11Pin {
                    uri: "pkcs11:serial=YK1;pin-value=1234".to_string(),
                }),
            ],
        };
        let errors = errors_of(&policy);
        assert_eq!(
            errors.len(),
            3,
            "bad threshold + slot-id + pin-value: {errors:?}"
        );
    }

    #[test]
    fn test_fleet_constructor_excludes_the_nano_from_group_three() {
        // The security property, asserted on the TYPE (the JSON-level twin of
        // this test lives in system_setup.rs). A chassis thief already holds the
        // nano; if it counted toward group 3 they would need only ONE Tang.
        let policy = rpi_policy();
        let group_three = match &policy.pins[2] {
            UnlockPin::Sss(g) => g,
            other => panic!("group 3 must be a nested group, got {}", other.kind()),
        };
        let token_group = match &group_three.pins[1] {
            UnlockPin::Sss(g) => g,
            other => panic!(
                "group 3's second share must be the token group, got {}",
                other.kind()
            ),
        };
        let uris: Vec<&str> = token_group
            .pins
            .iter()
            .map(|p| match p {
                UnlockPin::Pkcs11(k) => k.uri.as_str(),
                other => panic!("expected a pkcs11 share, got {}", other.kind()),
            })
            .collect();
        // Both directions: the carried keys ARE there (so a mis-navigated path
        // cannot pass vacuously) and the nano is NOT.
        assert_eq!(
            uris,
            vec!["pkcs11:serial=CARRIED0A", "pkcs11:serial=CARRIED0B"]
        );
        assert!(
            !uris.contains(&"pkcs11:serial=NANO0001"),
            "the nano must NEVER be a group-3 factor: {uris:?}"
        );
        // ...and it IS in group 2, so its absence above is an exclusion, not an
        // omission from the whole policy.
        let group_two = match &policy.pins[1] {
            UnlockPin::Sss(g) => g,
            other => panic!("group 2 must be a nested group, got {}", other.kind()),
        };
        assert!(group_two.pins.iter().any(|p| matches!(
            p,
            UnlockPin::Pkcs11(k) if k.uri == "pkcs11:serial=NANO0001"
        )));
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
