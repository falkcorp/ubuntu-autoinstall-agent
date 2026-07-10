// file: crates/uaa-control/src/db/migrations.rs
// version: 1.0.0
// guid: da676f20-2e42-46be-b0a0-3b39becef5d3
// last-edited: 2026-07-10

//! Embedded CRDB migrations.
//!
//! The normative schema lives in `migrations/0001_init.sql` and is compiled into the
//! binary with `include_str!` so no file needs to ship alongside the daemon. Unit
//! tests assert the SQL *text* only; the [`apply`] path is runtime-only and never runs
//! under `cargo test` (it needs a live CockroachDB, which the tests must not require).

const MIGRATION_0001: &str = include_str!("../../migrations/0001_init.sql");

/// The verbatim SQL for migration 0001 (the full normative registry schema).
pub fn migration_sql() -> &'static str {
    MIGRATION_0001
}

/// Apply pending migrations against a live CockroachDB connection.
///
/// Runtime-only: creates the `schema_migrations (version INT8 PRIMARY KEY,
/// applied_at TIMESTAMPTZ)` bookkeeping table if absent, then applies 0001 iff its
/// version row is missing, recording the version inside the same statement batch.
/// Never invoked by unit tests (which assert [`migration_sql`] text instead).
pub async fn apply(client: &tokio_postgres::Client) -> anyhow::Result<()> {
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
             version INT8 PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT now())",
        )
        .await?;

    let already: i64 = client
        .query_one(
            "SELECT count(*) FROM schema_migrations WHERE version = 1",
            &[],
        )
        .await?
        .get(0);

    if already == 0 {
        client.batch_execute(MIGRATION_0001).await?;
        client
            .execute("INSERT INTO schema_migrations (version) VALUES (1)", &[])
            .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLES: [&str; 10] = [
        "machines",
        "install_history",
        "enrollments",
        "yubikeys",
        "luks_credentials",
        "tang_servers",
        "discovered_macs",
        "audit_events",
        "audit_checkpoints",
        "saga_log",
    ];

    #[test]
    fn test_migration_sql_has_all_ten_tables() {
        let sql = migration_sql();
        let count = sql.matches("CREATE TABLE").count();
        assert_eq!(count, 10, "expected exactly 10 CREATE TABLE statements");
        for table in TABLES {
            let needle = format!("CREATE TABLE {table} ");
            assert!(
                sql.contains(&needle),
                "migration is missing table `{table}`"
            );
        }
    }

    #[test]
    fn test_migration_sql_wal_dedup_comment() {
        let sql = migration_sql();
        // audit_events serializes appends via unique_rowid()/hash chain, and the
        // install_history/WAL dedup key is documented in the schema comments.
        assert!(sql.contains("unique_rowid"), "missing unique_rowid marker");
        assert!(sql.contains("prev_hash"), "missing prev_hash marker");
        assert!(
            sql.contains("event_id UUID PRIMARY KEY"),
            "missing WAL dedup key (event_id)"
        );
    }
}
