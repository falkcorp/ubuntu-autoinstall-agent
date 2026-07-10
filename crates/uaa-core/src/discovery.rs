// file: crates/uaa-core/src/discovery.rs
// version: 1.1.0
// guid: 6d9252f2-a5f5-4b27-b92f-162ada48e6c0
// last-edited: 2026-07-10

//! Service discovery over mDNS (`_uaa._tcp.local.`) with static fallback.
//!
//! Implements spec Decision 11 (`docs/specs/constellation-design.md`, sketch C1):
//!
//! - [`resolve`] returns the **union** of mDNS-browsed and statically
//!   configured candidates — mDNS candidates first (browse order), then any
//!   static candidate whose `(host, port)` was not already seen. The static
//!   list is *always* included, not only when the mDNS browse comes back
//!   empty (per-endpoint-failure fallback, never only-on-empty-browse): a
//!   stale mDNS advertisement must never mask a valid static entry. Callers
//!   iterate the returned candidates under mTLS and accept the first that
//!   authenticates.
//! - An empty union is a **hard error**, never a guess (fail-closed): see
//!   [`ensure_nonempty`].
//! - [`advertise`] is a library function called **daemons only** — the
//!   client-only `uaa` CLI ships browse-only and must never call it (this
//!   cannot be enforced by the type system here, only documented).
//!
//! The mDNS transport is [`mdns_sd`] (pure Rust); no other mDNS/zeroconf
//! crate may be introduced. Because mDNS multicast cannot run
//! deterministically under CI (no multicast in the test sandbox), the
//! union/parse logic lives in plain sync functions ([`union_candidates`],
//! [`static_candidates`], [`ensure_nonempty`]) that are unit-tested
//! directly; [`resolve`] is a thin async composition of [`browse_mdns`] and
//! those sync helpers.

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{AutoInstallError, Result};

/// mDNS service type advertised/browsed by every `uaa` component.
const SERVICE_TYPE: &str = "_uaa._tcp.local.";

/// Canonical location of the static endpoints fallback file. Documented here
/// for error messages; callers may load from any path via
/// [`EndpointsFile::load_from`].
pub const DEFAULT_ENDPOINTS_PATH: &str = "/etc/uaa/endpoints.yaml";

/// Version used for static endpoints that predate version knowledge.
const DEFAULT_STATIC_VERSION: &str = "0.0.0";

/// The three service kinds advertised/discovered on `_uaa._tcp.local.`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceKind {
    Control,
    Web,
    Pxe,
}

impl ServiceKind {
    /// TXT record / static-endpoints-file spelling for this kind.
    pub fn as_txt(&self) -> &'static str {
        match self {
            ServiceKind::Control => "control",
            ServiceKind::Web => "web",
            ServiceKind::Pxe => "pxe",
        }
    }

    /// Parse the TXT record / static-endpoints-file spelling of a kind.
    pub fn from_txt(value: &str) -> Option<Self> {
        match value {
            "control" => Some(ServiceKind::Control),
            "web" => Some(ServiceKind::Web),
            "pxe" => Some(ServiceKind::Pxe),
            _ => None,
        }
    }
}

/// Where a [`ServiceInfo`] candidate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Mdns,
    Static,
}

/// A single discovered (or statically configured) service endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInfo {
    pub service: ServiceKind,
    pub version: semver::Version,
    pub host: IpAddr,
    pub port: u16,
    pub source: Source,
}

/// One row of `/etc/uaa/endpoints.yaml`.
///
/// `version` is optional in the file and defaults to `"0.0.0"` — static
/// entries may predate version knowledge.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StaticEndpoint {
    pub service: String,
    pub host: String,
    pub port: u16,
    #[serde(default = "default_static_version")]
    pub version: String,
}

fn default_static_version() -> String {
    DEFAULT_STATIC_VERSION.to_string()
}

/// Parsed `/etc/uaa/endpoints.yaml`.
///
/// An absent file is legal and loads as an empty list (`EndpointsFile::default()`);
/// the union with mDNS candidates may still be non-empty in that case. A
/// present-but-unparseable file (bad YAML or an unknown field) is a hard
/// [`AutoInstallError::ConfigError`] — fail-closed, never a silent empty list.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointsFile {
    #[serde(default)]
    pub endpoints: Vec<StaticEndpoint>,
}

impl EndpointsFile {
    /// Load `path`, treating a missing file as legal-and-empty and any parse
    /// failure as a fail-closed [`AutoInstallError::ConfigError`].
    pub fn load_from(path: &Path) -> Result<Self> {
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(err) => {
                return Err(AutoInstallError::ConfigError(format!(
                    "failed to read static endpoints file {}: {err}",
                    path.display()
                )));
            }
        };

        serde_yaml::from_str(&contents).map_err(|err| {
            AutoInstallError::ConfigError(format!(
                "failed to parse static endpoints file {}: {err}",
                path.display()
            ))
        })
    }
}

/// Handle to a registered mDNS advertisement. Dropping it unregisters the
/// service (best-effort — failures are logged, never panicked on).
pub struct DiscoveryHandle {
    daemon: mdns_sd::ServiceDaemon,
    fullname: String,
}

impl Drop for DiscoveryHandle {
    fn drop(&mut self) {
        if let Err(err) = self.daemon.unregister(&self.fullname) {
            tracing::warn!(
                "failed to unregister mDNS service {}: {err}",
                self.fullname
            );
        }
    }
}

/// Advertise this host's service over mDNS (`_uaa._tcp.local.`) with TXT
/// records `service`/`version`/`port`.
///
/// Callable by **daemons only** — the client-only `uaa` CLI ships
/// browse-only (spec Decision 11); this function must never be called from
/// CLI code paths. The type system cannot enforce that here, only document
/// it.
pub async fn advertise(info: &ServiceInfo) -> Result<DiscoveryHandle> {
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|err| {
        AutoInstallError::NetworkError(format!("failed to start mDNS daemon: {err}"))
    })?;

    let host_label = info.host.to_string().replace(['.', ':'], "-");
    let instance_name = format!("uaa-{}-{host_label}", info.service.as_txt());
    let hostname = format!("{instance_name}.local.");

    let version_str = info.version.to_string();
    let port_str = info.port.to_string();
    let properties = [
        ("service", info.service.as_txt()),
        ("version", version_str.as_str()),
        ("port", port_str.as_str()),
    ];

    let mdns_info = mdns_sd::ServiceInfo::new(
        SERVICE_TYPE,
        &instance_name,
        &hostname,
        info.host,
        info.port,
        &properties[..],
    )
    .map_err(|err| {
        AutoInstallError::NetworkError(format!("failed to build mDNS service info: {err}"))
    })?;

    let fullname = mdns_info.get_fullname().to_string();

    daemon.register(mdns_info).map_err(|err| {
        AutoInstallError::NetworkError(format!("failed to register mDNS service: {err}"))
    })?;

    Ok(DiscoveryHandle { daemon, fullname })
}

/// Browse `_uaa._tcp.local.` for `kind` until `timeout` elapses, returning
/// every candidate whose TXT `service` matches. Never aborts on a bad
/// candidate — an unparseable `version` is skipped with a warning. Daemon
/// spawn/browse failures degrade to an empty result with a warning so the
/// static fallback path still works on hosts without multicast; they are
/// NOT propagated as errors (that would defeat the fallback's purpose).
async fn browse_mdns(kind: ServiceKind, timeout: Duration) -> Vec<ServiceInfo> {
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(daemon) => daemon,
        Err(err) => {
            tracing::warn!(
                "mDNS daemon unavailable ({err}); falling back to static endpoints only"
            );
            return Vec::new();
        }
    };

    let receiver = match daemon.browse(SERVICE_TYPE) {
        Ok(receiver) => receiver,
        Err(err) => {
            tracing::warn!("mDNS browse failed to start ({err}); falling back to static endpoints only");
            let _ = daemon.shutdown();
            return Vec::new();
        }
    };

    let mut candidates = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, receiver.recv_async()).await {
            Ok(Ok(mdns_sd::ServiceEvent::ServiceResolved(resolved))) => {
                if let Some(candidate) = resolved_to_candidate(kind, &resolved) {
                    candidates.push(candidate);
                }
            }
            Ok(Ok(_other_event)) => continue,
            Ok(Err(_channel_closed)) => break,
            Err(_elapsed) => break,
        }
    }

    let _ = daemon.shutdown();
    candidates
}

/// Convert a resolved mDNS `ServiceInfo` into our candidate type, filtering
/// on the `service` TXT record (foreign `_uaa._tcp` instances) and skipping
/// (with a warning) candidates whose `version` TXT does not parse as semver.
fn resolved_to_candidate(kind: ServiceKind, resolved: &mdns_sd::ServiceInfo) -> Option<ServiceInfo> {
    let service_txt = resolved.get_property_val_str("service")?;
    if service_txt != kind.as_txt() {
        return None;
    }

    let version_txt = resolved
        .get_property_val_str("version")
        .unwrap_or(DEFAULT_STATIC_VERSION);
    let version = match semver::Version::parse(version_txt) {
        Ok(version) => version,
        Err(err) => {
            tracing::warn!(
                "skipping mDNS candidate {} with unparseable version {version_txt:?}: {err}",
                resolved.get_fullname()
            );
            return None;
        }
    };

    let host = resolved.get_addresses().iter().next().copied()?;

    Some(ServiceInfo {
        service: kind,
        version,
        host,
        port: resolved.get_port(),
        source: Source::Mdns,
    })
}

/// Filter `file` to `kind`, labeling every hit `Source::Static`. Unparseable
/// hosts are skipped with a warning (never abort). A missing `version` field
/// already defaulted to `"0.0.0"` by serde; a present-but-garbage version is
/// likewise skipped with a warning rather than aborting the whole load.
pub fn static_candidates(kind: ServiceKind, file: &EndpointsFile) -> Vec<ServiceInfo> {
    file.endpoints
        .iter()
        .filter(|entry| entry.service == kind.as_txt())
        .filter_map(|entry| {
            let host: IpAddr = match entry.host.parse() {
                Ok(host) => host,
                Err(err) => {
                    tracing::warn!(
                        "skipping static endpoint with unparseable host {:?}: {err}",
                        entry.host
                    );
                    return None;
                }
            };
            let version = match semver::Version::parse(&entry.version) {
                Ok(version) => version,
                Err(err) => {
                    tracing::warn!(
                        "skipping static endpoint with unparseable version {:?}: {err}",
                        entry.version
                    );
                    return None;
                }
            };
            Some(ServiceInfo {
                service: kind,
                version,
                host,
                port: entry.port,
                source: Source::Static,
            })
        })
        .collect()
}

/// Merge mDNS and static candidates into the union `resolve()` returns:
/// every mDNS candidate first (browse order), then every static candidate
/// whose `(host, port)` was not already present. Duplicates keep the mDNS
/// entry (it wins the `source` label). A static candidate is NEVER dropped
/// just because the browse returned something — the per-endpoint-failure
/// fallback loop belongs to the caller; this function's only job is to hand
/// over the full union.
pub fn union_candidates(mdns: Vec<ServiceInfo>, statics: Vec<ServiceInfo>) -> Vec<ServiceInfo> {
    let mut seen: HashSet<(IpAddr, u16)> = HashSet::with_capacity(mdns.len() + statics.len());
    let mut union = Vec::with_capacity(mdns.len() + statics.len());

    for candidate in mdns {
        seen.insert((candidate.host, candidate.port));
        union.push(candidate);
    }
    for candidate in statics {
        if seen.insert((candidate.host, candidate.port)) {
            union.push(candidate);
        }
    }

    union
}

/// Fail-closed post-condition shared by [`resolve`]: an empty candidate
/// union is always a hard error, never a guess. Kept as a small sync
/// function so it can be unit-tested without any async runtime or network.
fn ensure_nonempty(
    kind: ServiceKind,
    timeout: Duration,
    union: Vec<ServiceInfo>,
) -> Result<Vec<ServiceInfo>> {
    if union.is_empty() {
        return Err(AutoInstallError::NetworkError(format!(
            "resolve({kind:?}) found no mDNS answers and no static endpoints after {timeout:?} \
             (checked default static config {DEFAULT_ENDPOINTS_PATH}); resolve() never returns a guess"
        )));
    }
    Ok(union)
}

/// Resolve every known candidate for `kind`: the union of an mDNS browse
/// (up to `timeout`) and `static_fallback`'s entries for `kind`. mDNS
/// candidates come first in browse order; static candidates not already
/// present by `(host, port)` are always appended (per-endpoint-failure
/// fallback — never only-on-empty-browse). An empty union is a hard
/// [`AutoInstallError::NetworkError`], never `Ok(vec![])`.
pub async fn resolve(
    kind: ServiceKind,
    static_fallback: &EndpointsFile,
    timeout: Duration,
) -> Result<Vec<ServiceInfo>> {
    let mdns = browse_mdns(kind, timeout).await;
    let statics = static_candidates(kind, static_fallback);
    let union = union_candidates(mdns, statics);
    ensure_nonempty(kind, timeout, union)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn v(version: &str) -> semver::Version {
        semver::Version::parse(version).unwrap()
    }

    fn mdns_candidate(kind: ServiceKind, host: [u8; 4], port: u16) -> ServiceInfo {
        ServiceInfo {
            service: kind,
            version: v("1.0.0"),
            host: IpAddr::V4(Ipv4Addr::from(host)),
            port,
            source: Source::Mdns,
        }
    }

    fn static_candidate(kind: ServiceKind, host: [u8; 4], port: u16) -> ServiceInfo {
        ServiceInfo {
            service: kind,
            version: v("0.0.0"),
            host: IpAddr::V4(Ipv4Addr::from(host)),
            port,
            source: Source::Static,
        }
    }

    #[test]
    fn test_union_mdns_first_then_static() {
        let mdns = vec![mdns_candidate(ServiceKind::Control, [10, 0, 0, 1], 8443)];
        let statics = vec![static_candidate(ServiceKind::Control, [10, 0, 0, 2], 8443)];

        let union = union_candidates(mdns, statics);

        assert_eq!(union.len(), 2);
        assert_eq!(union[0].source, Source::Mdns);
        assert_eq!(union[0].host, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(union[1].source, Source::Static);
        assert_eq!(union[1].host, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
    }

    #[test]
    fn test_union_dedupes_same_host_port() {
        let mdns = vec![mdns_candidate(ServiceKind::Web, [10, 0, 0, 5], 443)];
        let statics = vec![static_candidate(ServiceKind::Web, [10, 0, 0, 5], 443)];

        let union = union_candidates(mdns, statics);

        assert_eq!(union.len(), 1);
        assert_eq!(union[0].source, Source::Mdns);
    }

    #[test]
    fn test_union_static_survives_nonempty_mdns() {
        // Anti-over-suppression: browse returned a candidate AND a
        // DIFFERENT static entry exists -> the static entry must still be
        // in the union. Fallback is per-endpoint-failure, never
        // only-on-empty-browse.
        let mdns = vec![mdns_candidate(ServiceKind::Pxe, [10, 0, 0, 9], 69)];
        let statics = vec![static_candidate(ServiceKind::Pxe, [10, 0, 0, 10], 69)];

        let union = union_candidates(mdns, statics);

        assert_eq!(union.len(), 2);
        assert!(union
            .iter()
            .any(|c| c.source == Source::Static && c.host == IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10))));
    }

    #[test]
    fn test_static_candidates_filters_kind() {
        let file = EndpointsFile {
            endpoints: vec![
                StaticEndpoint {
                    service: "control".to_string(),
                    host: "10.0.0.20".to_string(),
                    port: 8443,
                    version: "1.2.3".to_string(),
                },
                StaticEndpoint {
                    service: "web".to_string(),
                    host: "10.0.0.21".to_string(),
                    port: 443,
                    version: "1.2.3".to_string(),
                },
            ],
        };

        let web_only = static_candidates(ServiceKind::Web, &file);

        assert_eq!(web_only.len(), 1);
        assert_eq!(web_only[0].service, ServiceKind::Web);
        assert_eq!(web_only[0].host, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 21)));
        assert_eq!(web_only[0].source, Source::Static);
    }

    #[test]
    fn test_endpoints_file_absent_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let missing_path = dir.path().join("does-not-exist.yaml");

        let loaded = EndpointsFile::load_from(&missing_path).unwrap();

        assert!(loaded.endpoints.is_empty());
    }

    #[test]
    fn test_endpoints_file_invalid_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let bad_yaml_path = dir.path().join("bad.yaml");
        std::fs::write(&bad_yaml_path, "endpoints: [ this is not valid: yaml:::").unwrap();

        let err = EndpointsFile::load_from(&bad_yaml_path).unwrap_err();
        assert!(matches!(err, AutoInstallError::ConfigError(_)));

        let unknown_field_path = dir.path().join("unknown-field.yaml");
        std::fs::write(
            &unknown_field_path,
            "endpoints:\n  - service: control\n    host: 10.0.0.1\n    port: 8443\n    bogus: true\n",
        )
        .unwrap();

        let err = EndpointsFile::load_from(&unknown_field_path).unwrap_err();
        assert!(matches!(err, AutoInstallError::ConfigError(_)));
    }

    #[test]
    fn test_resolve_empty_union_is_error() {
        let err = ensure_nonempty(ServiceKind::Control, Duration::from_secs(2), Vec::new())
            .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("Control"));
        assert!(message.contains("no mDNS answers"));
        assert!(message.contains("never returns a guess"));
    }
}
