// file: crates/uaa-control/src/drain.rs
// version: 1.0.0
// guid: 6a2f14d9-8c73-4b1e-9f05-3d7ba6e21c48
// last-edited: 2026-07-30

//! The real [`NodeDrainer`]: run a host's authored drain policy from U0.
//!
//! [`crate::reinstall`] owns *when* a drain happens and what a failed one
//! means (always a refusal, never a forced wipe). This module owns *how*: it
//! resolves the host's applications out of the profile registry, reads the
//! [`DecommissionPolicy`] each one declares, and executes the authored
//! [`DecommissionStep`]s.
//!
//! **Nothing here is hostname-aware.** There is no "if len-serv-\*" and no list
//! of database hosts. `needs_drain` is
//! [`requires_drain`](uaa_core::network::ssh_installer::config::requires_drain)
//! over the resolved applications, so a host gets a safe reinstall by
//! authoring the block on its spec.
//!
//! **Why U0 and not the host.** The drain needs the node up and serving while
//! something polls until its replica count reaches zero — and that something is
//! about to be power-cycled into an installer. U0 already holds the CA and
//! issues the node certs, so it is the only party that can both drive the
//! decommission and observe it finish.
//!
//! **Fail-closed everywhere.** Every ambiguity resolves toward *refuse the
//! reinstall*: an unparseable `node status`, a node id we cannot pin down, a
//! replica count that will not reach zero inside the authored deadline. The one
//! deliberate exception is a host that is **not a cluster member at all**,
//! which is [`DrainStatus::Drained`] because there is genuinely nothing to
//! drain — see [`CockroachCluster::node_id_for`].
//!
//! **Argv, never a shell string.** Every command is built as a program plus an
//! argument vector. The arguments include values that came out of the registry
//! (unit names, addresses), and there is no interpolation step for a quoting
//! mistake to hide in.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;

use uaa_core::autoinstall::host_spec::HostSpec;
use uaa_core::network::ssh_installer::config::{
    requires_drain, ApplicationSpec, DecommissionPolicy, DecommissionStep,
};

use crate::reinstall::{Clock, DrainStatus, NodeDrainer};

// ── Seams ────────────────────────────────────────────────────────────────────

/// A host's resolved identity for drain purposes: which applications it runs
/// and which IP the cluster knows it by.
#[derive(Debug, Clone, PartialEq)]
pub struct DrainTarget {
    /// `network_address` from the resolved config, CIDR form (`172.16.3.96/23`).
    pub network_address: String,
    /// Applications resolved for this host, group defaults merged with host
    /// overrides. The drain policy is read from these.
    pub applications: Vec<ApplicationSpec>,
}

impl DrainTarget {
    /// The bare IP the cluster advertises this node under, CIDR stripped.
    pub fn ip(&self) -> &str {
        HostSpec::ip_without_cidr(&self.network_address)
    }
}

/// Resolves a hostname to its [`DrainTarget`].
///
/// A seam rather than a direct call to
/// [`resolve_from_registry`](crate::profiles::resolve::resolve_from_registry)
/// so drain tests do not need a populated profile store, and so a caller that
/// already holds a resolved config can hand it over without a second lookup.
#[async_trait]
pub trait DrainTargetResolver: Send + Sync {
    async fn resolve(&self, hostname: &str) -> Result<DrainTarget>;
}

/// The result of one command U0 ran.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    /// `Ok` only on a zero exit, with both streams folded into the error
    /// otherwise — a drain that silently ignored a non-zero `cockroach` exit
    /// would report success while the node still held ranges.
    pub fn ok(&self, what: &str) -> Result<&str> {
        if self.status == 0 {
            Ok(self.stdout.trim())
        } else {
            Err(anyhow!(
                "{what} exited {}: {}{}",
                self.status,
                self.stderr.trim(),
                if self.stdout.trim().is_empty() {
                    String::new()
                } else {
                    format!(" (stdout: {})", self.stdout.trim())
                }
            ))
        }
    }
}

/// Runs a command on U0.
#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, program: &str, args: &[String]) -> Result<CommandOutput>;
}

/// Where U0 keeps the pieces it needs to talk to the cluster.
///
/// Local operational config, deliberately NOT part of `FleetConfig`: that is
/// the fleet wire schema shared with placed artifacts, and none of this belongs
/// in a host's installed config.
#[derive(Debug, Clone)]
pub struct DrainerConfig {
    /// `cockroach` binary on U0.
    pub cockroach_bin: String,
    /// `--certs-dir` holding U0's client cert for the `root` user.
    pub certs_dir: String,
    /// `--host` for admin commands: any live cluster member. Never the node
    /// being drained — it is about to stop answering.
    pub admin_host: String,
    /// SSH user for [`DecommissionStep::StopUnit`].
    pub ssh_user: String,
}

impl Default for DrainerConfig {
    fn default() -> Self {
        Self {
            cockroach_bin: "/usr/local/bin/cockroach".to_string(),
            certs_dir: "/var/lib/cockroach/certs".to_string(),
            admin_host: String::new(),
            ssh_user: "jdfalk".to_string(),
        }
    }
}

// ── Cluster queries ──────────────────────────────────────────────────────────

/// One row of `cockroach node status`.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeStatusRow {
    pub id: u64,
    /// `address` column, `IP:port`.
    pub address: String,
    /// `replicas` from `--decommission`, absent on a plain status query.
    pub replicas: Option<u64>,
    /// `is_live`, absent when the column is not present.
    pub is_live: Option<bool>,
}

/// Parse `cockroach node status --format=tsv` output.
///
/// Column ORDER is not assumed — the header is read and columns are located by
/// name. CockroachDB has reordered and added columns across versions (the fleet
/// is mid-flight between v25.3 and v25.4), and a positional parse that silently
/// read the wrong column would be a drain reporting zero replicas for the wrong
/// node.
pub fn parse_node_status(tsv: &str) -> Result<Vec<NodeStatusRow>> {
    let mut lines = tsv.lines().filter(|l| !l.trim().is_empty());
    let header = lines
        .next()
        .ok_or_else(|| anyhow!("cockroach node status returned no output at all"))?;

    let index: HashMap<&str, usize> = header
        .split('\t')
        .enumerate()
        .map(|(i, name)| (name.trim(), i))
        .collect();

    let col = |name: &str| -> Result<usize> {
        index.get(name).copied().ok_or_else(|| {
            anyhow!(
                "cockroach node status has no {name:?} column (header: {:?}) — \
                 refusing to guess by position",
                header.trim()
            )
        })
    };
    let id_col = col("id")?;
    let address_col = col("address")?;
    let replicas_col = index.get("replicas").copied();
    let is_live_col = index.get("is_live").copied();

    let mut rows = Vec::new();
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let get = |i: usize| fields.get(i).map(|s| s.trim()).unwrap_or_default();

        // A short row means the output is not the shape we think it is. Skip
        // rather than index-panic, but never invent a value for it.
        if fields.len() <= id_col.max(address_col) {
            continue;
        }
        let id = get(id_col)
            .parse::<u64>()
            .with_context(|| format!("unparseable node id {:?}", get(id_col)))?;

        rows.push(NodeStatusRow {
            id,
            address: get(address_col).to_string(),
            replicas: replicas_col.and_then(|c| get(c).parse::<u64>().ok()),
            is_live: is_live_col.and_then(|c| match get(c) {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            }),
        });
    }
    Ok(rows)
}

/// Talks to CockroachDB through the `cockroach` CLI on U0.
pub struct CockroachCluster<'a> {
    runner: &'a dyn CommandRunner,
    config: &'a DrainerConfig,
}

impl<'a> CockroachCluster<'a> {
    pub fn new(runner: &'a dyn CommandRunner, config: &'a DrainerConfig) -> Self {
        Self { runner, config }
    }

    fn admin_args(&self, extra: &[&str]) -> Vec<String> {
        let mut args: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
        args.push(format!("--certs-dir={}", self.config.certs_dir));
        if !self.config.admin_host.is_empty() {
            args.push(format!("--host={}", self.config.admin_host));
        }
        args.push("--format=tsv".to_string());
        args
    }

    /// Every node the cluster currently knows, with replica counts.
    pub async fn node_status(&self) -> Result<Vec<NodeStatusRow>> {
        let args = self.admin_args(&["node", "status", "--decommission"]);
        let out = self.runner.run(&self.config.cockroach_bin, &args).await?;
        parse_node_status(out.ok("cockroach node status")?)
    }

    /// The node id the cluster knows `ip` by, or `None` if `ip` is not a member.
    ///
    /// `None` is NOT an error and the caller treats it as already-drained. A
    /// host that the cluster has never heard of — or has already fully
    /// decommissioned — holds no replicas by definition, so there is nothing a
    /// drain could accomplish. This is exactly len-serv-003's state: long since
    /// decommissioned, still running, unable to rejoin. Erroring there would
    /// make an already-safe host permanently un-reinstallable.
    ///
    /// Matching is on the address's IP part only. The advertise port is derived
    /// per-spec, and a port mismatch between the registry and a running node
    /// must not silently read as "not a member" — that is the one way this
    /// function could wrongly wave through a node still holding data.
    pub fn node_id_for(rows: &[NodeStatusRow], ip: &str) -> Result<Option<u64>> {
        let matches: Vec<&NodeStatusRow> = rows
            .iter()
            .filter(|r| {
                r.address
                    .rsplit_once(':')
                    .map(|(h, _)| h)
                    .unwrap_or(&r.address)
                    == ip
            })
            .collect();

        match matches.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(only.id)),
            // Two nodes on one IP is a cluster we do not understand. Guessing
            // which id to decommission could take out the wrong one, and
            // decommission is terminal.
            many => Err(anyhow!(
                "{ip} matches {} nodes in the cluster (ids {:?}) — refusing to guess \
                 which to decommission",
                many.len(),
                many.iter().map(|r| r.id).collect::<Vec<_>>()
            )),
        }
    }

    /// Start the decommission. `--wait=none` deliberately: the authored
    /// `timeout_secs` owns the deadline, not `cockroach`'s internal wait, so a
    /// policy that says "give up after an hour" means it.
    pub async fn decommission(&self, node_id: u64) -> Result<()> {
        let id = node_id.to_string();
        let args = self.admin_args(&["node", "decommission", &id, "--wait=none"]);
        self.runner
            .run(&self.config.cockroach_bin, &args)
            .await?
            .ok("cockroach node decommission")?;
        Ok(())
    }

    /// Replicas still on `node_id`. `None` once the node has left the roster
    /// entirely, which is a completed drain.
    pub async fn replicas_on(&self, node_id: u64) -> Result<Option<u64>> {
        let rows = self.node_status().await?;
        let Some(row) = rows.iter().find(|r| r.id == node_id) else {
            return Ok(None);
        };
        // A roster row whose replica count we cannot read is NOT zero.
        row.replicas
            .ok_or_else(|| {
                anyhow!(
                    "node {node_id} is still in the roster but its replica count is \
                     unreadable — refusing to treat an unknown count as drained"
                )
            })
            .map(Some)
    }
}

// ── The drainer ──────────────────────────────────────────────────────────────

/// [`NodeDrainer`] driven entirely by the host's authored specs.
pub struct SpecDrainer<'a> {
    pub resolver: &'a dyn DrainTargetResolver,
    pub runner: &'a dyn CommandRunner,
    pub clock: &'a (dyn Clock + Send + Sync),
    pub config: DrainerConfig,
}

impl SpecDrainer<'_> {
    /// Run one policy's steps. `Ok(None)` means fully drained.
    async fn run_policy(
        &self,
        hostname: &str,
        target: &DrainTarget,
        policy: &DecommissionPolicy,
    ) -> Result<Option<u64>> {
        let cluster = CockroachCluster::new(self.runner, &self.config);
        let deadline = Duration::from_secs(policy.timeout_secs);
        let started = self.clock.now();

        // Resolved once and threaded through the steps: `CockroachDecommission`
        // needs the id, and so does `WaitForZeroReplicas`. Re-resolving between
        // them would race the decommission removing the row.
        let mut node_id: Option<u64> = None;
        let mut resolved = false;

        for step in &policy.steps {
            match step {
                DecommissionStep::CockroachDecommission => {
                    let rows = cluster.node_status().await?;
                    node_id = CockroachCluster::node_id_for(&rows, target.ip())?;
                    resolved = true;
                    match node_id {
                        Some(id) => {
                            tracing::info!(
                                "drain {hostname}: decommissioning cockroach node {id} ({})",
                                target.ip()
                            );
                            cluster.decommission(id).await?;
                        }
                        None => tracing::info!(
                            "drain {hostname}: {} is not a cluster member — nothing to \
                             decommission",
                            target.ip()
                        ),
                    }
                }

                DecommissionStep::WaitForZeroReplicas => {
                    // Tolerate a policy that waits without an explicit
                    // decommission step by resolving the id here too.
                    if !resolved {
                        let rows = cluster.node_status().await?;
                        node_id = CockroachCluster::node_id_for(&rows, target.ip())?;
                        resolved = true;
                    }
                    let Some(id) = node_id else { continue };

                    if let Some(left) = self
                        .wait_for_zero_replicas(hostname, &cluster, id, started, deadline, policy)
                        .await?
                    {
                        return Ok(Some(left));
                    }
                }

                DecommissionStep::StopUnit { unit } => {
                    tracing::info!("drain {hostname}: stopping {unit}");
                    let args = [
                        "-o".to_string(),
                        "BatchMode=yes".to_string(),
                        // Matches every other SSH call U0 makes; a shared
                        // control socket has bitten this fleet before.
                        "-o".to_string(),
                        "ControlPath=none".to_string(),
                        format!("{}@{}", self.config.ssh_user, target.ip()),
                        "sudo".to_string(),
                        "systemctl".to_string(),
                        "stop".to_string(),
                        unit.clone(),
                    ];
                    self.runner
                        .run("ssh", &args)
                        .await?
                        .ok(&format!("systemctl stop {unit} on {hostname}"))?;
                }
            }
        }
        Ok(None)
    }

    /// Poll until the node holds nothing or the deadline expires.
    /// `Ok(None)` = drained, `Ok(Some(n))` = still holding `n` at the deadline.
    async fn wait_for_zero_replicas(
        &self,
        hostname: &str,
        cluster: &CockroachCluster<'_>,
        node_id: u64,
        started: SystemTime,
        deadline: Duration,
        policy: &DecommissionPolicy,
    ) -> Result<Option<u64>> {
        let poll = Duration::from_secs(policy.poll_interval_secs.max(1));
        loop {
            match cluster.replicas_on(node_id).await? {
                None => {
                    tracing::info!("drain {hostname}: node {node_id} left the roster — drained");
                    return Ok(None);
                }
                Some(0) => {
                    tracing::info!("drain {hostname}: node {node_id} holds 0 replicas — drained");
                    return Ok(None);
                }
                Some(left) => {
                    let elapsed = self.clock.now().duration_since(started).unwrap_or_default();
                    // Checked AFTER the poll so a deadline of zero still gets
                    // one real look at the cluster before refusing.
                    if elapsed >= deadline {
                        tracing::error!(
                            "drain {hostname}: node {node_id} still holds {left} replicas after \
                             {}s — refusing the reinstall",
                            elapsed.as_secs()
                        );
                        return Ok(Some(left));
                    }
                    tracing::info!(
                        "drain {hostname}: node {node_id} still holds {left} replicas, waiting"
                    );
                    self.clock.sleep(poll).await;
                }
            }
        }
    }
}

#[async_trait]
impl NodeDrainer for SpecDrainer<'_> {
    async fn needs_drain(&self, hostname: &str) -> Result<bool> {
        let target = self.resolver.resolve(hostname).await?;
        Ok(requires_drain(&target.applications))
    }

    async fn drain(&self, hostname: &str) -> Result<DrainStatus> {
        let target = self.resolver.resolve(hostname).await?;

        for app in &target.applications {
            let Some(policy) = app.decommission() else {
                continue;
            };
            if !policy.enabled {
                continue;
            }
            // Enforced at authoring time by `validate_resolved` rule 6, but a
            // registry row could predate that rule. An empty step list with
            // `enabled: true` would otherwise be a silent no-op drain followed
            // by a real wipe.
            if policy.steps.is_empty() {
                return Err(anyhow!(
                    "{hostname}: application {:?} declares decommission.enabled with no \
                     steps — refusing to treat an empty policy as a completed drain",
                    app.kind()
                ));
            }

            if let Some(replicas_left) = self.run_policy(hostname, &target, policy).await? {
                return Ok(DrainStatus::Incomplete { replicas_left });
            }
        }
        Ok(DrainStatus::Drained)
    }
}

// ── Real implementations ─────────────────────────────────────────────────────

/// Runs commands as real subprocesses on U0.
pub struct ProcessRunner;

#[async_trait]
impl CommandRunner for ProcessRunner {
    async fn run(&self, program: &str, args: &[String]) -> Result<CommandOutput> {
        let out = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await
            .with_context(|| format!("failed to spawn {program}"))?;
        Ok(CommandOutput {
            // A signal-killed child has no code; -1 is non-zero, so it fails
            // closed through `CommandOutput::ok`.
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// [`DrainTargetResolver`] backed by the profile registry.
pub struct RegistryResolver<'a> {
    pub store: &'a (dyn crate::profiles::store::ProfileStore + Send + Sync),
}

#[async_trait]
impl DrainTargetResolver for RegistryResolver<'_> {
    async fn resolve(&self, hostname: &str) -> Result<DrainTarget> {
        let config = crate::profiles::resolve::resolve_from_registry(self.store, hostname).await?;
        Ok(DrainTarget {
            network_address: config.network_address.clone(),
            applications: config.applications.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use uaa_core::network::ssh_installer::config::CockroachSpec;

    // ── Fakes ────────────────────────────────────────────────────────────────

    #[derive(Default)]
    struct FakeRunner {
        /// Queued responses, consumed in order.
        responses: Mutex<Vec<Result<CommandOutput, String>>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeRunner {
        fn with(responses: Vec<Result<CommandOutput, String>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn ok(stdout: &str) -> Result<CommandOutput, String> {
            Ok(CommandOutput {
                status: 0,
                stdout: stdout.to_string(),
                stderr: String::new(),
            })
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CommandRunner for FakeRunner {
        async fn run(&self, program: &str, args: &[String]) -> Result<CommandOutput> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{program} {}", args.join(" ")));
            let mut r = self.responses.lock().unwrap();
            if r.is_empty() {
                return Err(anyhow!("FakeRunner: unexpected extra call to {program}"));
            }
            match r.remove(0) {
                Ok(o) => Ok(o),
                Err(e) => Err(anyhow!(e)),
            }
        }
    }

    struct FakeResolver(DrainTarget);

    #[async_trait]
    impl DrainTargetResolver for FakeResolver {
        async fn resolve(&self, _hostname: &str) -> Result<DrainTarget> {
            Ok(self.0.clone())
        }
    }

    /// Never really sleeps, and advances a virtual clock by the slept duration
    /// so timeout tests terminate.
    struct FakeClock {
        now: Mutex<SystemTime>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                now: Mutex::new(SystemTime::UNIX_EPOCH),
            }
        }
    }

    #[async_trait]
    impl Clock for FakeClock {
        fn now(&self) -> SystemTime {
            *self.now.lock().unwrap()
        }
        async fn sleep(&self, dur: Duration) {
            let mut n = self.now.lock().unwrap();
            *n += dur;
        }
    }

    fn crdb_target(applications: Vec<ApplicationSpec>) -> DrainTarget {
        DrainTarget {
            network_address: "172.16.3.96/23".to_string(),
            applications,
        }
    }

    fn crdb_app(policy: DecommissionPolicy) -> ApplicationSpec {
        let mut spec: CockroachSpec =
            serde_yaml::from_str("seed_ip: 172.16.3.92").expect("minimal cockroach spec");
        spec.decommission = policy;
        ApplicationSpec::Cockroach(spec)
    }

    const STATUS_HEADER: &str = "id\taddress\tsql_address\tis_live\treplicas";

    fn status(rows: &[(&str, &str, u64)]) -> String {
        let mut s = String::from(STATUS_HEADER);
        for (id, addr, replicas) in rows {
            s.push_str(&format!("\n{id}\t{addr}\t{addr}\ttrue\t{replicas}"));
        }
        s
    }

    fn drainer<'a>(
        resolver: &'a FakeResolver,
        runner: &'a FakeRunner,
        clock: &'a FakeClock,
    ) -> SpecDrainer<'a> {
        SpecDrainer {
            resolver,
            runner,
            clock,
            config: DrainerConfig {
                admin_host: "172.16.3.92:36357".to_string(),
                ..DrainerConfig::default()
            },
        }
    }

    // ── Parsing ──────────────────────────────────────────────────────────────

    #[test]
    fn parses_status_by_column_name_not_position() {
        // Deliberately shuffled relative to the usual layout: a positional
        // parser would read the address as the id and blow up, or worse, read
        // some other column as `replicas` and report a false zero.
        let tsv = "replicas\tis_live\taddress\tid\n\
                   42\ttrue\t172.16.3.96:36357\t8";
        let rows = parse_node_status(tsv).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 8);
        assert_eq!(rows[0].address, "172.16.3.96:36357");
        assert_eq!(rows[0].replicas, Some(42));
        assert_eq!(rows[0].is_live, Some(true));
    }

    #[test]
    fn a_missing_required_column_is_an_error_not_a_guess() {
        let err = parse_node_status("nodeid\taddress\n8\t172.16.3.96:36357").unwrap_err();
        assert!(
            err.to_string().contains("no \"id\" column"),
            "expected a loud missing-column error, got: {err}"
        );
    }

    #[test]
    fn empty_output_is_an_error_not_an_empty_cluster() {
        // An empty roster and a failed query are indistinguishable here, and
        // treating "no output" as "no members" would report every node drained.
        assert!(parse_node_status("").is_err());
        assert!(parse_node_status("   \n\n").is_err());
    }

    #[test]
    fn node_id_matches_on_ip_ignoring_the_advertise_port() {
        let rows = parse_node_status(&status(&[
            ("7", "172.16.3.94:36357", 100),
            ("8", "172.16.3.96:36357", 42),
        ]))
        .unwrap();
        // The registry knows the host by IP; the port is spec-derived and must
        // not decide membership.
        assert_eq!(
            CockroachCluster::node_id_for(&rows, "172.16.3.96").unwrap(),
            Some(8)
        );
        assert_eq!(
            CockroachCluster::node_id_for(&rows, "172.16.3.99").unwrap(),
            None,
            "a non-member must read as None (nothing to drain), not an error"
        );
    }

    #[test]
    fn two_nodes_on_one_ip_refuses_to_guess() {
        let rows = parse_node_status(&status(&[
            ("8", "172.16.3.96:36357", 42),
            ("9", "172.16.3.96:36358", 7),
        ]))
        .unwrap();
        let err = CockroachCluster::node_id_for(&rows, "172.16.3.96").unwrap_err();
        assert!(err.to_string().contains("refusing to guess"), "got: {err}");
    }

    // ── needs_drain ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn needs_drain_reads_the_spec_not_the_hostname() {
        let runner = FakeRunner::default();
        let clock = FakeClock::new();

        let stateless = FakeResolver(crdb_target(vec![ApplicationSpec::PrometheusNodeExporter(
            serde_yaml::from_str("{}").unwrap(),
        )]));
        assert!(!drainer(&stateless, &runner, &clock)
            .needs_drain("len-serv-003")
            .await
            .unwrap());

        let clustered = FakeResolver(crdb_target(vec![crdb_app(
            DecommissionPolicy::cockroach_default(),
        )]));
        assert!(drainer(&clustered, &runner, &clock)
            .needs_drain("len-serv-003")
            .await
            .unwrap());

        // Same hostname both times: the answer came from the spec.
        assert!(
            runner.calls().is_empty(),
            "needs_drain must not touch the cluster"
        );
    }

    #[tokio::test]
    async fn a_disabled_policy_needs_no_drain() {
        let runner = FakeRunner::default();
        let clock = FakeClock::new();
        let resolver = FakeResolver(crdb_target(vec![crdb_app(DecommissionPolicy::default())]));
        assert!(!drainer(&resolver, &runner, &clock)
            .needs_drain("len-serv-003")
            .await
            .unwrap());
    }

    // ── drain, happy path ────────────────────────────────────────────────────

    #[tokio::test]
    async fn drain_decommissions_waits_then_stops_the_unit() {
        let runner = FakeRunner::with(vec![
            // node status (resolve id)
            FakeRunner::ok(&status(&[("8", "172.16.3.96:36357", 42)])),
            // node decommission
            FakeRunner::ok(""),
            // wait poll 1: still holding
            FakeRunner::ok(&status(&[("8", "172.16.3.96:36357", 17)])),
            // wait poll 2: drained
            FakeRunner::ok(&status(&[("8", "172.16.3.96:36357", 0)])),
            // systemctl stop
            FakeRunner::ok(""),
        ]);
        let clock = FakeClock::new();
        let resolver = FakeResolver(crdb_target(vec![crdb_app(
            DecommissionPolicy::cockroach_default(),
        )]));

        let outcome = drainer(&resolver, &runner, &clock)
            .drain("len-serv-003")
            .await
            .unwrap();
        assert_eq!(outcome, DrainStatus::Drained);

        let calls = runner.calls();
        // The order is the whole point: CockroachDB drains a LIVE node, so the
        // unit must still be running while replicas move.
        assert!(calls[1].contains("node decommission 8"), "{calls:?}");
        assert!(
            calls.last().unwrap().starts_with("ssh "),
            "the unit must be stopped LAST, after the drain: {calls:?}"
        );
        assert!(calls
            .last()
            .unwrap()
            .contains("systemctl stop cockroach.service"));
        assert!(
            calls[1].contains("--wait=none"),
            "the authored timeout owns the deadline, not cockroach's own wait: {calls:?}"
        );
    }

    #[tokio::test]
    async fn a_host_the_cluster_never_heard_of_is_already_drained() {
        // len-serv-003's real state: decommissioned long ago, still running.
        // Erroring here would make an already-safe host un-reinstallable.
        let runner = FakeRunner::with(vec![
            FakeRunner::ok(&status(&[("7", "172.16.3.94:36357", 100)])),
            FakeRunner::ok(""), // systemctl stop still runs
        ]);
        let clock = FakeClock::new();
        let resolver = FakeResolver(crdb_target(vec![crdb_app(
            DecommissionPolicy::cockroach_default(),
        )]));

        assert_eq!(
            drainer(&resolver, &runner, &clock)
                .drain("len-serv-003")
                .await
                .unwrap(),
            DrainStatus::Drained
        );
    }

    // ── drain, fail-closed paths ─────────────────────────────────────────────

    #[tokio::test]
    async fn replicas_that_never_reach_zero_report_incomplete() {
        let mut responses = vec![
            FakeRunner::ok(&status(&[("8", "172.16.3.96:36357", 42)])),
            FakeRunner::ok(""),
        ];
        // Stuck at 30 forever. The 3600s timeout / 30s poll means the virtual
        // clock needs 120 polls to expire.
        for _ in 0..200 {
            responses.push(FakeRunner::ok(&status(&[("8", "172.16.3.96:36357", 30)])));
        }
        let runner = FakeRunner::with(responses);
        let clock = FakeClock::new();
        let resolver = FakeResolver(crdb_target(vec![crdb_app(
            DecommissionPolicy::cockroach_default(),
        )]));

        let outcome = drainer(&resolver, &runner, &clock)
            .drain("len-serv-003")
            .await
            .unwrap();
        assert_eq!(
            outcome,
            DrainStatus::Incomplete { replicas_left: 30 },
            "a stuck drain must refuse, never fall through to the wipe"
        );
        // Crucially: the unit was never stopped and no further steps ran.
        assert!(
            !runner.calls().iter().any(|c| c.starts_with("ssh ")),
            "an incomplete drain must not proceed to later steps"
        );
    }

    #[tokio::test]
    async fn a_roster_row_with_an_unreadable_replica_count_is_not_zero() {
        // `replicas` column missing entirely — the node is still listed, so we
        // know nothing about what it holds. Must not read as drained.
        let runner = FakeRunner::with(vec![
            FakeRunner::ok("id\taddress\n8\t172.16.3.96:36357"),
            FakeRunner::ok(""),
            FakeRunner::ok("id\taddress\n8\t172.16.3.96:36357"),
        ]);
        let clock = FakeClock::new();
        let resolver = FakeResolver(crdb_target(vec![crdb_app(
            DecommissionPolicy::cockroach_default(),
        )]));

        let err = drainer(&resolver, &runner, &clock)
            .drain("len-serv-003")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("unreadable"),
            "expected a refusal to infer zero, got: {err}"
        );
    }

    #[tokio::test]
    async fn a_failing_cockroach_command_errors_rather_than_reporting_drained() {
        let runner = FakeRunner::with(vec![Ok(CommandOutput {
            status: 1,
            stdout: String::new(),
            stderr: "connection refused".to_string(),
        })]);
        let clock = FakeClock::new();
        let resolver = FakeResolver(crdb_target(vec![crdb_app(
            DecommissionPolicy::cockroach_default(),
        )]));

        let err = drainer(&resolver, &runner, &clock)
            .drain("len-serv-003")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("connection refused"), "got: {err}");
    }

    #[tokio::test]
    async fn a_failing_systemctl_stop_is_an_error() {
        let runner = FakeRunner::with(vec![
            FakeRunner::ok(&status(&[("8", "172.16.3.96:36357", 0)])),
            FakeRunner::ok(""),
            FakeRunner::ok(&status(&[("8", "172.16.3.96:36357", 0)])),
            Ok(CommandOutput {
                status: 255,
                stdout: String::new(),
                stderr: "Permission denied (publickey)".to_string(),
            }),
        ]);
        let clock = FakeClock::new();
        let resolver = FakeResolver(crdb_target(vec![crdb_app(
            DecommissionPolicy::cockroach_default(),
        )]));

        let err = drainer(&resolver, &runner, &clock)
            .drain("len-serv-003")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Permission denied"), "got: {err}");
    }

    #[tokio::test]
    async fn enabled_with_no_steps_refuses_instead_of_no_opping() {
        let runner = FakeRunner::default();
        let clock = FakeClock::new();
        let resolver = FakeResolver(crdb_target(vec![crdb_app(DecommissionPolicy {
            enabled: true,
            steps: Vec::new(),
            ..DecommissionPolicy::default()
        })]));

        let err = drainer(&resolver, &runner, &clock)
            .drain("len-serv-003")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no steps"), "got: {err}");
    }

    #[tokio::test]
    async fn a_stateless_host_touches_nothing() {
        let runner = FakeRunner::default();
        let clock = FakeClock::new();
        let resolver = FakeResolver(crdb_target(vec![ApplicationSpec::Zsh(
            serde_yaml::from_str("user: jdfalk").unwrap(),
        )]));

        assert_eq!(
            drainer(&resolver, &runner, &clock)
                .drain("rpi-serv-001")
                .await
                .unwrap(),
            DrainStatus::Drained
        );
        assert!(runner.calls().is_empty());
    }
}
