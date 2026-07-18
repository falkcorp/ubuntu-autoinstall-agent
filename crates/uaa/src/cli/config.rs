// file: crates/uaa/src/cli/config.rs
// version: 1.3.0
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

use uaa_core::config_place::{
    place_configs, PlaceOptions, PlaceReport, DEFAULT_DEST_BASE, DEFAULT_INSTALL_CA_CERT_PATH,
    DEFAULT_SRC_DIR, KNOWN_HOSTS,
};
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

        /// Hosts to place (default: all known hosts).
        hosts: Vec<String>,
    },
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

            let report = if from_registry {
                // Production profile store (the `/var/lib/uaa` snapshot).
                let store = SnapshotProfileStore::new(StatePaths::default());
                let dry_run = !no_dry_run;
                resolve_all_and_place(&store, base, dry_run).await?
            } else {
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

            // Exit 1 if any requested host was refused (mirrors the shell script).
            if !report.is_success() {
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
    use uaa_control::db::{HostGroupRow, HostProfileRow};
    use uuid::Uuid;

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

    fn group_row(id: Uuid, name: &str, pattern: &str) -> HostGroupRow {
        HostGroupRow {
            id,
            name: name.to_string(),
            hostname_pattern: pattern.to_string(),
            is_standalone: false,
            defaults: serde_json::json!({}),
            applications: serde_json::json!([]),
            content_hash: vec![],
            version: 1,
            created_at: None,
            updated_at: None,
        }
    }

    /// A `HostProfileRow` whose `overrides` reproduce `cfg` exactly, by
    /// round-tripping the resolved config back to the partial shape (minus
    /// `hostname`, which the allocation supplies, and `applications`, which the
    /// profile carries as its own typed list).
    fn profile_row_from_cfg(
        group_id: Uuid,
        identity: &str,
        hostname_override: Option<&str>,
        cfg: &InstallationConfig,
    ) -> HostProfileRow {
        let mut v = serde_json::to_value(cfg).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.remove("hostname");
        let applications = obj
            .remove("applications")
            .unwrap_or_else(|| serde_json::json!([]));
        HostProfileRow {
            id: Uuid::new_v4(),
            group_id,
            identity: identity.to_string(),
            hostname_override: hostname_override.map(str::to_string),
            overrides: v,
            applications,
            content_hash: vec![],
            version: 1,
            created_at: None,
            updated_at: None,
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
        let gid = Uuid::new_v4();
        store
            .put_group(group_row(gid, "len-serv", "{name}-{index:03}"), "op")
            .await
            .unwrap();
        // Seed ONLY len-serv-001 (profile + allocation). len-serv-002 is absent.
        let cfg = committed_config("len-serv-001");
        store
            .put_profile(profile_row_from_cfg(gid, "6c:4b:90:bc:39:b3", None, &cfg), "op")
            .await
            .unwrap();
        store.allocate_index(gid, "6c:4b:90:bc:39:b3").await.unwrap();

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

        // Three indexed len-serv hosts share one group; allocate in order so the
        // rendered hostnames are 001/002/003.
        let len_gid = Uuid::new_v4();
        store
            .put_group(group_row(len_gid, "len-serv", "{name}-{index:03}"), "op")
            .await
            .unwrap();
        let indexed = ["len-serv-001", "len-serv-002", "len-serv-003"];
        let macs = ["6c:4b:90:bc:39:b3", "6c:4b:90:bc:f8:a3", "6c:4b:90:bc:f7:f4"];
        for (host, mac) in indexed.iter().zip(macs.iter()) {
            let cfg = committed_config(host);
            store
                .put_profile(profile_row_from_cfg(len_gid, mac, None, &cfg), "op")
                .await
                .unwrap();
            let alloc = store.allocate_index(len_gid, mac).await.unwrap();
            assert_eq!(&alloc.hostname, host, "fixture allocation order must match");
        }

        // unimatrixone: pinned standalone host — profile hostname_override, no
        // index allocation.
        let uni_gid = Uuid::new_v4();
        let mut uni_group = group_row(uni_gid, "unimatrixone", "unimatrixone");
        uni_group.is_standalone = true;
        store.put_group(uni_group, "op").await.unwrap();
        let uni_cfg = committed_config("unimatrixone");
        store
            .put_profile(
                profile_row_from_cfg(uni_gid, "ac:1f:6b:40:fc:e2", Some("unimatrixone"), &uni_cfg),
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
}
