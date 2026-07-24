// file: crates/uaa-core/src/network/ssh_installer/applications.rs
// version: 1.3.1
// guid: dc8e60fb-8d31-4869-96bf-bf6203d3a530
// last-edited: 2026-07-23

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

use super::config::{ApplicationSpec, CockroachSpec, InstallationConfig, TangServerSpec};
use crate::autoinstall::host_spec::{HostSpec, LENSERV_MEMBER_IPS};
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
        self.chroot_exec(
            "mkdir -p /var/lib/cockroach/certs /var/lib/cockroach/data && \
             chown -R cockroach:cockroach /var/lib/cockroach",
        )
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

        // 4. Derive advertise/join. Members are sourced from the fleet's
        // canonical LENSERV_MEMBER_IPS (host_spec.rs) — InstallationConfig
        // has no per-host group/sibling field yet (that lands with
        // TASK-04/profiles); this is the only source of truth today, and
        // matches for_lenserv()'s own derivation exactly (see
        // test_cockroach_join_matches_host_spec).
        let members: Vec<String> = LENSERV_MEMBER_IPS.iter().map(|ip| ip.to_string()).collect();
        let (advertise, join) =
            derive_cockroach_endpoints(&config.network_address, &members, spec);
        let sql_addr = format!("{self_ip}:{}", spec.sql_port);

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
             \x20 --store=/var/lib/cockroach/data \\\n\
             \x20 --certs-dir=/var/lib/cockroach/certs \\\n\
             \x20 --listen-addr={advertise} \\\n\
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
            sql_addr = sql_addr,
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
            let kind = match app {
                ApplicationSpec::Cockroach(_) => "cockroach",
                ApplicationSpec::TangServer(_) => "tang-server",
            };
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
            enroll_tpm2: true,
            tpm2_pin: None,
            tpm2_pcr_ids: "7".into(),
            expect_fido2: true,
            install_ca_cert: "test-ca-pem".into(),
            applications: vec![],
            storage_mode: Default::default(),
            disks: Vec::new(),
            arch: Default::default(),
            role: Default::default(),
            firmware_quirks: Vec::new(),
            hooks: Default::default(),
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

    #[test]
    fn test_cockroach_join_matches_host_spec() {
        use crate::autoinstall::host_spec::{COCKROACH_PORT, COCKROACH_SERVER_IP};

        // len-serv-001 (172.16.3.92) against the real fleet member set.
        let self_addr = "172.16.3.92/23";
        let members: Vec<String> = LENSERV_MEMBER_IPS
            .iter()
            .map(|ip| format!("{ip}/23"))
            .collect();
        let mut spec = sample_cockroach_spec();
        spec.seed_ip = COCKROACH_SERVER_IP.to_string();
        spec.port = COCKROACH_PORT;

        let (_, join) = derive_cockroach_endpoints(self_addr, &members, &spec);

        // Computed directly against HostSpec::compute_join — proves there
        // is no second, divergent join implementation.
        let expected =
            HostSpec::compute_join(COCKROACH_SERVER_IP, LENSERV_MEMBER_IPS, "172.16.3.92", COCKROACH_PORT);
        assert_eq!(join, expected);
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
        assert!(
            unit_cmd.contains(&format!("--sql-addr={}:40001", HostSpec::ip_without_cidr(&sample_config().network_address))),
            "sql-addr must come from spec.sql_port, not a sed rewrite of the RPC port: {unit_cmd}"
        );
        assert!(
            !unit_cmd.contains("36257"),
            "no leftover sed 36357->36257 hack: {unit_cmd}"
        );
    }
}
