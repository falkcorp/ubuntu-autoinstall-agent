// file: crates/uaa-core/src/autoinstall/host_spec.rs
// version: 1.0.1
// guid: b1c2d3e4-f5a6-7b8c-9d0e-1f2a3b4c5d6e
// last-edited: 2026-07-10

//! Per-host inputs for rendering an autoinstall `user-data`.
//!
//! A [`HostSpec`] holds the small set of values that differ between machines —
//! everything else lives in the template. The fields map 1:1 to the template
//! placeholders (see [`crate::autoinstall::render`]).

/// The CockroachDB SQL/RPC port used in advertise/join strings on this fleet.
pub const COCKROACH_PORT: u16 = 36357;

/// The cluster's seed/server IP, always listed first in the join string.
pub const COCKROACH_SERVER_IP: &str = "172.16.2.30";

/// The Lenovo cluster member IPs, in canonical (ascending) order.
pub const LENSERV_MEMBER_IPS: &[&str] = &["172.16.3.92", "172.16.3.94", "172.16.3.96"];

/// The values that vary per host. Each maps to one template placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    /// `{{HOSTNAME}}` — e.g. `len-serv-003`.
    pub hostname: String,
    /// `{{NET_ADDRESS}}` — host IP **with CIDR**, e.g. `172.16.3.96/23`.
    pub network_address: String,
    /// `{{COCKROACH_ADVERTISE}}` — `ip:port`, e.g. `172.16.3.96:36357`.
    pub cockroach_advertise: String,
    /// `{{COCKROACH_JOIN}}` — comma-separated `ip:port` list (server first, then
    /// the other members, excluding self).
    pub cockroach_join: String,
}

impl HostSpec {
    /// Strip the `/cidr` suffix from an address, returning the bare IP.
    pub fn ip_without_cidr(address: &str) -> &str {
        address.split('/').next().unwrap_or(address)
    }

    /// Build the CockroachDB advertise string for a host IP: `ip:port`.
    pub fn compute_advertise(ip: &str, port: u16) -> String {
        format!("{ip}:{port}")
    }

    /// Build the CockroachDB join string: the server first, then every member
    /// except `self_ip`, preserving the given member order. Each entry is
    /// `ip:port`.
    pub fn compute_join(server_ip: &str, members: &[&str], self_ip: &str, port: u16) -> String {
        std::iter::once(server_ip)
            .chain(members.iter().copied().filter(|m| *m != self_ip))
            .map(|ip| format!("{ip}:{port}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Construct a spec for a Lenovo fleet host using the canonical server,
    /// member set, and port. `network_address` carries the CIDR (e.g.
    /// `172.16.3.96/23`); the bare IP is derived from it.
    pub fn for_lenserv(hostname: impl Into<String>, network_address: impl Into<String>) -> Self {
        let hostname = hostname.into();
        let network_address = network_address.into();
        let ip = Self::ip_without_cidr(&network_address).to_string();
        let cockroach_advertise = Self::compute_advertise(&ip, COCKROACH_PORT);
        let cockroach_join =
            Self::compute_join(COCKROACH_SERVER_IP, LENSERV_MEMBER_IPS, &ip, COCKROACH_PORT);
        Self {
            hostname,
            network_address,
            cockroach_advertise,
            cockroach_join,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_without_cidr_strips_suffix() {
        assert_eq!(HostSpec::ip_without_cidr("172.16.3.96/23"), "172.16.3.96");
        assert_eq!(HostSpec::ip_without_cidr("172.16.3.96"), "172.16.3.96");
    }

    #[test]
    fn join_puts_server_first_and_excludes_self() {
        // 003 = .96 → server, .92, .94
        let join = HostSpec::compute_join(
            COCKROACH_SERVER_IP,
            LENSERV_MEMBER_IPS,
            "172.16.3.96",
            COCKROACH_PORT,
        );
        assert_eq!(
            join,
            "172.16.2.30:36357,172.16.3.92:36357,172.16.3.94:36357"
        );
    }

    #[test]
    fn for_lenserv_matches_known_hosts() {
        // These are the exact strings verified against the live deployment.
        let s1 = HostSpec::for_lenserv("len-serv-001", "172.16.3.92/23");
        assert_eq!(s1.cockroach_advertise, "172.16.3.92:36357");
        assert_eq!(
            s1.cockroach_join,
            "172.16.2.30:36357,172.16.3.94:36357,172.16.3.96:36357"
        );

        let s2 = HostSpec::for_lenserv("len-serv-002", "172.16.3.94/23");
        assert_eq!(
            s2.cockroach_join,
            "172.16.2.30:36357,172.16.3.92:36357,172.16.3.96:36357"
        );

        let s3 = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");
        assert_eq!(
            s3.cockroach_join,
            "172.16.2.30:36357,172.16.3.92:36357,172.16.3.94:36357"
        );
    }
}
