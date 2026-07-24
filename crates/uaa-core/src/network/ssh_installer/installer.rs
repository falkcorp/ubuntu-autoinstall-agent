// file: crates/uaa-core/src/network/ssh_installer/installer.rs
// version: 2.14.1
// guid: sshins01-2345-6789-abcd-ef0123456789
// last-edited: 2026-07-23

//! Main SSH/local installer orchestrating all installation phases.
//!
//! Uses a `Box<dyn CommandExecutor>` runner so the same phase logic works
//! whether execution happens locally (`LocalClient`) or over SSH (`SshClient`).

use super::applications::ApplicationInstaller;
use super::config::{InstallationConfig, StorageMode, SystemInfo};
use super::disk_native::DiskNativeManager;
use super::disk_ops::DiskManager;
use super::investigation::SystemInvestigator;
use super::packages::PackageManager;
use super::partitions::partition_path;
use super::reset_partition::ResetPartitionStager;
use super::system_setup::SystemConfigurator;
use super::zfs_native::ZfsNativeManager;
use super::zfs_ops::ZfsManager;
use crate::network::{CommandExecutor, LocalClient, SshClient};
use crate::Result;
use std::collections::HashMap;
use tracing::{error, info};

/// Marker written into the installed target's /etc during Phase 4.
/// Presence on a RUNNING root means "we are inside an installed target"
/// (e.g. under `curtin in-target`) — install commands must then reconfigure,
/// never wipe. Curtin/subiquity callers create it via a late-command:
/// `curtin in-target -- sh -c 'touch /etc/uaa-target-marker'`.
pub const TARGET_MARKER_PATH: &str = "/etc/uaa-target-marker";

/// Which of phases 0..=6 run. Parsed from `--phases` / `--from-phase`.
///
/// The flagless install builds [`PhaseSelection::full`] (every phase selected,
/// `explicit == false`), so the default command stream is byte-identical to a
/// pre-flag run. A selective run sets `explicit == true` and only the chosen
/// phases execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseSelection {
    selected: [bool; 7],
    explicit: bool,
}

/// Zero-sized wipe token. The single field is private, so a `WipeAuthorization`
/// can only be minted inside this module via [`PhaseSelection::authorize_wipe`]
/// — which hands one back exactly when Phase 2 is selected. No token, no wipe:
/// every wipe-capable call site requires this value, so a selection that omits
/// Phase 2 makes a disk wipe structurally unreachable.
pub struct WipeAuthorization(pub(crate) ());

impl PhaseSelection {
    /// Every phase 0..=6 selected. Non-explicit: this is the flagless default,
    /// and its command stream matches the pre-flag installer exactly.
    pub fn full() -> Self {
        Self {
            selected: [true; 7],
            explicit: false,
        }
    }

    /// Parse a `--phases` spec: comma-separated single phases (`"5"`) and/or
    /// inclusive ranges (`"4-6"`), e.g. `"0,1,5"`. Fail-closed — every
    /// malformed input is an `Err`, never a best-effort selection.
    pub fn parse(spec: &str) -> std::result::Result<Self, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("empty --phases spec".to_string());
        }
        let mut selected = [false; 7];
        for token in spec.split(',') {
            let token = token.trim();
            if token.is_empty() {
                return Err(format!("empty phase token in \"{spec}\""));
            }
            match token.split_once('-') {
                Some((a, b)) => {
                    let a = Self::parse_phase(a)?;
                    let b = Self::parse_phase(b)?;
                    if a > b {
                        return Err(format!("reversed phase range \"{token}\""));
                    }
                    for phase in a..=b {
                        selected[phase as usize] = true;
                    }
                }
                None => {
                    let phase = Self::parse_phase(token)?;
                    selected[phase as usize] = true;
                }
            }
        }
        Ok(Self {
            selected,
            explicit: true,
        })
    }

    /// Parse a single `0..=6` phase number, rejecting anything else.
    fn parse_phase(s: &str) -> std::result::Result<u8, String> {
        let s = s.trim();
        let n: u8 = s
            .parse()
            .map_err(|_| format!("invalid phase \"{s}\" (expected 0..=6)"))?;
        if n > 6 {
            return Err(format!("phase {n} out of range (expected 0..=6)"));
        }
        Ok(n)
    }

    /// `--from-phase n` shorthand for `"n-6"`.
    pub fn from_phase(n: u8) -> std::result::Result<Self, String> {
        Self::parse(&format!("{n}-6"))
    }

    /// Whether the given phase is selected to run.
    pub fn contains(&self, phase: u8) -> bool {
        self.selected.get(phase as usize).copied().unwrap_or(false)
    }

    /// Whether the selection came from a flag (vs. the flagless `full()` default).
    pub fn is_explicit(&self) -> bool {
        self.explicit
    }

    /// Mint a wipe token iff Phase 2 (disk preparation) is selected. Every
    /// wipe-capable call site gates on the returned `Some`.
    pub fn authorize_wipe(&self) -> Option<WipeAuthorization> {
        if self.selected[2] {
            Some(WipeAuthorization(()))
        } else {
            None
        }
    }

    /// Whether a non-destructive LUKS reopen is needed: Phase 2 is NOT selected
    /// but some later phase (3..=6) is. Consumed by phase-rerun/TASK-02.
    pub fn needs_luks_reopen(&self) -> bool {
        !self.selected[2] && (3..=6).any(|p| self.selected[p])
    }

    /// Whether a non-destructive pool import is needed: Phases 2 AND 3 are both
    /// NOT selected but some later phase (4..=6) is. Consumed by
    /// phase-rerun/TASK-02.
    pub fn needs_pool_import(&self) -> bool {
        !self.selected[2] && !self.selected[3] && (4..=6).any(|p| self.selected[p])
    }
}

/// Installer that works over SSH or locally.
///
/// Call [`connect`] for SSH or [`connect_local`] for local execution before
/// any other method.
pub struct SshInstaller {
    runner: Box<dyn CommandExecutor>,
    connected: bool,
    variables: HashMap<String, String>,
    /// When set, POST per-phase status updates to this webhook URL. Advisory.
    report_url: Option<String>,
}

impl SshInstaller {
    /// Create a new installer (not yet connected).
    pub fn new() -> Self {
        Self {
            runner: Box::new(LocalClient::new()),
            connected: false,
            variables: HashMap::new(),
            report_url: None,
        }
    }

    /// Test-only constructor: pre-connected installer backed by an injected
    /// executor (e.g. a recording mock), so phase sequencing can be exercised
    /// without a real SSH/local target.
    #[cfg(test)]
    pub(crate) fn for_tests(runner: Box<dyn CommandExecutor>) -> Self {
        Self {
            runner,
            connected: true,
            variables: HashMap::new(),
            report_url: None,
        }
    }

    /// Enable per-phase status reporting to the given webhook URL (e.g.
    /// `http://172.16.2.30:25000/api/webhook`). `None` disables reporting.
    pub fn set_report_url(&mut self, url: Option<String>) {
        self.report_url = url;
    }

    /// Best-effort status report; no-op unless `--report-url` was set.
    async fn report(&self, config: &InstallationConfig, status: &str, progress: u8, message: &str) {
        if let Some(url) = &self.report_url {
            let src_ip = config
                .network_address
                .split('/')
                .next()
                .unwrap_or("")
                .to_string();
            super::status::post_status(url, &config.hostname, &src_ip, status, progress, message)
                .await;
        }
    }

    // -------------------------------------------------------------------------
    // Connection
    // -------------------------------------------------------------------------

    /// Connect to a remote target over SSH.
    pub async fn connect(&mut self, host: &str, username: &str) -> Result<()> {
        let mut client = SshClient::new();
        client.connect(host, username).await?;
        self.runner = Box::new(client);
        self.connected = true;
        info!("Successfully connected to {}@{}", username, host);
        Ok(())
    }

    /// Activate local installation mode (no SSH).
    pub async fn connect_local(&mut self) -> Result<()> {
        self.runner = Box::new(LocalClient::new());
        self.connected = true;
        info!("Local installation mode activated");
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Public API
    // -------------------------------------------------------------------------

    /// Investigate the target system and return collected info.
    pub async fn investigate_system(&mut self) -> Result<SystemInfo> {
        self.require_connected()?;
        let mut investigator = SystemInvestigator::new(&mut *self.runner);
        investigator.investigate_system().await
    }

    /// Full installation with optional hold-on-failure / pause-after-storage.
    pub async fn perform_installation_with_options_and_pause(
        &mut self,
        config: &InstallationConfig,
        hold_on_failure: bool,
        pause_after_storage: bool,
        selection: &PhaseSelection,
    ) -> Result<()> {
        if !hold_on_failure && !pause_after_storage {
            return self.perform_installation(config, selection).await;
        }
        self.require_connected()?;

        info!(
            "Starting ZFS+LUKS installation for {} (hold={}, pause-after-storage={})",
            config.hostname, hold_on_failure, pause_after_storage
        );

        let mut failed_phases: Vec<String> = Vec::new();
        let mut successful_phases: Vec<&str> = Vec::new();

        if let Err(e) = self.preflight_checks(config, selection).await {
            error!("✗ Preflight checks failed: {}", e);
        } else {
            info!("✓ Preflight checks passed");
        }

        // Non-destructive mount-existing-target prep: runs when Phase 2/3 are
        // skipped but a later phase needs a mounted target. Fail-closed — a hard
        // prep failure aborts BEFORE any selected phase runs.
        if selection.needs_luks_reopen() || selection.needs_pool_import() {
            self.mount_existing_target(config, selection).await?;
        }

        macro_rules! run_phase {
            ($label:expr, $fut:expr) => {{
                match $fut.await {
                    Ok(_) => {
                        successful_phases.push($label);
                    }
                    Err(e) => {
                        failed_phases.push(format!("{}: {}", $label, e));
                        return self
                            .enter_hold_mode(
                                &format!("{} failed", $label),
                                &successful_phases,
                                &failed_phases,
                            )
                            .await;
                    }
                }
            }};
        }

        if selection.contains(0) {
            run_phase!("Phase 0: Setup variables", self.setup_installation_variables(config));
        } else {
            info!("Phase 0: Setup variables — SKIPPED (--phases)");
        }
        if selection.contains(1) {
            run_phase!("Phase 1: Package installation", self.phase_1_package_installation());
        } else {
            info!("Phase 1: Package installation — SKIPPED (--phases)");
        }
        if let Some(wipe_auth) = selection.authorize_wipe() {
            run_phase!("Phase 2: Disk preparation", self.phase_2_disk_preparation(config, &wipe_auth));
        } else {
            info!("Phase 2: Disk preparation — SKIPPED (--phases)");
        }
        if selection.contains(3) {
            run_phase!("Phase 3: ZFS creation", self.phase_3_zfs_creation(config));
        } else {
            info!("Phase 3: ZFS creation — SKIPPED (--phases)");
        }

        if pause_after_storage {
            self.print_next_commands_after_storage(config).await?;
            return self
                .enter_hold_mode(
                    "Paused after storage per user request",
                    &successful_phases,
                    &failed_phases,
                )
                .await;
        }

        if selection.contains(4) {
            run_phase!("Phase 4: Base system", self.phase_4_base_system(config));
        } else {
            info!("Phase 4: Base system — SKIPPED (--phases)");
        }
        if selection.contains(5) {
            run_phase!(
                "Phase 5: System configuration",
                self.phase_5_system_configuration(config)
            );
        } else {
            info!("Phase 5: System configuration — SKIPPED (--phases)");
        }
        if selection.contains(6) {
            run_phase!("Phase 6: Final setup", self.phase_6_final_setup(config));
        } else {
            info!("Phase 6: Final setup — SKIPPED (--phases)");
        }

        self.generate_installation_report(&successful_phases, &failed_phases)
            .await;
        info!(
            "🎉 Installation completed successfully for {}",
            config.hostname
        );
        Ok(())
    }

    /// Full installation with standard error collection (continues past failures).
    pub async fn perform_installation(
        &mut self,
        config: &InstallationConfig,
        selection: &PhaseSelection,
    ) -> Result<()> {
        self.require_connected()?;

        info!("Starting ZFS+LUKS installation for {}", config.hostname);

        let mut failed_phases: Vec<String> = Vec::new();
        let mut successful_phases: Vec<&str> = Vec::new();

        self.report(config, "running", 5, "Installation starting").await;

        macro_rules! run_phase {
            ($label:expr, $progress:expr, $fut:expr) => {{
                self.report(config, "running", $progress, &format!("{} — starting", $label))
                    .await;
                match $fut.await {
                    Ok(_) => {
                        info!("✓ Phase completed: {}", $label);
                        successful_phases.push($label);
                    }
                    Err(e) => {
                        error!("✗ Phase failed — {}: {}", $label, e);
                        failed_phases.push(format!("{}: {}", $label, e));
                        self.collect_and_log_debug_info().await;
                        self.report(config, "failed", $progress, &format!("{}: {}", $label, e))
                            .await;
                    }
                }
            }};
        }

        match self.preflight_checks(config, selection).await {
            Ok(_) => info!("✓ Preflight checks passed"),
            Err(e) => {
                error!("✗ Preflight checks failed: {}", e);
                self.collect_and_log_debug_info().await;
            }
        }

        // Non-destructive mount-existing-target prep: runs when Phase 2/3 are
        // skipped but a later phase needs a mounted target. Fail-closed — a hard
        // prep failure aborts BEFORE any selected phase runs.
        if selection.needs_luks_reopen() || selection.needs_pool_import() {
            self.mount_existing_target(config, selection).await?;
        }

        if selection.contains(0) {
            run_phase!("Phase 0: Setup variables", 10, self.setup_installation_variables(config));
        } else {
            info!("Phase 0: Setup variables — SKIPPED (--phases)");
        }
        if selection.contains(1) {
            run_phase!("Phase 1: Package installation", 20, self.phase_1_package_installation());
        } else {
            info!("Phase 1: Package installation — SKIPPED (--phases)");
        }
        if let Some(wipe_auth) = selection.authorize_wipe() {
            run_phase!("Phase 2: Disk preparation", 35, self.phase_2_disk_preparation(config, &wipe_auth));
        } else {
            info!("Phase 2: Disk preparation — SKIPPED (--phases)");
        }
        if selection.contains(3) {
            run_phase!("Phase 3: ZFS creation", 50, self.phase_3_zfs_creation(config));
        } else {
            info!("Phase 3: ZFS creation — SKIPPED (--phases)");
        }
        if selection.contains(4) {
            run_phase!("Phase 4: Base system", 75, self.phase_4_base_system(config));
        } else {
            info!("Phase 4: Base system — SKIPPED (--phases)");
        }
        if selection.contains(5) {
            run_phase!(
                "Phase 5: System configuration",
                90,
                self.phase_5_system_configuration(config)
            );
        } else {
            info!("Phase 5: System configuration — SKIPPED (--phases)");
        }
        if selection.contains(6) {
            run_phase!("Phase 6: Final setup", 95, self.phase_6_final_setup(config));
        } else {
            info!("Phase 6: Final setup — SKIPPED (--phases)");
        }

        self.generate_installation_report(&successful_phases, &failed_phases)
            .await;

        if failed_phases.is_empty() {
            info!(
                "🎉 Installation completed successfully for {}",
                config.hostname
            );
            self.report(config, "success", 100, &format!("{} installed", config.hostname))
                .await;
            Ok(())
        } else {
            error!(
                "❌ Installation completed with {} failed phases",
                failed_phases.len()
            );
            self.report(
                config,
                "failed",
                100,
                &format!("{} install failed: {} phase(s)", config.hostname, failed_phases.len()),
            )
            .await;
            Err(crate::error::AutoInstallError::InstallationError(format!(
                "Installation failed: {} phases failed",
                failed_phases.len()
            )))
        }
    }

    /// In-target (curtin-compatible) mode: the binary is already running INSIDE
    /// the installed/target chroot. Runs ONLY post-install configuration
    /// (Phase 5: GRUB, LUKS crypttab, dracut, Tang, install-CA trust anchor) by
    /// bind-mounting / to
    /// /mnt/targetos so the existing chroot-based Phase-5 code is reused
    /// unchanged. NEVER runs preflight_checks (it wipes residual state),
    /// Phases 1-4 (packages/disk prep/ZFS/debootstrap), or Phase 6
    /// (final_cleanup would zpool-export / cryptsetup-close the RUNNING root).
    pub async fn perform_in_target_configuration(&mut self, config: &InstallationConfig) -> Result<()> {
        self.require_connected()?;

        if self
            .runner
            .execute(&format!("test -f {}", TARGET_MARKER_PATH))
            .await
            .is_err()
        {
            return Err(crate::error::AutoInstallError::ValidationError(
                "no /etc/uaa-target-marker — refusing in-target configuration outside an installed target".into(),
            ));
        }

        self.runner
            .execute("mkdir -p /mnt/targetos && { mountpoint -q /mnt/targetos || mount --bind / /mnt/targetos; }")
            .await?;

        self.phase_5_system_configuration(config).await?;

        let _ = self
            .runner
            .execute("umount -R /mnt/targetos 2>/dev/null || umount -l /mnt/targetos 2>/dev/null || true")
            .await;

        info!(
            "In-target post-install configuration completed for {}",
            config.hostname
        );
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Private helpers
    // -------------------------------------------------------------------------

    fn require_connected(&self) -> Result<()> {
        if !self.connected {
            Err(crate::error::AutoInstallError::SshError(
                "Not connected to target system".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    async fn preflight_checks(
        &mut self,
        config: &InstallationConfig,
        selection: &PhaseSelection,
    ) -> Result<()> {
        info!("Running preflight checks");

        let ping = self
            .runner
            .execute(
                "ping -c 1 -w 2 1.1.1.1 >/dev/null 2>&1 || ping -c 1 -w 2 8.8.8.8 >/dev/null 2>&1",
            )
            .await;
        if ping.is_err() {
            return Err(crate::error::AutoInstallError::ValidationError(
                "No basic network connectivity (ICMP)".to_string(),
            ));
        }

        let release = config.debootstrap_release.as_deref().unwrap_or("resolute");
        let mirror = config
            .debootstrap_mirror
            .as_deref()
            .unwrap_or("http://archive.ubuntu.com/ubuntu/");
        let release_url = format!("{}/dists/{}/Release", mirror.trim_end_matches('/'), release);
        let head_cmd = format!("curl -fsI '{}' >/dev/null", release_url);
        if self.runner.execute(&head_cmd).await.is_err() {
            let fallback_url = format!(
                "http://old-releases.ubuntu.com/ubuntu/dists/{}/Release",
                release
            );
            let fallback_cmd = format!("curl -fsI '{}' >/dev/null", fallback_url);
            if self.runner.execute(&fallback_cmd).await.is_err() {
                return Err(crate::error::AutoInstallError::ValidationError(format!(
                    "Debootstrap mirror not reachable for {}",
                    release
                )));
            }
            info!("Mirror check: primary unreachable; old-releases is reachable");
        }

        self.runner.execute("mkdir -p /mnt/targetos").await?;
        let non_empty = self
            .runner
            .check_silent("test -z \"$(ls -A /mnt/targetos 2>/dev/null)\"")
            .await;
        if non_empty.is_err() || !non_empty.unwrap_or(true) {
            info!("Preflight: /mnt/targetos is not empty; proceeding carefully");
        }

        let has_bpool = self
            .runner
            .check_silent("zpool list -H bpool >/dev/null 2>&1")
            .await
            .unwrap_or(false);
        let has_rpool = self
            .runner
            .check_silent("zpool list -H rpool >/dev/null 2>&1")
            .await
            .unwrap_or(false);
        let luks_active = self
            .runner
            .check_silent("cryptsetup status luks >/dev/null 2>&1")
            .await
            .unwrap_or(false);
        let target_has_mounts = self
            .runner
            .check_silent("mount | grep -q '/mnt/targetos'")
            .await
            .unwrap_or(false);

        if has_bpool || has_rpool || luks_active || target_has_mounts {
            info!(
                "Preflight: residual state detected (bpool={} rpool={} luks={} mounts={})",
                has_bpool, has_rpool, luks_active, target_has_mounts
            );
            match selection.authorize_wipe() {
                Some(_auth) => {
                    info!("Preflight: Phase 2 selected — recovering (wipe authorized)");
                    let mut disk_manager = DiskManager::new(&mut *self.runner);
                    let _ = disk_manager
                        .recover_after_failure_and_wipe(config, &_auth)
                        .await;
                }
                None => {
                    // Selective mode omitting Phase 2: residual state is the
                    // EXPECTED input for a re-run. Log and continue — NEVER wipe.
                    // The non-destructive mount-existing-target prep (below,
                    // gated on needs_luks_reopen/needs_pool_import) reuses this
                    // state. This bypass is reachable ONLY with an explicit
                    // selection; every flagless run has authorize_wipe() == Some
                    // and wipes on residual exactly as before.
                    info!(
                        "Preflight: residual state detected (bpool={} rpool={} luks={} mounts={}) — expected in selective mode; NOT wiping",
                        has_bpool, has_rpool, luks_active, target_has_mounts
                    );
                }
            }
        }

        Ok(())
    }

    /// Non-destructive prep so a selective run that skips Phases 2-3 can reach an
    /// EXISTING installed disk. Normalizes stale mounts (umount only), re-opens
    /// LUKS (idempotent), imports `rpool` then `bpool`, and mounts
    /// `/` then `/boot` then the ESP — the order is load-bearing (faea48e). It
    /// NEVER wipes, formats, `zpool export`s, or `cryptsetup close`s: healthy
    /// residual state is the EXPECTED input and is reused, not torn down. Called
    /// AFTER preflight, BEFORE the first selected phase; a hard failure aborts
    /// the run fail-closed.
    async fn mount_existing_target(
        &mut self,
        config: &InstallationConfig,
        selection: &PhaseSelection,
    ) -> Result<()> {
        info!("Prep: mounting existing target for selective re-run");

        // 1) Normalize stale state: replay ONLY the umount inverse ops from
        //    SystemConfigurator::final_cleanup. NEVER zpool export / cryptsetup
        //    close — healthy pools and mappers are reused, not torn down.
        for cmd in [
            "umount -R /mnt/targetos/sys || true",
            "umount -R /mnt/targetos/proc || true",
            "umount -R /mnt/targetos/dev || true",
            "umount -R /mnt/targetos/run || true",
            "umount /mnt/targetos/boot/efi || true",
        ] {
            let _ = self.runner.execute(cmd).await;
        }

        // 2) Re-open LUKS (idempotent) when a later phase needs it.
        if selection.needs_luks_reopen() {
            let mut dm = DiskManager::new(&mut *self.runner);
            dm.reopen_luks_if_needed(config).await?;
        }

        // 3) Import pools (rpool then bpool) + ordered mount when a later phase
        //    needs a mounted target.
        if selection.needs_pool_import() {
            // ESP path: GUID PARTTYPE detection (copied verbatim from
            // system_setup::build_esp_detection_command — that file is not in
            // this task's file list); fall back to partition 1 of the configured
            // disk (suffix-aware) when detection is empty.
            let esp_guid = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
            let esp_detect_cmd = format!(
                "bash -lc 'lsblk -rP -o PATH,PARTTYPE | grep -i \"PARTTYPE=\\\"{0}\\\"\" | head -n1 | sed -n \"s/.*PATH=\\\"\\([^\\\" ]*\\)\\\".*/\\1/p\"'",
                esp_guid
            );
            let detected = self
                .runner
                .execute_with_output(&esp_detect_cmd)
                .await
                .unwrap_or_default();
            let esp_partition = if detected.trim().is_empty() {
                partition_path(&config.disk_device, 1)
            } else {
                detected.trim().to_string()
            };

            let mut zm = ZfsManager::new(&mut *self.runner, &mut self.variables);
            zm.import_pools_for_rerun().await?;
            zm.mount_target_for_rerun(&esp_partition).await?;
        }

        // 4) Chroot bind mounts: do NOTHING here — the idempotent
        //    `mountpoint -q … || mount --rbind …` blocks in
        //    configure_system_in_chroot (Phase 4) and configure_grub_in_chroot
        //    (Phase 5) re-establish them when those phases run.

        info!("Prep: existing target mounted");
        Ok(())
    }

    async fn collect_and_log_debug_info(&mut self) {
        info!("Collecting debug information...");
        match self.runner.collect_debug_info().await {
            Ok(debug_info) => {
                error!(
                    "=== DEBUG INFORMATION ===\n{}\n=== END DEBUG INFORMATION ===",
                    debug_info
                );

                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs().to_string())
                    .unwrap_or_else(|_| "0".to_string());
                let remote_dir = "/var/tmp/uaalogs";
                let remote_path = format!("{}/install-debug-{}.log", remote_dir, ts);
                let _ = self
                    .runner
                    .execute(&format!("mkdir -p {}", remote_dir))
                    .await;
                let _ = self
                    .runner
                    .execute(&format!(
                        "bash -lc 'cat > {} << \'EOF\'\n{}\nEOF'",
                        remote_path,
                        debug_info.replace('\'', "'\\''")
                    ))
                    .await;

                let local_dir = format!(
                    "{}/logs/{}",
                    std::env::current_dir()
                        .ok()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| ".".to_string()),
                    self.variables
                        .get("HOSTNAME")
                        .cloned()
                        .unwrap_or_else(|| "unknown-host".to_string())
                );
                let _ = std::fs::create_dir_all(&local_dir);
                let local_path = format!(
                    "{}/{}",
                    local_dir,
                    std::path::Path::new(&remote_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "debug.log".to_string())
                );
                if let Err(e) = self.runner.download_file(&remote_path, &local_path).await {
                    error!("Failed to download debug log: {}", e);
                } else {
                    info!("Saved debug log to {}", local_path);
                }
            }
            Err(e) => error!("Failed to collect debug information: {}", e),
        }
    }

    async fn generate_installation_report(
        &mut self,
        successful_phases: &[&str],
        failed_phases: &[String],
    ) {
        info!("=== INSTALLATION REPORT ===");
        info!(
            "Successful: {}  Failed: {}",
            successful_phases.len(),
            failed_phases.len()
        );
        for p in successful_phases {
            info!("  ✓ {}", p);
        }
        for p in failed_phases {
            error!("  ✗ {}", p);
        }
        if !failed_phases.is_empty() {
            error!(
                "Check /var/log/syslog, 'zpool status', 'cryptsetup status luks', 'lsblk', 'mount'"
            );
        }
        info!("=== END INSTALLATION REPORT ===");
    }

    async fn enter_hold_mode(
        &mut self,
        reason: &str,
        successful_phases: &[&str],
        failed_phases: &[String],
    ) -> Result<()> {
        error!(
            "🔒 Hold-on-failure enabled — stopping immediately: {}",
            reason
        );
        self.collect_and_log_debug_info().await;
        self.generate_installation_report(successful_phases, failed_phases)
            .await;

        let keepalive = "bash -lc 'echo \"[uaa] Hold mode — system mounted for debugging.\"; echo \"Press Ctrl-C when done.\"; while true; do sleep 3600; done'";
        let _ = self.runner.execute(keepalive).await;

        Err(crate::error::AutoInstallError::InstallationError(
            "Installation halted (hold-on-failure)".to_string(),
        ))
    }

    async fn print_next_commands_after_storage(
        &mut self,
        config: &InstallationConfig,
    ) -> Result<()> {
        use tracing::warn;
        warn!("=== PAUSE AFTER STORAGE REQUESTED ===");
        warn!("Completed: partitioning, formatting, LUKS, ZFS pools/datasets.");
        warn!("Next commands (run manually on the target):");
        for c in build_next_commands_after_storage(config) {
            warn!("  {}", c);
        }
        warn!("=== END OF NEXT COMMANDS ===");
        Ok(())
    }

    async fn setup_installation_variables(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Setting up installation variables");

        self.runner.execute("systemctl stop zed || true").await?;
        // Best-effort: `timedatectl` sets the LIVE env's clock, which is cosmetic
        // — the installed system's timezone is written in-chroot
        // (setup_basic_system_files). In a live ISO it can time out on the D-Bus
        // call (no NTP daemon / no network time), which must NOT fail the whole
        // install (observed timing out on U1 with `Connection timed out`).
        let _ = self
            .runner
            .execute(&format!(
                "timedatectl set-timezone {} 2>/dev/null || true",
                config.timezone
            ))
            .await;
        let _ = self
            .runner
            .execute("timedatectl set-ntp on 2>/dev/null || true")
            .await;

        let vars = [
            ("DISK", config.disk_device.as_str()),
            ("TIMEZONE", config.timezone.as_str()),
            ("HOSTNAME", config.hostname.as_str()),
            // NOTE: the LUKS passphrase is intentionally NOT exported here. It
            // is delivered to cryptsetup via a 0600 keyfile in
            // DiskManager::setup_luks_encryption; exporting it would put the
            // secret on a command line and in /proc/<pid>/environ.
            ("ROOT_PASSWORD", config.root_password.as_str()),
            ("NET_ET_INTERFACE", config.network_interface.as_str()),
            ("NET_ET_ADDRESS", config.network_address.as_str()),
            ("NET_ET_GATEWAY", config.network_gateway.as_str()),
            ("NET_ET_SEARCH", config.network_search.as_str()),
        ];

        for (key, value) in vars {
            self.runner
                .execute(&format!("export {}='{}'", key, value))
                .await?;
            self.variables.insert(key.to_string(), value.to_string());
        }

        let nameservers = config.network_nameservers.join(" ");
        self.runner
            .execute(&format!("export NET_ET_NAMESERVERS=({})", nameservers))
            .await?;

        Ok(())
    }

    async fn phase_1_package_installation(&mut self) -> Result<()> {
        info!("Phase 1: Package installation");
        let mut pm = PackageManager::new(&mut *self.runner);
        pm.install_required_packages().await?;
        info!("Phase 1 completed");
        Ok(())
    }

    async fn phase_2_disk_preparation(
        &mut self,
        config: &InstallationConfig,
        auth: &WipeAuthorization,
    ) -> Result<()> {
        info!("Phase 2: Disk preparation (storage_mode = {:?})", config.storage_mode);
        // storage_mode selects the disk path; PlainLuks (default) is the proven
        // single-disk Lenovo path, byte-identical to before; NativeKeystore is
        // the multi-disk U1 / server-profile partitioner.
        match config.storage_mode {
            StorageMode::PlainLuks => {
                let mut dm = DiskManager::new(&mut *self.runner);
                dm.prepare_disk(config, auth).await?;
            }
            StorageMode::NativeKeystore => {
                let mut dm = DiskNativeManager::new(&mut *self.runner);
                dm.prepare_disks(config, auth).await?;
            }
        }
        info!("Phase 2 completed");
        Ok(())
    }

    async fn phase_3_zfs_creation(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Phase 3: ZFS creation (storage_mode = {:?})", config.storage_mode);
        match config.storage_mode {
            StorageMode::PlainLuks => {
                let mut zm = ZfsManager::new(&mut *self.runner, &mut self.variables);
                zm.create_zfs_pools(config).await?;
            }
            StorageMode::NativeKeystore => {
                let mut zm = ZfsNativeManager::new(&mut *self.runner, &mut self.variables);
                zm.create_native_pools(config).await?;
            }
        }
        info!("Phase 3 completed");
        Ok(())
    }

    async fn phase_4_base_system(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Phase 4: Base system");
        let mut sc = SystemConfigurator::new(&mut *self.runner);
        sc.install_base_system(config).await?;
        if let Err(e) = self
            .runner
            .execute(&format!(
                "printf 'installed-by=uaa\\n' > /mnt/targetos{p} && chmod 0644 /mnt/targetos{p}",
                p = TARGET_MARKER_PATH
            ))
            .await
        {
            tracing::warn!("Could not write target marker (non-fatal): {}", e);
        }
        info!("Phase 4 completed");
        Ok(())
    }

    async fn phase_5_system_configuration(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Phase 5: System configuration");
        {
            // Stage RESET p2 (recovery ISO + tarball + gated helper + GRUB drop-in).
            // Non-fatal by design: a staging problem must not break the proven 7/7 flow.
            let mut rp = ResetPartitionStager::new(&mut *self.runner);
            if let Err(e) = rp.stage(config).await {
                tracing::warn!("RESET partition staging skipped: {e}");
            }
        }
        let mut sc = SystemConfigurator::new(&mut *self.runner);
        sc.configure_zfs_in_chroot(config).await?;
        sc.configure_grub_in_chroot(config).await?;
        sc.setup_luks_key_in_chroot(config).await?;
        sc.install_ca_cert_in_chroot(config).await?;
        // Applications may rely on the install CA trust anchor written above
        // (e.g. fetching a node cert over HTTP from the control server), so
        // this step must run after it. FAIL-CLOSED: an application failing
        // to install is a failed deployment and propagates with `?`, unlike
        // the non-fatal ResetPartitionStager wrapper above.
        let mut ai = ApplicationInstaller::new(&mut *self.runner);
        ai.install(config).await?;
        info!("Phase 5 completed");
        Ok(())
    }

    async fn phase_6_final_setup(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Phase 6: Final setup");
        let mut sc = SystemConfigurator::new(&mut *self.runner);
        sc.final_cleanup(config).await?;
        info!("Phase 6 completed — {} installed", config.hostname);
        Ok(())
    }
}

impl Default for SshInstaller {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the list of manual commands that would run after storage setup.
/// Used by pause-after-storage and tests.
pub(super) fn build_next_commands_after_storage(config: &InstallationConfig) -> Vec<String> {
    let esp_part = partition_path(&config.disk_device, 1);
    let p4 = partition_path(&config.disk_device, 4);
    let release = config.debootstrap_release.as_deref().unwrap_or("resolute");
    vec![
        "mkdir -p /mnt/targetos/boot/efi".to_string(),
        format!("mount {} /mnt/targetos/boot/efi", esp_part),
        format!(
            "debootstrap {} /mnt/targetos {}",
            release,
            config
                .debootstrap_mirror
                .as_deref()
                .unwrap_or("http://archive.ubuntu.com/ubuntu/")
        ),
        format!(
            "debootstrap {} /mnt/targetos {} # fallback",
            release, "http://old-releases.ubuntu.com/ubuntu/"
        ),
        "mkdir -p /mnt/targetos/etc/apt/sources.list.d".to_string(),
        format!("bash -lc 'cat > /mnt/targetos/etc/apt/sources.list.d/ubuntu.sources <<\'EOF\'\nTypes: deb\nURIs: http://archive.ubuntu.com/ubuntu/\nSuites: {rel}\nComponents: main restricted universe multiverse\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n\nTypes: deb\nURIs: http://security.ubuntu.com/ubuntu\nSuites: {rel}-security\nComponents: main restricted universe multiverse\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\nEOF'", rel=release),
        "rm -f /mnt/targetos/etc/apt/sources.list || true".to_string(),
        "mount --rbind /dev /mnt/targetos/dev".to_string(),
        "mount --make-private /mnt/targetos/dev".to_string(),
        "mount -t devpts devpts /mnt/targetos/dev/pts || true".to_string(),
        "mount --rbind /proc /mnt/targetos/proc".to_string(),
        "mount --make-private /mnt/targetos/proc".to_string(),
        "mount --rbind /sys /mnt/targetos/sys".to_string(),
        "mount --make-private /mnt/targetos/sys".to_string(),
        "mount --rbind /run /mnt/targetos/run".to_string(),
        "mount --make-private /mnt/targetos/run".to_string(),
        "echo 'nameserver 1.1.1.1' > /mnt/targetos/etc/resolv.conf".to_string(),
        format!("bash -lc 'ESP_UUID=$(blkid -s UUID -o value {e} 2>/dev/null || true); if [ -n \"$ESP_UUID\" ]; then echo \"UUID=$ESP_UUID /boot/efi vfat umask=0077 0 1\" >> /mnt/targetos/etc/fstab; fi'", e=esp_part),
        "chroot /mnt/targetos bash -lc '[ -d /sys/firmware/efi/efivars ] || mkdir -p /sys/firmware/efi/efivars; mountpoint -q /sys/firmware/efi/efivars || mount -t efivarfs efivarfs /sys/firmware/efi/efivars || true'".to_string(),
        "chroot /mnt/targetos bash -lc 'apt update'".to_string(),
        // Package set matched to the clean 26.04 install on len-serv-003: dracut
        // (never initramfs-tools), zfs-dracut (never zfs-initramfs), base clevis
        // (the tang pin is bundled — no clevis-tang pkg), and systemd-cryptsetup +
        // tpm2/fido2 stacks for the TPM2+PIN and YubiKey keyslots.
        "chroot /mnt/targetos bash -lc 'DEBIAN_FRONTEND=noninteractive apt install -y grub-efi-amd64 grub-efi-amd64-signed linux-image-generic shim-signed dracut dracut-network zfs-dracut zfsutils-linux zfs-zed efibootmgr cryptsetup dosfstools clevis clevis-luks clevis-dracut clevis-systemd systemd-cryptsetup tpm2-tools tpm-udev libfido2-1'".to_string(),
        "chroot /mnt/targetos bash -lc 'DEBIAN_FRONTEND=noninteractive apt purge -y os-prober || true'".to_string(),
        format!("bash -lc 'UUID=$(blkid -s UUID -o value {p4} 2>/dev/null || true); DEV=\"{p4}\"; [ -n \"$UUID\" ] && DEV=\"/dev/disk/by-uuid/$UUID\"; echo \"luks $DEV none luks,discard,initramfs\" > /mnt/targetos/etc/crypttab'"),
        "chroot /mnt/targetos bash -lc 'dracut --regenerate-all --force'".to_string(),
        // --uefi-secure-boot lays down the signed shim chain (shimx64.efi ->
        // grubx64.efi) so Secure Boot can be enabled without reinstalling.
        "chroot /mnt/targetos bash -lc 'grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=ubuntu --uefi-secure-boot --recheck'".to_string(),
        "chroot /mnt/targetos bash -lc 'grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=ubuntu --uefi-secure-boot --recheck --no-nvram' # fallback".to_string(),
        "chroot /mnt/targetos bash -lc 'update-grub'".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::ssh_installer::config::{InitramfsType, InstallationConfig};

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

    #[test]
    fn test_build_next_commands_contains_core_steps() {
        let cfg = sample_config();
        let cmds = build_next_commands_after_storage(&cfg);
        assert!(cmds.iter().any(|c| c.contains("debootstrap resolute")));
        assert!(cmds.iter().any(|c| c.contains("dracut --regenerate-all")));
        assert!(cmds.iter().any(|c| c.contains("grub-install")));
        assert!(cmds.iter().any(|c| c.contains("update-grub")));
    }

    #[test]
    fn test_installer_default_not_connected() {
        let installer = SshInstaller::new();
        assert!(!installer.connected);
    }

    // ── Phase-selection unit tests ────────────────────────────────────────────

    #[test]
    fn test_phase_selection_parse_single_range_list() {
        let single = PhaseSelection::parse("5").unwrap();
        assert!(single.is_explicit());
        assert!(single.contains(5));
        for p in [0u8, 1, 2, 3, 4, 6] {
            assert!(!single.contains(p), "phase {p} should not be selected");
        }

        let range = PhaseSelection::parse("4-6").unwrap();
        assert!(range.is_explicit());
        for p in [4u8, 5, 6] {
            assert!(range.contains(p));
        }
        for p in [0u8, 1, 2, 3] {
            assert!(!range.contains(p));
        }

        let list = PhaseSelection::parse("0,1,5").unwrap();
        assert!(list.is_explicit());
        for p in [0u8, 1, 5] {
            assert!(list.contains(p));
        }
        for p in [2u8, 3, 4, 6] {
            assert!(!list.contains(p));
        }
    }

    #[test]
    fn test_phase_selection_parse_rejects_invalid() {
        for spec in ["", "7", "6-4", "a", "1,,2"] {
            assert!(
                PhaseSelection::parse(spec).is_err(),
                "spec {spec:?} should be rejected"
            );
        }
    }

    #[test]
    fn test_phase_selection_default_is_full_not_explicit() {
        let full = PhaseSelection::full();
        for p in 0u8..=6 {
            assert!(full.contains(p));
        }
        assert!(!full.is_explicit());
        assert!(full.authorize_wipe().is_some());
    }

    #[test]
    fn test_authorize_wipe_denied_without_phase2() {
        assert!(PhaseSelection::parse("4-6")
            .unwrap()
            .authorize_wipe()
            .is_none());
        assert!(PhaseSelection::parse("2-6")
            .unwrap()
            .authorize_wipe()
            .is_some());
    }

    #[test]
    fn test_from_phase_shorthand() {
        let sel = PhaseSelection::from_phase(4).unwrap();
        assert_eq!(sel, PhaseSelection::parse("4-6").unwrap());
    }

    #[test]
    fn test_needs_prep_matrix() {
        let five = PhaseSelection::parse("5").unwrap();
        assert!(five.needs_luks_reopen());
        assert!(five.needs_pool_import());

        let three_six = PhaseSelection::parse("3-6").unwrap();
        assert!(three_six.needs_luks_reopen());
        assert!(!three_six.needs_pool_import());

        let two_six = PhaseSelection::parse("2-6").unwrap();
        assert!(!two_six.needs_luks_reopen());
        assert!(!two_six.needs_pool_import());

        let full = PhaseSelection::full();
        assert!(!full.needs_luks_reopen());
        assert!(!full.needs_pool_import());
    }

    // ── Recording executor for phase-sequencing tests ─────────────────────────

    use std::sync::{Arc, Mutex};

    /// Records every command routed through the executor into a shared log so
    /// tests can assert which commands a selective/full run issued. Mirrors the
    /// `RecordingMock` pattern in `src/autoinstall/place.rs`.
    #[derive(Clone, Default)]
    struct RecordingExecutor {
        /// Command → preset response (drives `check_silent` / output methods).
        responses: HashMap<String, String>,
        /// Ordered log of every command string seen.
        commands: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingExecutor {
        fn new() -> Self {
            Self::default()
        }

        fn with_responses(pairs: &[(&str, &str)]) -> Self {
            Self {
                responses: pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                commands: Arc::new(Mutex::new(vec![])),
            }
        }

        fn recorded(&self) -> Vec<String> {
            self.commands.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
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
            Ok(self.responses.get(cmd).cloned().unwrap_or_default())
        }
        async fn execute_with_error_collection(
            &mut self,
            cmd: &str,
            _desc: &str,
        ) -> Result<(i32, String, String)> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok((0, self.responses.get(cmd).cloned().unwrap_or_default(), String::new()))
        }
        async fn check_silent(&mut self, cmd: &str) -> Result<bool> {
            self.commands.lock().unwrap().push(cmd.to_string());
            Ok(!self.responses.get(cmd).map_or(true, |s| s.is_empty()))
        }
        async fn collect_debug_info(&mut self) -> Result<String> {
            Ok(String::new())
        }
        async fn upload_file(&mut self, _local: &str, _remote: &str) -> Result<()> {
            Ok(())
        }
        async fn download_file(&mut self, _remote: &str, _local: &str) -> Result<()> {
            Ok(())
        }
        fn disconnect(&mut self) {}
    }

    fn contains_cmd(cmds: &[String], needle: &str) -> bool {
        cmds.iter().any(|c| c.contains(needle))
    }

    fn position_cmd(cmds: &[String], needle: &str) -> Option<usize> {
        cmds.iter().position(|c| c.contains(needle))
    }

    #[tokio::test]
    async fn test_selective_run_skips_unselected_phases() {
        // --phases 5 triggers the non-destructive mount-existing-target prep
        // (needs_luks_reopen + needs_pool_import), which discovers the root
        // dataset — preset it so prep completes and Phase 5 actually runs.
        let mock = RecordingExecutor::with_responses(&[(
            "zfs list -H -o name -r rpool/ROOT | grep -m1 '^rpool/ROOT/ubuntu_'",
            "rpool/ROOT/ubuntu_abc123",
        )]);
        let mut installer = SshInstaller::for_tests(Box::new(mock.clone()));
        let selection = PhaseSelection::parse("5").unwrap();
        let cfg = sample_config();

        let _ = installer.perform_installation(&cfg, &selection).await;

        let cmds = mock.recorded();
        for forbidden in [
            "wipefs",
            "sgdisk",
            "debootstrap",
            "zpool create",
            "cryptsetup luksFormat",
        ] {
            assert!(
                !contains_cmd(&cmds, forbidden),
                "selective --phases 5 run must not issue {forbidden:?}"
            );
        }
        assert!(
            contains_cmd(&cmds, "grub"),
            "Phase 5 should still issue a grub command"
        );
    }

    #[tokio::test]
    async fn test_phase5_installs_ca_cert_trust_anchor() {
        // Same --phases 5 setup as test_selective_run_skips_unselected_phases.
        let mock = RecordingExecutor::with_responses(&[(
            "zfs list -H -o name -r rpool/ROOT | grep -m1 '^rpool/ROOT/ubuntu_'",
            "rpool/ROOT/ubuntu_abc123",
        )]);
        let mut installer = SshInstaller::for_tests(Box::new(mock.clone()));
        let selection = PhaseSelection::parse("5").unwrap();
        let cfg = sample_config();

        let _ = installer.perform_installation(&cfg, &selection).await;

        let cmds = mock.recorded();
        assert!(
            contains_cmd(&cmds, "/mnt/targetos/etc/uaa/install-ca.crt"),
            "Phase 5 should write the install CA cert into the target"
        );
        assert!(
            contains_cmd(&cmds, &cfg.install_ca_cert),
            "written cert content should match config.install_ca_cert"
        );
    }

    #[tokio::test]
    async fn test_phase5_writes_placeholder_verbatim_when_ca_unplaced() {
        // A config placed before the install CA was reachable still carries
        // the literal placeholder; phase 5 must write it as-is (fail-closed —
        // uaa enroll treats an unparseable CA as the missing-CA case) rather
        // than erroring out or silently skipping the file.
        let mock = RecordingExecutor::with_responses(&[(
            "zfs list -H -o name -r rpool/ROOT | grep -m1 '^rpool/ROOT/ubuntu_'",
            "rpool/ROOT/ubuntu_abc123",
        )]);
        let mut installer = SshInstaller::for_tests(Box::new(mock.clone()));
        let selection = PhaseSelection::parse("5").unwrap();
        let mut cfg = sample_config();
        cfg.install_ca_cert = crate::config_place::PLACEHOLDER.to_string();

        let result = installer.perform_installation(&cfg, &selection).await;
        assert!(result.is_ok(), "phase 5 must not fail on an unplaced CA: {result:?}");

        let cmds = mock.recorded();
        assert!(
            contains_cmd(&cmds, "REPLACE_AT_PLACE_TIME"),
            "placeholder should be written verbatim, not silently dropped"
        );
    }

    #[tokio::test]
    async fn test_default_run_full_sequence() {
        let mock = RecordingExecutor::new();
        let mut installer = SshInstaller::for_tests(Box::new(mock.clone()));
        let selection = PhaseSelection::full();
        let cfg = sample_config();

        let _ = installer.perform_installation(&cfg, &selection).await;

        let cmds = mock.recorded();
        let wipefs = position_cmd(&cmds, "wipefs -a").expect("wipefs -a expected");
        let sgdisk = position_cmd(&cmds, "sgdisk").expect("sgdisk expected");
        let zpool = position_cmd(&cmds, "zpool create").expect("zpool create expected");
        // Match the Phase-4 debootstrap *invocation*, not the "debootstrap"
        // package name apt installs back in Phase 1.
        let debootstrap =
            position_cmd(&cmds, "debootstrap resolute /mnt/targetos").expect("debootstrap expected");
        let grub = position_cmd(&cmds, "grub-install").expect("grub-install expected");

        assert!(
            wipefs <= sgdisk && sgdisk < zpool && zpool < debootstrap && debootstrap < grub,
            "expected wipefs<=sgdisk<zpool<debootstrap<grub, got {wipefs} {sgdisk} {zpool} {debootstrap} {grub}"
        );
    }

    #[tokio::test]
    async fn test_preflight_selective_no_wipe_on_residual() {
        // Residual rpool present, but --phases 5 omits Phase 2 → guard refuses.
        let mock = RecordingExecutor::with_responses(&[(
            "zpool list -H rpool >/dev/null 2>&1",
            "rpool",
        )]);
        let mut installer = SshInstaller::for_tests(Box::new(mock.clone()));
        let selection = PhaseSelection::parse("5").unwrap();
        let cfg = sample_config();

        let _ = installer.perform_installation(&cfg, &selection).await;

        let cmds = mock.recorded();
        assert!(
            !contains_cmd(&cmds, "wipefs"),
            "residual selective run must not wipe"
        );
        assert!(
            !contains_cmd(&cmds, "sgdisk --zap-all"),
            "residual selective run must not zap GPT"
        );
    }

    #[tokio::test]
    async fn test_default_run_still_wipes_on_residual() {
        // ANTI-OVER-SUPPRESSION: the same residual state under the flagless full
        // selection MUST still wipe — the guard must never block the normal path.
        let mock = RecordingExecutor::with_responses(&[(
            "zpool list -H rpool >/dev/null 2>&1",
            "rpool",
        )]);
        let mut installer = SshInstaller::for_tests(Box::new(mock.clone()));
        let selection = PhaseSelection::full();
        let cfg = sample_config();

        let _ = installer.perform_installation(&cfg, &selection).await;

        let cmds = mock.recorded();
        assert!(
            contains_cmd(&cmds, "wipefs -a"),
            "full install on residual state must still wipe"
        );
    }

    // ── Mount-existing-target prep (phase-rerun/TASK-02) ──────────────────────

    /// The dataset-discovery command that `mount_target_for_rerun` issues; preset
    /// its output so prep can complete in the mock.
    const ROOT_DISCOVER_CMD: &str =
        "zfs list -H -o name -r rpool/ROOT | grep -m1 '^rpool/ROOT/ubuntu_'";

    #[tokio::test]
    async fn test_preflight_selective_skips_recovery_wipe() {
        // Residual rpool present + --phases 5 (omits Phase 2): preflight must
        // BYPASS the recovery-wipe (log + continue), returning Ok, and issue no
        // destructive command.
        let mock = RecordingExecutor::with_responses(&[(
            "zpool list -H rpool >/dev/null 2>&1",
            "rpool",
        )]);
        let mut installer = SshInstaller::for_tests(Box::new(mock.clone()));
        let selection = PhaseSelection::parse("5").unwrap();
        let cfg = sample_config();

        let result = installer.preflight_checks(&cfg, &selection).await;
        assert!(result.is_ok(), "selective preflight must bypass, not error");

        let cmds = mock.recorded();
        for forbidden in ["wipefs", "sgdisk --zap-all", "zpool destroy", "cryptsetup close"] {
            assert!(
                !contains_cmd(&cmds, forbidden),
                "selective preflight bypass must not issue {forbidden:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_prep_pool_already_imported_skips_import() {
        // Both pools already imported → import is skipped entirely.
        let mock = RecordingExecutor::with_responses(&[
            ("zpool list -H rpool >/dev/null 2>&1", "rpool"),
            ("zpool list -H bpool >/dev/null 2>&1", "bpool"),
            (ROOT_DISCOVER_CMD, "rpool/ROOT/ubuntu_abc123"),
        ]);
        let mut installer = SshInstaller::for_tests(Box::new(mock.clone()));
        let selection = PhaseSelection::parse("5").unwrap();
        let cfg = sample_config();

        let _ = installer.perform_installation(&cfg, &selection).await;

        let cmds = mock.recorded();
        assert!(
            !contains_cmd(&cmds, "zpool import"),
            "already-imported pools must not be re-imported"
        );
    }

    #[tokio::test]
    async fn test_prep_luks_already_open_skips_open() {
        // Mapper already open → no cryptsetup open is issued.
        let mock = RecordingExecutor::with_responses(&[
            ("cryptsetup status luks >/dev/null 2>&1", "active"),
            (ROOT_DISCOVER_CMD, "rpool/ROOT/ubuntu_abc123"),
        ]);
        let mut installer = SshInstaller::for_tests(Box::new(mock.clone()));
        let selection = PhaseSelection::parse("5").unwrap();
        let cfg = sample_config();

        let _ = installer.perform_installation(&cfg, &selection).await;

        let cmds = mock.recorded();
        assert!(
            !contains_cmd(&cmds, "cryptsetup open"),
            "already-open LUKS mapper must not be reopened"
        );
    }

    #[tokio::test]
    async fn test_prep_mount_order_root_then_boot_then_esp() {
        // THE faea48e regression test: / (rpool ROOT) before /boot (bpool BOOT)
        // before the ESP.
        let mock = RecordingExecutor::with_responses(&[(
            ROOT_DISCOVER_CMD,
            "rpool/ROOT/ubuntu_abc123",
        )]);
        let mut installer = SshInstaller::for_tests(Box::new(mock.clone()));
        let selection = PhaseSelection::parse("5").unwrap();
        let cfg = sample_config();

        let _ = installer.perform_installation(&cfg, &selection).await;

        let cmds = mock.recorded();
        let root = position_cmd(&cmds, "zfs mount rpool/ROOT/ubuntu_abc123")
            .expect("root mount expected");
        let boot = position_cmd(&cmds, "zfs mount bpool/BOOT/ubuntu_abc123")
            .expect("boot mount expected");
        // Match the ESP *mount* specifically (the normalize step umounts the same
        // path first, so an unqualified `/mnt/targetos/boot/efi` would mis-match).
        let esp = position_cmd(&cmds, "mount /dev/nvme0n1p1 /mnt/targetos/boot/efi")
            .expect("ESP mount expected");
        assert!(
            root < boot && boot < esp,
            "expected root<boot<esp, got {root} {boot} {esp}"
        );
    }

    #[tokio::test]
    async fn test_prep_normalizes_partial_mounts_first() {
        // The umount inverse ops run BEFORE any import/mount, and prep never
        // exports a pool.
        let mock = RecordingExecutor::with_responses(&[(
            ROOT_DISCOVER_CMD,
            "rpool/ROOT/ubuntu_abc123",
        )]);
        let mut installer = SshInstaller::for_tests(Box::new(mock.clone()));
        let selection = PhaseSelection::parse("5").unwrap();
        let cfg = sample_config();

        let _ = installer.perform_installation(&cfg, &selection).await;

        let cmds = mock.recorded();
        let last_umount = [
            "umount -R /mnt/targetos/sys",
            "umount -R /mnt/targetos/proc",
            "umount -R /mnt/targetos/dev",
            "umount -R /mnt/targetos/run",
            "umount /mnt/targetos/boot/efi",
        ]
        .iter()
        .map(|n| position_cmd(&cmds, n).expect("normalize umount expected"))
        .max()
        .unwrap();
        let first_state_change = [
            position_cmd(&cmds, "zpool import"),
            position_cmd(&cmds, "zfs mount"),
        ]
        .into_iter()
        .flatten()
        .min()
        .expect("an import or mount expected");
        assert!(
            last_umount < first_state_change,
            "normalize umounts must precede import/mount, got {last_umount} vs {first_state_change}"
        );
        assert!(
            !contains_cmd(&cmds, "zpool export"),
            "prep must never export a pool"
        );
    }

    #[tokio::test]
    async fn test_prep_import_uses_altroot_no_automount() {
        // Every zpool import must carry -N (no automount) and -R /mnt/targetos.
        let mock = RecordingExecutor::with_responses(&[(
            ROOT_DISCOVER_CMD,
            "rpool/ROOT/ubuntu_abc123",
        )]);
        let mut installer = SshInstaller::for_tests(Box::new(mock.clone()));
        let selection = PhaseSelection::parse("5").unwrap();
        let cfg = sample_config();

        let _ = installer.perform_installation(&cfg, &selection).await;

        let cmds = mock.recorded();
        let imports: Vec<&String> =
            cmds.iter().filter(|c| c.contains("zpool import")).collect();
        assert!(!imports.is_empty(), "prep must import at least one pool");
        for cmd in imports {
            assert!(
                cmd.contains("-N") && cmd.contains("-R /mnt/targetos"),
                "import must use -N and -R /mnt/targetos: {cmd}"
            );
        }
    }
}
