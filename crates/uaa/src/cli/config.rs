// file: crates/uaa/src/cli/config.rs
// version: 1.4.0
// guid: a0de168b-3b68-4f34-8fe2-c4e513d40d70
// last-edited: 2026-07-18

//! `uaa config` — server-local placement of per-host InstallationConfig files.
//!
//! Ports `scripts/deploy-usb-configs.sh`: `uaa config place` copies
//! `<src>/<host>.yaml` to `<dest>/<hexmac>/uaa.yaml` (mode 0644), optionally
//! injecting place-time secrets from `--inject-from`. Injection is server-local
//! only — there is NO HTTP secret-write API, by design.
//!
//! **`--from-registry` (DS-OPS-03)** switches the config SOURCE from the
//! hand-authored `<host>.yaml` to a config resolved from the profile registry
//! (group defaults + host overrides + hostname allocation). Resolution lives in
//! `uaa-control` (`resolve_from_registry`); this CLI wires it to the unchanged
//! `uaa_core::config_place` placement pipeline. Two safety properties are ON by
//! default and can only be relaxed explicitly. First, `--from-registry` is OFF
//! by default (the `<host>.yaml` path is unchanged). Second, when it is ON,
//! `--dry-run` is ON unless `--no-dry-run` is passed: the command prints a
//! resolved-vs-committed diff and writes NOTHING, and the previous `uaa.yaml`
//! is copied to `.bak` before any real overwrite.
//!
//! **DS-OPS-05 — reify (the inverse of resolve): backfill + shadow-registration.**
//! `config backfill` reads the committed `<host>.yaml` fleet and POPULATES the
//! profile registry from it (idempotent, best-effort per host), and
//! `config place --register` additionally shadow-writes each successfully placed
//! host into the registry AFTER placement, best-effort. Both wrap the one reify
//! core, `uaa_control::register_from_config`. Both write to the real
//! `SnapshotProfileStore` at `/var/lib/uaa` (uaa-owned) and so must run
//! server-side as the `uaa` user (or a future operator-API path) — the
//! subcommand help repeats this run-requirement.
//!
//! **Do NOT chase a live `--from-registry` dry-run zero-diff as the reify
//! acceptance test.** That dry-run diffs the RAW committed file TEXT (comments,
//! key order, omitted serde defaults) against the serialized resolved config, so
//! cosmetic non-zero diffs are EXPECTED there even when reify is byte-perfect at
//! the STRUCT level. The load-bearing proof is the unit round-trip
//! (`test_resolved_equals_committed_by_struct_equality` here, plus the reify-core
//! round-trip in `uaa_control::profiles::reify`): reify(cfg) → resolve == cfg via
//! canonical serialization. Both sides through the same serializer.

use std::path::Path;

use uaa_core::config_place::{
    mac_for_host, place_configs, PlaceOptions, PlaceReport, DEFAULT_DEST_BASE,
    DEFAULT_INSTALL_CA_CERT_PATH, DEFAULT_SRC_DIR, KNOWN_HOSTS,
};
use uaa_core::network::InstallationConfig;
use uaa_control::db::store::StatePaths;
use uaa_control::profiles::store::{ProfileStore, SnapshotProfileStore};

#[derive(Debug, clap::Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum ConfigCommand {
    /// Place per-host configs server-locally at <dest>/<hexmac>/uaa.yaml (0644).
    Place {
        /// Directory of <host>.yaml source files.
        #[arg(long, default_value = DEFAULT_SRC_DIR)]
        src: String,

        /// Cloud-init web root (files land at <dest>/<hexmac>/uaa.yaml).
        #[arg(long, default_value = DEFAULT_DEST_BASE)]
        dest: String,

        /// Optional per-host secrets file for place-time injection (server-local only).
        #[arg(long)]
        inject_from: Option<String>,

        /// Resolve each host's config from the profile registry (group
        /// defaults, host overrides, and hostname allocation) instead of
        /// reading a hand-authored <host>.yaml. OFF by default.
        /// Behavior-changing and data-overwriting: see --no-dry-run.
        #[arg(long)]
        from_registry: bool,

        /// Only meaningful with --from-registry. Actually WRITE the resolved
        /// configs. Without it, --from-registry is a dry run: it prints a
        /// resolved-vs-committed diff and a count and writes NOTHING. With it,
        /// each host's previous uaa.yaml is copied to uaa.yaml.bak before the
        /// overwrite.
        #[arg(long)]
        no_dry_run: bool,

        /// Shadow-register each successfully placed host into the profile
        /// registry (DS-OPS-05), AFTER placement. Non-destructive: a registry
        /// write failure is logged and swallowed — it NEVER fails or alters the
        /// placement. Only takes effect on the file-based path (ignored with
        /// --from-registry, whose source already IS the registry). Writes to
        /// /var/lib/uaa (uaa-owned) — run as the `uaa` user.
        #[arg(long)]
        register: bool,

        /// Hosts to place (default: all known hosts).
        hosts: Vec<String>,
    },

    /// Backfill the profile registry from the committed <host>.yaml fleet
    /// (DS-OPS-05). Reifies every known host into the registry so a later
    /// `config place --from-registry` reconstructs it exactly. Idempotent and
    /// best-effort per host (a failure on one host does not abort the rest;
    /// re-run to converge). Writes to /var/lib/uaa (uaa-owned) — run as the
    /// `uaa` user, server-side.
    Backfill {
        /// Directory of <host>.yaml source files.
        #[arg(long, default_value = DEFAULT_SRC_DIR)]
        src: String,
    },
}

/// The fleet's group/allocation policy for a host name, derived from its naming
/// convention (the same split the DS-OPS-03 M2 fixture encodes by hand):
/// - a host ending in `-<digits>` (e.g. `len-serv-001`) is an INDEXED member of
///   the group named by its prefix (`len-serv`), pattern `{name}-{index:03}`,
///   with NO pinned hostname_override (the hostname allocation supplies the name);
/// - anything else (e.g. `unimatrixone`) is a STANDALONE host in its own group:
///   the name is a fixed pattern, pinned via hostname_override, no allocation.
struct GroupSpec {
    group_name: String,
    hostname_pattern: String,
    is_standalone: bool,
    hostname_override: Option<String>,
}

fn group_spec_for_host(host: &str) -> GroupSpec {
    if let Some((prefix, suffix)) = host.rsplit_once('-') {
        if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) {
            return GroupSpec {
                group_name: prefix.to_string(),
                hostname_pattern: "{name}-{index:03}".to_string(),
                is_standalone: false,
                hostname_override: None,
            };
        }
    }
    GroupSpec {
        group_name: host.to_string(),
        hostname_pattern: host.to_string(),
        is_standalone: true,
        hostname_override: Some(host.to_string()),
    }
}

/// Reify ONE host from its committed `<src>/<host>.yaml` into `store`. Reads the
/// PRE-injection committed file (never a placed/injected copy — that would carry
/// real luks_key/root_password/tpm2_pin, and persisting those into the registry
/// snapshot would leak secrets). Shared by backfill and shadow-registration; the
/// per-host `Result` lets each caller decide abort vs best-effort.
async fn reify_host_from_file(
    store: &dyn ProfileStore,
    src_dir: &Path,
    host: &str,
    actor: &str,
) -> Result<(), String> {
    let mac =
        mac_for_host(host).ok_or_else(|| "unknown host (add its MAC to mac_for_host)".to_string())?;
    let path = src_dir.join(format!("{host}.yaml"));
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let cfg: InstallationConfig =
        serde_yaml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?;
    let spec = group_spec_for_host(host);
    uaa_control::register_from_config(
        store,
        &spec.group_name,
        &spec.hostname_pattern,
        spec.is_standalone,
        mac,
        spec.hostname_override.as_deref(),
        &cfg,
        actor,
    )
    .await
    .map_err(|e| e.to_string())
}

/// Per-host outcome of a [`backfill_from_files`] run.
#[derive(Debug, Default)]
pub struct BackfillReport {
    /// Hosts reified into the registry.
    pub registered: Vec<String>,
    /// `(host, reason)` per host that failed to reify.
    pub failed: Vec<(String, String)>,
}

/// Reify every [`KNOWN_HOSTS`] committed `<src>/<host>.yaml` into `store`
/// (DS-OPS-05 backfill). Best-effort per host — a failure is recorded and does
/// NOT abort the others — and idempotent (re-run-to-converge is safe). Indexed
/// hosts are processed in HOSTNAME-SORTED order: `allocate_index` renders
/// 001/002/003 in binding order, so reifying out of order would bind a machine
/// to the wrong name (`register_from_config` guards this and would fail loudly).
/// Zero-padded indices sort correctly lexicographically, so a plain sort suffices.
///
/// Split from `config_command` so it is unit-testable against an in-tempdir
/// `SnapshotProfileStore` (never the real `/var/lib/uaa`).
pub async fn backfill_from_files(
    store: &dyn ProfileStore,
    src_dir: &Path,
    actor: &str,
) -> BackfillReport {
    let mut hosts: Vec<&str> = KNOWN_HOSTS.to_vec();
    hosts.sort_unstable();
    let mut report = BackfillReport::default();
    for host in hosts {
        match reify_host_from_file(store, src_dir, host, actor).await {
            Ok(()) => report.registered.push(host.to_string()),
            Err(reason) => report.failed.push((host.to_string(), reason)),
        }
    }
    report
}

/// Shadow-register (DS-OPS-05) every host that was successfully PLACED by the
/// file-based path, best-effort. **DEFINING PROPERTY: this can NEVER fail or
/// alter a placement.** It returns `()` (no error to propagate), and by the time
/// it runs, placement is already fully committed on disk — every failure here
/// (a missing/unreadable source, a parse error, a registry write error) is
/// logged as a WARNING and swallowed. Indexed hosts are reified in
/// hostname-sorted order (the allocation-order contract, same as backfill).
///
/// Unit-testable against an in-tempdir store; the real store is wired in
/// `config_command`.
async fn shadow_register_placed(
    store: &dyn ProfileStore,
    src_dir: &Path,
    placed_hosts: &[String],
    actor: &str,
) {
    let mut hosts: Vec<&String> = placed_hosts.iter().collect();
    hosts.sort_unstable();
    for host in hosts {
        if let Err(reason) = reify_host_from_file(store, src_dir, host, actor).await {
            eprintln!("WARN shadow-register {host}: {reason} (placement unaffected)");
        }
    }
}

/// Resolve EVERY requested host from the profile registry, then place —
/// all-or-nothing (DS-OPS-03). If ANY host fails to resolve, returns the error
/// and places NOTHING (a half-placed fleet is worse than an unplaced one). When
/// `dry_run` is set (the `--from-registry` default), placement writes nothing
/// and records a resolved-vs-committed diff per host instead.
///
/// Split out from `config_command` (which owns `process::exit`) so it is
/// unit-testable against an in-tempdir `SnapshotProfileStore`.
pub async fn resolve_all_and_place(
    store: &dyn ProfileStore,
    base: PlaceOptions,
    dry_run: bool,
) -> uaa_core::Result<PlaceReport> {
    // Host set: explicit args, else the known fleet (matching `place_configs`'s
    // own empty-hosts default, so the resolved set and the placed set agree).
    let hosts: Vec<String> = if base.hosts.is_empty() {
        KNOWN_HOSTS.iter().map(|s| s.to_string()).collect()
    } else {
        base.hosts.clone()
    };

    // Resolve every host BEFORE placing any — an unresolvable host is a loud
    // error, never a partial placement.
    let mut resolved = Vec::with_capacity(hosts.len());
    for host in &hosts {
        let cfg = uaa_control::resolve_from_registry(store, host)
            .await
            .map_err(|e| {
                uaa_core::error::AutoInstallError::ConfigError(format!(
                    "resolve {host} from registry: {e}"
                ))
            })?;
        resolved.push((host.clone(), cfg));
    }

    let opts = PlaceOptions {
        hosts,
        from_registry: Some(resolved),
        dry_run,
        ..base
    };
    place_configs(&opts)
}

pub async fn config_command(args: ConfigArgs) -> uaa_core::Result<()> {
    match args.command {
        ConfigCommand::Place {
            src,
            dest,
            inject_from,
            from_registry,
            no_dry_run,
            register,
            hosts,
        } => {
            let base = PlaceOptions {
                src_dir: src.into(),
                dest_base: dest.into(),
                inject_from: inject_from.map(Into::into),
                hosts,
                install_ca_cert_path: DEFAULT_INSTALL_CA_CERT_PATH.into(),
                from_registry: None,
                dry_run: false,
            };

            // Capture shadow-registration inputs BEFORE the branch: the
            // from-registry arm consumes `base` by value, so `base.*` is not
            // usable afterward. The requested host set mirrors `place_configs`'s
            // own empty-hosts default so `placed = requested − refused` is exact.
            let shadow_src = base.src_dir.clone();
            let requested: Vec<String> = if base.hosts.is_empty() {
                KNOWN_HOSTS.iter().map(|s| s.to_string()).collect()
            } else {
                base.hosts.clone()
            };

            let report = if from_registry {
                // Production profile store (the `/var/lib/uaa` snapshot).
                let store = SnapshotProfileStore::new(StatePaths::default());
                let dry_run = !no_dry_run;
                resolve_all_and_place(&store, base, dry_run).await?
            } else {
                // `?` here means a placement-pipeline error returns BEFORE the
                // registry is ever touched — the first half of the shadow-write
                // no-op guarantee.
                place_configs(&base)?
            };

            for (host, diff) in &report.dry_run_diffs {
                println!("DRY-RUN {host} (resolved vs committed):");
                println!("{diff}");
            }
            for placed in &report.placed {
                println!("PLACED  {placed}");
            }
            for (host, reason) in &report.refused {
                eprintln!("REFUSED {host}: {reason}");
            }
            if !report.dry_run_diffs.is_empty() {
                println!(
                    "DRY-RUN: {} host(s) previewed, nothing written. Re-run with --no-dry-run to place.",
                    report.dry_run_diffs.len()
                );
            }

            // Shadow-registration (DS-OPS-05, --register). Runs ONLY on the
            // file-based path, AFTER placement is fully committed on disk and
            // BEFORE the exit-on-refused check below (so hosts that DID place are
            // still shadow-written even when another host was refused). It reifies
            // the successfully placed hosts best-effort; `shadow_register_placed`
            // returns `()` and swallows every error, so it structurally cannot
            // fail or change the placement — the DEFINING no-op property.
            if register && !from_registry {
                let placed: Vec<String> = requested
                    .into_iter()
                    .filter(|h| !report.refused.iter().any(|(rh, _)| rh == h))
                    .collect();
                let store = SnapshotProfileStore::new(StatePaths::default());
                shadow_register_placed(&store, &shadow_src, &placed, "config-place-register").await;
            }

            // Exit 1 if any requested host was refused (mirrors the shell script).
            if !report.is_success() {
                std::process::exit(1);
            }
            Ok(())
        }

        ConfigCommand::Backfill { src } => {
            // Real profile store (the `/var/lib/uaa` snapshot) — run as `uaa`.
            let store = SnapshotProfileStore::new(StatePaths::default());
            let report = backfill_from_files(&store, Path::new(&src), "config-backfill").await;
            for host in &report.registered {
                println!("REGISTERED {host}");
            }
            for (host, reason) in &report.failed {
                eprintln!("FAILED     {host}: {reason}");
            }
            println!(
                "BACKFILL: {} registered, {} failed. Re-run to converge (idempotent).",
                report.registered.len(),
                report.failed.len()
            );
            // Exit 1 if any host failed, so an incomplete backfill is visible;
            // the successful hosts still landed (best-effort, not all-or-nothing).
            if !report.failed.is_empty() {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uaa_core::network::InstallationConfig;

    fn base_opts(dest: &std::path::Path) -> PlaceOptions {
        PlaceOptions {
            src_dir: std::path::PathBuf::from("/nonexistent-src"),
            dest_base: dest.to_path_buf(),
            inject_from: None,
            hosts: Vec::new(),
            install_ca_cert_path: std::path::PathBuf::from("/nonexistent/ca.crt"),
            from_registry: None,
            dry_run: false,
        }
    }

    fn committed_config(host: &str) -> InstallationConfig {
        let path = format!(
            "{}/../../examples/configs/install/{host}.yaml",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading committed {path}: {e}"));
        serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parsing committed {host}: {e}"))
    }

    #[tokio::test]
    async fn test_resolution_failure_places_nothing() {
        // One host resolves, another does not (no allocation/profile). The whole
        // run must Err and write NOTHING — never place the resolvable host alone.
        let dir = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));
        // Seed ONLY len-serv-001, through the real reify core. len-serv-002 is
        // never registered, so it cannot resolve.
        uaa_control::register_from_config(
            &store,
            "len-serv",
            "{name}-{index:03}",
            false,
            "6c:4b:90:bc:39:b3",
            None,
            &committed_config("len-serv-001"),
            "op",
        )
        .await
        .unwrap();

        let mut base = base_opts(dest.path());
        base.hosts = vec!["len-serv-001".to_string(), "len-serv-002".to_string()];

        let result = resolve_all_and_place(&store, base, false).await;
        assert!(result.is_err(), "a host that fails to resolve must fail the whole run");
        assert!(
            result.unwrap_err().to_string().contains("len-serv-002"),
            "the error must name the unresolvable host"
        );
        assert!(
            std::fs::read_dir(dest.path()).unwrap().next().is_none(),
            "resolution failure must place NOTHING (no partial fleet)"
        );
    }

    #[tokio::test]
    async fn test_resolved_equals_committed_by_struct_equality() {
        // ── THE M2 GATE ──────────────────────────────────────────────────
        // A registry seeded to reproduce each committed fleet YAML must resolve
        // back to a struct-equal InstallationConfig. `InstallationConfig` has no
        // `PartialEq` (TangServer blocks it, deliberately), so equality is
        // proven via canonical serialization: both the resolved config and the
        // parsed committed config are re-serialized through the SAME serializer,
        // eliminating the committed file's comments and omitted defaults. Equal
        // canonical YAML == equal structured value. This is the load-bearing
        // proof that the registry migration is faithful for every fleet host.
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));

        // Seed the whole fleet through the REAL reify core (DS-OPS-05). This is
        // the extraction proof for instruction #1: the fixture that empirically
        // round-trips is now the production `register_from_config`, not a private
        // test copy. Indexed hosts share one "len-serv" group and are reified in
        // hostname-sorted order (the allocation-order contract 001/002/003);
        // unimatrixone is a pinned standalone (hostname_override, no allocation).
        let indexed = ["len-serv-001", "len-serv-002", "len-serv-003"];
        let macs = ["6c:4b:90:bc:39:b3", "6c:4b:90:bc:f8:a3", "6c:4b:90:bc:f7:f4"];
        for (host, mac) in indexed.iter().zip(macs.iter()) {
            uaa_control::register_from_config(
                &store,
                "len-serv",
                "{name}-{index:03}",
                false,
                mac,
                None,
                &committed_config(host),
                "op",
            )
            .await
            .unwrap_or_else(|e| panic!("reify {host}: {e}"));
        }
        uaa_control::register_from_config(
            &store,
            "unimatrixone",
            "unimatrixone",
            true,
            "ac:1f:6b:40:fc:e2",
            Some("unimatrixone"),
            &committed_config("unimatrixone"),
            "op",
        )
        .await
        .unwrap();

        for host in ["len-serv-001", "len-serv-002", "len-serv-003", "unimatrixone"] {
            let resolved = uaa_control::resolve_from_registry(&store, host)
                .await
                .unwrap_or_else(|e| panic!("resolving {host}: {e}"));
            let committed = committed_config(host);
            assert_eq!(
                serde_yaml::to_string(&resolved).unwrap(),
                serde_yaml::to_string(&committed).unwrap(),
                "resolved config for {host} must equal the committed YAML (canonical form)"
            );
        }
    }

    /// Repo-relative path to the committed `<host>.yaml` fleet.
    fn committed_src_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(format!(
            "{}/../../examples/configs/install",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    #[test]
    fn test_group_spec_derivation_matches_fleet_convention() {
        let indexed = group_spec_for_host("len-serv-001");
        assert_eq!(indexed.group_name, "len-serv");
        assert_eq!(indexed.hostname_pattern, "{name}-{index:03}");
        assert!(!indexed.is_standalone);
        assert_eq!(indexed.hostname_override, None);

        let standalone = group_spec_for_host("unimatrixone");
        assert_eq!(standalone.group_name, "unimatrixone");
        assert!(standalone.is_standalone);
        assert_eq!(standalone.hostname_override.as_deref(), Some("unimatrixone"));
    }

    #[tokio::test]
    async fn test_backfill_registers_whole_fleet_and_round_trips() {
        // Backfill from the real committed fleet YAML into a tempdir store, then
        // prove reify→resolve round-trips every host (the DS-OPS-05 acceptance
        // gate expressed through the backfill entrypoint).
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));
        let src = committed_src_dir();

        let report = backfill_from_files(&store, &src, "test").await;
        assert!(
            report.failed.is_empty(),
            "backfill must register every fleet host; failed: {:?}",
            report.failed
        );
        assert_eq!(report.registered.len(), KNOWN_HOSTS.len());

        for host in KNOWN_HOSTS {
            let resolved = uaa_control::resolve_from_registry(&store, host)
                .await
                .unwrap_or_else(|e| panic!("resolving {host} after backfill: {e}"));
            assert_eq!(
                serde_yaml::to_string(&resolved).unwrap(),
                serde_yaml::to_string(&committed_config(host)).unwrap(),
                "backfilled {host} must resolve back to its committed YAML"
            );
        }

        // Idempotent: a second pass adds no duplicate groups and still succeeds.
        let again = backfill_from_files(&store, &src, "test").await;
        assert!(again.failed.is_empty(), "re-run must also succeed");
        assert_eq!(
            store.list_groups().await.unwrap().len(),
            2,
            "one len-serv group + one unimatrixone group, no duplicates on re-run"
        );
    }

    #[tokio::test]
    async fn test_shadow_register_reifies_placed_hosts() {
        // Simulate the post-placement shadow write for the indexed fleet: each
        // placed host is reified from its committed source and becomes resolvable.
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));
        let placed = vec![
            "len-serv-001".to_string(),
            "len-serv-002".to_string(),
            "len-serv-003".to_string(),
        ];
        shadow_register_placed(&store, &committed_src_dir(), &placed, "test").await;
        for host in ["len-serv-001", "len-serv-002", "len-serv-003"] {
            let resolved = uaa_control::resolve_from_registry(&store, host)
                .await
                .unwrap_or_else(|e| panic!("resolving shadow-registered {host}: {e}"));
            assert_eq!(resolved.hostname, host);
        }
    }

    #[tokio::test]
    async fn test_shadow_register_swallows_errors_and_never_panics() {
        // THE no-op guarantee, unit form: point shadow at a source dir with NO
        // host files so every reify fails. shadow_register_placed must return
        // normally (each error logged + swallowed, never propagated, never a
        // panic) and leave nothing registered.
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotProfileStore::new(StatePaths::under(dir.path()));
        let empty_src = tempfile::tempdir().unwrap();
        let placed = vec!["len-serv-001".to_string(), "unimatrixone".to_string()];

        // Returns () — there is no error to propagate, by construction.
        shadow_register_placed(&store, empty_src.path(), &placed, "test").await;

        // Nothing landed: the failed reify never wrote a resolvable host.
        assert!(
            uaa_control::resolve_from_registry(&store, "len-serv-001")
                .await
                .is_err(),
            "a swallowed shadow failure must not have registered anything"
        );
    }
}
