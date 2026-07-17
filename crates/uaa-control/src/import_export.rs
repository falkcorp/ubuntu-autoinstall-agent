// file: crates/uaa-control/src/import_export.rs
// version: 1.1.1
// guid: 4f8e14f9-bb1f-48d3-88ce-91f6accaaf77
// last-edited: 2026-07-17

//! Registry import/export (parity with the legacy JSON registries).
//!
//! Filled by control TASK-02 (CT-02). Backs the `import`/`export` subcommands in
//! `main.rs` (wired by the coordinator — see the TASK-02 completion report for the
//! exact one-line wiring).
//!
//! * [`import_from`] — Decision 22: `uaa-control import --from <dir>` reads the three
//!   legacy JSON registries (`registry.json`, `yubikey-registry.json`,
//!   `tang-registry.json`, ground truth: `scripts/autoinstall-agent.py`) and inserts
//!   each row ONLY if its primary key is absent from the store. A pre-existing CRDB
//!   row — even a stale-looking one — is NEVER touched: a wrong upsert here would
//!   de-approve live hosts and null out bound TPM EKs during a rollback-retry cycle.
//! * [`export_to_json`] — Decision 16: `uaa-control export --to-json <dir>` re-hydrates
//!   the same three JSON files from CRDB (the rollback path: disable this daemon,
//!   export, re-enable `autoinstall-agent.service`).
//!
//! Timestamps: the JSON ground truth uses UNIX INTEGER SECONDS; the row types
//! (`db::mod`) store `TIMESTAMPTZ` columns as `Option<String>`. This module's
//! canonical text form for that `String` is the decimal UNIX-seconds representation
//! (matching `db::store`'s own `now_epoch_secs` convention) — converting `unix int ->
//! decimal string -> unix int` is exact and needs no extra date/time dependency.
//!
//! Fail-closed malformed-JSON semantics: each of the three files is parsed to a
//! complete `serde_json::Value` before any row from it is touched, so a parse error
//! can never leave a half-imported file — the whole file's parse either fully
//! succeeds or nothing from it is inserted. A parse error aborts the rest of the
//! import (the offending filename is named in the error).

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use crate::db::registry::RegistryStore;
use crate::db::{BootTarget, MachineRow, MachineStatus, TangServerRow, YubikeyRow};

const REGISTRY_FILE: &str = "registry.json";
const YUBIKEY_REGISTRY_FILE: &str = "yubikey-registry.json";
const TANG_REGISTRY_FILE: &str = "tang-registry.json";

/// Per-table counts, reused for both [`ImportReport::inserted`] and
/// [`ImportReport::skipped`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RegistryCounts {
    pub machines: usize,
    pub yubikeys: usize,
    pub tang: usize,
}

/// Outcome of [`import_from`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub inserted: RegistryCounts,
    pub skipped: RegistryCounts,
    /// Ground-truth filenames that were absent from the import dir (warned, not
    /// fatal — the other files still import).
    pub files_missing: Vec<String>,
}

/// Outcome of [`export_to_json`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExportReport {
    pub machines: usize,
    pub yubikeys: usize,
    pub tang: usize,
}

/// Mirrors the Python ground truth `normalize_mac` (`scripts/autoinstall-agent.py`):
/// lowercase, then `-`/`.` separators collapse to `:`.
pub fn normalize_mac(mac: &str) -> String {
    mac.to_lowercase().replace(['-', '.'], ":")
}

/// JSON `Value` holding a UNIX-seconds integer -> the row types' canonical
/// `TIMESTAMPTZ` text form (decimal string). `None` for anything that isn't a number
/// (absent key, wrong type — treated as absent, never a hard error: edge semantics
/// only promise a *hard* error for malformed JSON at the whole-file level).
fn unix_to_ts(v: &Value) -> Option<String> {
    v.as_i64().map(|n| n.to_string())
}

/// The inverse of [`unix_to_ts`]: canonical text form -> UNIX-seconds integer.
fn ts_to_unix(s: &str) -> Option<i64> {
    s.parse::<i64>().ok()
}

fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Import the three legacy JSON registries from `dir` into `store`, using the
/// `*_if_absent` store methods exclusively (Decision 22: a pre-existing row is
/// counted `skipped`, never touched). An absent file is warned and skipped — the
/// other files still import. A malformed JSON file is a hard error naming the file;
/// nothing from a malformed file is ever inserted (see module docs).
pub async fn import_from(dir: &Path, store: &dyn RegistryStore) -> Result<ImportReport> {
    let mut report = ImportReport::default();

    match import_machines(dir, store).await? {
        Some((inserted, skipped)) => {
            report.inserted.machines = inserted;
            report.skipped.machines = skipped;
        }
        None => {
            tracing::warn!(file = REGISTRY_FILE, "registry import: file absent, skipping");
            report.files_missing.push(REGISTRY_FILE.to_string());
        }
    }

    match import_yubikeys(dir, store).await? {
        Some((inserted, skipped)) => {
            report.inserted.yubikeys = inserted;
            report.skipped.yubikeys = skipped;
        }
        None => {
            tracing::warn!(
                file = YUBIKEY_REGISTRY_FILE,
                "registry import: file absent, skipping"
            );
            report.files_missing.push(YUBIKEY_REGISTRY_FILE.to_string());
        }
    }

    match import_tang(dir, store).await? {
        Some((inserted, skipped)) => {
            report.inserted.tang = inserted;
            report.skipped.tang = skipped;
        }
        None => {
            tracing::warn!(
                file = TANG_REGISTRY_FILE,
                "registry import: file absent, skipping"
            );
            report.files_missing.push(TANG_REGISTRY_FILE.to_string());
        }
    }

    Ok(report)
}

/// Read `dir/name` as a JSON object. `Ok(None)` = file absent (the caller warns and
/// continues with the other files). `Err` = malformed JSON or a JSON value that isn't
/// an object, named in the error — fail-closed: the whole file is parsed before any
/// row is touched, so a parse error never leaves a half-imported file.
fn read_json_dict(dir: &Path, name: &str) -> Result<Option<Map<String, Value>>> {
    let path = dir.join(name);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let value: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("malformed JSON in {}", path.display()))?;
    match value {
        Value::Object(map) => Ok(Some(map)),
        other => anyhow::bail!(
            "malformed JSON in {}: expected a JSON object, got {other}",
            path.display()
        ),
    }
}

/// Log (debug-level, never error) any JSON object key not in `known` — the pinned
/// "unknown extra keys are ignored" edge semantic.
fn log_unknown_keys(context: &str, obj: &Map<String, Value>, known: &[&str]) {
    for key in obj.keys() {
        if !known.contains(&key.as_str()) {
            tracing::debug!(%context, key = %key, "registry import: ignoring unknown key");
        }
    }
}

async fn import_machines(
    dir: &Path,
    store: &dyn RegistryStore,
) -> Result<Option<(usize, usize)>> {
    let Some(map) = read_json_dict(dir, REGISTRY_FILE)? else {
        return Ok(None);
    };

    const KNOWN: &[&str] = &[
        "hostname",
        "ip",
        "type",
        "status",
        "registered_at",
        "approved_at",
        "last_seen",
        "last_ip",
        "tpm_ek",
    ];

    let mut inserted = 0usize;
    let mut skipped = 0usize;
    for (raw_mac, entry) in map {
        let mac = normalize_mac(&raw_mac);
        let Some(obj) = entry.as_object() else {
            tracing::debug!(mac = %mac, "registry import: entry is not an object, skipping");
            continue;
        };
        log_unknown_keys(&mac, obj, KNOWN);

        let row = MachineRow {
            mac: mac.clone(),
            hostname: obj
                .get("hostname")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            ip: obj.get("ip").and_then(Value::as_str).map(str::to_string),
            r#type: obj
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("lenovo")
                .to_string(),
            status: obj
                .get("status")
                .and_then(Value::as_str)
                .map(|s| MachineStatus::from(s.to_string()))
                .unwrap_or(MachineStatus::Pending),
            boot_target: BootTarget::LocalDisk,
            tpm_ek: obj.get("tpm_ek").and_then(Value::as_str).map(str::to_string),
            registered_at: obj
                .get("registered_at")
                .and_then(unix_to_ts)
                .or_else(|| Some(now_unix().to_string())),
            approved_at: obj.get("approved_at").and_then(unix_to_ts),
            last_seen: obj.get("last_seen").and_then(unix_to_ts),
            last_ip: obj.get("last_ip").and_then(Value::as_str).map(str::to_string),
            installed_at: None,
            last_install_status: None,
            updated_at: None,
            app_reports: Vec::new(),
            last_app_status_at: None,
        };

        if store.insert_machine_if_absent(row).await? {
            inserted += 1;
        } else {
            skipped += 1;
        }
    }
    Ok(Some((inserted, skipped)))
}

async fn import_yubikeys(
    dir: &Path,
    store: &dyn RegistryStore,
) -> Result<Option<(usize, usize)>> {
    let Some(map) = read_json_dict(dir, YUBIKEY_REGISTRY_FILE)? else {
        return Ok(None);
    };

    const KNOWN: &[&str] = &[
        "fingerprint",
        "gpg_pubkey",
        "ssh_pubkey",
        "comment",
        "serial",
        "status",
        "registered_at",
    ];

    let mut inserted = 0usize;
    let mut skipped = 0usize;
    for (fingerprint, entry) in map {
        let Some(obj) = entry.as_object() else {
            tracing::debug!(fingerprint = %fingerprint, "yubikey import: entry is not an object, skipping");
            continue;
        };
        log_unknown_keys(&fingerprint, obj, KNOWN);

        let row = YubikeyRow {
            fingerprint: fingerprint.clone(),
            gpg_pubkey: obj
                .get("gpg_pubkey")
                .and_then(Value::as_str)
                .map(str::to_string),
            ssh_pubkey: obj
                .get("ssh_pubkey")
                .and_then(Value::as_str)
                .map(str::to_string),
            comment: obj.get("comment").and_then(Value::as_str).map(str::to_string),
            serial: obj.get("serial").and_then(Value::as_str).map(str::to_string),
            status: obj
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .to_string(),
            registered_at: obj
                .get("registered_at")
                .and_then(unix_to_ts)
                .or_else(|| Some(now_unix().to_string())),
        };

        if store.insert_yubikey_if_absent(row).await? {
            inserted += 1;
        } else {
            skipped += 1;
        }
    }
    Ok(Some((inserted, skipped)))
}

async fn import_tang(dir: &Path, store: &dyn RegistryStore) -> Result<Option<(usize, usize)>> {
    let Some(map) = read_json_dict(dir, TANG_REGISTRY_FILE)? else {
        return Ok(None);
    };

    const KNOWN: &[&str] = &["hostname", "ip", "tang_url", "adv_keys", "last_seen"];

    let mut inserted = 0usize;
    let mut skipped = 0usize;
    for (hostname, entry) in map {
        let Some(obj) = entry.as_object() else {
            tracing::debug!(hostname = %hostname, "tang import: entry is not an object, skipping");
            continue;
        };
        log_unknown_keys(&hostname, obj, KNOWN);

        let row = TangServerRow {
            hostname: hostname.clone(),
            ip: obj.get("ip").and_then(Value::as_str).map(str::to_string),
            tang_url: obj
                .get("tang_url")
                .and_then(Value::as_str)
                .map(str::to_string),
            adv_keys: obj.get("adv_keys").cloned(),
            last_seen: obj.get("last_seen").and_then(unix_to_ts),
        };

        if store.insert_tang_if_absent(row).await? {
            inserted += 1;
        } else {
            skipped += 1;
        }
    }
    Ok(Some((inserted, skipped)))
}

/// Re-hydrate the three legacy JSON registries from `store` into `dir` (Decision 16
/// rollback). Overwrites the target files — that is the point. Each file is written
/// tmp+rename, mirroring `db::store::write_snapshot`'s atomic-write contract.
pub async fn export_to_json(dir: &Path, store: &dyn RegistryStore) -> Result<ExportReport> {
    let machines = store.list_machines().await?;
    let yubikeys = store.list_yubikeys().await?;
    let tang = store.list_tang_servers().await?;

    write_machines(dir, &machines)?;
    write_yubikeys(dir, &yubikeys)?;
    write_tang(dir, &tang)?;

    Ok(ExportReport {
        machines: machines.len(),
        yubikeys: yubikeys.len(),
        tang: tang.len(),
    })
}

fn write_machines(dir: &Path, rows: &[MachineRow]) -> Result<()> {
    let mut map = Map::new();
    for row in rows {
        let mut entry = Map::new();
        entry.insert("hostname".into(), Value::String(row.hostname.clone()));
        entry.insert("type".into(), Value::String(row.r#type.clone()));
        entry.insert(
            "status".into(),
            Value::String(String::from(row.status.clone())),
        );
        if let Some(ts) = row.registered_at.as_deref().and_then(ts_to_unix) {
            entry.insert("registered_at".into(), Value::from(ts));
        }
        if let Some(ts) = row.approved_at.as_deref().and_then(ts_to_unix) {
            entry.insert("approved_at".into(), Value::from(ts));
        }
        if let Some(ts) = row.last_seen.as_deref().and_then(ts_to_unix) {
            entry.insert("last_seen".into(), Value::from(ts));
        }
        if let Some(ip) = &row.last_ip {
            entry.insert("last_ip".into(), Value::String(ip.clone()));
        }
        if let Some(ek) = &row.tpm_ek {
            entry.insert("tpm_ek".into(), Value::String(ek.clone()));
        }
        map.insert(row.mac.clone(), Value::Object(entry));
    }
    atomic_write_json(&dir.join(REGISTRY_FILE), &Value::Object(map))
}

fn write_yubikeys(dir: &Path, rows: &[YubikeyRow]) -> Result<()> {
    let mut map = Map::new();
    for row in rows {
        let mut entry = Map::new();
        if let Some(v) = &row.gpg_pubkey {
            entry.insert("gpg_pubkey".into(), Value::String(v.clone()));
        }
        if let Some(v) = &row.ssh_pubkey {
            entry.insert("ssh_pubkey".into(), Value::String(v.clone()));
        }
        if let Some(v) = &row.comment {
            entry.insert("comment".into(), Value::String(v.clone()));
        }
        if let Some(v) = &row.serial {
            entry.insert("serial".into(), Value::String(v.clone()));
        }
        entry.insert("status".into(), Value::String(row.status.clone()));
        if let Some(ts) = row.registered_at.as_deref().and_then(ts_to_unix) {
            entry.insert("registered_at".into(), Value::from(ts));
        }
        map.insert(row.fingerprint.clone(), Value::Object(entry));
    }
    atomic_write_json(&dir.join(YUBIKEY_REGISTRY_FILE), &Value::Object(map))
}

fn write_tang(dir: &Path, rows: &[TangServerRow]) -> Result<()> {
    let mut map = Map::new();
    for row in rows {
        let mut entry = Map::new();
        if let Some(v) = &row.ip {
            entry.insert("ip".into(), Value::String(v.clone()));
        }
        if let Some(v) = &row.tang_url {
            entry.insert("tang_url".into(), Value::String(v.clone()));
        }
        if let Some(v) = &row.adv_keys {
            entry.insert("adv_keys".into(), v.clone());
        }
        if let Some(ts) = row.last_seen.as_deref().and_then(ts_to_unix) {
            entry.insert("last_seen".into(), Value::from(ts));
        }
        map.insert(row.hostname.clone(), Value::Object(entry));
    }
    atomic_write_json(&dir.join(TANG_REGISTRY_FILE), &Value::Object(map))
}

/// tmp+rename, mirroring `db::store::write_snapshot`'s atomic-write contract: write
/// the full document to `<path>.tmp`, then rename over the live file so a reader
/// never observes a half-written export.
fn atomic_write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    let bytes = serde_json::to_vec_pretty(value)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::registry::MemRegistryStore;

    fn write_fixture(dir: &Path, name: &str, json: &str) {
        std::fs::write(dir.join(name), json).unwrap();
    }

    fn sample_machine(mac: &str) -> MachineRow {
        MachineRow {
            mac: mac.to_string(),
            hostname: "h1".into(),
            ip: None,
            r#type: "lenovo".into(),
            status: MachineStatus::Approved,
            boot_target: BootTarget::LocalDisk,
            tpm_ek: Some("ek123".into()),
            registered_at: Some("1600000000".into()),
            approved_at: Some("1600000001".into()),
            last_seen: None,
            last_ip: None,
            installed_at: None,
            last_install_status: None,
            updated_at: None,
            app_reports: Vec::new(),
            last_app_status_at: None,
        }
    }

    #[tokio::test]
    async fn test_import_inserts_fresh_rows() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(
            dir.path(),
            REGISTRY_FILE,
            r#"{"AA:BB:CC:DD:EE:FF": {"hostname": "h1", "type": "lenovo", "status": "approved", "registered_at": 1700000000}}"#,
        );
        write_fixture(
            dir.path(),
            YUBIKEY_REGISTRY_FILE,
            r#"{"FINGERPRINT1": {"gpg_pubkey": "gpg", "ssh_pubkey": "ssh", "comment": "c", "serial": "s", "status": "approved", "registered_at": 1700000000}}"#,
        );
        write_fixture(
            dir.path(),
            TANG_REGISTRY_FILE,
            r#"{"tanghost": {"ip": "10.0.0.1", "tang_url": "http://10.0.0.1", "adv_keys": [], "last_seen": 1700000000}}"#,
        );

        let store = MemRegistryStore::new();
        let report = import_from(dir.path(), &store).await.unwrap();

        assert_eq!(report.inserted.machines, 1);
        assert_eq!(report.inserted.yubikeys, 1);
        assert_eq!(report.inserted.tang, 1);
        assert_eq!(report.skipped, RegistryCounts::default());
        assert!(report.files_missing.is_empty());
    }

    /// Decision 22 no-clobber law: a pre-existing approved row with a bound TPM EK
    /// survives an import whose fixture says `status=pending` for the same mac.
    #[tokio::test]
    async fn test_import_is_insert_if_absent() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(
            dir.path(),
            REGISTRY_FILE,
            r#"{"aa:bb:cc:dd:ee:ff": {"hostname": "h1", "type": "lenovo", "status": "pending", "registered_at": 1700000000}}"#,
        );
        write_fixture(dir.path(), YUBIKEY_REGISTRY_FILE, "{}");
        write_fixture(dir.path(), TANG_REGISTRY_FILE, "{}");

        let store = MemRegistryStore::new();
        store
            .insert_machine_if_absent(sample_machine("aa:bb:cc:dd:ee:ff"))
            .await
            .unwrap();

        let report = import_from(dir.path(), &store).await.unwrap();

        assert_eq!(report.inserted.machines, 0);
        assert_eq!(report.skipped.machines, 1);
        let row = store
            .get_machine("aa:bb:cc:dd:ee:ff")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.status,
            MachineStatus::Approved,
            "no-clobber: pre-existing approved row must survive"
        );
        assert_eq!(
            row.tpm_ek.as_deref(),
            Some("ek123"),
            "no-clobber: bound TPM EK must survive"
        );
    }

    #[tokio::test]
    async fn test_import_twice_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(
            dir.path(),
            REGISTRY_FILE,
            r#"{"aa:bb:cc:dd:ee:ff": {"hostname": "h1", "type": "lenovo", "status": "pending"}}"#,
        );
        write_fixture(dir.path(), YUBIKEY_REGISTRY_FILE, "{}");
        write_fixture(dir.path(), TANG_REGISTRY_FILE, "{}");

        let store = MemRegistryStore::new();
        let r1 = import_from(dir.path(), &store).await.unwrap();
        assert_eq!(r1.inserted.machines, 1);

        let r2 = import_from(dir.path(), &store).await.unwrap();
        assert_eq!(r2.inserted.machines, 0, "second import inserts 0");
        assert_eq!(r2.skipped.machines, 1);
    }

    #[tokio::test]
    async fn test_import_missing_file_warns_and_continues() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(
            dir.path(),
            REGISTRY_FILE,
            r#"{"aa:bb:cc:dd:ee:ff": {"hostname": "h1", "type": "lenovo", "status": "pending"}}"#,
        );
        // yubikey-registry.json and tang-registry.json are absent.

        let store = MemRegistryStore::new();
        let report = import_from(dir.path(), &store).await.unwrap();

        assert_eq!(report.inserted.machines, 1, "the other file still imports");
        assert_eq!(
            report.files_missing,
            vec![
                YUBIKEY_REGISTRY_FILE.to_string(),
                TANG_REGISTRY_FILE.to_string()
            ]
        );
    }

    #[tokio::test]
    async fn test_import_malformed_json_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(dir.path(), REGISTRY_FILE, "{ this is not valid json");
        write_fixture(dir.path(), YUBIKEY_REGISTRY_FILE, "{}");
        write_fixture(dir.path(), TANG_REGISTRY_FILE, "{}");

        let store = MemRegistryStore::new();
        let result = import_from(dir.path(), &store).await;

        assert!(result.is_err(), "malformed registry.json must hard-error");
        assert!(
            result.unwrap_err().to_string().contains(REGISTRY_FILE),
            "error must name the offending file"
        );
        assert_eq!(
            store.list_machines().await.unwrap().len(),
            0,
            "nothing inserted from the bad file"
        );
    }

    #[tokio::test]
    async fn test_import_normalizes_macs() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(
            dir.path(),
            REGISTRY_FILE,
            r#"{"AA-BB-CC-DD-EE-FF": {"hostname": "h1", "type": "lenovo", "status": "pending"}}"#,
        );
        write_fixture(dir.path(), YUBIKEY_REGISTRY_FILE, "{}");
        write_fixture(dir.path(), TANG_REGISTRY_FILE, "{}");

        let store = MemRegistryStore::new();
        import_from(dir.path(), &store).await.unwrap();

        let row = store.get_machine("aa:bb:cc:dd:ee:ff").await.unwrap();
        assert!(
            row.is_some(),
            "AA-BB-CC-DD-EE-FF must normalize to aa:bb:cc:dd:ee:ff"
        );
    }

    #[tokio::test]
    async fn test_export_python_shape() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemRegistryStore::new();
        store
            .insert_machine_if_absent(sample_machine("aa:bb:cc:dd:ee:ff"))
            .await
            .unwrap();

        export_to_json(dir.path(), &store).await.unwrap();

        let text = std::fs::read_to_string(dir.path().join(REGISTRY_FILE)).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        let obj = value.as_object().expect("dict-keyed export");
        let entry = obj
            .get("aa:bb:cc:dd:ee:ff")
            .and_then(Value::as_object)
            .expect("keyed by mac");

        assert_eq!(entry["registered_at"], Value::from(1600000000i64));
        assert_eq!(entry["approved_at"], Value::from(1600000001i64));
        assert_eq!(entry["tpm_ek"], Value::String("ek123".into()));
        assert!(
            !entry.contains_key("last_seen"),
            "None fields must be omitted, not written as null"
        );
        assert!(!entry.contains_key("last_ip"));
    }

    #[tokio::test]
    async fn test_export_import_round_trip_inserts_zero() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(
            dir.path(),
            REGISTRY_FILE,
            r#"{"aa:bb:cc:dd:ee:ff": {"hostname": "h1", "type": "lenovo", "status": "approved", "registered_at": 1700000000}}"#,
        );
        write_fixture(
            dir.path(),
            YUBIKEY_REGISTRY_FILE,
            r#"{"FP1": {"status": "approved", "registered_at": 1700000000}}"#,
        );
        write_fixture(
            dir.path(),
            TANG_REGISTRY_FILE,
            r#"{"tanghost": {"ip": "10.0.0.1", "last_seen": 1700000000}}"#,
        );

        let store = MemRegistryStore::new();
        let r1 = import_from(dir.path(), &store).await.unwrap();
        assert_eq!(r1.inserted.machines + r1.inserted.yubikeys + r1.inserted.tang, 3);

        let out_dir = tempfile::tempdir().unwrap();
        export_to_json(out_dir.path(), &store).await.unwrap();

        let r2 = import_from(out_dir.path(), &store).await.unwrap();
        assert_eq!(r2.inserted, RegistryCounts::default(), "round-trip inserts 0");
        assert_eq!(r2.skipped.machines, 1);
        assert_eq!(r2.skipped.yubikeys, 1);
        assert_eq!(r2.skipped.tang, 1);
    }

    /// Anti-over-suppression: a fresh mac (not pre-seeded) really inserts through the
    /// no-clobber guard, fields intact — the guard doesn't block legitimate new rows.
    #[tokio::test]
    async fn test_import_absent_rows_actually_insert() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(
            dir.path(),
            REGISTRY_FILE,
            r#"{"aa:bb:cc:dd:ee:ff": {"hostname": "fresh-host", "type": "lenovo", "status": "pending", "registered_at": 1700000000, "tpm_ek": "ekabc"}}"#,
        );
        write_fixture(dir.path(), YUBIKEY_REGISTRY_FILE, "{}");
        write_fixture(dir.path(), TANG_REGISTRY_FILE, "{}");

        let store = MemRegistryStore::new();
        // Pre-seed an unrelated mac so the no-clobber guard is active but must not
        // block the fresh insert below.
        store
            .insert_machine_if_absent(sample_machine("11:22:33:44:55:66"))
            .await
            .unwrap();

        let report = import_from(dir.path(), &store).await.unwrap();
        assert_eq!(
            report.inserted.machines, 1,
            "fresh mac must actually insert through the guard"
        );

        let row = store
            .get_machine("aa:bb:cc:dd:ee:ff")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.hostname, "fresh-host");
        assert_eq!(
            row.tpm_ek.as_deref(),
            Some("ekabc"),
            "fields intact on fresh insert"
        );
    }
}
