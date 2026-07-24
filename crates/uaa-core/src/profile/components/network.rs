// file: crates/uaa-core/src/profile/components/network.rs
// version: 1.0.0
// guid: 3f7d4a8c-2b1e-4f9a-8d3c-5e6b7a9c0d1f
// last-edited: 2026-07-23

use serde::{Deserialize, Serialize};

/// Network addressing mode: DHCP or static IP.
///
/// This enum replaces the magic `network_address == "dhcp"` string sentinel used in
/// the flat wire fields of [`InstallationConfigPartial`]. It provides type-safe,
/// self-documenting addressing configuration.
///
/// **Serialization:** Uses tagged format, serializing as `{"type":"dhcp"}` for DHCP
/// mode or `{"type":"static","address":"192.0.2.1/24","gateway":"192.0.2.1"}` for
/// static mode.
///
/// **PS-LOWER-12 mapping:** When lowering to the flat wire format used by the
/// installer:
/// - `Addressing::Dhcp` → `network_address="dhcp"` + `network_gateway=""`
/// - `Addressing::Static{address, gateway}` → `network_address=address` + `network_gateway=gateway`
///
/// This transformation is handled by PS-LOWER-12; this module only defines the
/// authoring-time enum.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Addressing {
    #[default]
    Dhcp,
    Static {
        address: String,
        gateway: String,
    },
}

/// Partial network configuration: all fields optional. Part of the authoring-time
/// [`InstallationConfigPartial`], allowing per-host or per-group network overrides
/// without restating unchanged fields.
///
/// Each field mirrors the corresponding wire field in
/// [`InstallationConfigPartial`](crate::profile::InstallationConfigPartial), but uses
/// the higher-level [`Addressing`] enum instead of flat `network_address` /
/// `network_gateway` strings.
///
/// **Serde mode:** Rejects unknown fields, and all fields default to `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NetworkConfigPartial {
    /// Network interface name (e.g. "eth0").
    pub interface: Option<String>,
    /// Addressing mode and configuration.
    pub addressing: Option<Addressing>,
    /// DNS search domain(s).
    pub search: Option<String>,
    /// DNS nameservers.
    pub nameservers: Option<Vec<String>>,
    /// Netplan renderer (e.g. "systemd", "networkd").
    pub renderer: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addressing_dhcp_default() {
        let addr = Addressing::default();
        assert_eq!(addr, Addressing::Dhcp);
    }

    #[test]
    fn test_addressing_dhcp_serialization() {
        let addr = Addressing::Dhcp;
        let json = serde_json::to_string(&addr).unwrap();
        assert_eq!(json, r#"{"type":"dhcp"}"#);
    }

    #[test]
    fn test_addressing_dhcp_roundtrip() {
        let addr = Addressing::Dhcp;
        let json = serde_json::to_string(&addr).unwrap();
        let deserialized: Addressing = serde_json::from_str(&json).unwrap();
        assert_eq!(addr, deserialized);
    }

    #[test]
    fn test_addressing_static_serialization() {
        let addr = Addressing::Static {
            address: "192.0.2.1/24".to_string(),
            gateway: "192.0.2.1".to_string(),
        };
        let json = serde_json::to_string(&addr).unwrap();
        // Check that it contains the required fields
        assert!(json.contains(r#""type":"static""#));
        assert!(json.contains(r#""address":"192.0.2.1/24""#));
        assert!(json.contains(r#""gateway":"192.0.2.1""#));
    }

    #[test]
    fn test_addressing_static_roundtrip() {
        let addr = Addressing::Static {
            address: "192.0.2.1/24".to_string(),
            gateway: "192.0.2.1".to_string(),
        };
        let json = serde_json::to_string(&addr).unwrap();
        let deserialized: Addressing = serde_json::from_str(&json).unwrap();
        assert_eq!(addr, deserialized);
    }

    #[test]
    fn test_addressing_static_missing_gateway_fails() {
        let json = r#"{"type":"static","address":"192.0.2.1/24"}"#;
        let result: Result<Addressing, _> = serde_json::from_str(json);
        assert!(result.is_err(), "missing gateway field must fail deserialization");
    }

    #[test]
    fn test_network_config_partial_default() {
        let config = NetworkConfigPartial::default();
        assert_eq!(
            config,
            NetworkConfigPartial {
                interface: None,
                addressing: None,
                search: None,
                nameservers: None,
                renderer: None,
            }
        );
    }

    #[test]
    fn test_network_config_partial_dhcp_roundtrip() {
        let config = NetworkConfigPartial {
            interface: Some("eth0".to_string()),
            addressing: Some(Addressing::Dhcp),
            search: Some("example.com".to_string()),
            nameservers: Some(vec!["8.8.8.8".to_string()]),
            renderer: Some("systemd".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: NetworkConfigPartial = serde_json::from_str(&json).unwrap();
        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_network_config_partial_static_roundtrip() {
        let config = NetworkConfigPartial {
            interface: Some("eth0".to_string()),
            addressing: Some(Addressing::Static {
                address: "192.0.2.1/24".to_string(),
                gateway: "192.0.2.1".to_string(),
            }),
            search: Some("example.com".to_string()),
            nameservers: Some(vec!["8.8.8.8".to_string()]),
            renderer: Some("systemd".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: NetworkConfigPartial = serde_json::from_str(&json).unwrap();
        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_network_config_partial_rejects_unknown_fields() {
        let json = r#"{"interface":"eth0","unknown_field":"value"}"#;
        let result: Result<NetworkConfigPartial, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "unknown field must fail deserialization"
        );
    }
}
