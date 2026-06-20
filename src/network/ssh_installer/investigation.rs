// file: src/network/ssh_installer/investigation.rs
// version: 1.2.0
// guid: sshinv01-2345-6789-abcd-ef0123456789

//! System investigation capabilities for SSH installation

use super::config::SystemInfo;
use crate::Result;
use tracing::{info, warn};

pub struct SystemInvestigator<'a, T: ?Sized> {
    executor: &'a mut T,
}

impl<'a, T: ?Sized> SystemInvestigator<'a, T>
where
    T: crate::network::CommandExecutor,
{
    pub fn new(executor: &'a mut T) -> Self {
        Self { executor }
    }

    /// Perform comprehensive system investigation
    pub async fn investigate_system(&mut self) -> Result<SystemInfo> {
        info!("Starting comprehensive system investigation");

        let system_info = SystemInfo {
            hostname: self.get_command_output("hostname").await?,
            kernel_version: self.get_command_output("uname -r").await?,
            os_release: self
                .get_command_output("cat /etc/os-release | head -1")
                .await?,
            disk_info: self.investigate_disks().await?,
            network_info: self.investigate_network().await?,
            available_tools: self.check_available_tools().await?,
            memory_info: self.get_command_output("free -h").await?,
            cpu_info: self.get_command_output("lscpu").await?,
        };

        info!("System investigation completed");
        Ok(system_info)
    }

    /// Investigate disk configuration
    async fn investigate_disks(&mut self) -> Result<String> {
        info!("Investigating disk configuration");

        let mut disk_info = String::new();

        // List all block devices
        disk_info.push_str("=== Block Devices ===\n");
        disk_info.push_str(&self.get_command_output("lsblk -a").await?);
        disk_info.push_str("\n\n");

        // Show disk details
        disk_info.push_str("=== Disk Details ===\n");
        disk_info.push_str(&self.get_command_output("fdisk -l").await?);
        disk_info.push_str("\n\n");

        // Check for existing ZFS pools
        disk_info.push_str("=== ZFS Pools ===\n");
        let zfs_pools = self
            .get_command_output("zpool list 2>/dev/null || echo 'No ZFS pools found'")
            .await?;
        disk_info.push_str(&zfs_pools);
        disk_info.push_str("\n\n");

        // Check for LUKS devices
        disk_info.push_str("=== LUKS Devices ===\n");
        let luks_devices = self
            .get_command_output(
                "cryptsetup status luks 2>/dev/null || echo 'No LUKS devices found'",
            )
            .await?;
        disk_info.push_str(&luks_devices);
        disk_info.push_str("\n\n");

        // Check mounted filesystems
        disk_info.push_str("=== Mounted Filesystems ===\n");
        disk_info.push_str(&self.get_command_output("mount | grep '^/dev'").await?);

        Ok(disk_info)
    }

    /// Investigate network configuration
    async fn investigate_network(&mut self) -> Result<String> {
        info!("Investigating network configuration");

        let mut network_info = String::new();

        // Network interfaces
        network_info.push_str("=== Network Interfaces ===\n");
        network_info.push_str(&self.get_command_output("ip addr show").await?);
        network_info.push_str("\n\n");

        // Routing table
        network_info.push_str("=== Routing Table ===\n");
        network_info.push_str(&self.get_command_output("ip route show").await?);
        network_info.push_str("\n\n");

        // DNS configuration
        network_info.push_str("=== DNS Configuration ===\n");
        network_info.push_str(
            &self
                .get_command_output("cat /etc/resolv.conf 2>/dev/null || echo 'No resolv.conf'")
                .await?,
        );

        Ok(network_info)
    }

    /// Check what tools are available on the system
    async fn check_available_tools(&mut self) -> Result<Vec<String>> {
        info!("Checking available tools");

        let required_tools = [
            "zfs",
            "zpool",
            "cryptsetup",
            "debootstrap",
            "chroot",
            "sgdisk",
            "gdisk",
            "fdisk",
            "mkfs.ext4",
            "mkfs.vfat",
            "mount",
            "umount",
            "rsync",
            "wget",
            "curl",
        ];

        let mut available = Vec::new();
        let mut missing = Vec::new();

        for tool in &required_tools {
            match self
                .executor
                .execute(&format!("command -v {} >/dev/null 2>&1", tool))
                .await
            {
                Ok(_) => available.push(tool.to_string()),
                Err(_) => missing.push(tool.to_string()),
            }
        }

        if !missing.is_empty() {
            warn!("Missing tools: {:?}", missing);
        }

        Ok(available)
    }

    async fn get_command_output(&mut self, command: &str) -> Result<String> {
        self.executor.execute_with_output(command).await
    }
}
