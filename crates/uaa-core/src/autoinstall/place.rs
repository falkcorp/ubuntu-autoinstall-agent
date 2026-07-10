// file: crates/uaa-core/src/autoinstall/place.rs
// version: 1.1.0
// guid: d3e4f5a6-b7c8-9d0e-1f2a-3b4c5d6e7f8a
// last-edited: 2026-07-10

//! Placement & drive: write the rendered seed into the server's iPXE netboot
//! tree, optionally flip the boot target to `custom-autoinstall`, and optionally
//! trigger a reboot on the target host.
//!
//! # Flow
//! 1. Render `user-data` from template + [`HostSpec`].
//! 2. Render minimal `meta-data` (cloud-init instance-id + local-hostname).
//! 3. Resolve the `hexmac` directory on the netboot server by reading the
//!    hostname symlink (`/var/www/html/cloud-init/<hostname> → <hexmac>`).
//! 4. SCP both files into `/var/www/html/cloud-init/<hexmac>/` on the server.
//! 5. Optionally GET `http://<server>:25000/api/flip/<hostname>?target=custom-autoinstall`.
//! 6. Optionally SSH to the target host and issue `sudo reboot`.
//!
//! # Notes
//! - Files are group-writable via ACL on the server — no `sudo` needed for SCP.
//! - The flip API requires the machine to have `status: approved` in the server
//!   registry; calling it on an unapproved machine returns HTTP 403.
//! - A reboot SSH connection is expected to drop mid-flight; the error is silenced.

use std::io::Write;

use crate::{
    autoinstall::{host_spec::HostSpec, render::render_user_data},
    error::AutoInstallError,
    network::executor::CommandExecutor,
    Result,
};

// ── Server-side constants ─────────────────────────────────────────────────────
//
// These are the DEFAULTS sourced by `crate::fleet::FleetConfig` — the single
// source of truth for the literal values. Runtime code below reads the live
// value through `crate::fleet::fleet()`, not these consts directly.

/// Where cloud-init seeds live on the netboot server.
///
/// DEFAULT sourced by `fleet::FleetConfig::cloud_init_base`.
pub const CLOUD_INIT_BASE: &str = "/var/www/html/cloud-init";

/// Default netboot/API server.
///
/// DEFAULT sourced by `fleet::FleetConfig::netboot_server`.
pub const DEFAULT_NETBOOT_SERVER: &str = "172.16.2.30";

/// Default SSH user for the netboot server and lenserv hosts.
///
/// DEFAULT sourced by `fleet::FleetConfig::server_user`.
pub const DEFAULT_SERVER_USER: &str = "jdfalk";

/// Port where the flip API listens.
///
/// DEFAULT sourced by `fleet::FleetConfig::flip_api_port`.
pub const FLIP_API_PORT: u16 = 25000;

/// Boot target name for autoinstall.
pub const TARGET_AUTOINSTALL: &str = "custom-autoinstall";

// ── Pure helpers ──────────────────────────────────────────────────────────────

/// Convert a MAC address (any separator format) to the lowercase hex string
/// used as the cloud-init directory name, e.g. `6c:4b:90:bc:f7:f4` →
/// `6c4b90bcf7f4`.
pub fn hexmac_from_mac(mac: &str) -> String {
    mac.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect()
}

/// Render the minimal cloud-init `meta-data` for a host.
///
/// The `instance-id` is set to the hostname; it just needs to be stable and
/// unique per machine — the autoinstall process doesn't inspect it beyond
/// that.
pub fn render_meta_data(spec: &HostSpec) -> String {
    format!(
        "instance-id: {}\nlocal-hostname: {}\n",
        spec.hostname, spec.hostname
    )
}

/// Build the remote path for a seed file inside the cloud-init tree.
///
/// `hexmac` is the directory name (no path separator); `filename` is
/// `user-data` or `meta-data`.
pub fn seed_path(hexmac: &str, filename: &str) -> String {
    let cloud_init_base = &crate::fleet::fleet().cloud_init_base;
    format!("{cloud_init_base}/{hexmac}/{filename}")
}

/// Build the flip API URL for a hostname and target.
pub fn flip_url(server: &str, hostname: &str, target: &str) -> String {
    let flip_api_port = crate::fleet::fleet().flip_api_port;
    format!("http://{server}:{flip_api_port}/api/flip/{hostname}?target={target}")
}

// ── Result types ──────────────────────────────────────────────────────────────

/// Outcome of the flip API call.
#[derive(Debug, Clone)]
pub struct FlipResult {
    pub ok: bool,
    pub message: String,
}

/// Full report from a `place` operation.
#[derive(Debug)]
pub struct PlaceReport {
    /// The hexmac directory where files were written.
    pub hexmac: String,
    /// Remote path of the written `user-data`.
    pub user_data_path: String,
    /// Remote path of the written `meta-data`.
    pub meta_data_path: String,
    /// Flip API result, if `--flip` was requested.
    pub flip: Option<FlipResult>,
    /// Whether a reboot was triggered on the target.
    pub rebooted: bool,
    /// True when `--dry-run` was set (nothing was actually written).
    pub dry_run: bool,
}

impl PlaceReport {
    /// Print a human-readable summary to stdout.
    pub fn print(&self) {
        if self.dry_run {
            println!("[DRY RUN] Would write seed to:");
        } else {
            println!("Wrote seed to:");
        }
        println!("  {}", self.user_data_path);
        println!("  {}", self.meta_data_path);

        if let Some(ref flip) = self.flip {
            let prefix = if self.dry_run { "[DRY RUN] " } else { "" };
            let mark = if flip.ok { "ok" } else { "FAIL" };
            println!("{prefix}Flip: [{mark}] {}", flip.message);
        }

        if self.rebooted {
            println!("Reboot: triggered (connection will drop — that is expected)");
        }
    }
}

// ── Async helpers ─────────────────────────────────────────────────────────────

/// Resolve the hexmac directory for `hostname` by reading the server symlink.
///
/// The server maintains `/var/www/html/cloud-init/<hostname> → <hexmac>`.
/// We read this symlink via SSH to avoid requiring the caller to know or
/// supply the MAC address.
pub async fn resolve_hexmac(
    server: &mut dyn CommandExecutor,
    hostname: &str,
) -> Result<String> {
    let cloud_init_base = &crate::fleet::fleet().cloud_init_base;
    let symlink_path = format!("{cloud_init_base}/{hostname}");
    let output = server
        .execute_with_output(&format!("readlink {symlink_path}"))
        .await?;
    let hexmac = output.trim().to_string();
    if hexmac.is_empty() {
        return Err(AutoInstallError::ConfigError(format!(
            "No cloud-init directory registered for hostname '{hostname}' on the netboot server. \
             Run register-gen.py or 'register-len-server.sh' first."
        )));
    }
    Ok(hexmac)
}

/// Write `user-data` and `meta-data` seed files to the server's cloud-init
/// tree via SCP.
///
/// Uses two local temp files (one per seed), then calls `upload_file` for
/// each. The temp files are removed after upload regardless of outcome.
pub async fn write_seed(
    server: &mut dyn CommandExecutor,
    hexmac: &str,
    user_data: &str,
    meta_data: &str,
) -> Result<(String, String)> {
    let ud_remote = seed_path(hexmac, "user-data");
    let md_remote = seed_path(hexmac, "meta-data");

    // Write user-data to temp file
    let mut ud_tmp = tempfile::NamedTempFile::new()
        .map_err(AutoInstallError::IoError)?;
    ud_tmp
        .write_all(user_data.as_bytes())
        .map_err(AutoInstallError::IoError)?;
    ud_tmp.flush().map_err(AutoInstallError::IoError)?;

    // Write meta-data to temp file
    let mut md_tmp = tempfile::NamedTempFile::new()
        .map_err(AutoInstallError::IoError)?;
    md_tmp
        .write_all(meta_data.as_bytes())
        .map_err(AutoInstallError::IoError)?;
    md_tmp.flush().map_err(AutoInstallError::IoError)?;

    server
        .upload_file(
            ud_tmp.path().to_str().unwrap_or("/tmp/ud"),
            &ud_remote,
        )
        .await?;

    server
        .upload_file(
            md_tmp.path().to_str().unwrap_or("/tmp/md"),
            &md_remote,
        )
        .await?;

    Ok((ud_remote, md_remote))
}

/// Call the netboot server's flip API.
///
/// A 403 response means the machine is not yet approved; the function returns
/// `FlipResult { ok: false, message }` rather than an error so the caller can
/// surface a helpful message.
pub async fn call_flip_api(url: &str) -> Result<FlipResult> {
    let response = reqwest::get(url).await?;
    let status = response.status();
    let body: serde_json::Value = response.json().await?;

    let ok = body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let message = body
        .get("message")
        .or_else(|| body.get("error"))
        .and_then(|v| v.as_str())
        .unwrap_or("no message")
        .to_string();

    if status == reqwest::StatusCode::FORBIDDEN {
        let fleet = crate::fleet::fleet();
        let netboot_server = &fleet.netboot_server;
        let flip_api_port = fleet.flip_api_port;
        return Ok(FlipResult {
            ok: false,
            message: format!(
                "403 Forbidden — machine not approved for reinstall. \
                 Run: curl http://{netboot_server}:{flip_api_port}/api/approve/<mac>"
            ),
        });
    }

    Ok(FlipResult { ok, message })
}

// ── Options struct ────────────────────────────────────────────────────────────

/// Options for the full `place_and_drive` operation.
pub struct PlaceOpts<'a> {
    pub netboot_server: &'a str,
    pub template: Option<&'a str>,
    pub flip: bool,
    pub reboot: bool,
    pub dry_run: bool,
}

impl<'a> PlaceOpts<'a> {
    pub fn new(netboot_server: &'a str) -> Self {
        Self {
            netboot_server,
            template: None,
            flip: false,
            reboot: false,
            dry_run: false,
        }
    }
}

// ── Orchestrator ──────────────────────────────────────────────────────────────

/// Full placement + drive sequence.
///
/// `server_conn` must already be connected to the netboot server (172.16.2.30).
/// `target_conn` is only required when `opts.reboot` is true; it must be
/// connected to the lenserv host.
pub async fn place_and_drive(
    server_conn: &mut dyn CommandExecutor,
    target_conn: Option<&mut dyn CommandExecutor>,
    spec: &HostSpec,
    opts: &PlaceOpts<'_>,
) -> Result<PlaceReport> {
    use crate::autoinstall::render::default_template;

    // 1. Render seeds
    let template_body;
    let template = match opts.template {
        Some(path) => {
            template_body = std::fs::read_to_string(path).map_err(AutoInstallError::IoError)?;
            template_body.as_str()
        }
        None => default_template(),
    };
    let user_data = render_user_data(template, spec)?;
    let meta_data = render_meta_data(spec);

    // 2. Resolve hexmac from the server's hostname symlink
    let hexmac = resolve_hexmac(server_conn, &spec.hostname).await?;

    let user_data_path = seed_path(&hexmac, "user-data");
    let meta_data_path = seed_path(&hexmac, "meta-data");

    if opts.dry_run {
        println!(
            "[DRY RUN] Would write {} bytes to {}",
            user_data.len(),
            user_data_path
        );
        println!(
            "[DRY RUN] Would write {} bytes to {}",
            meta_data.len(),
            meta_data_path
        );
        if opts.flip {
            println!(
                "[DRY RUN] Would GET {}",
                flip_url(opts.netboot_server, &spec.hostname, TARGET_AUTOINSTALL)
            );
        }
        if opts.reboot {
            println!(
                "[DRY RUN] Would SSH to {} and run: sudo reboot",
                HostSpec::ip_without_cidr(&spec.network_address)
            );
        }
        return Ok(PlaceReport {
            hexmac,
            user_data_path,
            meta_data_path,
            flip: None,
            rebooted: false,
            dry_run: true,
        });
    }

    // 3. Write seed files
    let (ud_path, md_path) = write_seed(server_conn, &hexmac, &user_data, &meta_data).await?;

    // 4. Flip boot target
    let flip_result = if opts.flip {
        let url = flip_url(opts.netboot_server, &spec.hostname, TARGET_AUTOINSTALL);
        Some(call_flip_api(&url).await?)
    } else {
        None
    };

    // 5. Reboot target
    let rebooted = if opts.reboot {
        if let Some(target) = target_conn {
            // reboot drops the connection — ignore the resulting error
            let _ = target.execute("sudo reboot").await;
            true
        } else {
            return Err(AutoInstallError::ConfigError(
                "--reboot requires a target connection; pass target_conn".to_string(),
            ));
        }
    } else {
        false
    };

    Ok(PlaceReport {
        hexmac,
        user_data_path: ud_path,
        meta_data_path: md_path,
        flip: flip_result,
        rebooted,
        dry_run: false,
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autoinstall::host_spec::HostSpec;
    use crate::network::executor::CommandExecutor;
    use crate::Result;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    // ── Recording mock executor ───────────────────────────────────────────────

    #[derive(Clone, Default)]
    struct RecordingMock {
        /// Command → preset output
        responses: HashMap<String, String>,
        /// Ordered log of all calls: ("method", "arg1", "arg2")
        calls: Arc<Mutex<Vec<(String, String, String)>>>,
    }

    impl RecordingMock {
        fn with_responses(pairs: &[(&str, &str)]) -> Self {
            Self {
                responses: pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                calls: Arc::new(Mutex::new(vec![])),
            }
        }

        fn recorded(&self) -> Vec<(String, String, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CommandExecutor for RecordingMock {
        async fn connect(&mut self, host: &str, user: &str) -> Result<()> {
            self.calls.lock().unwrap().push(("connect".into(), host.into(), user.into()));
            Ok(())
        }
        async fn execute(&mut self, cmd: &str) -> Result<()> {
            self.calls.lock().unwrap().push(("execute".into(), cmd.into(), String::new()));
            Ok(())
        }
        async fn execute_with_output(&mut self, cmd: &str) -> Result<String> {
            self.calls.lock().unwrap().push(("execute_with_output".into(), cmd.into(), String::new()));
            Ok(self.responses.get(cmd).cloned().unwrap_or_default())
        }
        async fn execute_with_error_collection(
            &mut self, cmd: &str, _desc: &str,
        ) -> Result<(i32, String, String)> {
            Ok((0, self.responses.get(cmd).cloned().unwrap_or_default(), String::new()))
        }
        async fn check_silent(&mut self, cmd: &str) -> Result<bool> {
            Ok(!self.responses.get(cmd).map_or(true, |s| s.is_empty()))
        }
        async fn collect_debug_info(&mut self) -> Result<String> {
            Ok(String::new())
        }
        async fn upload_file(&mut self, local: &str, remote: &str) -> Result<()> {
            self.calls.lock().unwrap().push(("upload_file".into(), local.into(), remote.into()));
            Ok(())
        }
        async fn download_file(&mut self, remote: &str, local: &str) -> Result<()> {
            self.calls.lock().unwrap().push(("download_file".into(), remote.into(), local.into()));
            Ok(())
        }
        fn disconnect(&mut self) {}
    }

    // ── Pure function tests ───────────────────────────────────────────────────

    #[test]
    fn hexmac_strips_colons() {
        assert_eq!(hexmac_from_mac("6c:4b:90:bc:f7:f4"), "6c4b90bcf7f4");
    }

    #[test]
    fn hexmac_strips_dashes() {
        assert_eq!(hexmac_from_mac("6C-4B-90-BC-F7-F4"), "6c4b90bcf7f4");
    }

    #[test]
    fn hexmac_already_bare() {
        assert_eq!(hexmac_from_mac("6c4b90bcf7f4"), "6c4b90bcf7f4");
    }

    #[test]
    fn meta_data_contains_hostname() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");
        let md = render_meta_data(&spec);
        assert!(md.contains("instance-id: len-serv-003"));
        assert!(md.contains("local-hostname: len-serv-003"));
    }

    #[test]
    fn meta_data_ends_with_newline() {
        let spec = HostSpec::for_lenserv("len-serv-001", "172.16.3.92/23");
        assert!(render_meta_data(&spec).ends_with('\n'));
    }

    #[test]
    fn seed_path_is_correct() {
        assert_eq!(
            seed_path("6c4b90bcf7f4", "user-data"),
            "/var/www/html/cloud-init/6c4b90bcf7f4/user-data"
        );
        assert_eq!(
            seed_path("6c4b90bcf7f4", "meta-data"),
            "/var/www/html/cloud-init/6c4b90bcf7f4/meta-data"
        );
    }

    #[test]
    fn flip_url_format() {
        let url = flip_url("172.16.2.30", "len-serv-003", "custom-autoinstall");
        assert_eq!(
            url,
            "http://172.16.2.30:25000/api/flip/len-serv-003?target=custom-autoinstall"
        );
    }

    // ── resolve_hexmac tests ─────────────────────────────────────────────────

    #[tokio::test]
    async fn resolve_hexmac_returns_symlink_target() {
        let mut mock = RecordingMock::with_responses(&[(
            "readlink /var/www/html/cloud-init/len-serv-003",
            "6c4b90bcf7f4\n",
        )]);
        let result = resolve_hexmac(&mut mock, "len-serv-003").await.unwrap();
        assert_eq!(result, "6c4b90bcf7f4");
    }

    #[tokio::test]
    async fn resolve_hexmac_errors_on_missing_hostname() {
        let mut mock = RecordingMock::with_responses(&[]);
        let err = resolve_hexmac(&mut mock, "unknown-host").await.unwrap_err();
        assert!(err.to_string().contains("No cloud-init directory registered"));
    }

    // ── write_seed tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn write_seed_uploads_both_files_to_correct_paths() {
        let mut mock = RecordingMock::default();
        let ud = "#cloud-config\nautoinstall:\n  version: 1\n";
        let md = "instance-id: len-serv-003\nlocal-hostname: len-serv-003\n";

        let (ud_path, md_path) = write_seed(&mut mock, "6c4b90bcf7f4", ud, md).await.unwrap();

        assert_eq!(ud_path, "/var/www/html/cloud-init/6c4b90bcf7f4/user-data");
        assert_eq!(md_path, "/var/www/html/cloud-init/6c4b90bcf7f4/meta-data");

        let calls = mock.recorded();
        let upload_calls: Vec<_> = calls.iter().filter(|(m, _, _)| m == "upload_file").collect();
        assert_eq!(upload_calls.len(), 2);
        // Check remote paths (second arg to upload_file)
        let remote_paths: Vec<&str> = upload_calls.iter().map(|(_, _, r)| r.as_str()).collect();
        assert!(remote_paths.contains(&"/var/www/html/cloud-init/6c4b90bcf7f4/user-data"));
        assert!(remote_paths.contains(&"/var/www/html/cloud-init/6c4b90bcf7f4/meta-data"));
    }

    // ── place_and_drive dry-run test ─────────────────────────────────────────

    #[tokio::test]
    async fn place_and_drive_dry_run_does_not_upload() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");
        let mut server = RecordingMock::with_responses(&[(
            "readlink /var/www/html/cloud-init/len-serv-003",
            "6c4b90bcf7f4\n",
        )]);

        let opts = PlaceOpts {
            netboot_server: DEFAULT_NETBOOT_SERVER,
            template: None,
            flip: true,
            reboot: false,
            dry_run: true,
        };

        let report = place_and_drive(&mut server, None, &spec, &opts)
            .await
            .unwrap();

        assert!(report.dry_run);
        assert!(report.flip.is_none());
        assert!(!report.rebooted);

        // No upload_file calls in dry-run
        let calls = server.recorded();
        let uploads: Vec<_> = calls.iter().filter(|(m, _, _)| m == "upload_file").collect();
        assert!(uploads.is_empty());
    }

    #[tokio::test]
    async fn place_and_drive_writes_and_does_not_flip_without_flag() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");
        let mut server = RecordingMock::with_responses(&[(
            "readlink /var/www/html/cloud-init/len-serv-003",
            "6c4b90bcf7f4\n",
        )]);

        let opts = PlaceOpts {
            netboot_server: DEFAULT_NETBOOT_SERVER,
            template: None,
            flip: false,
            reboot: false,
            dry_run: false,
        };

        let report = place_and_drive(&mut server, None, &spec, &opts)
            .await
            .unwrap();

        assert!(!report.dry_run);
        assert!(report.flip.is_none()); // No flip was requested
        assert_eq!(report.hexmac, "6c4b90bcf7f4");

        let calls = server.recorded();
        let uploads: Vec<_> = calls.iter().filter(|(m, _, _)| m == "upload_file").collect();
        assert_eq!(uploads.len(), 2);
    }

    #[tokio::test]
    async fn place_and_drive_triggers_reboot_on_target() {
        let spec = HostSpec::for_lenserv("len-serv-003", "172.16.3.96/23");
        let mut server = RecordingMock::with_responses(&[(
            "readlink /var/www/html/cloud-init/len-serv-003",
            "6c4b90bcf7f4\n",
        )]);
        let mut target = RecordingMock::default();

        let opts = PlaceOpts {
            netboot_server: DEFAULT_NETBOOT_SERVER,
            template: None,
            flip: false,
            reboot: true,
            dry_run: false,
        };

        let report = place_and_drive(&mut server, Some(&mut target), &spec, &opts)
            .await
            .unwrap();

        assert!(report.rebooted);

        let target_calls = target.recorded();
        let reboot_calls: Vec<_> = target_calls
            .iter()
            .filter(|(m, cmd, _)| m == "execute" && cmd.contains("reboot"))
            .collect();
        assert_eq!(reboot_calls.len(), 1);
    }

    #[tokio::test]
    async fn place_and_drive_errors_when_hexmac_missing() {
        let spec = HostSpec::for_lenserv("not-registered", "10.0.0.1/24");
        let mut server = RecordingMock::default(); // no symlink response → empty string

        let opts = PlaceOpts {
            netboot_server: DEFAULT_NETBOOT_SERVER,
            template: None,
            flip: false,
            reboot: false,
            dry_run: false,
        };

        let err = place_and_drive(&mut server, None, &spec, &opts)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("No cloud-init directory registered"));
    }
}
