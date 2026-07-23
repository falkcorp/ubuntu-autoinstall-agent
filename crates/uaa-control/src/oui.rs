// file: crates/uaa-control/src/oui.rs
// version: 1.0.0
// guid: b3d81f42-5c07-4e19-9a6d-2f4c8e17b530
// last-edited: 2026-07-23

//! MAC → vendor (OUI) lookup and device classification for the discovery inbox.
//!
//! **Why this exists:** the neighbor-table scanner ([`crate::discovered`]) posts
//! *every* device that ARPs/DHCPs on 172.16.2.0/23 — phones, watches, TVs, smart
//! plugs — most of which are not UAA install targets. This module lets the SPA
//! (and, crucially, [`crate::operator::handlers::backfill_discovered_named`])
//! bucket those out of the machine triage view.
//!
//! **Two independent signals, strongest first:**
//! 1. **Locally-administered bit.** Modern phones/watches (iOS, Android) default
//!    to *randomized private* Wi-Fi MACs, which set bit `0x02` of the first
//!    octet. A real server always boots with its burned-in *universal* MAC. So
//!    this bit alone flags the iPhone/Apple-Watch case with no vendor data at
//!    all. (Note: QEMU/KVM `52:54:00:…` MACs are also locally-administered, so
//!    the vm-gate VMs classify as [`DeviceCategory::NonMachine`] — fine here.)
//! 2. **OUI vendor.** For devices using their real (universal) MAC, the first
//!    three octets identify the manufacturer via the embedded IEEE MA-L registry
//!    ([`OUI_DATA`]). A small keyword classifier maps the org name to a class.
//!
//! Vendor + class are pure functions of the MAC, so they are derived *on read*
//! ([`crate::discovered::DiscoveredStore::list`]) and never persisted — the
//! on-disk `discovered-macs.json` shape stays byte-identical.
//!
//! **Regenerate [`OUI_DATA`]** (rarely needed) from the public IEEE registry:
//! ```text
//! curl -fsSL https://standards-oui.ieee.org/oui/oui.csv | \
//!   python3 -c 'import csv,sys; [print(f"{r[1].lower()},{" ".join(r[2].split())}") \
//!     for r in csv.reader(sys.stdin) if len(r)>2 and len(r[1])==6]' | sort > oui_data.csv
//! ```
//! then restore the `#`-comment version header at the top of the file.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Embedded IEEE MA-L OUI registry, trimmed to `oui_hex_lowercase,organization`.
/// `#`-prefixed lines are comments; data lines split on the FIRST comma only
/// (organization names may themselves contain commas).
const OUI_DATA: &str = include_str!("oui_data.csv");

/// Lazily-parsed OUI table: 24-bit prefix (first three octets, high byte zero)
/// → organization name (a slice into [`OUI_DATA`]). Built once on first lookup.
fn table() -> &'static HashMap<u32, &'static str> {
    static TABLE: OnceLock<HashMap<u32, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut map = HashMap::with_capacity(40_000);
        for line in OUI_DATA.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((prefix, org)) = line.split_once(',') else {
                continue;
            };
            if let Some(key) = hex6_to_key(prefix) {
                map.insert(key, org);
            }
        }
        map
    })
}

/// Parse exactly six hex chars (`ac1f6b`) to a 24-bit key, or `None`.
fn hex6_to_key(hex: &str) -> Option<u32> {
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// The 24-bit OUI key of any MAC string (accepts `:`/`-`/`.`/no separators), or
/// `None` if it lacks at least six leading hex digits.
fn oui_key(mac: &str) -> Option<u32> {
    let hex: String = mac.chars().filter(|c| c.is_ascii_hexdigit()).take(6).collect();
    hex6_to_key(&hex)
}

/// Manufacturer for `mac` from the IEEE registry, or `None` for an unknown or
/// randomized (locally-administered) prefix.
pub fn lookup_vendor(mac: &str) -> Option<&'static str> {
    oui_key(mac).and_then(|k| table().get(&k).copied())
}

/// Class of a discovered device, derived from its MAC. Serialized lowercase so
/// the SPA can filter on the wire value (`"machine"` / `"na"` / `"unknown"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceCategory {
    /// A provisionable server / PC / SBC (real OUI from a computer vendor).
    Machine,
    /// **Not** a UAA install target: phone, watch, tablet, TV, IoT, or a
    /// randomized private MAC. The "NA" bucket the operator asked for — hidden
    /// from the default triage view and never auto-promoted into the registry.
    #[serde(rename = "na")]
    NonMachine,
    /// Universal MAC but the vendor is unknown or unclassified — can't decide,
    /// so it stays visible.
    #[default]
    Unknown,
}

impl DeviceCategory {
    /// `true` for [`DeviceCategory::Unknown`] — the `skip_serializing_if` gate
    /// that keeps the derived field out of the persisted on-disk row.
    pub fn is_unknown(&self) -> bool {
        matches!(self, DeviceCategory::Unknown)
    }
}

/// Vendor substrings (case-insensitive, verified against the real IEEE org
/// strings) that denote consumer / mobile / IoT gear you never PXE-install
/// Ubuntu onto → the NA bucket. **This is the only list where a wrong entry is
/// costly**: a false match here *hides* a device, whereas a false `Machine`/
/// `Unknown` only mislabels a still-visible row. Apple is deliberate: no fleet
/// install target is Apple, so Macs / iPhones / Watches belong in NA.
const NON_MACHINE_VENDORS: &[&str] = &[
    "apple",
    "samsung electronics",
    "google",
    "amazon",
    "espressif", // ESP32/ESP8266 — the chip behind most DIY/smart-home IoT
    "sonos",
    "nest labs",
    "fitbit",
    "garmin",
    "xiaomi",
    "oneplus",
    "huawei device",
    "ecobee",
    "tp-link",
    "meross",
];

/// Vendor substrings that denote a provisionable server / PC / SBC → Machine.
/// `intel corp` is included because the fleet's server NICs are predominantly
/// Intel; biasing an ambiguous Intel MAC toward the (still-visible) Machine
/// class is safer than risking a real server landing in NA.
const MACHINE_VENDORS: &[&str] = &[
    "super micro",
    "supermicro",
    "lenovo",
    "dell inc",
    "hewlett", // Hewlett Packard / Hewlett Packard Enterprise
    "asustek",
    "asrock",
    "giga-byte", // IEEE spells it "GIGA-BYTE TECHNOLOGY"
    "micro-star", // MSI
    "raspberry pi",
    "intel corp",
    "quanta",
    "advantech",
];

/// Classify a device from its MAC and (already-resolved) vendor. See the module
/// docs for the two-signal rationale.
pub fn classify(mac: &str, vendor: Option<&str>) -> DeviceCategory {
    // Signal 1: a locally-administered (randomized/private) MAC is a phone/watch
    // or a VM — never a burned-in-MAC server.
    if let Some(key) = oui_key(mac) {
        let first_octet = (key >> 16) as u8;
        if first_octet & 0x02 != 0 {
            return DeviceCategory::NonMachine;
        }
    }
    // Signal 2: vendor keyword. NA is checked first so a consumer vendor is
    // never accidentally shadowed by a Machine substring.
    match vendor {
        Some(v) => {
            let v = v.to_ascii_lowercase();
            if NON_MACHINE_VENDORS.iter().any(|k| v.contains(k)) {
                DeviceCategory::NonMachine
            } else if MACHINE_VENDORS.iter().any(|k| v.contains(k)) {
                DeviceCategory::Machine
            } else {
                DeviceCategory::Unknown
            }
        }
        None => DeviceCategory::Unknown,
    }
}

/// Vendor + class for a canonical MAC, for enriching a discovery row on read.
pub fn enrich(mac: &str) -> (Option<String>, DeviceCategory) {
    let vendor = lookup_vendor(mac);
    let category = classify(mac, vendor);
    (vendor.map(str::to_string), category)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_oui_resolves_vendor() {
        // ac:1f:6b — U1's own Super Micro board (fleet ground truth).
        assert_eq!(lookup_vendor("ac:1f:6b:40:fc:e2"), Some("Super Micro Computer, Inc."));
        // Intel, Apple — high-volume prefixes.
        assert_eq!(lookup_vendor("00:1b:21:00:00:01"), Some("Intel Corporate"));
        assert_eq!(lookup_vendor("00:03:93:aa:bb:cc"), Some("Apple, Inc."));
    }

    #[test]
    fn accepts_any_separator_style() {
        assert!(lookup_vendor("ac1f6b40fce2").is_some());
        assert!(lookup_vendor("ac-1f-6b-40-fc-e2").is_some());
        assert!(lookup_vendor("AC:1F:6B:40:FC:E2").is_some());
    }

    #[test]
    fn unknown_and_malformed_are_none() {
        // A universally-administered but unassigned-looking prefix.
        assert_eq!(lookup_vendor("02:00:00:00:00:00"), None);
        assert_eq!(lookup_vendor("not-a-mac"), None);
        assert_eq!(lookup_vendor(""), None);
    }

    #[test]
    fn locally_administered_mac_is_non_machine_regardless_of_vendor() {
        // 0x02 bit set → randomized private MAC (phone/watch) even with no vendor.
        assert_eq!(classify("02:11:22:33:44:55", None), DeviceCategory::NonMachine);
        // 0x06 also has the LAA bit → still NA.
        assert_eq!(classify("06:aa:bb:cc:dd:ee", None), DeviceCategory::NonMachine);
        // QEMU/KVM MACs (52:54:00) are locally-administered → NA (documented).
        assert_eq!(classify("52:54:00:12:34:56", None), DeviceCategory::NonMachine);
    }

    #[test]
    fn universal_mac_classifies_by_vendor() {
        // ac:1f:6b is universal (0xac & 0x02 == 0) + Super Micro → Machine.
        assert_eq!(classify("ac:1f:6b:40:fc:e2", Some("Super Micro Computer, Inc.")), DeviceCategory::Machine);
        // Apple with a universal MAC → still NA (deliberate).
        assert_eq!(classify("00:03:93:aa:bb:cc", Some("Apple, Inc.")), DeviceCategory::NonMachine);
        // Unknown vendor, universal MAC → Unknown (stays visible).
        assert_eq!(classify("00:1b:21:00:00:01", Some("Some Unlisted Vendor Ltd")), DeviceCategory::Unknown);
    }

    #[test]
    fn enrich_pairs_vendor_and_category() {
        let (vendor, category) = enrich("ac:1f:6b:40:fc:e2");
        assert_eq!(vendor.as_deref(), Some("Super Micro Computer, Inc."));
        assert_eq!(category, DeviceCategory::Machine);
    }

    #[test]
    fn category_serializes_lowercase_with_na_alias() {
        assert_eq!(serde_json::to_string(&DeviceCategory::Machine).unwrap(), "\"machine\"");
        assert_eq!(serde_json::to_string(&DeviceCategory::NonMachine).unwrap(), "\"na\"");
        assert_eq!(serde_json::to_string(&DeviceCategory::Unknown).unwrap(), "\"unknown\"");
    }

    #[test]
    fn table_parses_the_whole_registry() {
        // Sanity: the embedded CSV parsed into a large table (guards a botched
        // regeneration that strips most rows).
        assert!(table().len() > 30_000, "OUI table looks truncated: {}", table().len());
    }
}
