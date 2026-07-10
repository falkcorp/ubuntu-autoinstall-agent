// file: src/network/ssh_installer/installer.rs
// version: 2.6.0
// guid: sshins01-2345-6789-abcd-ef0123456789
// last-edited: 2026-07-10

//! Main SSH/local installer orchestrating all installation phases.
//!
//! Uses a `Box<dyn CommandExecutor>` runner so the same phase logic works
//! whether execution happens locally (`LocalClient`) or over SSH (`SshClient`).

use super::config::{InstallationConfig, SystemInfo};
use super::disk_ops::DiskManager;
use super::investigation::SystemInvestigator;
use super::packages::PackageManager;
use super::partitions::partition_path;
use super::system_setup::SystemConfigurator;
use super::zfs_ops::ZfsManager;
use crate::network::{CommandExecutor, LocalClient, SshClient};
use crate::Result;
use std::collections::HashMap;
use tracing::{error, info};

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
    ) -> Result<()> {
        if !hold_on_failure && !pause_after_storage {
            return self.perform_installation(config).await;
        }
        self.require_connected()?;

        info!(
            "Starting ZFS+LUKS installation for {} (hold={}, pause-after-storage={})",
            config.hostname, hold_on_failure, pause_after_storage
        );

        let mut failed_phases: Vec<String> = Vec::new();
        let mut successful_phases: Vec<&str> = Vec::new();

        if let Err(e) = self.preflight_checks(config).await {
            error!("✗ Preflight checks failed: {}", e);
        } else {
            info!("✓ Preflight checks passed");
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

        run_phase!("Phase 0: Setup variables", self.setup_installation_variables(config));
        run_phase!("Phase 1: Package installation", self.phase_1_package_installation());
        run_phase!("Phase 2: Disk preparation", self.phase_2_disk_preparation(config));
        run_phase!("Phase 3: ZFS creation", self.phase_3_zfs_creation(config));

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

        run_phase!("Phase 4: Base system", self.phase_4_base_system(config));
        run_phase!(
            "Phase 5: System configuration",
            self.phase_5_system_configuration(config)
        );
        run_phase!("Phase 6: Final setup", self.phase_6_final_setup(config));

        self.generate_installation_report(&successful_phases, &failed_phases)
            .await;
        info!(
            "🎉 Installation completed successfully for {}",
            config.hostname
        );
        Ok(())
    }

    /// Full installation with standard error collection (continues past failures).
    pub async fn perform_installation(&mut self, config: &InstallationConfig) -> Result<()> {
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

        match self.preflight_checks(config).await {
            Ok(_) => info!("✓ Preflight checks passed"),
            Err(e) => {
                error!("✗ Preflight checks failed: {}", e);
                self.collect_and_log_debug_info().await;
            }
        }

        run_phase!("Phase 0: Setup variables", 10, self.setup_installation_variables(config));
        run_phase!("Phase 1: Package installation", 20, self.phase_1_package_installation());
        run_phase!("Phase 2: Disk preparation", 35, self.phase_2_disk_preparation(config));
        run_phase!("Phase 3: ZFS creation", 50, self.phase_3_zfs_creation(config));
        run_phase!("Phase 4: Base system", 75, self.phase_4_base_system(config));
        run_phase!(
            "Phase 5: System configuration",
            90,
            self.phase_5_system_configuration(config)
        );
        run_phase!("Phase 6: Final setup", 95, self.phase_6_final_setup(config));

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

    async fn preflight_checks(&mut self, config: &InstallationConfig) -> Result<()> {
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
                "Preflight: residual state detected (bpool={} rpool={} luks={} mounts={}); recovering",
                has_bpool, has_rpool, luks_active, target_has_mounts
            );
            let mut disk_manager = DiskManager::new(&mut *self.runner);
            let _ = disk_manager.recover_after_failure_and_wipe(config).await;
        }

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
        self.runner
            .execute(&format!("timedatectl set-timezone {}", config.timezone))
            .await?;
        self.runner.execute("timedatectl set-ntp on").await?;

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

    async fn phase_2_disk_preparation(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Phase 2: Disk preparation");
        let mut dm = DiskManager::new(&mut *self.runner);
        dm.prepare_disk(config).await?;
        info!("Phase 2 completed");
        Ok(())
    }

    async fn phase_3_zfs_creation(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Phase 3: ZFS creation");
        let mut zm = ZfsManager::new(&mut *self.runner, &mut self.variables);
        zm.create_zfs_pools(config).await?;
        info!("Phase 3 completed");
        Ok(())
    }

    async fn phase_4_base_system(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Phase 4: Base system");
        let mut sc = SystemConfigurator::new(&mut *self.runner);
        sc.install_base_system(config).await?;
        info!("Phase 4 completed");
        Ok(())
    }

    async fn phase_5_system_configuration(&mut self, config: &InstallationConfig) -> Result<()> {
        info!("Phase 5: System configuration");
        let mut sc = SystemConfigurator::new(&mut *self.runner);
        sc.configure_zfs_in_chroot(config).await?;
        sc.configure_grub_in_chroot(config).await?;
        sc.setup_luks_key_in_chroot(config).await?;
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
}
