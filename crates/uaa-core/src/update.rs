// file: crates/uaa-core/src/update.rs
// version: 1.1.0
// guid: 1b780c88-775f-4b79-a5cd-96fe32acdad0
// last-edited: 2026-07-10

//! Signed self-update library (spec C7, Decisions 9/10).
//!
//! Manifest `http://<uaa-web>:8081/uaa/manifest.json` + a detached `.sig`
//! file travel as plain bytes; this module never speaks HTTP itself — every
//! fetch goes through the [`Fetcher`] seam so unit tests inject bytes and
//! production callers (elsewhere) wire in a real HTTP client. No HTTP
//! client crate may appear as a dependency of this file.
//!
//! # Verify order (LOCKED, fail-closed at every step)
//!
//! 1. [`verify_manifest_sig`] — manifest signature must verify under either
//!    embedded pubkey slot (current or next — Decision 10 rotation repair)
//!    BEFORE the manifest bytes are ever parsed as JSON.
//! 2. [`select_entry`] — apply the `min_version` floor / newer-only rules.
//! 3. Download the artifact bytes (via [`Fetcher`]).
//! 4. [`verify_artifact`] — sha256 compared first, then the artifact
//!    signature (same dual-slot rule).
//! 5. [`stage`] — write `<install_path>.new` (0755, fsynced).
//! 6. [`apply`] — atomic two-rename swap (current -> `.prev`, `.new` ->
//!    current). Restart is the CALLER's job, never this module's.
//!
//! No step may be skipped or reordered; any failure at any step is a hard
//! `Err` and nothing downstream of that step runs (in particular: a bad
//! manifest signature must never trigger an artifact download, and a bad
//! sha256/signature must never leave a `.new` file behind).
//!
//! # `min_version` floor semantics
//!
//! A manifest entry is only ever a valid update candidate when
//! `entry.version > current.version` (never a downgrade) AND
//! `entry.version >= manifest.min_version` (the entry meets the manifest's
//! own floor). `current.version >= manifest.min_version` is **not**
//! required — an old binary below the floor must still be allowed to climb
//! up through it. A manifest whose best matching entry is `<=` current is
//! `UpdateOutcome::UpToDate` (not an error) — this makes a replayed
//! old-but-validly-signed manifest's "downgrade" unrepresentable: the
//! candidate is simply below current and gets ignored, never applied.
//! An entry that IS newer than current but still sits below the manifest's
//! own `min_version` floor is a self-inconsistent manifest and is a hard
//! `Err` (never staged, never applied).
//!
//! # Dual-pubkey rotation
//!
//! Every signature (manifest AND artifact) is valid if it verifies under
//! EITHER of the two embedded `VerifyingKey`s (`pubkeys[0]` = current,
//! `pubkeys[1]` = next). Both slots failing is a hard error. This is how a
//! key rotation stages through the fleet before the old key retires
//! (Decision 10); the real update PRIVATE key lives offline in the operator
//! password manager and is never present in this repo.
//!
//! # Hold pin
//!
//! `install_path.with_extension("hold")` existing suppresses
//! `ApplyMode::TimerAuto` with `UpdateOutcome::Held`, checked BEFORE any
//! fetch (zero [`Fetcher`] calls). `ApplyMode::StageOnly` ignores the hold —
//! staging is harmless and apply is manual anyway.
//!
//! # Crash recovery
//!
//! [`apply`] performs two `std::fs::rename` calls on the same filesystem
//! (atomic each, not atomic together): current -> `<install_path>.prev`,
//! then `<install_path>.new` -> current. A crash between the two renames
//! leaves `.prev` AND `.new` present with the current path missing;
//! re-running [`apply`] detects this orphaned-`.new` state and completes
//! the second rename (it does not re-attempt the already-completed first
//! rename).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AutoInstallError;
use crate::Result;

// ── Types ────────────────────────────────────────────────────────────────────

/// One published binary in a [`Manifest`].
///
/// `sha256` is a lowercase hex digest of the artifact bytes; `sig` is the
/// base64 (standard alphabet, padded) encoding of the raw 64-byte ed25519
/// signature over the artifact bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryEntry {
    pub name: String,
    pub version: semver::Version,
    pub target: String,
    /// Lowercase hex sha256 digest of the artifact bytes.
    pub sha256: String,
    /// Base64 (standard, padded) encoding of the raw ed25519 signature over
    /// the artifact bytes.
    pub sig: String,
    pub url: String,
}

/// The fleet update manifest: every published binary plus the global
/// downgrade floor. Travels as JSON with a detached `.sig` file (base64,
/// standard alphabet, padded) covering the manifest bytes exactly as
/// fetched — never the parsed/re-serialized form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub binaries: Vec<BinaryEntry>,
    /// See the module-level "`min_version` floor semantics" section: no
    /// entry below this version is ever a valid update candidate, even for
    /// a `current` far below it.
    pub min_version: semver::Version,
}

/// Identity of the binary calling [`self_update`]: what it is, what version
/// it currently is, and where it lives on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryIdentity {
    pub name: String,
    pub version: semver::Version,
    pub target: String,
    pub install_path: PathBuf,
}

/// Who is allowed to complete the atomic apply step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    /// Fleet agents / CLI: a fully valid update applies immediately
    /// (subject to the hold pin).
    TimerAuto,
    /// The three server daemons: check and stage only; apply happens only
    /// on an explicit operator `--apply` (Decision 9a). The hold pin does
    /// not affect this mode.
    StageOnly,
}

/// Result of one [`self_update`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// No matching entry is newer than `current`; nothing was fetched
    /// beyond the manifest itself.
    UpToDate,
    /// `.hold` marker present and `mode` was `TimerAuto`; no fetch was
    /// performed at all.
    Held,
    /// `<install_path>.new` was written and verified but not applied
    /// (`ApplyMode::StageOnly`).
    Staged(PathBuf),
    /// The atomic rename completed; `prev` holds the path to the
    /// pre-update binary for rollback.
    Applied {
        new_version: semver::Version,
        prev: PathBuf,
    },
}

/// Fetch seam so [`self_update`] never depends on a concrete HTTP client.
/// Production callers wire in a real implementation elsewhere (outside this
/// module); unit tests here use an in-memory recording mock — mirrors the
/// `CommandExecutor` mock idiom used by `network::executor`.
#[async_trait::async_trait]
pub trait Fetcher {
    /// Fetch the bytes at `url`, or an error (never partial success).
    async fn fetch(&self, url: &str) -> Result<Vec<u8>>;
}

// ── Base64 helpers ───────────────────────────────────────────────────────────

/// Encode raw bytes as standard, padded base64 — the wire format for both
/// manifest and artifact signatures. Production code here only ever
/// *verifies* signatures (the manifest/artifact producer, outside this
/// crate, does the encoding); this helper is `cfg(test)` because only the
/// test key-signing helpers below need to go the other way.
#[cfg(test)]
fn base64_encode(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

/// Decode standard, padded base64 text (surrounding whitespace tolerated)
/// back to raw bytes.
fn base64_decode(text: &str) -> std::result::Result<Vec<u8>, base64::DecodeError> {
    BASE64.decode(text.trim())
}

/// Hand-rolled lowercase hex encoder (no `hex` crate needed for 32 bytes).
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Verify `msg` against `sig_b64` under either embedded pubkey slot. `true`
/// iff at least one slot verifies. Uses `verify_strict` (rejects
/// non-canonical signatures / small-order points) rather than plain
/// `verify` — appropriate for a security-critical signature gate.
fn verify_dual(msg: &[u8], sig_b64: &str, pubkeys: &[VerifyingKey; 2]) -> Result<bool> {
    let raw = base64_decode(sig_b64)
        .map_err(|e| AutoInstallError::ValidationError(format!("signature is not valid base64: {e}")))?;
    let signature = Signature::from_slice(&raw)
        .map_err(|e| AutoInstallError::ValidationError(format!("signature has invalid length: {e}")))?;
    Ok(pubkeys
        .iter()
        .any(|pk| pk.verify_strict(msg, &signature).is_ok()))
}

// ── Pipeline step 1: manifest signature ─────────────────────────────────────

/// Verify `manifest_bytes` against `sig_bytes` (the fetched `.sig` file
/// contents — UTF-8 base64 text) under either pubkey slot, THEN parse the
/// manifest JSON. Both slots failing, non-UTF8/non-base64 signature bytes,
/// or invalid JSON are all fail-closed `Err`s; nothing downstream runs.
fn verify_manifest_sig(
    manifest_bytes: &[u8],
    sig_bytes: &[u8],
    pubkeys: &[VerifyingKey; 2],
) -> Result<Manifest> {
    let sig_text = std::str::from_utf8(sig_bytes).map_err(|e| {
        AutoInstallError::ValidationError(format!("manifest signature is not valid UTF-8: {e}"))
    })?;
    if !verify_dual(manifest_bytes, sig_text, pubkeys)? {
        return Err(AutoInstallError::ValidationError(
            "manifest signature invalid under both update keys".to_string(),
        ));
    }
    serde_json::from_slice(manifest_bytes)
        .map_err(|e| AutoInstallError::ValidationError(format!("manifest JSON invalid: {e}")))
}

// ── Pipeline step 2: entry selection + min_version floor ───────────────────

/// Pick the best `name`+`target` match for `current` and apply the
/// `min_version` floor / newer-only rules (see the module-level "`min_version`
/// floor semantics" section — spelled out again here since this is the only
/// function that enforces it):
///
/// - no matching entry, or the best matching entry's version is `<=`
///   `current.version` -> `Ok(None)` (up to date / downgrade unrepresentable,
///   never an error).
/// - the best matching entry's version is `>` `current.version` but `<`
///   `manifest.min_version` -> `Err` (the manifest is self-inconsistent:
///   it advertises an "upgrade" that violates its own floor).
/// - otherwise -> `Ok(Some(entry))`, a valid candidate.
fn select_entry<'a>(
    manifest: &'a Manifest,
    current: &BinaryIdentity,
) -> Result<Option<&'a BinaryEntry>> {
    let candidate = manifest
        .binaries
        .iter()
        .filter(|e| e.name == current.name && e.target == current.target)
        .max_by_key(|e| e.version.clone());

    let Some(entry) = candidate else {
        return Ok(None);
    };

    if entry.version <= current.version {
        return Ok(None);
    }

    if entry.version < manifest.min_version {
        return Err(AutoInstallError::ValidationError(format!(
            "manifest entry {}@{} is below its own min_version floor {}",
            entry.name, entry.version, manifest.min_version
        )));
    }

    Ok(Some(entry))
}

// ── Pipeline step 3: artifact verification ──────────────────────────────────

/// Verify `bytes` (the fetched artifact) against `entry`: sha256 hex digest
/// compared FIRST, then the artifact signature under either pubkey slot.
/// Never writes anything; the error names which check failed.
fn verify_artifact(bytes: &[u8], entry: &BinaryEntry, pubkeys: &[VerifyingKey; 2]) -> Result<()> {
    let digest = hex_encode(&Sha256::digest(bytes));
    if !digest.eq_ignore_ascii_case(&entry.sha256) {
        return Err(AutoInstallError::ValidationError(format!(
            "artifact sha256 mismatch: expected {} got {digest}",
            entry.sha256
        )));
    }

    if !verify_dual(bytes, &entry.sig, pubkeys)? {
        return Err(AutoInstallError::ValidationError(
            "artifact signature invalid under both update keys".to_string(),
        ));
    }

    Ok(())
}

// ── Pipeline step 4: stage ───────────────────────────────────────────────────

/// Write the verified artifact bytes to `<install_path>.new` (mode 0755,
/// fsynced) and return that path. Never touches `install_path` itself.
fn stage(bytes: &[u8], install_path: &Path) -> Result<PathBuf> {
    let new_path = install_path.with_extension("new");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&new_path)?;
    file.write_all(bytes)?;
    file.sync_all()?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(new_path)
}

// ── Pipeline step 5: apply ───────────────────────────────────────────────────

/// Complete the atomic swap: `install_path` -> `<install_path>.prev`, then
/// `<install_path>.new` -> `install_path`. Returns the `.prev` path.
///
/// Also completes an orphaned `.new` left behind by a crash between the two
/// renames (current path missing, `.new` present): in that case the first
/// rename already happened, so this only performs the second.
fn apply(install_path: &Path) -> Result<PathBuf> {
    let new_path = install_path.with_extension("new");
    let prev_path = install_path.with_extension("prev");

    if !install_path.exists() {
        if new_path.exists() {
            // Crash recovery: the first rename already completed.
            std::fs::rename(&new_path, install_path)?;
            return Ok(prev_path);
        }
        return Err(AutoInstallError::ValidationError(format!(
            "apply: neither {} nor its staged .new exist",
            install_path.display()
        )));
    }

    if !new_path.exists() {
        return Err(AutoInstallError::ValidationError(format!(
            "apply: no staged .new file at {}",
            new_path.display()
        )));
    }

    std::fs::rename(install_path, &prev_path)?;
    std::fs::rename(&new_path, install_path)?;
    Ok(prev_path)
}

// ── Composition ──────────────────────────────────────────────────────────────

/// Run the full signed self-update pipeline for `current` against the
/// manifest at `manifest_url` (its detached signature is fetched from
/// `format!("{manifest_url}.sig")`), verifying against `pubkeys` (current +
/// next), and applying `mode`'s semantics. See the module-level docs for the
/// locked, fail-closed verify order.
pub async fn self_update(
    current: &BinaryIdentity,
    manifest_url: &str,
    pubkeys: &[VerifyingKey; 2],
    mode: ApplyMode,
    fetch: &dyn Fetcher,
) -> Result<UpdateOutcome> {
    if mode == ApplyMode::TimerAuto && current.install_path.with_extension("hold").exists() {
        return Ok(UpdateOutcome::Held);
    }

    let manifest_bytes = fetch.fetch(manifest_url).await?;
    let sig_url = format!("{manifest_url}.sig");
    let sig_bytes = fetch.fetch(&sig_url).await?;

    let manifest = verify_manifest_sig(&manifest_bytes, &sig_bytes, pubkeys)?;

    let entry = match select_entry(&manifest, current)? {
        Some(entry) => entry.clone(),
        None => return Ok(UpdateOutcome::UpToDate),
    };

    let artifact_bytes = fetch.fetch(&entry.url).await?;
    verify_artifact(&artifact_bytes, &entry, pubkeys)?;

    let staged = stage(&artifact_bytes, &current.install_path)?;

    match mode {
        ApplyMode::StageOnly => Ok(UpdateOutcome::Staged(staged)),
        ApplyMode::TimerAuto => {
            let prev = apply(&current.install_path)?;
            Ok(UpdateOutcome::Applied {
                new_version: entry.version,
                prev,
            })
        }
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::tempdir;

    /// Recording mock [`Fetcher`]: serves canned bytes by URL and records
    /// every URL requested (in order) so tests can assert exactly which
    /// fetches happened — in particular, that a failed check short-circuits
    /// before any later fetch.
    struct MockFetcher {
        payloads: HashMap<String, Vec<u8>>,
        calls: Mutex<Vec<String>>,
    }

    impl MockFetcher {
        fn new() -> Self {
            Self {
                payloads: HashMap::new(),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn with(mut self, url: &str, bytes: Vec<u8>) -> Self {
            self.payloads.insert(url.to_string(), bytes);
            self
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("mock mutex poisoned").clone()
        }
    }

    #[async_trait::async_trait]
    impl Fetcher for MockFetcher {
        async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
            self.calls
                .lock()
                .expect("mock mutex poisoned")
                .push(url.to_string());
            self.payloads
                .get(url)
                .cloned()
                .ok_or_else(|| AutoInstallError::NetworkError(format!("mock: no payload for {url}")))
        }
    }

    fn throwaway_key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    fn entry(name: &str, version: &str, target: &str, bytes: &[u8], key: &SigningKey) -> BinaryEntry {
        let digest = hex_encode(&Sha256::digest(bytes));
        let sig: Signature = key.sign(bytes);
        BinaryEntry {
            name: name.to_string(),
            version: semver::Version::parse(version).expect("valid semver"),
            target: target.to_string(),
            sha256: digest,
            sig: base64_encode(&sig.to_bytes()),
            url: format!("https://example.invalid/{name}-{version}"),
        }
    }

    /// Build a signed manifest: returns (manifest JSON bytes, detached
    /// signature bytes as they'd be fetched from the `.sig` file).
    fn signed_manifest(
        entries: Vec<BinaryEntry>,
        min_version: &str,
        key: &SigningKey,
    ) -> (Vec<u8>, Vec<u8>) {
        let manifest = Manifest {
            binaries: entries,
            min_version: semver::Version::parse(min_version).expect("valid semver"),
        };
        let bytes = serde_json::to_vec(&manifest).expect("serialize manifest");
        let sig: Signature = key.sign(&bytes);
        (bytes, base64_encode(&sig.to_bytes()).into_bytes())
    }

    fn identity(name: &str, version: &str, target: &str, install_path: &Path) -> BinaryIdentity {
        BinaryIdentity {
            name: name.to_string(),
            version: semver::Version::parse(version).expect("valid semver"),
            target: target.to_string(),
            install_path: install_path.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn test_bad_manifest_sig_rejected() {
        let key = throwaway_key();
        let other_key = throwaway_key();
        let artifact = b"binary-v2".to_vec();
        let entries = vec![entry("uaa-agent", "2.0.0", "x86_64", &artifact, &key)];
        let (manifest_bytes, _valid_sig) = signed_manifest(entries, "1.0.0", &key);
        // Sign with a THIRD key that is in neither pubkey slot -> both slots fail.
        let tampered_sig: Signature = other_key.sign(&manifest_bytes);
        let tampered_sig_bytes = base64_encode(&tampered_sig.to_bytes()).into_bytes();

        let dir = tempdir().expect("tempdir");
        let install_path = dir.path().join("uaa-agent");
        std::fs::write(&install_path, b"binary-v1").expect("write current binary");
        let current = identity("uaa-agent", "1.0.0", "x86_64", &install_path);

        let pubkeys = [key.verifying_key(), throwaway_key().verifying_key()];
        let fetcher = MockFetcher::new()
            .with("https://manifest.invalid/manifest.json", manifest_bytes)
            .with(
                "https://manifest.invalid/manifest.json.sig",
                tampered_sig_bytes,
            );

        let result = self_update(
            &current,
            "https://manifest.invalid/manifest.json",
            &pubkeys,
            ApplyMode::TimerAuto,
            &fetcher,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(
            fetcher.calls(),
            vec![
                "https://manifest.invalid/manifest.json".to_string(),
                "https://manifest.invalid/manifest.json.sig".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn test_second_slot_key_accepted() {
        let current_key = throwaway_key();
        let next_key = throwaway_key();
        let artifact = b"binary-v2".to_vec();
        let entries = vec![entry("uaa-agent", "2.0.0", "x86_64", &artifact, &next_key)];
        // Manifest signed by the NEXT ("slot 1") key.
        let (manifest_bytes, sig_bytes) = signed_manifest(entries, "1.0.0", &next_key);

        let dir = tempdir().expect("tempdir");
        let install_path = dir.path().join("uaa-agent");
        std::fs::write(&install_path, b"binary-v1").expect("write current binary");
        let current = identity("uaa-agent", "1.0.0", "x86_64", &install_path);

        let pubkeys = [current_key.verifying_key(), next_key.verifying_key()];
        let fetcher = MockFetcher::new()
            .with("https://manifest.invalid/manifest.json", manifest_bytes)
            .with("https://manifest.invalid/manifest.json.sig", sig_bytes)
            .with(
                "https://example.invalid/uaa-agent-2.0.0",
                artifact.clone(),
            );

        let outcome = self_update(
            &current,
            "https://manifest.invalid/manifest.json",
            &pubkeys,
            ApplyMode::StageOnly,
            &fetcher,
        )
        .await
        .expect("update should succeed with slot-1 key");

        assert!(matches!(outcome, UpdateOutcome::Staged(_)));
    }

    #[tokio::test]
    async fn test_older_or_equal_version_is_uptodate() {
        let key = throwaway_key();
        let artifact = b"binary-v1".to_vec();

        for entry_version in ["1.0.0", "0.9.0"] {
            let entries = vec![entry(
                "uaa-agent",
                entry_version,
                "x86_64",
                &artifact,
                &key,
            )];
            let (manifest_bytes, sig_bytes) = signed_manifest(entries, "0.1.0", &key);

            let dir = tempdir().expect("tempdir");
            let install_path = dir.path().join("uaa-agent");
            std::fs::write(&install_path, b"binary-current").expect("write current binary");
            let current = identity("uaa-agent", "1.0.0", "x86_64", &install_path);

            let pubkeys = [key.verifying_key(), throwaway_key().verifying_key()];
            let fetcher = MockFetcher::new()
                .with("https://manifest.invalid/manifest.json", manifest_bytes)
                .with("https://manifest.invalid/manifest.json.sig", sig_bytes);

            let outcome = self_update(
                &current,
                "https://manifest.invalid/manifest.json",
                &pubkeys,
                ApplyMode::TimerAuto,
                &fetcher,
            )
            .await
            .expect("up-to-date is not an error");

            assert_eq!(outcome, UpdateOutcome::UpToDate);
            // Only the 2 manifest fetches; no artifact download.
            assert_eq!(fetcher.calls().len(), 2);
        }
    }

    #[tokio::test]
    async fn test_min_version_floor_blocks_replay() {
        let key = throwaway_key();
        let artifact = b"binary-v1.1".to_vec();

        // Case 1: replayed manifest offers a version BELOW current -> UpToDate.
        {
            let entries = vec![entry("uaa-agent", "1.1.0", "x86_64", &artifact, &key)];
            let (manifest_bytes, sig_bytes) = signed_manifest(entries, "1.0.0", &key);

            let dir = tempdir().expect("tempdir");
            let install_path = dir.path().join("uaa-agent");
            std::fs::write(&install_path, b"binary-current").expect("write current binary");
            let current = identity("uaa-agent", "1.2.0", "x86_64", &install_path);

            let pubkeys = [key.verifying_key(), throwaway_key().verifying_key()];
            let fetcher = MockFetcher::new()
                .with("https://manifest.invalid/manifest.json", manifest_bytes)
                .with("https://manifest.invalid/manifest.json.sig", sig_bytes);

            let outcome = self_update(
                &current,
                "https://manifest.invalid/manifest.json",
                &pubkeys,
                ApplyMode::TimerAuto,
                &fetcher,
            )
            .await
            .expect("downgrade replay is not an error");

            assert_eq!(outcome, UpdateOutcome::UpToDate);
        }

        // Case 2: entry is newer than current but below the manifest's own
        // min_version floor -> hard Err, never applied.
        {
            let artifact_13 = b"binary-v1.3".to_vec();
            let entries = vec![entry(
                "uaa-agent",
                "1.3.0",
                "x86_64",
                &artifact_13,
                &key,
            )];
            let (manifest_bytes, sig_bytes) = signed_manifest(entries, "2.0.0", &key);

            let dir = tempdir().expect("tempdir");
            let install_path = dir.path().join("uaa-agent");
            std::fs::write(&install_path, b"binary-current").expect("write current binary");
            let current = identity("uaa-agent", "1.0.0", "x86_64", &install_path);

            let pubkeys = [key.verifying_key(), throwaway_key().verifying_key()];
            let fetcher = MockFetcher::new()
                .with("https://manifest.invalid/manifest.json", manifest_bytes)
                .with("https://manifest.invalid/manifest.json.sig", sig_bytes);

            let result = self_update(
                &current,
                "https://manifest.invalid/manifest.json",
                &pubkeys,
                ApplyMode::TimerAuto,
                &fetcher,
            )
            .await;

            assert!(result.is_err());
        }
    }

    #[tokio::test]
    async fn test_sha256_mismatch_rejected() {
        let key = throwaway_key();
        let artifact = b"binary-v2".to_vec();
        let mut bad_entry = entry("uaa-agent", "2.0.0", "x86_64", &artifact, &key);
        bad_entry.sha256 = "0".repeat(64); // definitely wrong

        let (manifest_bytes, sig_bytes) = signed_manifest(vec![bad_entry], "1.0.0", &key);

        let dir = tempdir().expect("tempdir");
        let install_path = dir.path().join("uaa-agent");
        std::fs::write(&install_path, b"binary-v1").expect("write current binary");
        let current = identity("uaa-agent", "1.0.0", "x86_64", &install_path);

        let pubkeys = [key.verifying_key(), throwaway_key().verifying_key()];
        let fetcher = MockFetcher::new()
            .with("https://manifest.invalid/manifest.json", manifest_bytes)
            .with("https://manifest.invalid/manifest.json.sig", sig_bytes)
            .with(
                "https://example.invalid/uaa-agent-2.0.0",
                artifact.clone(),
            );

        let result = self_update(
            &current,
            "https://manifest.invalid/manifest.json",
            &pubkeys,
            ApplyMode::TimerAuto,
            &fetcher,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().to_lowercase().contains("sha256"));
        assert!(!install_path.with_extension("new").exists());
    }

    #[tokio::test]
    async fn test_bad_artifact_sig_rejected() {
        let key = throwaway_key();
        let wrong_key = throwaway_key();
        let artifact = b"binary-v2".to_vec();
        // Good sha256 (computed against `key`'s entry helper would sign
        // correctly, so build the entry by hand with a wrong-key signature).
        let digest = hex_encode(&Sha256::digest(&artifact));
        let wrong_sig: Signature = wrong_key.sign(&artifact);
        let bad_entry = BinaryEntry {
            name: "uaa-agent".to_string(),
            version: semver::Version::parse("2.0.0").expect("valid semver"),
            target: "x86_64".to_string(),
            sha256: digest,
            sig: base64_encode(&wrong_sig.to_bytes()),
            url: "https://example.invalid/uaa-agent-2.0.0".to_string(),
        };

        let (manifest_bytes, sig_bytes) = signed_manifest(vec![bad_entry], "1.0.0", &key);

        let dir = tempdir().expect("tempdir");
        let install_path = dir.path().join("uaa-agent");
        std::fs::write(&install_path, b"binary-v1").expect("write current binary");
        let current = identity("uaa-agent", "1.0.0", "x86_64", &install_path);

        let pubkeys = [key.verifying_key(), throwaway_key().verifying_key()];
        let fetcher = MockFetcher::new()
            .with("https://manifest.invalid/manifest.json", manifest_bytes)
            .with("https://manifest.invalid/manifest.json.sig", sig_bytes)
            .with(
                "https://example.invalid/uaa-agent-2.0.0",
                artifact.clone(),
            );

        let result = self_update(
            &current,
            "https://manifest.invalid/manifest.json",
            &pubkeys,
            ApplyMode::TimerAuto,
            &fetcher,
        )
        .await;

        assert!(result.is_err());
        assert!(!install_path.with_extension("new").exists());
    }

    #[tokio::test]
    async fn test_hold_pin_suppresses_timer() {
        let key = throwaway_key();
        let artifact = b"binary-v2".to_vec();
        let entries = vec![entry("uaa-agent", "2.0.0", "x86_64", &artifact, &key)];
        let (manifest_bytes, sig_bytes) = signed_manifest(entries, "1.0.0", &key);

        let dir = tempdir().expect("tempdir");
        let install_path = dir.path().join("uaa-agent");
        std::fs::write(&install_path, b"binary-v1").expect("write current binary");
        std::fs::write(install_path.with_extension("hold"), b"").expect("write hold marker");
        let current = identity("uaa-agent", "1.0.0", "x86_64", &install_path);

        let pubkeys = [key.verifying_key(), throwaway_key().verifying_key()];
        let fetcher = MockFetcher::new()
            .with(
                "https://manifest.invalid/manifest.json",
                manifest_bytes.clone(),
            )
            .with(
                "https://manifest.invalid/manifest.json.sig",
                sig_bytes.clone(),
            )
            .with(
                "https://example.invalid/uaa-agent-2.0.0",
                artifact.clone(),
            );

        let outcome = self_update(
            &current,
            "https://manifest.invalid/manifest.json",
            &pubkeys,
            ApplyMode::TimerAuto,
            &fetcher,
        )
        .await
        .expect("held is not an error");

        assert_eq!(outcome, UpdateOutcome::Held);
        assert!(fetcher.calls().is_empty(), "hold must be checked before any fetch");

        // StageOnly ignores the hold and stages normally.
        let outcome = self_update(
            &current,
            "https://manifest.invalid/manifest.json",
            &pubkeys,
            ApplyMode::StageOnly,
            &fetcher,
        )
        .await
        .expect("StageOnly ignores hold");

        assert!(matches!(outcome, UpdateOutcome::Staged(_)));
    }

    #[tokio::test]
    async fn test_stage_only_never_renames() {
        let key = throwaway_key();
        let artifact = b"binary-v2-contents".to_vec();
        let entries = vec![entry("uaa-agent", "2.0.0", "x86_64", &artifact, &key)];
        let (manifest_bytes, sig_bytes) = signed_manifest(entries, "1.0.0", &key);

        let dir = tempdir().expect("tempdir");
        let install_path = dir.path().join("uaa-agent");
        let original_bytes = b"binary-v1-original".to_vec();
        std::fs::write(&install_path, &original_bytes).expect("write current binary");
        let current = identity("uaa-agent", "1.0.0", "x86_64", &install_path);

        let pubkeys = [key.verifying_key(), throwaway_key().verifying_key()];
        let fetcher = MockFetcher::new()
            .with("https://manifest.invalid/manifest.json", manifest_bytes)
            .with("https://manifest.invalid/manifest.json.sig", sig_bytes)
            .with(
                "https://example.invalid/uaa-agent-2.0.0",
                artifact.clone(),
            );

        let outcome = self_update(
            &current,
            "https://manifest.invalid/manifest.json",
            &pubkeys,
            ApplyMode::StageOnly,
            &fetcher,
        )
        .await
        .expect("stage-only happy path succeeds");

        let new_path = install_path.with_extension("new");
        assert_eq!(outcome, UpdateOutcome::Staged(new_path.clone()));
        assert!(new_path.exists());
        assert_eq!(std::fs::read(&new_path).expect("read staged"), artifact);
        assert_eq!(
            std::fs::read(&install_path).expect("read current"),
            original_bytes,
            "StageOnly must never touch the current binary"
        );
        assert!(!install_path.with_extension("prev").exists());
    }

    #[tokio::test]
    async fn test_timer_auto_applies_with_prev() {
        let key = throwaway_key();
        let artifact = b"binary-v2-contents".to_vec();
        let entries = vec![entry("uaa-agent", "2.0.0", "x86_64", &artifact, &key)];
        // Signed with slot-0 ("current") key.
        let (manifest_bytes, sig_bytes) = signed_manifest(entries, "1.0.0", &key);

        let dir = tempdir().expect("tempdir");
        let install_path = dir.path().join("uaa-agent");
        let original_bytes = b"binary-v1-original".to_vec();
        std::fs::write(&install_path, &original_bytes).expect("write current binary");
        let current = identity("uaa-agent", "1.0.0", "x86_64", &install_path);

        let pubkeys = [key.verifying_key(), throwaway_key().verifying_key()];
        let fetcher = MockFetcher::new()
            .with("https://manifest.invalid/manifest.json", manifest_bytes)
            .with("https://manifest.invalid/manifest.json.sig", sig_bytes)
            .with(
                "https://example.invalid/uaa-agent-2.0.0",
                artifact.clone(),
            );

        let outcome = self_update(
            &current,
            "https://manifest.invalid/manifest.json",
            &pubkeys,
            ApplyMode::TimerAuto,
            &fetcher,
        )
        .await
        .expect("fully valid update must apply, not be blocked by any guard");

        let prev_path = install_path.with_extension("prev");
        match outcome {
            UpdateOutcome::Applied { new_version, prev } => {
                assert_eq!(new_version, semver::Version::parse("2.0.0").unwrap());
                assert_eq!(prev, prev_path);
            }
            other => panic!("expected Applied, got {other:?}"),
        }

        assert_eq!(
            std::fs::read(&install_path).expect("read current"),
            artifact,
            "current path must now hold the new bytes"
        );
        assert_eq!(
            std::fs::read(&prev_path).expect("read prev"),
            original_bytes,
            "prev path must hold the old bytes for rollback"
        );
        assert!(
            !install_path.with_extension("new").exists(),
            ".new must be gone after a completed apply"
        );
    }
}
