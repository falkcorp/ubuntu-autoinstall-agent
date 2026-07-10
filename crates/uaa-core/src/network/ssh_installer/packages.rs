// file: crates/uaa-core/src/network/ssh_installer/packages.rs
// version: 2.1.1
// guid: sshpkg01-2345-6789-abcd-ef0123456789
// last-edited: 2026-07-10

//! Package management for SSH installation

use crate::network::CommandExecutor;
use crate::Result;
use tracing::info;

pub struct PackageManager<'a> {
    runner: &'a mut dyn CommandExecutor,
}

impl<'a> PackageManager<'a> {
    pub fn new(runner: &'a mut dyn CommandExecutor) -> Self {
        Self { runner }
    }

    /// Install required packages for installation
    pub async fn install_required_packages(&mut self) -> Result<()> {
        info!("Installing required packages");

        // Update package lists first
        self.runner.execute("apt-get update").await?;

        // Install ZFS utilities specifically
        self.runner
            .execute("DEBIAN_FRONTEND=noninteractive apt-get install -y zfsutils-linux")
            .await?;

        // Install other required packages.
        // NOTE (live environment, not the target): clevis + clevis-luks are
        // required HERE because `clevis luks bind` for Tang enrollment runs on the
        // live host against the LUKS partition (the mapper isn't visible in the
        // chroot). The 26.04 live-server ISO does NOT ship clevis, so without this
        // Tang enrollment silently skips. mdadm is needed to assemble/query IMSM
        // (BIOS fake-RAID) volumes like unimatrixone's /dev/md126; harmless on
        // hosts without md devices.
        let packages = [
            "cryptsetup",
            "parted",
            "gdisk",
            "debootstrap",
            "dosfstools",
            "xfsprogs",
            "util-linux",
            "clevis",
            "clevis-luks",
            "mdadm",
        ];

        let install_cmd = format!(
            "DEBIAN_FRONTEND=noninteractive apt-get install -y {}",
            packages.join(" ")
        );
        self.runner.execute(&install_cmd).await?;

        info!("Required packages installed successfully");
        Ok(())
    }

    /// Check if specific tools are available
    pub async fn check_tool_availability(&mut self, tools: &[&str]) -> Result<Vec<String>> {
        let mut available = Vec::new();

        for tool in tools {
            match self
                .runner
                .execute(&format!("command -v {} >/dev/null 2>&1", tool))
                .await
            {
                Ok(_) => available.push(tool.to_string()),
                Err(_) => continue,
            }
        }

        Ok(available)
    }
}
