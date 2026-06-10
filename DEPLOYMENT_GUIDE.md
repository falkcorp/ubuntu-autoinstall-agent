<!-- file: DEPLOYMENT_GUIDE.md -->
<!-- version: 1.0.0 -->
<!-- guid: g8h9i0j1-k2l3-4567-8901-234567ghijkl -->

# Ubuntu AutoInstall Agent - Deployment Guide

This guide explains how to use the Ubuntu AutoInstall Agent for automated Ubuntu server deployment with golden images and LUKS encryption.

## Overview

The Ubuntu AutoInstall Agent provides:

- **Golden Image Creation**: Creates custom Ubuntu images with pre-installed packages
- **LUKS Encryption**: Full disk encryption for security
- **SSH Deployment**: Remote deployment to target machines
- **Zero Manual Intervention**: Fully automated installation process

## Prerequisites

Run the system check first:

```bash
sudo ./ubuntu-autoinstall-agent check-prereqs
```

### Required System Tools

On Ubuntu/Debian:
```bash
sudo apt update
sudo apt install qemu-kvm qemu-utils libguestfs-tools genisoimage cryptsetup
```

On Red Hat/CentOS/Fedora:
```bash
sudo dnf install qemu-kvm qemu-img guestfs-tools genisoimage cryptsetup
```

### System Requirements

- **Memory**: 4GB+ RAM recommended for image building
- **Storage**: 20GB+ free space for temporary files
- **KVM**: Hardware virtualization support (optional but recommended)
- **Root Access**: Required for disk operations and LUKS setup

## Basic Workflow

### 1. Create a Golden Image

```bash
# Create basic Ubuntu 26.04 image
sudo ./ubuntu-autoinstall-agent create-image

# Create custom image with specification
sudo ./ubuntu-autoinstall-agent create-image \
  --spec examples/specs/ubuntu-26.04-secure.yaml \
  --output /var/lib/ubuntu-autoinstall/images/secure-web-server.qcow2
```

### 2. Prepare Target Machine

Boot the target machine into a rescue environment (Ubuntu Live CD, rescue mode, etc.) with SSH access.

### 3. Configure Target

Create or customize a target configuration:

```yaml
# target-config.yaml
hostname: web-server-001
architecture: amd64
disk_device: /dev/sda
timezone: UTC

network:
  interface: eth0
  dhcp: true
  dns_servers: [1.1.1.1, 1.0.0.1]

users:
  - name: admin
    sudo: true
    ssh_keys:
      - "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIYour_SSH_Key_Here"

luks_config:
  passphrase: "${LUKS_PASSPHRASE}"
  cipher: aes-xts-plain64
  key_size: 512
  hash: sha256

packages:
  - openssh-server
  - curl
  - htop
```

### 4. Deploy to Target

```bash
# Set LUKS passphrase
export LUKS_PASSPHRASE="your-strong-passphrase"

# Deploy via SSH
sudo ./ubuntu-autoinstall-agent deploy \
  --target 192.168.1.100 \
  --config target-config.yaml \
  --via-ssh
```

## Configuration Files

### Image Specifications

Define what goes into your golden images:

```yaml
# examples/specs/ubuntu-26.04-minimal.yaml
ubuntu_version: "26.04"
architecture: amd64

base_packages:
  - openssh-server
  - curl
  - wget
  - htop

vm_config:
  memory_mb: 2048
  disk_size_gb: 20
  cpu_cores: 2
```

### Target Configurations

Define how images are deployed:

```yaml
# examples/configs/production-server.yaml
hostname: prod-web-001
architecture: amd64
disk_device: /dev/nvme0n1
timezone: America/New_York

network:
  interface: ens18
  dhcp: false
  ip_address: 192.168.1.100/24
  gateway: 192.168.1.1
  dns_servers: [1.1.1.1, 1.0.0.1]

users:
  - name: admin
    sudo: true
    ssh_keys:
      - "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIYour_Key_Here"

luks_config:
  passphrase: "${LUKS_PASSPHRASE}"
  cipher: aes-xts-plain64
  key_size: 512
  hash: sha256

packages:
  - nginx
  - certbot
  - fail2ban
```

## Security Considerations

1. **Environment Variables**: Use environment variables for sensitive data like LUKS passphrases
2. **SSH Keys**: Always use SSH key authentication
3. **Network Security**: Ensure secure network communication
4. **LUKS Passphrases**: Use strong, randomly generated passphrases

## Troubleshooting

### Common Issues

1. **Missing Dependencies**: Run `check-prereqs` command
2. **KVM Unavailable**: Ensure virtualization is enabled in BIOS
3. **SSH Connection Fails**: Verify target is in rescue mode with SSH enabled
4. **Insufficient Resources**: Ensure adequate memory and disk space

### Logs

- Image creation logs: `/var/log/ubuntu-autoinstall/`
- System logs: Check `journalctl` and `/var/log/syslog`

### Validation

```bash
# Validate an image
./ubuntu-autoinstall-agent validate --image /path/to/image.qcow2

# List available images
./ubuntu-autoinstall-agent list-images

# Test deployment (dry run)
./ubuntu-autoinstall-agent deploy --target 192.168.1.100 --config config.yaml --via-ssh --dry-run
```

## Production Deployment

1. **Image Repository**: Set up centralized image storage
2. **Automation**: Integrate with CI/CD pipelines
3. **Monitoring**: Monitor deployment success/failure
4. **Backup**: Maintain backup images and configurations
5. **Testing**: Test deployments in staging environments

## Advanced Usage

### Custom Scripts

Add custom scripts to image specifications:

```yaml
custom_scripts:
  - /path/to/setup-monitoring.sh
  - /path/to/security-hardening.sh
```

### ARM64 Support

Create ARM64 images:

```bash
./ubuntu-autoinstall-agent create-image --arch arm64
```

### Cleanup Old Images

```bash
# Remove images older than 30 days
./ubuntu-autoinstall-agent cleanup --older-than-days 30

# Dry run to see what would be deleted
./ubuntu-autoinstall-agent cleanup --older-than-days 30 --dry-run
```

This system provides a complete solution for automated Ubuntu server deployment with security and reproducibility.
