// file: crates/uaa-core/src/network/ssh_installer/applications.rs
// version: 1.0.0
// guid: dc8e60fb-8d31-4869-96bf-bf6203d3a530
// last-edited: 2026-07-17

//! `ApplicationInstaller`: dispatches per-application installation for
//! `config.applications` (DS-APP-02).
//!
//! **FAIL-CLOSED by design.** Unlike `ResetPartitionStager` (a non-fatal
//! recovery nicety), an application failing to install is a failed
//! deployment. Every error path here propagates with `?` out to
//! `phase_5_system_configuration` and fails the install. Never
//! warn-and-continue.
//!
//! This module ships the dispatch loop and scaffold only. The Cockroach
//! install body is DS-APP-03 (a later task); until then a config that
//! requests `cockroach` fails loudly with a `ConfigError` naming it, rather
//! than silently reporting success having installed nothing.
//!
//! With `applications: []` (every committed config today) `install()` is a
//! no-op: zero commands are executed and `Ok(())` is returned, so behavior
//! is byte-identical to before this module existed.

use super::config::{ApplicationSpec, CockroachSpec, InstallationConfig};
use crate::network::CommandExecutor;
use crate::Result;
use std::collections::HashSet;

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
            }
        }
        Ok(())
    }

    /// TASK-03 (DS-APP-03) fills this in. Scaffold only: returns a
    /// not-implemented `ConfigError` naming cockroach so a config that
    /// requests it fails loudly rather than silently installing nothing.
    async fn install_cockroach(
        &mut self,
        _config: &InstallationConfig,
        _spec: &CockroachSpec,
    ) -> Result<()> {
        let _ = &mut self.runner; // borrowed for the executor seam DS-APP-03 will use
        Err(crate::error::AutoInstallError::ConfigError(
            "cockroach application install not yet implemented (DS-APP-03)".to_string(),
        ))
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
    async fn test_cockroach_scaffold_returns_not_implemented() {
        let mut executor = RecordingExecutor::new();
        let mut config = sample_config();
        config.applications = vec![ApplicationSpec::Cockroach(sample_cockroach_spec())];
        let mut installer = ApplicationInstaller::new(&mut executor);

        let result = installer.install(&config).await;

        let err = result.expect_err("cockroach scaffold must fail loudly");
        assert!(err.to_string().contains("DS-APP-03"));
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
}
