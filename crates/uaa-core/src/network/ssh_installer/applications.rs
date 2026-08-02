// file: crates/uaa-core/src/network/ssh_installer/applications.rs
// version: 1.4.2
// guid: dc8e60fb-8d31-4869-96bf-bf6203d3a530
// last-edited: 2026-08-02

//! `ApplicationInstaller`: dispatches per-application installation for
//! `config.applications` (DS-APP-02).
//!
//! **FAIL-CLOSED by design.** Unlike `ResetPartitionStager` (a non-fatal
//! recovery nicety), an application failing to install is a failed
//! deployment. Every error path here propagates with `?` out to
//! `phase_5_system_configuration` and fails the install. Never
//! warn-and-continue.
//!
//! The Cockroach install body (DS-APP-03) ports `setup_cockroachdb.sh` — a
//! script that previously lived only on the netboot server, was fetched
//! over plain HTTP at first boot, and `rm`'d itself after running — into a
//! chroot-executed Rust step. Removing that fetch-and-exec from the boot
//! path is a real security improvement, not just a refactor.
//!
//! With `applications: []` (every committed config today) `install()` is a
//! no-op: zero commands are executed and `Ok(())` is returned, so behavior
//! is byte-identical to before this module existed.

use super::config::{
    ApplicationSpec, CanonicalLivepatchSpec, CockroachRolloutAgentSpec, CockroachSpec,
    InstallationConfig, PrometheusNodeExporterSpec, ReportStatusSpec, TangServerSpec, ZshSpec,
};
use crate::autoinstall::host_spec::HostSpec;
use crate::error::AutoInstallError;
use crate::network::CommandExecutor;
use crate::Result;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use std::collections::HashSet;
use std::io::Write as _;

/// The only filenames the `/api/certs/` response is permitted to write.
/// `fname` in the fetched JSON is attacker-reachable (the fetch is plain
/// HTTP; a network MITM controls the response), so it is checked against
/// this allowlist before ever being used as part of a path — this is what
/// keeps a key like `../../etc/cron.d/x` from escaping the certs dir.
const COCKROACH_CERT_FILENAMES: &[&str] = &["ca.crt", "node.crt", "node.key"];

/// Authoring placeholder for secret-bearing fields, substituted at place time.
/// Kept in sync with `config.rs`'s `default_placeholder`.
const PLACEHOLDER: &str = "REPLACE_AT_PLACE_TIME";

/// The on-disk directory a CockroachDB `--store` value points at.
///
/// Accepts both forms cockroach takes: the bare path
/// (`/var/lib/cockroach/data`, len-serv-003's drifted form) and the key=value
/// form used by len-serv-001/002
/// (`path=/var/lib/cockroach/cockroach-data,attrs=ssd,size=.5`). Without this,
/// `mkdir -p` on the raw value would create a directory literally named
/// `path=...,attrs=ssd,size=.5` and cockroach would then create its real store
/// as root.
pub fn store_directory(store: &str) -> &str {
    for field in store.split(',') {
        if let Some(path) = field.strip_prefix("path=") {
            return path;
        }
    }
    store
}

/// Directory component of a path, for `ReadWritePaths` on a log FILE.
/// systemd sandboxing grants directories; naming the file itself would leave
/// the agent unable to rotate or recreate its own log.
fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => ".",
    }
}

/// Installs every application declared in `InstallationConfig::applications`
/// into the target. Mirrors `ResetPartitionStager`'s module shape: a
/// self-contained struct borrowing the phase's executor, with one primary
/// `pub async fn` taking `&InstallationConfig`.
pub struct ApplicationInstaller<'a> {
    runner: &'a mut dyn CommandExecutor,
}

impl<'a> ApplicationInstaller<'a> {
    /// Create a new installer borrowing the phase's command executor.
    pub fn new(runner: &'a mut dyn CommandExecutor) -> Self {
        Self { runner }
    }

    /// Install every application in `config.applications` into the target.
    /// Empty list = no-op returning `Ok(())` with zero commands executed.
    /// FAIL-CLOSED: any application's failure propagates and fails the
    /// install; this must never be wrapped in a warn-and-continue pattern
    /// by callers.
    pub async fn install(&mut self, config: &InstallationConfig) -> Result<()> {
        if config.applications.is_empty() {
            return Ok(());
        }
        Self::reject_duplicates(&config.applications)?;
        for app in &config.applications {
            match app {
                ApplicationSpec::Cockroach(spec) => {
                    self.install_cockroach(config, spec).await?;
                }
                ApplicationSpec::TangServer(spec) => {
                    self.install_tang_server(&config.hostname, spec).await?;
                }
                ApplicationSpec::CockroachRolloutAgent(spec) => {
                    self.install_cockroach_rollout_agent(spec).await?;
                }
                ApplicationSpec::PrometheusNodeExporter(spec) => {
                    self.install_prometheus_node_exporter(spec).await?;
                }
                ApplicationSpec::CanonicalLivepatch(spec) => {
                    self.install_canonical_livepatch(spec).await?;
                }
                ApplicationSpec::ReportStatus(spec) => {
                    self.install_report_status(spec).await?;
                }
                ApplicationSpec::Zsh(spec) => {
                    self.install_zsh(spec).await?;
                }
            }
        }
        Ok(())
    }

    /// TangServer applications are expressibility-only for now: no applier
    /// exists (rpi Tang is provisioned outside this installer today), so
    /// dispatch is a no-op skip — never an error, never a panic — logged at
    /// warn so an authored-but-unactioned application is visible in logs.
    async fn install_tang_server(&mut self, hostname: &str, _spec: &TangServerSpec) -> Result<()> {
        tracing::warn!(
            "TangServer application authored but installer not implemented (host={hostname}) — skipping"
        );
        Ok(())
    }

    /// CockroachDB rollout agent: binary, env file, unit.
    ///
    /// Unit shape is a transcription of the one running on len-serv-001/002
    /// (read 2026-07-30), including `EnvironmentFile=-` (leading `-` so a
    /// missing env file is non-fatal) and the `ProtectSystem=strict` +
    /// explicit `ReadWritePaths` sandbox. `ReadWritePaths` must include the
    /// cockroach binary path: the agent replaces that binary during a rollout,
    /// and under `ProtectSystem=strict` it cannot without an explicit grant.
    async fn install_cockroach_rollout_agent(
        &mut self,
        spec: &CockroachRolloutAgentSpec,
    ) -> Result<()> {
        let bin = &spec.binary_path;
        self.chroot_exec(&format!(
            "curl -fsSL -o {bin} '{}' && chmod 0755 {bin}",
            spec.binary_url
        ))
        .await?;

        self.chroot_exec(&format!(
            "mkdir -p {} $(dirname {}) && chown -R cockroach:cockroach {} $(dirname {})",
            spec.artifacts_dir, spec.audit_log, spec.artifacts_dir, spec.audit_log
        ))
        .await?;

        // Quoted heredoc: the env file is data, never shell-expanded. A
        // placeholder secret is written through as-is and substituted at place
        // time — it is an authoring state, not an install failure.
        if spec.database_url == PLACEHOLDER {
            tracing::warn!(
                "cockroach-rollout-agent database_url is still {PLACEHOLDER} — \
                 writing env file with the placeholder; substitute it at place time"
            );
        }
        let env = format!(
            "# file: /etc/cockroach-rollout-agent.env\n\
             # Written by uaa. Host-local; do not commit.\n\
             CROACH_ROLLOUT_DATABASE_URL={db}\n\
             CROACH_ROLLOUT_SSL_ROOT_CERT={certs}/ca.crt\n\
             CROACH_ROLLOUT_SSL_CLIENT_CERT={certs}/node.crt\n\
             CROACH_ROLLOUT_SSL_CLIENT_KEY={certs}/node.key\n\
             CROACH_ROLLOUT_ARTIFACTS_DIR={artifacts}\n\
             CROACH_ROLLOUT_AUDIT_LOG={audit}\n\
             CROACH_ROLLOUT_BINARY_PATH=/usr/local/bin/cockroach\n\
             CROACH_ROLLOUT_SERVICE={service}\n",
            db = spec.database_url,
            certs = spec.certs_dir,
            artifacts = spec.artifacts_dir,
            audit = spec.audit_log,
            service = spec.service,
        );
        self.runner
            .execute(&format!(
                "cat > /mnt/targetos/etc/cockroach-rollout-agent.env \
                 <<'UAA_CRRA_ENV_EOF'\n{env}UAA_CRRA_ENV_EOF"
            ))
            .await?;
        self.chroot_exec("chmod 0640 /etc/cockroach-rollout-agent.env")
            .await?;
        self.chroot_exec("chown root:cockroach /etc/cockroach-rollout-agent.env")
            .await?;

        let unit = format!(
            "[Unit]\n\
             Description=CockroachDB Rollout Agent\n\
             After=network-online.target cockroach.service\n\
             Wants=network-online.target\n\
             [Service]\n\
             Type=simple\n\
             User=cockroach\n\
             Group=cockroach\n\
             EnvironmentFile=-/etc/cockroach-rollout-agent.env\n\
             ExecStart={bin} daemon\n\
             Restart=on-failure\n\
             RestartSec=10s\n\
             NoNewPrivileges=true\n\
             PrivateTmp=true\n\
             ProtectSystem=strict\n\
             ReadWritePaths={audit_dir} {artifacts} /usr/local/bin/cockroach\n\
             [Install]\n\
             WantedBy=multi-user.target\n",
            bin = bin,
            audit_dir = parent_dir(&spec.audit_log),
            artifacts = spec.artifacts_dir,
        );
        self.runner
            .execute(&format!(
                "mkdir -p /mnt/targetos/etc/systemd/system && \
                 cat > /mnt/targetos/etc/systemd/system/cockroach-rollout-agent.service \
                 <<'UAA_CRRA_UNIT_EOF'\n{unit}UAA_CRRA_UNIT_EOF"
            ))
            .await?;
        self.chroot_exec("systemctl daemon-reload").await?;

        // len-serv-003 carried this unit INSTALLED BUT DISABLED as of
        // 2026-07-28. Default reproduces that rather than silently enabling a
        // service the fleet deliberately leaves off.
        if spec.enabled {
            self.chroot_exec("systemctl enable cockroach-rollout-agent")
                .await?;
        } else {
            tracing::info!(
                "cockroach-rollout-agent installed but left disabled (spec.enabled=false)"
            );
        }
        Ok(())
    }

    /// Prometheus node exporter. Distro package, so apt owns the version.
    async fn install_prometheus_node_exporter(
        &mut self,
        spec: &PrometheusNodeExporterSpec,
    ) -> Result<()> {
        self.chroot_exec(
            "DEBIAN_FRONTEND=noninteractive apt-get install -y prometheus-node-exporter",
        )
        .await?;

        // The packaged unit reads ARGS from /etc/default/prometheus-node-exporter,
        // so override there rather than editing the vendor unit.
        if !spec.listen_address.is_empty() {
            let defaults = format!(
                "ARGS=\"--web.listen-address={}\"\n",
                spec.listen_address
            );
            self.runner
                .execute(&format!(
                    "cat > /mnt/targetos/etc/default/prometheus-node-exporter \
                     <<'UAA_NODEEXP_EOF'\n{defaults}UAA_NODEEXP_EOF"
                ))
                .await?;
        }
        self.chroot_exec("systemctl enable prometheus-node-exporter")
            .await?;
        Ok(())
    }

    /// Canonical Livepatch (snap).
    ///
    /// Enabling requires a real token. If the spec still carries the authoring
    /// placeholder we install the snap and SKIP the enable rather than running
    /// `canonical-livepatch enable REPLACE_AT_PLACE_TIME`, which would fail
    /// and abort the whole install for what is an authoring state.
    async fn install_canonical_livepatch(&mut self, spec: &CanonicalLivepatchSpec) -> Result<()> {
        self.chroot_exec("snap install canonical-livepatch").await?;
        if spec.key == PLACEHOLDER {
            tracing::warn!(
                "canonical-livepatch key is still {PLACEHOLDER} — snap installed but NOT enabled; \
                 substitute the token at place time and run `canonical-livepatch enable <key>`"
            );
            return Ok(());
        }
        // Quoted heredoc keeps the token off the command line.
        self.runner
            .execute(&format!(
                "chroot /mnt/targetos /bin/bash -s <<'UAA_LIVEPATCH_EOF'\n\
                 canonical-livepatch enable {}\n\
                 UAA_LIVEPATCH_EOF",
                spec.key
            ))
            .await?;
        Ok(())
    }

    /// `/usr/local/bin/report-status.sh` — the cloud-init webhook reporter.
    ///
    /// Transcribed from the deployed script on len-serv-002 (read 2026-07-30),
    /// with the previously hardcoded webhook URL lifted into the spec.
    async fn install_report_status(&mut self, spec: &ReportStatusSpec) -> Result<()> {
        let script = format!(
            "#!/bin/bash\n\
             # Report status script. Written by uaa.\n\
             WEBHOOK_URL=\"{url}\"\n\
             HOSTNAME=$(hostname)\n\
             TIMESTAMP=$(date +%s)\n\
             SOURCE_IP=$(hostname -I | awk '{{print $1}}')\n\
             \n\
             STATUS=$1\n\
             PROGRESS=$2\n\
             MESSAGE=$3\n\
             \n\
             json_payload=$(jq -n \\\n\
             \x20   --arg origin \"cloud-init\" \\\n\
             \x20   --argjson timestamp \"$TIMESTAMP\" \\\n\
             \x20   --arg event_type \"status_update\" \\\n\
             \x20   --arg name \"$HOSTNAME\" \\\n\
             \x20   --arg description \"Cloud-init status update\" \\\n\
             \x20   --arg source_ip \"$SOURCE_IP\" \\\n\
             \x20   --arg status \"${{STATUS:-pending}}\" \\\n\
             \x20   --argjson progress \"${{PROGRESS:-null}}\" \\\n\
             \x20   --arg message \"${{MESSAGE:-}}\" \\\n\
             \x20   '{{\"origin\": $origin, \"timestamp\": $timestamp, \
             \"event_type\": $event_type, \"name\": $name, \
             \"description\": $description, \"source_ip\": $source_ip, \
             \"status\": $status, \"progress\": $progress, \"message\": $message}}')\n\
             \n\
             curl -X POST \"$WEBHOOK_URL\" -H \"Content-Type: application/json\" -d \"$json_payload\"\n",
            url = spec.webhook_url,
        );
        self.runner
            .execute(&format!(
                "mkdir -p /mnt/targetos/usr/local/bin && \
                 cat > /mnt/targetos/usr/local/bin/report-status.sh \
                 <<'UAA_REPORTSTATUS_EOF'\n{script}UAA_REPORTSTATUS_EOF"
            ))
            .await?;
        self.chroot_exec("chmod 0755 /usr/local/bin/report-status.sh")
            .await?;
        // The script pipes through jq; without it every invocation is a
        // silent no-op that still exits 0.
        self.chroot_exec("DEBIAN_FRONTEND=noninteractive apt-get install -y jq curl")
            .await?;
        Ok(())
    }

    /// zsh as an operator login shell, optionally with oh-my-zsh.
    async fn install_zsh(&mut self, spec: &ZshSpec) -> Result<()> {
        self.chroot_exec("DEBIAN_FRONTEND=noninteractive apt-get install -y zsh")
            .await?;
        // `chsh` against a user that does not exist yet is a hard failure, and
        // Phase 5 runs after user provisioning, so guard rather than assume.
        self.chroot_exec(&format!(
            "getent passwd {user} >/dev/null && chsh -s /usr/bin/zsh {user}",
            user = spec.user
        ))
        .await?;

        if spec.oh_my_zsh {
            // Unattended install; RUNZSH=no keeps it from exec'ing a shell and
            // hanging the install, CHSH=no because chsh already ran above.
            self.chroot_exec(&format!(
                "su - {user} -c 'RUNZSH=no CHSH=no sh -c \"$(curl -fsSL \
                 https://raw.githubusercontent.com/ohmyzsh/ohmyzsh/master/tools/install.sh)\"'",
                user = spec.user
            ))
            .await?;
        }
        Ok(())
    }

    /// Install and start a CockroachDB node in the target, porting
    /// `setup_cockroachdb.sh` step for step: arch detect → download+install
    /// binary → `useradd`/dirs/`chown` → cert fetch + write + perms →
    /// write `/etc/systemd/system/cockroach.service` → `daemon-reload` →
    /// `enable` → `start`. Every step propagates with `?` — a partially
    /// installed node is a failed deployment, never a warning.
    async fn install_cockroach(
        &mut self,
        config: &InstallationConfig,
        spec: &CockroachSpec,
    ) -> Result<()> {
        let self_ip = HostSpec::ip_without_cidr(&config.network_address);

        // 1. Arch-aware download and install of the cockroach binary.
        let install_binary_cmd = format!(
            "ARCH=$(uname -m); if [ \"$ARCH\" = \"aarch64\" ] || [ \"$ARCH\" = \"arm64\" ]; then \
             CRDB_ARCH=linux-arm64; else CRDB_ARCH=linux-amd64; fi; \
             curl -sSfL \"https://binaries.cockroachdb.com/cockroach-{version}.${{CRDB_ARCH}}.tgz\" | tar xz -C /tmp && \
             cp -f \"/tmp/cockroach-{version}.${{CRDB_ARCH}}/cockroach\" /usr/local/bin/cockroach && \
             rm -rf \"/tmp/cockroach-{version}.${{CRDB_ARCH}}\"",
            version = spec.version
        );
        self.chroot_exec(&install_binary_cmd).await?;

        // 2. cockroach user + data/certs directories.
        self.chroot_exec("useradd -r -m -d /var/lib/cockroach cockroach 2>/dev/null || true")
            .await?;
        // The data directory must match whatever `--store` actually points at,
        // or cockroach starts by creating an unowned directory as root.
        let store_dir = store_directory(&spec.store);
        self.chroot_exec(&format!(
            "mkdir -p /var/lib/cockroach/certs {store_dir} && \
             chown -R cockroach:cockroach /var/lib/cockroach {store_dir}",
        ))
        .await?;

        // 3. Fetch node certs from the install CA endpoint. Fail-closed:
        // any HTTP failure or `ok: false` body aborts before a unit that
        // would fail to start is ever written.
        let cert_url = format!(
            "http://172.16.2.30:25000/api/certs/{}?ip={}",
            config.hostname, self_ip
        );
        let cert_json = self
            .runner
            .execute_with_output(&Self::chroot_wrap(&format!("curl -fsSL \"{cert_url}\"")))
            .await
            .map_err(|e| {
                AutoInstallError::ConfigError(format!(
                    "cert fetch from {cert_url} failed: {e}"
                ))
            })?;
        let parsed: serde_json::Value = serde_json::from_str(&cert_json).map_err(|e| {
            AutoInstallError::ConfigError(format!(
                "cert fetch from {cert_url} returned unparseable JSON ({e}); body: {cert_json}"
            ))
        })?;
        let ok = parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if !ok {
            return Err(AutoInstallError::ConfigError(format!(
                "cert fetch from {cert_url} returned ok:false; body: {cert_json}"
            )));
        }
        let certs = parsed.get("certs").and_then(|v| v.as_object()).ok_or_else(|| {
            AutoInstallError::ConfigError(format!(
                "cert fetch from {cert_url} response missing 'certs' object; body: {cert_json}"
            ))
        })?;
        for (fname, b64_val) in certs {
            // Allowlist BEFORE anything else touches `fname` — this is the
            // only thing standing between an MITM'd response and a
            // path-traversal write outside the certs directory.
            if !COCKROACH_CERT_FILENAMES.contains(&fname.as_str()) {
                return Err(AutoInstallError::ConfigError(format!(
                    "cert fetch from {cert_url}: unexpected cert filename {fname:?} \
                     (expected one of {COCKROACH_CERT_FILENAMES:?}); refusing to write it"
                )));
            }
            let b64 = b64_val.as_str().ok_or_else(|| {
                AutoInstallError::ConfigError(format!(
                    "cert fetch from {cert_url}: cert entry '{fname}' is not a string"
                ))
            })?;
            let decoded = BASE64.decode(b64.trim()).map_err(|e| {
                AutoInstallError::ConfigError(format!(
                    "cert fetch from {cert_url}: cert '{fname}' is not valid base64: {e}"
                ))
            })?;

            // Decode in Rust and ship the raw bytes via upload_file (SCP
            // for a real SSH target, a plain copy for local mode) instead
            // of interpolating the fetched content into a shell command.
            // `fname` is allowlisted above and `decoded` is delivered as
            // file bytes, not shell text, so neither value is ever parsed
            // as shell syntax — the earlier `echo "{b64}" | base64 -d`
            // interpolated BOTH untrusted values into a root shell command,
            // which was a command-injection hole this port introduced
            // (the python original had no shell here at all).
            let mut tmp = tempfile::NamedTempFile::new().map_err(AutoInstallError::IoError)?;
            tmp.write_all(&decoded).map_err(AutoInstallError::IoError)?;
            tmp.flush().map_err(AutoInstallError::IoError)?;
            self.runner
                .upload_file(
                    tmp.path().to_str().unwrap_or("/tmp/uaa-cockroach-cert"),
                    &format!("/mnt/targetos/var/lib/cockroach/certs/{fname}"),
                )
                .await?;
        }
        self.chroot_exec("chown cockroach:cockroach /var/lib/cockroach/certs/*")
            .await?;
        self.chroot_exec(
            "chmod 644 /var/lib/cockroach/certs/ca.crt /var/lib/cockroach/certs/node.crt",
        )
        .await?;
        self.chroot_exec("chmod 600 /var/lib/cockroach/certs/node.key")
            .await?;

        // 4. Derive advertise/join. Members are sourced from
        // `config.cockroach_members` — populated by uaa-control's
        // `resolve_from_registry` from the active group allocation
        // (PS-COCKROACH-16), never a hardcoded constant.
        let (advertise, join) =
            derive_cockroach_endpoints(&config.network_address, &config.cockroach_members, spec);
        // len-serv-001/002 leave listen/sql PORT-ONLY and bind advertise to the
        // IP. len-serv-003 drifted to IP-bound listen/sql, which is why
        // `cockroach sql --host=127.0.0.1:36257` is refused there and works on
        // 001/002. Standardize on the 001/002 form.
        //
        // NOTE: only listen and sql become port-only. `--advertise-addr` MUST
        // stay `IP:port` — it is the address peers dial to reach this node, and
        // a port-only advertise breaks cluster join.
        let listen_addr = format!(":{}", spec.port);
        let sql_addr = format!(":{}", spec.sql_port);

        // 5. Write the systemd unit directly at its host-visible path
        // (/mnt/targetos/... is the target's own root, already mounted) so
        // the unit content never needs to be nested inside a quoted chroot
        // argument.
        let unit = format!(
            "[Unit]\n\
             Description=CockroachDB\n\
             After=network-online.target\n\
             [Service]\n\
             User=cockroach\n\
             ExecStart=/usr/local/bin/cockroach start \\\n\
             \x20 --store={store} \\\n\
             \x20 --certs-dir=/var/lib/cockroach/certs \\\n\
             \x20 --listen-addr={listen_addr} \\\n\
             \x20 --advertise-addr={advertise} \\\n\
             \x20 --sql-addr={sql_addr} \\\n\
             \x20 --join={join} \\\n\
             \x20 --cache={cache} \\\n\
             \x20 --max-sql-memory={max_sql} \\\n\
             \x20 --locality={locality} \\\n\
             \x20 --http-addr={http_addr}\n\
             Restart=always\n\
             RestartSec=10s\n\
             LimitNOFILE=500000\n\
             [Install]\n\
             WantedBy=multi-user.target\n",
            advertise = advertise,
            listen_addr = listen_addr,
            sql_addr = sql_addr,
            store = spec.store,
            join = join,
            cache = spec.cache,
            max_sql = spec.max_sql_memory,
            locality = spec.locality,
            http_addr = spec.http_addr,
        );
        self.runner
            .execute(&format!(
                "mkdir -p /mnt/targetos/etc/systemd/system && \
                 cat > /mnt/targetos/etc/systemd/system/cockroach.service <<'UAA_CRDB_UNIT_EOF'\n\
                 {unit}UAA_CRDB_UNIT_EOF"
            ))
            .await?;

        // 6. Enable and start.
        self.chroot_exec("systemctl daemon-reload").await?;
        self.chroot_exec("systemctl enable cockroach").await?;
        self.chroot_exec("systemctl start cockroach").await?;

        Ok(())
    }

    /// Wrap `cmd` for execution inside the target chroot, mirroring
    /// `system_setup.rs`'s established shape. `cmd` must not contain a
    /// literal single quote.
    fn chroot_wrap(cmd: &str) -> String {
        format!("chroot /mnt/targetos bash -lc '{cmd}'")
    }

    /// Run `cmd` inside the target chroot via the borrowed executor,
    /// propagating any failure with `?`.
    async fn chroot_exec(&mut self, cmd: &str) -> Result<()> {
        self.runner.execute(&Self::chroot_wrap(cmd)).await
    }

    /// Reject a config listing the same application kind more than once,
    /// before running anything. Two nodes of the same app on one host is
    /// always a config mistake; installing the second over the first would
    /// silently corrupt the first.
    fn reject_duplicates(apps: &[ApplicationSpec]) -> Result<()> {
        let mut seen: HashSet<&'static str> = HashSet::new();
        for app in apps {
            let kind = app.kind();
            if !seen.insert(kind) {
                return Err(crate::error::AutoInstallError::ConfigError(format!(
                    "duplicate application kind in config: {kind}"
                )));
            }
        }
        Ok(())
    }
}

/// Build (advertise, join) for this host. `members` are sibling
/// `network_address` values (CIDR form) from the group, EXCLUDING
/// soft-released ones.
///
/// Strips CIDR from self and every member before calling
/// [`HostSpec::compute_join`] — `compute_join` filters self BY IP, so an
/// unstripped self never matches and the node would list itself in its own
/// join string.
pub fn derive_cockroach_endpoints(
    self_network_address: &str,
    members: &[String],
    spec: &CockroachSpec,
) -> (String, String) {
    let self_ip = HostSpec::ip_without_cidr(self_network_address);
    let member_ips: Vec<&str> = members
        .iter()
        .map(|m| HostSpec::ip_without_cidr(m))
        .collect();
    let advertise = HostSpec::compute_advertise(self_ip, spec.port);
    let join = HostSpec::compute_join(&spec.seed_ip, &member_ips, self_ip, spec.port);
    (advertise, join)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::ssh_installer::config::InitramfsType;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    /// Records every command routed through the executor into a shared log
    /// so tests can assert on the recorded-command count, not just
    /// `is_ok()`. Mirrors `installer.rs`'s `RecordingExecutor`.
    #[derive(Clone, Default)]
    struct RecordingExecutor {
        commands: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingExecutor {
        fn new() -> Self {
            Self::default()
        }

        fn recorded(&self) -> Vec<String> {
            self.commands.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CommandExecutor for RecordingExecutor {
        async fn connect(&mut self, _host: &str, _user: &str) -> Result<()> {
            Ok(())
        }
        async fn execute(&mut self, cmd: &str) -> Result<()> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok(())
        }
        async fn execute_with_output(&mut self, cmd: &str) -> Result<String> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok(String::new())
        }
        async fn execute_with_error_collection(
            &mut self,
            cmd: &str,
            _desc: &str,
        ) -> Result<(i32, String, String)> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok((0, String::new(), String::new()))
        }
        async fn check_silent(&mut self, cmd: &str) -> Result<bool> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok(true)
        }
        async fn collect_debug_info(&mut self) -> Result<String> {
            Ok(String::new())
        }
        async fn upload_file(&mut self, _local_path: &str, _remote_path: &str) -> Result<()> {
            Ok(())
        }
        async fn download_file(&mut self, _remote_path: &str, _local_path: &str) -> Result<()> {
            Ok(())
        }
        fn disconnect(&mut self) {}
    }

    fn sample_config() -> InstallationConfig {
        InstallationConfig {
            hostname: "test-host".into(),
            disk_device: "/dev/nvme0n1".into(),
            timezone: "UTC".into(),
            luks_key: "key".into(),
            root_password: "root".into(),
            network_interface: "eth0".into(),
            network_address: "192.0.2.10/24".into(),
            network_gateway: "192.0.2.1".into(),
            network_search: "example.test".into(),
            network_nameservers: vec!["1.1.1.1".into()],
            network_renderer: crate::network::ssh_installer::config::default_network_renderer(),
            debootstrap_release: None,
            debootstrap_mirror: None,
            initramfs_type: InitramfsType::Dracut,
            tang_servers: vec![],
            tang_threshold: 2,
            ssh_authorized_keys: vec![],
            users: Vec::new(),
            enroll_tpm2: true,
            tpm2_pin: None,
            tpm2_pcr_ids: "7".into(),
            expect_fido2: true,
            install_ca_cert: "test-ca-pem".into(),
            applications: vec![],
            cockroach_members: Vec::new(),
            storage_mode: Default::default(),
            disks: Vec::new(),
            arch: Default::default(),
            role: Default::default(),
            firmware_quirks: Vec::new(),
            hooks: Default::default(),
            unlock_sss: None,
        }
    }

    fn sample_cockroach_spec() -> CockroachSpec {
        CockroachSpec {
            version: "v23.1.0".into(),
            port: 26257,
            sql_port: 26257,
            http_addr: "0.0.0.0:8080".into(),
            seed_ip: "192.0.2.50".into(),
            cache: "25%".into(),
            max_sql_memory: "25%".into(),
            locality: "region=default".into(),
            store: "path=/var/lib/cockroach/cockroach-data,attrs=ssd,size=.5".into(),
            decommission: crate::network::ssh_installer::config::DecommissionPolicy::cockroach_default(),
        }
    }

    fn sample_tang_server_spec() -> TangServerSpec {
        TangServerSpec {
            port: 80,
            key_directory: "/etc/tang/keys".into(),
        }
    }

    #[tokio::test]
    async fn test_empty_applications_runs_no_commands() {
        let mut executor = RecordingExecutor::new();
        let config = sample_config();
        let mut installer = ApplicationInstaller::new(&mut executor);

        let result = installer.install(&config).await;

        assert!(result.is_ok());
        assert_eq!(executor.recorded().len(), 0);
    }

    #[tokio::test]
    async fn test_duplicate_application_kind_rejected() {
        let mut executor = RecordingExecutor::new();
        let mut config = sample_config();
        config.applications = vec![
            ApplicationSpec::Cockroach(sample_cockroach_spec()),
            ApplicationSpec::Cockroach(sample_cockroach_spec()),
        ];
        let mut installer = ApplicationInstaller::new(&mut executor);

        let result = installer.install(&config).await;

        let err = result.expect_err("duplicate application kinds must be rejected");
        assert!(err.to_string().contains("cockroach"));
        assert_eq!(
            executor.recorded().len(),
            0,
            "rejection must happen before any command executes"
        );
    }

    #[tokio::test]
    async fn test_tang_server_dispatch_is_noop_skip() {
        // TangServer is expressibility-only for now (rpi, no applier): the
        // dispatch must skip it with Ok(()) and zero commands, never error
        // or panic.
        let mut executor = RecordingExecutor::new();
        let mut config = sample_config();
        config.applications = vec![ApplicationSpec::TangServer(sample_tang_server_spec())];
        let mut installer = ApplicationInstaller::new(&mut executor);

        let result = installer.install(&config).await;

        assert!(result.is_ok(), "TangServer dispatch must not error: {result:?}");
        assert_eq!(
            executor.recorded().len(),
            0,
            "TangServer has no applier yet; must run zero commands"
        );
    }

    #[tokio::test]
    async fn test_duplicate_tang_server_kind_rejected() {
        let mut executor = RecordingExecutor::new();
        let mut config = sample_config();
        config.applications = vec![
            ApplicationSpec::TangServer(sample_tang_server_spec()),
            ApplicationSpec::TangServer(sample_tang_server_spec()),
        ];
        let mut installer = ApplicationInstaller::new(&mut executor);

        let result = installer.install(&config).await;

        let err = result.expect_err("duplicate tang-server application kinds must be rejected");
        assert!(err.to_string().contains("tang-server"));
        assert_eq!(
            executor.recorded().len(),
            0,
            "rejection must happen before any command executes"
        );
    }

    #[tokio::test]
    async fn test_application_failure_propagates() {
        // Anti-over-suppression: prove a real application failure is not
        // swallowed anywhere in the dispatch loop.
        let mut executor = RecordingExecutor::new();
        let mut config = sample_config();
        config.applications = vec![ApplicationSpec::Cockroach(sample_cockroach_spec())];
        let mut installer = ApplicationInstaller::new(&mut executor);

        let result = installer.install(&config).await;

        assert!(result.is_err());
    }

    // --- derive_cockroach_endpoints: pure-function tests, no executor ---

    /// The GATE for PS-COCKROACH-16 (retiring the hardcoded fleet-member-IPs
    /// constant this module used to import from `host_spec`): the
    /// roster-derived (advertise, join) for each of the 3 len-serv nodes must
    /// equal the exact literal strings the retired constant produced —
    /// asserted here as bare literals (not re-derived from a second call into
    /// `HostSpec::compute_join`), so a regression in either
    /// `derive_cockroach_endpoints` or the group roster this task wires in
    /// cannot silently change the fleet's live join strings. These are the
    /// same values `host_spec.rs`'s `for_lenserv_matches_known_hosts`
    /// asserts for `HostSpec::for_lenserv`.
    #[test]
    fn test_cockroach_join_matches_former_lenserv_member_ips_constant() {
        use crate::autoinstall::host_spec::{COCKROACH_PORT, COCKROACH_SERVER_IP};

        let members: Vec<String> = ["172.16.3.92", "172.16.3.94", "172.16.3.96"]
            .iter()
            .map(|ip| ip.to_string())
            .collect();
        let mut spec = sample_cockroach_spec();
        spec.seed_ip = COCKROACH_SERVER_IP.to_string();
        spec.port = COCKROACH_PORT;

        let (advertise_1, join_1) = derive_cockroach_endpoints("172.16.3.92/23", &members, &spec);
        assert_eq!(advertise_1, "172.16.3.92:36357");
        assert_eq!(join_1, "172.16.2.30:36357,172.16.3.94:36357,172.16.3.96:36357");

        let (advertise_2, join_2) = derive_cockroach_endpoints("172.16.3.94/23", &members, &spec);
        assert_eq!(advertise_2, "172.16.3.94:36357");
        assert_eq!(join_2, "172.16.2.30:36357,172.16.3.92:36357,172.16.3.96:36357");

        let (advertise_3, join_3) = derive_cockroach_endpoints("172.16.3.96/23", &members, &spec);
        assert_eq!(advertise_3, "172.16.3.96:36357");
        assert_eq!(join_3, "172.16.2.30:36357,172.16.3.92:36357,172.16.3.94:36357");
    }

    #[test]
    fn test_derive_strips_cidr() {
        let mut spec = sample_cockroach_spec();
        spec.seed_ip = "172.16.2.30".to_string();
        let members = vec!["172.16.3.92/23".to_string(), "172.16.3.94/23".to_string()];

        let (advertise, join) = derive_cockroach_endpoints("172.16.3.92/23", &members, &spec);

        assert_eq!(advertise, format!("172.16.3.92:{}", spec.port));
        assert!(!advertise.contains('/'), "advertise must not carry a CIDR suffix");
        assert!(
            !join.contains("172.16.3.92/23:"),
            "join must not contain an unstripped self entry: {join}"
        );
        assert!(
            !join.contains("172.16.3.92:"),
            "self is a member (not the seed) and must be filtered out entirely: {join}"
        );
    }

    #[test]
    fn test_derive_excludes_released_members() {
        let mut spec = sample_cockroach_spec();
        spec.seed_ip = "172.16.2.30".to_string();
        // .96 is soft-released, so the caller omits it from `members`.
        let members = vec!["172.16.3.92/23".to_string(), "172.16.3.94/23".to_string()];

        let (_, join) = derive_cockroach_endpoints("172.16.3.92/23", &members, &spec);

        assert!(!join.contains("172.16.3.96"), "released member leaked into join: {join}");
    }

    #[test]
    fn test_derive_seed_is_self_is_legal() {
        let mut spec = sample_cockroach_spec();
        spec.seed_ip = "172.16.3.92".to_string();
        let members = vec!["172.16.3.92/23".to_string(), "172.16.3.94/23".to_string()];

        let (advertise, join) = derive_cockroach_endpoints("172.16.3.92/23", &members, &spec);

        assert!(join.starts_with(&advertise), "seed must be listed first: {join}");
        assert_eq!(
            join.matches("172.16.3.92:").count(),
            1,
            "a seed joining itself must appear exactly once, not duplicated: {join}"
        );
    }

    #[test]
    fn test_derive_zero_members() {
        let mut spec = sample_cockroach_spec();
        spec.seed_ip = "172.16.2.30".to_string();

        let (_, join) = derive_cockroach_endpoints("172.16.3.92/23", &[], &spec);

        assert_eq!(join, format!("172.16.2.30:{}", spec.port));
    }

    // --- install_cockroach: mock-executor tests ---

    /// Cert-fetch response the mock hands back for the `/api/certs/` chroot
    /// command. `Success` uses distinct-but-valid-base64 payloads.
    #[derive(Clone)]
    enum CertResponse {
        Success,
        HttpFailure,
        OkFalse,
        /// Response's `certs` object uses a key outside the allowlist —
        /// simulates a MITM'd plain-HTTP response attempting a
        /// path-traversal write (e.g. `../../etc/cron.d/x`).
        BadFilename,
    }

    /// Records every command like `RecordingExecutor`, but additionally
    /// scripts the `/api/certs/` response so cert-fetch success/failure
    /// paths are testable without a network.
    #[derive(Clone)]
    struct CockroachTestExecutor {
        commands: Arc<Mutex<Vec<String>>>,
        cert_response: CertResponse,
    }

    impl CockroachTestExecutor {
        fn new(cert_response: CertResponse) -> Self {
            Self {
                commands: Arc::new(Mutex::new(Vec::new())),
                cert_response,
            }
        }

        fn recorded(&self) -> Vec<String> {
            self.commands.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CommandExecutor for CockroachTestExecutor {
        async fn connect(&mut self, _host: &str, _user: &str) -> Result<()> {
            Ok(())
        }
        async fn execute(&mut self, cmd: &str) -> Result<()> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok(())
        }
        async fn execute_with_output(&mut self, cmd: &str) -> Result<String> {
            self.commands.lock().unwrap().push(cmd.to_string());
            if cmd.contains("/api/certs/") {
                return match &self.cert_response {
                    CertResponse::Success => Ok(
                        r#"{"ok":true,"certs":{"ca.crt":"Y2E=","node.crt":"bm9kZQ==","node.key":"a2V5"}}"#
                            .to_string(),
                    ),
                    CertResponse::HttpFailure => Err(AutoInstallError::ProcessError {
                        command: cmd.to_string(),
                        exit_code: Some(22),
                        stderr: "curl: (22) The requested URL returned error: 404".to_string(),
                    }),
                    CertResponse::OkFalse => {
                        Ok(r#"{"ok":false,"error":"no cert issued for host"}"#.to_string())
                    }
                    CertResponse::BadFilename => Ok(
                        r#"{"ok":true,"certs":{"../../etc/cron.d/evil":"ZXZpbA=="}}"#.to_string(),
                    ),
                };
            }
            Ok(String::new())
        }
        async fn execute_with_error_collection(
            &mut self,
            cmd: &str,
            _desc: &str,
        ) -> Result<(i32, String, String)> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok((0, String::new(), String::new()))
        }
        async fn check_silent(&mut self, cmd: &str) -> Result<bool> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok(true)
        }
        async fn collect_debug_info(&mut self) -> Result<String> {
            Ok(String::new())
        }
        async fn upload_file(&mut self, local_path: &str, remote_path: &str) -> Result<()> {
            // Record what actually got shipped, including a roundtrip
            // through the real local file so a base64-decode bug would
            // show up as a byte-count mismatch, not just "a call happened".
            let bytes = std::fs::read(local_path).unwrap_or_default();
            self.commands
                .lock()
                .unwrap()
                .push(format!("upload_file remote={remote_path} bytes={}", bytes.len()));
            Ok(())
        }
        async fn download_file(&mut self, _remote_path: &str, _local_path: &str) -> Result<()> {
            Ok(())
        }
        fn disconnect(&mut self) {}
    }

    #[tokio::test]
    async fn test_cockroach_writes_unit_and_starts() {
        let mut executor = CockroachTestExecutor::new(CertResponse::Success);
        let mut config = sample_config();
        config.applications = vec![ApplicationSpec::Cockroach(sample_cockroach_spec())];
        let mut installer = ApplicationInstaller::new(&mut executor);

        let result = installer.install(&config).await;

        assert!(result.is_ok(), "expected success, got {result:?}");
        let commands = executor.recorded();
        let idx = |needle: &str| {
            commands
                .iter()
                .position(|c| c.contains(needle))
                .unwrap_or_else(|| panic!("no recorded command contains {needle:?}: {commands:?}"))
        };
        let curl_idx = idx("binaries.cockroachdb.com");
        let useradd_idx = idx("useradd");
        let cert_idx = idx("/api/certs/");
        let unit_idx = idx("cockroach.service");
        let reload_idx = idx("daemon-reload");
        let enable_idx = idx("systemctl enable cockroach");
        let start_idx = idx("systemctl start cockroach");

        assert!(curl_idx < useradd_idx, "binary download must precede useradd");
        assert!(useradd_idx < cert_idx, "useradd must precede cert fetch");
        assert!(cert_idx < unit_idx, "certs must be fetched before the unit is written");
        assert!(unit_idx < reload_idx, "unit must be written before daemon-reload");
        assert!(reload_idx < enable_idx, "daemon-reload must precede enable");
        assert!(enable_idx < start_idx, "enable must precede start");

        // Anti-over-suppression companion to
        // test_cert_response_rejects_unexpected_filename: the allowlist
        // check must not reject the three *legitimate* cert filenames.
        let uploads: Vec<&String> = commands.iter().filter(|c| c.starts_with("upload_file")).collect();
        assert_eq!(uploads.len(), 3, "expected exactly 3 cert uploads: {commands:?}");
        for fname in COCKROACH_CERT_FILENAMES {
            assert!(
                uploads.iter().any(|u| u.contains(&format!("certs/{fname}"))),
                "missing upload for {fname}: {commands:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_cert_response_rejects_unexpected_filename() {
        // Simulates a MITM'd plain-HTTP response naming a cert file outside
        // the allowlist (path traversal attempt). Must be rejected before
        // anything is uploaded or the node is started.
        let mut executor = CockroachTestExecutor::new(CertResponse::BadFilename);
        let mut config = sample_config();
        config.applications = vec![ApplicationSpec::Cockroach(sample_cockroach_spec())];
        let mut installer = ApplicationInstaller::new(&mut executor);

        let result = installer.install(&config).await;

        let err = result.expect_err("unexpected cert filename must be rejected");
        assert!(
            err.to_string().contains("evil") || err.to_string().contains("unexpected"),
            "error should name the problem: {err}"
        );
        let commands = executor.recorded();
        assert!(
            !commands.iter().any(|c| c.starts_with("upload_file")),
            "no cert file may be written once an unexpected filename is seen: {commands:?}"
        );
        assert!(
            !commands.iter().any(|c| c.contains("systemctl start cockroach")),
            "must never start a node when the cert response was rejected: {commands:?}"
        );
    }

    #[tokio::test]
    async fn test_cert_fetch_failure_propagates() {
        // Non-2xx (curl -fsSL failure).
        let mut executor = CockroachTestExecutor::new(CertResponse::HttpFailure);
        let mut config = sample_config();
        config.applications = vec![ApplicationSpec::Cockroach(sample_cockroach_spec())];
        let mut installer = ApplicationInstaller::new(&mut executor);
        let result = installer.install(&config).await;
        assert!(result.is_err(), "non-2xx cert fetch must fail the install");
        assert!(
            !executor
                .recorded()
                .iter()
                .any(|c| c.contains("systemctl start cockroach")),
            "must never start a node whose certs are missing"
        );

        // `ok: false` body (HTTP succeeded, cert issuance failed).
        let mut executor = CockroachTestExecutor::new(CertResponse::OkFalse);
        let mut config = sample_config();
        config.applications = vec![ApplicationSpec::Cockroach(sample_cockroach_spec())];
        let mut installer = ApplicationInstaller::new(&mut executor);
        let result = installer.install(&config).await;
        assert!(result.is_err(), "ok:false cert fetch must fail the install");
        assert!(
            !executor
                .recorded()
                .iter()
                .any(|c| c.contains("systemctl start cockroach")),
            "must never start a node whose certs are missing"
        );
    }

    #[tokio::test]
    async fn test_sql_port_from_spec_not_sed() {
        let mut executor = CockroachTestExecutor::new(CertResponse::Success);
        let mut config = sample_config();
        let mut spec = sample_cockroach_spec();
        spec.port = 40000;
        spec.sql_port = 40001;
        config.applications = vec![ApplicationSpec::Cockroach(spec)];
        let mut installer = ApplicationInstaller::new(&mut executor);

        let result = installer.install(&config).await;

        assert!(result.is_ok(), "expected success, got {result:?}");
        let commands = executor.recorded();
        let unit_cmd = commands
            .iter()
            .find(|c| c.contains("cockroach.service"))
            .expect("unit write recorded");
        // Port-only, per the len-serv-001/002 form. The original assertion
        // expected `{ip}:40001`; that was len-serv-003's drifted IP-bound form,
        // which is exactly why `--host=127.0.0.1:36257` is refused on 003.
        // The property this test actually guards — sql-addr derives from
        // spec.sql_port rather than a sed rewrite of the RPC port — is
        // unchanged and still asserted.
        assert!(
            unit_cmd.contains("--sql-addr=:40001"),
            "sql-addr must come from spec.sql_port, not a sed rewrite of the RPC port: {unit_cmd}"
        );
        assert!(
            !unit_cmd.contains(&format!(
                "--sql-addr={}",
                HostSpec::ip_without_cidr(&sample_config().network_address)
            )),
            "sql-addr must NOT be IP-bound (that is len-serv-003's drift): {unit_cmd}"
        );
        // advertise-addr, by contrast, MUST stay IP-bound — it is what peers
        // dial. A port-only advertise silently breaks cluster join.
        assert!(
            unit_cmd.contains(&format!(
                "--advertise-addr={}:40000",
                HostSpec::ip_without_cidr(&sample_config().network_address)
            )),
            "advertise-addr must stay IP-bound: {unit_cmd}"
        );
        assert!(
            !unit_cmd.contains("36257"),
            "no leftover sed 36357->36257 hack: {unit_cmd}"
        );
    }
}
