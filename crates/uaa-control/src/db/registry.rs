// file: crates/uaa-control/src/db/registry.rs
// version: 1.1.0
// guid: 02d40065-96da-4469-b679-b8bfd4f0b8b3
// last-edited: 2026-07-10

//! Registry CRUD against CockroachDB (the `RegistryStore` trait + tokio-postgres impl).
//!
//! Filled by control TASK-02 (CT-02). Row types live in `db::mod`; this module adds
//! the query/mutation methods.
//!
//! Two implementations:
//!   * [`PgRegistryStore`] — the real tokio-postgres impl against CockroachDB (spec
//!     Decision 5). Every SQL string is a `pub(crate) const`, asserted textually by
//!     this module's tests; the queries themselves are never executed under
//!     `cargo test` (no live database in the test path). Timestamps and UUIDs are
//!     bound/read as text with an explicit `::TIMESTAMPTZ` / `::UUID` SQL-side cast —
//!     this keeps the crate on the workspace's existing `tokio-postgres = "0.7"` dep
//!     with no extra `with-chrono-0_4` / `with-uuid-1` / `with-serde_json-1` feature
//!     flags, so nothing was added to any Cargo.toml.
//!   * [`MemRegistryStore`] — an in-memory `HashMap`-backed store, ALWAYS compiled
//!     (not `#[cfg(test)]`): this module's own tests use it, `import_export.rs`'s
//!     tests use it, and sibling control-task tests may reuse it as a store that
//!     needs no live CockroachDB.
//!
//! insert-if-absent semantics are pinned by spec Decision 22: `ON CONFLICT (<pk>) DO
//! NOTHING`, NEVER `DO UPDATE` — an all-column upsert on rollback-retry was shown to
//! de-approve live hosts and null out bound TPM EKs. The one deliberate exception is
//! `upsert_tang_server` (tang checkin is a real last-seen-wins overwrite, matching the
//! Python `tang[hostname] = {...}` semantics) — realized via CockroachDB's `UPSERT
//! INTO` extension so the literal string `DO UPDATE` still never appears in this file.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Result;
use uuid::Uuid;

use super::{BootTarget, LuksCredentialRow, MachineRow, MachineStatus, TangServerRow, YubikeyRow};

/// Typed registry CRUD (spec Decisions 16/22).
///
/// insert-if-absent methods return `Ok(true)` iff the row was newly inserted;
/// `Ok(false)` means a row with that primary key already existed and was left
/// COMPLETELY untouched — the no-clobber law a rollback-retry import depends on.
#[async_trait::async_trait]
pub trait RegistryStore: Send + Sync {
    async fn get_machine(&self, mac: &str) -> Result<Option<MachineRow>>;
    async fn list_machines(&self) -> Result<Vec<MachineRow>>;
    /// `Ok(true)` = inserted; `Ok(false)` = a row for `row.mac` already existed and
    /// was left untouched (Decision 22 no-clobber law).
    async fn insert_machine_if_absent(&self, row: MachineRow) -> Result<bool>;
    async fn update_machine_status(
        &self,
        mac: &str,
        status: MachineStatus,
        approved_at: Option<String>,
    ) -> Result<()>;
    async fn touch_last_seen(&self, mac: &str, ip: Option<String>) -> Result<()>;
    async fn set_boot_target(&self, mac: &str, boot_target: BootTarget) -> Result<()>;
    async fn list_yubikeys(&self) -> Result<Vec<YubikeyRow>>;
    /// `Ok(true)` = inserted; `Ok(false)` = pre-existing fingerprint, untouched.
    async fn insert_yubikey_if_absent(&self, row: YubikeyRow) -> Result<bool>;
    /// Full listing — needed by `import_export::export_to_json` (Decision 16 rollback
    /// re-hydration). Not explicitly enumerated in the task brief's method list but
    /// required to fulfil that goal; see the TASK-02 completion report.
    async fn list_tang_servers(&self) -> Result<Vec<TangServerRow>>;
    /// Checkin semantics — a REAL last-seen-wins upsert (unlike every other table,
    /// tang checkins mirror the Python's full-overwrite `tang[hostname] = {...}`).
    /// Only the import path uses insert-if-absent; live checkins use this method.
    async fn upsert_tang_server(&self, row: TangServerRow) -> Result<()>;
    /// `Ok(true)` = inserted; `Ok(false)` = pre-existing hostname, untouched. Import
    /// path ONLY — live checkins use [`RegistryStore::upsert_tang_server`].
    async fn insert_tang_if_absent(&self, row: TangServerRow) -> Result<bool>;
    async fn insert_luks_credential(&self, row: LuksCredentialRow) -> Result<()>;
    async fn list_luks_credentials(&self, mac: &str) -> Result<Vec<LuksCredentialRow>>;
    async fn revoke_luks_credential(&self, id: Uuid) -> Result<()>;
}

// ─────────────────────────────────────────────────────────────────────────────
// PgRegistryStore — real tokio-postgres impl (runtime-only; never exercised by
// `cargo test`, which has no live CockroachDB).
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) const SQL_GET_MACHINE: &str = "\
    SELECT mac, hostname, ip, type, status, boot_target, tpm_ek, \
           registered_at::STRING AS registered_at, approved_at::STRING AS approved_at, \
           last_seen::STRING AS last_seen, last_ip, installed_at::STRING AS installed_at, \
           last_install_status, updated_at::STRING AS updated_at \
    FROM machines WHERE mac = $1";

pub(crate) const SQL_LIST_MACHINES: &str = "\
    SELECT mac, hostname, ip, type, status, boot_target, tpm_ek, \
           registered_at::STRING AS registered_at, approved_at::STRING AS approved_at, \
           last_seen::STRING AS last_seen, last_ip, installed_at::STRING AS installed_at, \
           last_install_status, updated_at::STRING AS updated_at \
    FROM machines ORDER BY mac";

pub(crate) const SQL_INSERT_MACHINE_IF_ABSENT: &str = "\
    INSERT INTO machines \
      (mac, hostname, ip, type, status, boot_target, tpm_ek, registered_at, approved_at, \
       last_seen, last_ip, installed_at, last_install_status, updated_at) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8::TIMESTAMPTZ, $9::TIMESTAMPTZ, $10::TIMESTAMPTZ, \
            $11, $12::TIMESTAMPTZ, $13, $14::TIMESTAMPTZ) \
    ON CONFLICT (mac) DO NOTHING";

pub(crate) const SQL_UPDATE_MACHINE_STATUS: &str =
    "UPDATE machines SET status = $2, approved_at = $3::TIMESTAMPTZ, updated_at = now() WHERE mac = $1";

pub(crate) const SQL_TOUCH_LAST_SEEN: &str =
    "UPDATE machines SET last_seen = now(), last_ip = $2, updated_at = now() WHERE mac = $1";

pub(crate) const SQL_SET_BOOT_TARGET: &str =
    "UPDATE machines SET boot_target = $2, updated_at = now() WHERE mac = $1";

pub(crate) const SQL_LIST_YUBIKEYS: &str = "\
    SELECT fingerprint, gpg_pubkey, ssh_pubkey, comment, serial, status, \
           registered_at::STRING AS registered_at \
    FROM yubikeys ORDER BY fingerprint";

pub(crate) const SQL_INSERT_YUBIKEY_IF_ABSENT: &str = "\
    INSERT INTO yubikeys (fingerprint, gpg_pubkey, ssh_pubkey, comment, serial, status, registered_at) \
    VALUES ($1, $2, $3, $4, $5, $6, $7::TIMESTAMPTZ) \
    ON CONFLICT (fingerprint) DO NOTHING";

pub(crate) const SQL_LIST_TANG_SERVERS: &str = "\
    SELECT hostname, ip, tang_url, adv_keys::STRING AS adv_keys, last_seen::STRING AS last_seen \
    FROM tang_servers ORDER BY hostname";

// CockroachDB `UPSERT INTO` extension: last-write-wins on the primary key, matching
// the Python checkin's full-overwrite semantics — WITHOUT the literal `DO UPDATE`
// pattern Decision 22 forbids for the insert-if-absent paths in this file.
pub(crate) const SQL_UPSERT_TANG_SERVER: &str = "\
    UPSERT INTO tang_servers (hostname, ip, tang_url, adv_keys, last_seen) \
    VALUES ($1, $2, $3, $4::JSONB, $5::TIMESTAMPTZ)";

pub(crate) const SQL_INSERT_TANG_IF_ABSENT: &str = "\
    INSERT INTO tang_servers (hostname, ip, tang_url, adv_keys, last_seen) \
    VALUES ($1, $2, $3, $4::JSONB, $5::TIMESTAMPTZ) \
    ON CONFLICT (hostname) DO NOTHING";

pub(crate) const SQL_INSERT_LUKS_CREDENTIAL: &str = "\
    INSERT INTO luks_credentials (id, mac, yubikey_serial, role, luks_keyslot, enrolled_at, revoked_at) \
    VALUES ($1::UUID, $2, $3, $4, $5, $6::TIMESTAMPTZ, $7::TIMESTAMPTZ)";

pub(crate) const SQL_LIST_LUKS_CREDENTIALS: &str = "\
    SELECT id::STRING AS id, mac, yubikey_serial, role, luks_keyslot, \
           enrolled_at::STRING AS enrolled_at, revoked_at::STRING AS revoked_at \
    FROM luks_credentials WHERE mac = $1 ORDER BY enrolled_at";

pub(crate) const SQL_REVOKE_LUKS_CREDENTIAL: &str =
    "UPDATE luks_credentials SET revoked_at = now() WHERE id = $1::UUID";

/// Real [`RegistryStore`]: tokio-postgres against CockroachDB. Constructed only at
/// runtime (`main.rs` builds one from config); unit tests never build it.
pub struct PgRegistryStore {
    client: tokio_postgres::Client,
}

impl PgRegistryStore {
    pub fn new(client: tokio_postgres::Client) -> Self {
        Self { client }
    }
}

fn row_to_machine(row: &tokio_postgres::Row) -> MachineRow {
    MachineRow {
        mac: row.get("mac"),
        hostname: row.get("hostname"),
        ip: row.get("ip"),
        r#type: row.get("type"),
        status: MachineStatus::from(row.get::<_, String>("status")),
        boot_target: BootTarget::from(row.get::<_, String>("boot_target")),
        tpm_ek: row.get("tpm_ek"),
        registered_at: row.get("registered_at"),
        approved_at: row.get("approved_at"),
        last_seen: row.get("last_seen"),
        last_ip: row.get("last_ip"),
        installed_at: row.get("installed_at"),
        last_install_status: row.get("last_install_status"),
        updated_at: row.get("updated_at"),
    }
}

fn row_to_yubikey(row: &tokio_postgres::Row) -> YubikeyRow {
    YubikeyRow {
        fingerprint: row.get("fingerprint"),
        gpg_pubkey: row.get("gpg_pubkey"),
        ssh_pubkey: row.get("ssh_pubkey"),
        comment: row.get("comment"),
        serial: row.get("serial"),
        status: row.get("status"),
        registered_at: row.get("registered_at"),
    }
}

fn row_to_tang(row: &tokio_postgres::Row) -> Result<TangServerRow> {
    let adv_keys_text: Option<String> = row.get("adv_keys");
    Ok(TangServerRow {
        hostname: row.get("hostname"),
        ip: row.get("ip"),
        tang_url: row.get("tang_url"),
        adv_keys: adv_keys_from_text(adv_keys_text)?,
        last_seen: row.get("last_seen"),
    })
}

fn row_to_luks(row: &tokio_postgres::Row) -> Result<LuksCredentialRow> {
    let id_text: String = row.get("id");
    Ok(LuksCredentialRow {
        id: uuid_from_text(&id_text)?,
        mac: row.get("mac"),
        yubikey_serial: row.get("yubikey_serial"),
        role: row.get("role"),
        luks_keyslot: row.get("luks_keyslot"),
        enrolled_at: row.get("enrolled_at"),
        revoked_at: row.get("revoked_at"),
    })
}

fn adv_keys_to_text(v: &Option<serde_json::Value>) -> Result<Option<String>> {
    v.as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(Into::into)
}

fn adv_keys_from_text(s: Option<String>) -> Result<Option<serde_json::Value>> {
    s.map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(Into::into)
}

fn uuid_to_text(id: Uuid) -> String {
    id.to_string()
}

fn uuid_from_text(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(Into::into)
}

#[async_trait::async_trait]
impl RegistryStore for PgRegistryStore {
    async fn get_machine(&self, mac: &str) -> Result<Option<MachineRow>> {
        let row = self.client.query_opt(SQL_GET_MACHINE, &[&mac]).await?;
        Ok(row.as_ref().map(row_to_machine))
    }

    async fn list_machines(&self) -> Result<Vec<MachineRow>> {
        let rows = self.client.query(SQL_LIST_MACHINES, &[]).await?;
        Ok(rows.iter().map(row_to_machine).collect())
    }

    async fn insert_machine_if_absent(&self, row: MachineRow) -> Result<bool> {
        let status: String = row.status.into();
        let boot_target: String = row.boot_target.into();
        let n = self
            .client
            .execute(
                SQL_INSERT_MACHINE_IF_ABSENT,
                &[
                    &row.mac,
                    &row.hostname,
                    &row.ip,
                    &row.r#type,
                    &status,
                    &boot_target,
                    &row.tpm_ek,
                    &row.registered_at,
                    &row.approved_at,
                    &row.last_seen,
                    &row.last_ip,
                    &row.installed_at,
                    &row.last_install_status,
                    &row.updated_at,
                ],
            )
            .await?;
        Ok(n == 1)
    }

    async fn update_machine_status(
        &self,
        mac: &str,
        status: MachineStatus,
        approved_at: Option<String>,
    ) -> Result<()> {
        let status: String = status.into();
        self.client
            .execute(SQL_UPDATE_MACHINE_STATUS, &[&mac, &status, &approved_at])
            .await?;
        Ok(())
    }

    async fn touch_last_seen(&self, mac: &str, ip: Option<String>) -> Result<()> {
        self.client
            .execute(SQL_TOUCH_LAST_SEEN, &[&mac, &ip])
            .await?;
        Ok(())
    }

    async fn set_boot_target(&self, mac: &str, boot_target: BootTarget) -> Result<()> {
        let boot_target: String = boot_target.into();
        self.client
            .execute(SQL_SET_BOOT_TARGET, &[&mac, &boot_target])
            .await?;
        Ok(())
    }

    async fn list_yubikeys(&self) -> Result<Vec<YubikeyRow>> {
        let rows = self.client.query(SQL_LIST_YUBIKEYS, &[]).await?;
        Ok(rows.iter().map(row_to_yubikey).collect())
    }

    async fn insert_yubikey_if_absent(&self, row: YubikeyRow) -> Result<bool> {
        let n = self
            .client
            .execute(
                SQL_INSERT_YUBIKEY_IF_ABSENT,
                &[
                    &row.fingerprint,
                    &row.gpg_pubkey,
                    &row.ssh_pubkey,
                    &row.comment,
                    &row.serial,
                    &row.status,
                    &row.registered_at,
                ],
            )
            .await?;
        Ok(n == 1)
    }

    async fn list_tang_servers(&self) -> Result<Vec<TangServerRow>> {
        let rows = self.client.query(SQL_LIST_TANG_SERVERS, &[]).await?;
        rows.iter().map(row_to_tang).collect()
    }

    async fn upsert_tang_server(&self, row: TangServerRow) -> Result<()> {
        let adv_keys = adv_keys_to_text(&row.adv_keys)?;
        self.client
            .execute(
                SQL_UPSERT_TANG_SERVER,
                &[&row.hostname, &row.ip, &row.tang_url, &adv_keys, &row.last_seen],
            )
            .await?;
        Ok(())
    }

    async fn insert_tang_if_absent(&self, row: TangServerRow) -> Result<bool> {
        let adv_keys = adv_keys_to_text(&row.adv_keys)?;
        let n = self
            .client
            .execute(
                SQL_INSERT_TANG_IF_ABSENT,
                &[&row.hostname, &row.ip, &row.tang_url, &adv_keys, &row.last_seen],
            )
            .await?;
        Ok(n == 1)
    }

    async fn insert_luks_credential(&self, row: LuksCredentialRow) -> Result<()> {
        let id = uuid_to_text(row.id);
        self.client
            .execute(
                SQL_INSERT_LUKS_CREDENTIAL,
                &[
                    &id,
                    &row.mac,
                    &row.yubikey_serial,
                    &row.role,
                    &row.luks_keyslot,
                    &row.enrolled_at,
                    &row.revoked_at,
                ],
            )
            .await?;
        Ok(())
    }

    async fn list_luks_credentials(&self, mac: &str) -> Result<Vec<LuksCredentialRow>> {
        let rows = self.client.query(SQL_LIST_LUKS_CREDENTIALS, &[&mac]).await?;
        rows.iter().map(row_to_luks).collect()
    }

    async fn revoke_luks_credential(&self, id: Uuid) -> Result<()> {
        let id = uuid_to_text(id);
        self.client
            .execute(SQL_REVOKE_LUKS_CREDENTIAL, &[&id])
            .await?;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MemRegistryStore — in-memory store. ALWAYS compiled (test/degraded support; used
// by this module's tests AND by sibling control-task tests).
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct MemState {
    machines: HashMap<String, MachineRow>,
    yubikeys: HashMap<String, YubikeyRow>,
    tang_servers: HashMap<String, TangServerRow>,
    luks_credentials: HashMap<Uuid, LuksCredentialRow>,
}

/// In-memory [`RegistryStore`]. `*_if_absent` leaves a pre-existing row COMPLETELY
/// untouched and returns `Ok(false)` — the same no-clobber contract [`PgRegistryStore`]
/// enforces in SQL. Test/degraded support; used by sibling task tests.
pub struct MemRegistryStore {
    state: Mutex<MemState>,
}

impl MemRegistryStore {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(MemState::default()),
        }
    }
}

impl Default for MemRegistryStore {
    fn default() -> Self {
        Self::new()
    }
}

fn now_epoch_secs_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

#[async_trait::async_trait]
impl RegistryStore for MemRegistryStore {
    async fn get_machine(&self, mac: &str) -> Result<Option<MachineRow>> {
        Ok(self.state.lock().unwrap().machines.get(mac).cloned())
    }

    async fn list_machines(&self) -> Result<Vec<MachineRow>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .machines
            .values()
            .cloned()
            .collect())
    }

    async fn insert_machine_if_absent(&self, row: MachineRow) -> Result<bool> {
        let mut st = self.state.lock().unwrap();
        if st.machines.contains_key(&row.mac) {
            return Ok(false);
        }
        st.machines.insert(row.mac.clone(), row);
        Ok(true)
    }

    async fn update_machine_status(
        &self,
        mac: &str,
        status: MachineStatus,
        approved_at: Option<String>,
    ) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if let Some(row) = st.machines.get_mut(mac) {
            row.status = status;
            row.approved_at = approved_at;
        }
        Ok(())
    }

    async fn touch_last_seen(&self, mac: &str, ip: Option<String>) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if let Some(row) = st.machines.get_mut(mac) {
            row.last_seen = Some(now_epoch_secs_string());
            row.last_ip = ip;
        }
        Ok(())
    }

    async fn set_boot_target(&self, mac: &str, boot_target: BootTarget) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if let Some(row) = st.machines.get_mut(mac) {
            row.boot_target = boot_target;
        }
        Ok(())
    }

    async fn list_yubikeys(&self) -> Result<Vec<YubikeyRow>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .yubikeys
            .values()
            .cloned()
            .collect())
    }

    async fn insert_yubikey_if_absent(&self, row: YubikeyRow) -> Result<bool> {
        let mut st = self.state.lock().unwrap();
        if st.yubikeys.contains_key(&row.fingerprint) {
            return Ok(false);
        }
        st.yubikeys.insert(row.fingerprint.clone(), row);
        Ok(true)
    }

    async fn list_tang_servers(&self) -> Result<Vec<TangServerRow>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .tang_servers
            .values()
            .cloned()
            .collect())
    }

    async fn upsert_tang_server(&self, row: TangServerRow) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        st.tang_servers.insert(row.hostname.clone(), row);
        Ok(())
    }

    async fn insert_tang_if_absent(&self, row: TangServerRow) -> Result<bool> {
        let mut st = self.state.lock().unwrap();
        if st.tang_servers.contains_key(&row.hostname) {
            return Ok(false);
        }
        st.tang_servers.insert(row.hostname.clone(), row);
        Ok(true)
    }

    async fn insert_luks_credential(&self, row: LuksCredentialRow) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        st.luks_credentials.insert(row.id, row);
        Ok(())
    }

    async fn list_luks_credentials(&self, mac: &str) -> Result<Vec<LuksCredentialRow>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .luks_credentials
            .values()
            .filter(|r| r.mac == mac)
            .cloned()
            .collect())
    }

    async fn revoke_luks_credential(&self, id: Uuid) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if let Some(row) = st.luks_credentials.get_mut(&id) {
            row.revoked_at = Some(now_epoch_secs_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_machine(mac: &str) -> MachineRow {
        MachineRow {
            mac: mac.to_string(),
            hostname: "h1".into(),
            ip: None,
            r#type: "lenovo".into(),
            status: MachineStatus::Pending,
            boot_target: BootTarget::LocalDisk,
            tpm_ek: None,
            registered_at: None,
            approved_at: None,
            last_seen: None,
            last_ip: None,
            installed_at: None,
            last_install_status: None,
            updated_at: None,
        }
    }

    /// Pins Decision 22: the machine insert-if-absent SQL const carries `ON CONFLICT
    /// (mac) DO NOTHING` and NEVER `DO UPDATE`. Same for yubikeys/tang import-path
    /// consts; `upsert_tang_server` uses CockroachDB's `UPSERT INTO` (checkin
    /// semantics) instead of `DO UPDATE`.
    #[test]
    fn test_sql_const_pins_on_conflict() {
        assert!(SQL_INSERT_MACHINE_IF_ABSENT.contains("ON CONFLICT (mac) DO NOTHING"));
        assert!(!SQL_INSERT_MACHINE_IF_ABSENT.contains("DO UPDATE"));
        assert!(SQL_INSERT_YUBIKEY_IF_ABSENT.contains("ON CONFLICT (fingerprint) DO NOTHING"));
        assert!(!SQL_INSERT_YUBIKEY_IF_ABSENT.contains("DO UPDATE"));
        assert!(SQL_INSERT_TANG_IF_ABSENT.contains("ON CONFLICT (hostname) DO NOTHING"));
        assert!(!SQL_INSERT_TANG_IF_ABSENT.contains("DO UPDATE"));
        assert!(
            !SQL_UPSERT_TANG_SERVER.contains("DO UPDATE"),
            "tang checkin upsert uses CRDB UPSERT INTO, not DO UPDATE"
        );
        assert!(SQL_UPSERT_TANG_SERVER.contains("UPSERT INTO"));
    }

    #[tokio::test]
    async fn test_mem_insert_if_absent_true_for_new_mac() {
        let store = MemRegistryStore::new();
        let inserted = store
            .insert_machine_if_absent(sample_machine("aa:bb:cc:dd:ee:ff"))
            .await
            .unwrap();
        assert!(inserted);
    }

    #[tokio::test]
    async fn test_mem_insert_if_absent_false_and_unchanged_for_existing_mac() {
        let store = MemRegistryStore::new();
        store
            .insert_machine_if_absent(sample_machine("aa:bb:cc:dd:ee:ff"))
            .await
            .unwrap();

        let mut second = sample_machine("aa:bb:cc:dd:ee:ff");
        second.hostname = "different".into();
        let inserted = store.insert_machine_if_absent(second).await.unwrap();

        assert!(!inserted);
        let row = store
            .get_machine("aa:bb:cc:dd:ee:ff")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.hostname, "h1", "pre-existing row must be untouched");
    }

    #[tokio::test]
    async fn test_mem_upsert_tang_server_overwrites() {
        let store = MemRegistryStore::new();
        store
            .upsert_tang_server(TangServerRow {
                hostname: "tang1".into(),
                ip: Some("10.0.0.1".into()),
                tang_url: None,
                adv_keys: None,
                last_seen: Some("1".into()),
            })
            .await
            .unwrap();
        store
            .upsert_tang_server(TangServerRow {
                hostname: "tang1".into(),
                ip: Some("10.0.0.2".into()),
                tang_url: None,
                adv_keys: None,
                last_seen: Some("2".into()),
            })
            .await
            .unwrap();

        let rows = store.list_tang_servers().await.unwrap();
        assert_eq!(rows.len(), 1, "checkin upsert replaces, not appends");
        assert_eq!(rows[0].ip.as_deref(), Some("10.0.0.2"));
    }
}
