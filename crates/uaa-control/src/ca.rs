// file: crates/uaa-control/src/ca.rs
// version: 1.2.0
// guid: 19da2b5c-91d8-4a97-ba3b-de299638e434
// last-edited: 2026-07-14

//! Dedicated install CA (rcgen) — mint/sign agent certs, CRL (Decision 6).
//!
//! Filled by pki PK-01 (this task): the CA lifecycle
//! ([`InstallCa::load_or_create`], [`InstallCa::sign_agent_csr`],
//! [`InstallCa::ca_cert_pem`]). `crates/uaa-control/src/enroll.rs` (also PK-01) is the
//! only caller of this module. PK-03 extends this same file NEXT, serialized behind
//! this task (collision row: CT-01 stub → PK-01 → PK-03) — it adds
//! `issue_service`/CRL publish and REUSES [`InstallCa`] rather than writing a second
//! CA type; the `pub` surface here is kept intentionally minimal and stable for that.
//!
//! 2026-07-14: added [`InstallCa::issue_server_cert`] — a server-leaf (not
//! agent-leaf) mint used by `listeners.rs` to TLS-terminate the `:15000` operator
//! plane so it can sit behind the Cloudflare Tunnel origin
//! (`~/repos/temp/cloudflare-one/HANDOFF.md` §1/§4.3). Reuses [`InstallCa`] per the
//! note above rather than adding a second CA type.
//!
//! # NEVER the CockroachDB CA (Decision 6, locked)
//!
//! This is a dedicated keypair generated once and persisted under a
//! caller-supplied directory (production default `/var/lib/uaa/ca/`), loaded ONLY by
//! uaa-control. No code path in this file reads any database-server CA material —
//! the registry's CA (used to secure the SQL wire protocol) and this install CA
//! (used to mint agent/service identity certs) are two completely separate,
//! non-overlapping trust roots.
//!
//! # Custody (spec Decision 6 "Repair")
//!
//! An offline, encrypted backup of `ca.key` plus a documented restore procedure is a
//! P0 ship-gate (M3) — that runbook is a coordinator-owned documentation deliverable,
//! explicitly NOT built in this file. What this module guarantees mechanically:
//! `ca.key` is written `0600`, the CA directory `0700`, and — critically — an
//! existing CA is always LOADED, never regenerated (regenerating would silently
//! orphan every certificate already issued against the old key).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use chrono::{Datelike, Duration as ChronoDuration, Utc};
use rcgen::{
    date_time_ymd, BasicConstraints, Certificate, CertificateParams,
    CertificateSigningRequestParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, Ia5String,
    IsCa, KeyPair, KeyUsagePurpose, SanType, PKCS_ECDSA_P256_SHA256,
};

const CA_KEY_FILE: &str = "ca.key";
const CA_CERT_FILE: &str = "ca.crt";
/// Owner-only directory mode for the CA's home (spec Decision 6).
const CA_DIR_MODE: u32 = 0o700;
/// Owner-only file mode for the CA private key (spec Decision 6).
const CA_KEY_MODE: u32 = 0o600;
/// World-readable mode for the CA certificate (it is the public trust anchor agents
/// pin — distributing it widely is the point; only the key is secret).
const CA_CERT_MODE: u32 = 0o644;
/// Agent certificate lifetime (spec Decision 6 / C6): 90 days.
const AGENT_CERT_LIFETIME_DAYS: i64 = 90;
/// Install CA root lifetime. Long-lived by design — rotating the root itself is an
/// operator-driven backup/restore event (the M3 ship-gate runbook), not code here.
const CA_LIFETIME_DAYS: i64 = 3650;

/// A loaded (or freshly created) install CA: keypair plus self-signed root, kept
/// in-process as an [`rcgen::Certificate`] so [`InstallCa::sign_agent_csr`] can issue
/// against it without re-reading the filesystem per call.
///
/// NEVER the CockroachDB CA (Decision 6) — see the module doc.
pub struct InstallCa {
    key_pair: KeyPair,
    /// In-memory issuer identity (distinguished name / key-id method / key usages)
    /// used to sign children — reconstructed on load, kept from creation otherwise.
    cert: Certificate,
    /// The exact PEM bytes persisted on disk (or just written on create) — this is
    /// the authoritative trust-anchor text served to agents/operators, independent of
    /// whatever a re-signed in-memory [`Certificate`] would currently serialize to.
    cert_pem: String,
}

impl InstallCa {
    /// Load an existing CA from `ca_dir`, or create one on first use.
    ///
    /// `ca_dir` is ALWAYS a constructor parameter — never hard-coded — so tests use
    /// `tempfile::tempdir()`; the production default is `/var/lib/uaa/ca/`.
    ///
    /// Create-if-absent: when `ca.key`/`ca.crt` are both present they are loaded
    /// VERBATIM (never regenerated). On first creation, `ca_dir` is made `0700`,
    /// `ca.key` is written `0600`, `ca.crt` `0644`.
    pub fn load_or_create(ca_dir: &Path) -> anyhow::Result<InstallCa> {
        let key_path = ca_dir.join(CA_KEY_FILE);
        let cert_path = ca_dir.join(CA_CERT_FILE);

        if key_path.is_file() && cert_path.is_file() {
            Self::load(&key_path, &cert_path)
        } else {
            Self::create(ca_dir, &key_path, &cert_path)
        }
    }

    fn load(key_path: &Path, cert_path: &Path) -> anyhow::Result<InstallCa> {
        let key_pem = fs::read_to_string(key_path)?;
        let key_pair = KeyPair::from_pem(&key_pem).map_err(|e| {
            anyhow::anyhow!("install CA key at {} is corrupt: {e}", key_path.display())
        })?;

        let cert_pem = fs::read_to_string(cert_path)?;
        let params = CertificateParams::from_ca_cert_pem(&cert_pem).map_err(|e| {
            anyhow::anyhow!("install CA cert at {} is corrupt: {e}", cert_path.display())
        })?;
        // Reconstruct an in-memory issuer `Certificate` from the loaded params/key —
        // used only for its subject/key-id/key-usage identity when signing children,
        // never written back to disk (the persisted `cert_pem` above stays the
        // authoritative trust-anchor text).
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| anyhow::anyhow!("failed to reconstruct install CA identity: {e}"))?;

        Ok(InstallCa {
            key_pair,
            cert,
            cert_pem,
        })
    }

    fn create(ca_dir: &Path, key_path: &Path, cert_path: &Path) -> anyhow::Result<InstallCa> {
        fs::create_dir_all(ca_dir)?;
        fs::set_permissions(ca_dir, fs::Permissions::from_mode(CA_DIR_MODE))?;

        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| anyhow::anyhow!("install CA keypair generation failed: {e}"))?;

        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "uaa install CA");
        params.distinguished_name = dn;
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        set_validity_days(&mut params, CA_LIFETIME_DAYS);

        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| anyhow::anyhow!("install CA self-signing failed: {e}"))?;
        let cert_pem = cert.pem();
        let key_pem = key_pair.serialize_pem();

        write_owner_mode(key_path, key_pem.as_bytes(), CA_KEY_MODE)?;
        write_owner_mode(cert_path, cert_pem.as_bytes(), CA_CERT_MODE)?;

        Ok(InstallCa {
            key_pair,
            cert,
            cert_pem,
        })
    }

    /// Sign `csr_pem` into a 90-day agent certificate whose SAN is EXACTLY the DNS
    /// `hostname` plus the URI `uaa-mac:<mac>` — the server-approved identity, not
    /// whatever extensions the CSR itself happened to carry. `rcgen` verifies the
    /// CSR's self-signature (proof of possession) as part of parsing; a CSR that does
    /// not verify is rejected here, before anything is signed.
    ///
    /// NEVER the CockroachDB CA — this always signs with THIS install CA's key.
    pub fn sign_agent_csr(
        &self,
        csr_pem: &str,
        hostname: &str,
        mac: &str,
    ) -> anyhow::Result<String> {
        let mut csr = CertificateSigningRequestParams::from_pem(csr_pem)
            .map_err(|e| anyhow::anyhow!("invalid CSR: {e}"))?;

        let dns = Ia5String::try_from(hostname.to_string())
            .map_err(|e| anyhow::anyhow!("hostname is not a valid SAN string: {e}"))?;
        let mac_uri = Ia5String::try_from(format!("uaa-mac:{mac}"))
            .map_err(|e| anyhow::anyhow!("mac is not a valid SAN string: {e}"))?;
        csr.params.subject_alt_names = vec![SanType::DnsName(dns), SanType::URI(mac_uri)];
        csr.params.is_ca = IsCa::NoCa;
        set_validity_days(&mut csr.params, AGENT_CERT_LIFETIME_DAYS);

        let signed = csr
            .signed_by(&self.cert, &self.key_pair)
            .map_err(|e| anyhow::anyhow!("signing agent certificate failed: {e}"))?;
        Ok(signed.pem())
    }

    /// The install CA's own certificate, PEM-encoded — the trust anchor served to
    /// agents/operators. NEVER the CockroachDB CA.
    pub fn ca_cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// Mint a fresh TLS server-leaf cert (SAN = `names`, `id-kp-serverAuth`) signed
    /// by this install CA. Returns `(cert_pem, key_pem)`.
    ///
    /// Unlike [`InstallCa::sign_agent_csr`], this generates its own keypair rather
    /// than signing a caller-supplied CSR — there is no remote agent proving
    /// possession here, just this same process about to terminate TLS with the
    /// result. Nothing is persisted to disk: the caller (`listeners.rs`) holds both
    /// PEMs in memory for the process lifetime, so a fresh leaf is minted on every
    /// restart. The Decision 6 custody/backup requirements apply to the CA root
    /// (`ca.key`) only, never to these short-lived leaves.
    ///
    /// `names` may mix DNS names and IP addresses (`rcgen::CertificateParams::new`
    /// sniffs each string and files it under the right `SanType` automatically).
    ///
    /// NEVER the CockroachDB CA — this always signs with THIS install CA's key.
    pub fn issue_server_cert(&self, names: &[String]) -> anyhow::Result<(String, String)> {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| anyhow::anyhow!("server cert keypair generation failed: {e}"))?;

        let mut params = CertificateParams::new(names.to_vec())
            .map_err(|e| anyhow::anyhow!("invalid server cert SAN list {names:?}: {e}"))?;
        let mut dn = DistinguishedName::new();
        dn.push(
            DnType::CommonName,
            names.first().map(String::as_str).unwrap_or("uaa-control"),
        );
        params.distinguished_name = dn;
        params.is_ca = IsCa::NoCa;
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        set_validity_days(&mut params, AGENT_CERT_LIFETIME_DAYS);

        let cert = params
            .signed_by(&key_pair, &self.cert, &self.key_pair)
            .map_err(|e| anyhow::anyhow!("signing server certificate failed: {e}"))?;

        Ok((cert.pem(), key_pair.serialize_pem()))
    }
}

/// Set `params.not_before`/`params.not_after` to a `[now, now + days]` window, at
/// day granularity (`rcgen::date_time_ymd`) — plenty precise for a 90-day agent cert
/// or a multi-year CA root, and avoids depending on the `time` crate directly (it is
/// only a transitive dependency of `rcgen`, pulled in via type inference here).
fn set_validity_days(params: &mut CertificateParams, days: i64) {
    let now = Utc::now();
    let end = now + ChronoDuration::days(days);
    params.not_before = date_time_ymd(now.year(), now.month() as u8, now.day() as u8);
    params.not_after = date_time_ymd(end.year(), end.month() as u8, end.day() as u8);
}

/// Write `contents` to `path` with `mode`, creating/truncating as needed, then
/// re-assert the mode via `set_permissions` (belt-and-suspenders against an
/// umask-affected `OpenOptions::mode` on some platforms — mirrors the pattern
/// already used by `crate::audit`/`crate::db::store`).
fn write_owner_mode(path: &Path, contents: &[u8], mode: u32) -> anyhow::Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)?;
    f.write_all(contents)?;
    f.flush()?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use uaa_core::pki::{generate_keypair_and_csr, AgentIdentity};
    use x509_parser::pem::parse_x509_pem;

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn dir_mode_of(path: &Path) -> u32 {
        mode_of(path)
    }

    fn test_csr() -> (String, AgentIdentity) {
        let identity = AgentIdentity {
            hostname: "testhost".to_string(),
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
        };
        let (_key_pem, csr_pem) = generate_keypair_and_csr(&identity).unwrap();
        (csr_pem, identity)
    }

    #[test]
    fn test_ca_load_or_create_idempotent() {
        let dir = tempdir().unwrap();
        let ca_dir = dir.path().join("ca");

        let first = InstallCa::load_or_create(&ca_dir).unwrap();
        let first_pem = first.ca_cert_pem().to_string();

        assert_eq!(dir_mode_of(&ca_dir), 0o700, "CA dir must be 0700");
        assert_eq!(
            mode_of(&ca_dir.join("ca.key")),
            0o600,
            "ca.key must be 0600"
        );
        assert_eq!(
            mode_of(&ca_dir.join("ca.crt")),
            0o644,
            "ca.crt must be 0644"
        );

        // Second load must return the IDENTICAL cert text — never regenerated (a
        // regenerated CA would orphan every already-issued cert).
        let second = InstallCa::load_or_create(&ca_dir).unwrap();
        assert_eq!(
            second.ca_cert_pem(),
            first_pem,
            "load must never regenerate the CA"
        );
    }

    #[test]
    fn test_ca_cert_pem_is_not_a_second_registry_trust_root() {
        let dir = tempdir().unwrap();
        let ca = InstallCa::load_or_create(&dir.path().join("ca")).unwrap();
        assert!(ca.ca_cert_pem().contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn test_approve_signs_90d_san() {
        let dir = tempdir().unwrap();
        let ca = InstallCa::load_or_create(&dir.path().join("ca")).unwrap();
        let (csr_pem, identity) = test_csr();

        let cert_pem = ca
            .sign_agent_csr(&csr_pem, &identity.hostname, &identity.mac)
            .unwrap();

        let (_, pem) = parse_x509_pem(cert_pem.as_bytes()).unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents).unwrap();
        let validity = cert.validity();
        let lifetime_days =
            (validity.not_after.timestamp() - validity.not_before.timestamp()) / 86_400;
        assert!(
            (89..=91).contains(&lifetime_days),
            "expected ~90d lifetime, got {lifetime_days}d"
        );

        let mut found_dns = false;
        let mut found_uri = false;
        for ext in cert.extensions() {
            if let x509_parser::extensions::ParsedExtension::SubjectAlternativeName(san) =
                ext.parsed_extension()
            {
                for name in &san.general_names {
                    match name {
                        x509_parser::extensions::GeneralName::DNSName(dns)
                            if *dns == identity.hostname =>
                        {
                            found_dns = true
                        }
                        x509_parser::extensions::GeneralName::URI(uri)
                            if *uri == format!("uaa-mac:{}", identity.mac) =>
                        {
                            found_uri = true
                        }
                        _ => {}
                    }
                }
            }
        }
        assert!(found_dns, "expected DNS SAN = hostname");
        assert!(found_uri, "expected URI SAN = uaa-mac:<mac>");
    }

    #[test]
    fn test_issue_server_cert_has_serverauth_eku_and_san() {
        let dir = tempdir().unwrap();
        let ca = InstallCa::load_or_create(&dir.path().join("ca")).unwrap();

        let (cert_pem, key_pem) = ca
            .issue_server_cert(&["uaa.jdfalk.com".to_string(), "172.16.2.30".to_string()])
            .unwrap();

        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(key_pem.contains("BEGIN PRIVATE KEY"));

        let (_, pem) = parse_x509_pem(cert_pem.as_bytes()).unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents).unwrap();

        // Leaf, not a CA — must not be usable to mint further certs.
        assert!(!cert
            .basic_constraints()
            .unwrap()
            .map(|bc| bc.value.ca)
            .unwrap_or(false));

        let mut found_dns = false;
        let mut found_ip = false;
        let mut found_server_auth = false;
        for ext in cert.extensions() {
            match ext.parsed_extension() {
                x509_parser::extensions::ParsedExtension::SubjectAlternativeName(san) => {
                    for name in &san.general_names {
                        match name {
                            x509_parser::extensions::GeneralName::DNSName(dns)
                                if *dns == "uaa.jdfalk.com" =>
                            {
                                found_dns = true
                            }
                            x509_parser::extensions::GeneralName::IPAddress(ip)
                                if *ip == [172, 16, 2, 30] =>
                            {
                                found_ip = true
                            }
                            _ => {}
                        }
                    }
                }
                x509_parser::extensions::ParsedExtension::ExtendedKeyUsage(eku) => {
                    found_server_auth = eku.server_auth;
                }
                _ => {}
            }
        }
        assert!(found_dns, "expected DNS SAN uaa.jdfalk.com");
        assert!(found_ip, "expected IP SAN 172.16.2.30");
        assert!(found_server_auth, "expected id-kp-serverAuth EKU");

        // Must chain to the same CA that signed agent certs, not a stray root.
        let (_, ca_pem_parsed) = parse_x509_pem(ca.ca_cert_pem().as_bytes()).unwrap();
        let (_, ca_cert) = x509_parser::parse_x509_certificate(&ca_pem_parsed.contents).unwrap();
        assert!(cert.verify_signature(Some(ca_cert.public_key())).is_ok());
    }

    #[test]
    fn test_sign_agent_csr_rejects_garbage() {
        let dir = tempdir().unwrap();
        let ca = InstallCa::load_or_create(&dir.path().join("ca")).unwrap();
        let result = ca.sign_agent_csr("not a csr", "host", "aa:bb:cc:dd:ee:ff");
        assert!(result.is_err());
    }

    /// Production-dominant path: `create` runs ONCE ever; every subsequent daemon
    /// start hits `load`, which reconstructs the in-memory issuer via
    /// `from_ca_cert_pem(...).self_signed(&key_pair)`. This proves that
    /// reconstruction actually chains correctly — a cert signed AFTER a fresh
    /// `load_or_create` (second process, same `ca_dir`) must validate against the
    /// SAME persisted `ca_cert_pem` and carry the right SANs.
    #[test]
    fn test_sign_agent_csr_after_reload_chains_to_persisted_ca() {
        let dir = tempdir().unwrap();
        let ca_dir = dir.path().join("ca");

        // First "process": create the CA, note its persisted trust-anchor text.
        let created = InstallCa::load_or_create(&ca_dir).unwrap();
        let persisted_ca_pem = created.ca_cert_pem().to_string();
        drop(created);

        // Second "process": load (never regenerate) the SAME CA from disk.
        let loaded = InstallCa::load_or_create(&ca_dir).unwrap();
        assert_eq!(
            loaded.ca_cert_pem(),
            persisted_ca_pem,
            "load must not regenerate"
        );

        let (csr_pem, identity) = test_csr();
        let cert_pem = loaded
            .sign_agent_csr(&csr_pem, &identity.hostname, &identity.mac)
            .unwrap();

        // The child must verify against the PERSISTED CA cert (not just whatever
        // `loaded` happens to hold in memory) — parse both and check the issuer's
        // public key signed the child.
        let (_, ca_pem_parsed) = parse_x509_pem(persisted_ca_pem.as_bytes()).unwrap();
        let (_, ca_cert) = x509_parser::parse_x509_certificate(&ca_pem_parsed.contents).unwrap();
        let (_, child_pem) = parse_x509_pem(cert_pem.as_bytes()).unwrap();
        let (_, child_cert) = x509_parser::parse_x509_certificate(&child_pem.contents).unwrap();
        assert!(
            child_cert
                .verify_signature(Some(ca_cert.public_key()))
                .is_ok(),
            "cert signed by a RELOADED CA must verify against the persisted ca.crt"
        );

        let mut found_dns = false;
        for ext in child_cert.extensions() {
            if let x509_parser::extensions::ParsedExtension::SubjectAlternativeName(san) =
                ext.parsed_extension()
            {
                for name in &san.general_names {
                    if let x509_parser::extensions::GeneralName::DNSName(dns) = name {
                        if *dns == identity.hostname {
                            found_dns = true;
                        }
                    }
                }
            }
        }
        assert!(
            found_dns,
            "reloaded-CA-signed cert must still carry the hostname SAN"
        );
    }
}
