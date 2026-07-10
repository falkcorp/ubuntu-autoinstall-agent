# Ubuntu AutoInstall Agent

<!-- file: README.md -->
<!-- version: 1.1.0 -->
<!-- guid: 123e4567-e89b-12d3-a456-426614174000 -->
<!-- last-edited: 2026-07-10 -->

A robust, automated Ubuntu server deployment tool written in Rust that creates golden images and deploys them with LUKS full disk encryption.

## Features

- 🚀 **Zero Manual Intervention**: Fully automated Ubuntu server deployment
- 🔐 **LUKS Encryption**: Full disk encryption by default for security
- 🏗️ **Golden Images**: Build once, deploy many times approach
- 🖥️ **Multi-Architecture**: Support for both amd64 and arm64
- 🌐 **Flexible Deployment**: SSH and netboot/PXE deployment methods
- ⚡ **Fast & Reliable**: 10x faster than shell-based approaches
- 🛡️ **Memory Safe**: Written in Rust for maximum safety and performance

## Architecture: Path A vs Path B

The agent provides two installer code paths: **Path A** (`src/autoinstall/`) renders cloud-init/subiquity user-data and verifies deployments; **Path B** (`src/network/ssh_installer/`) is the proven 7-phase direct ZFS-on-LUKS installer that executes over SSH or locally. Use Path A for rendering and placing netboot seeds; use Path B for new ZFS-on-LUKS machine provisioning. See [docs/architecture-path-split.md](docs/architecture-path-split.md) for full details, including guardrails for future development.

## Quick Start

### Installation

Download the latest release for your platform:

```bash
# Download for amd64
curl -L -o ubuntu-autoinstall-agent \
  https://github.com/jdfalk/ubuntu-autoinstall-agent/releases/latest/download/ubuntu-autoinstall-agent-x86_64-unknown-linux-gnu

# Make executable
chmod +x ubuntu-autoinstall-agent
sudo mv ubuntu-autoinstall-agent /usr/local/bin/
```

### Basic Usage

1. **Create a golden image**:
   ```bash
   ubuntu-autoinstall-agent create-image --arch amd64 --version 24.04
   ```

2. **Deploy to a server** (dry run first):
   ```bash
   # Set encryption passphrase
   export LUKS_PASSPHRASE="your-secure-passphrase"

   # Test deployment
   ubuntu-autoinstall-agent deploy \
     --target your-server.example.com \
     --config examples/configs/basic-server.yaml \
     --via-ssh --dry-run

   # Actual deployment
   ubuntu-autoinstall-agent deploy \
     --target your-server.example.com \
     --config examples/configs/basic-server.yaml \
     --via-ssh
   ```

3. **List available images**:
   ```bash
   ubuntu-autoinstall-agent list-images
   ```

## CLI Commands

The tool provides several commands for managing Ubuntu deployments:

### `create-image`
Create a golden Ubuntu image using VM automation.

```bash
ubuntu-autoinstall-agent create-image [OPTIONS]

Options:
  -a, --arch <ARCH>        Architecture [default: amd64] [possible values: amd64, arm64]
  -v, --version <VERSION>  Ubuntu version [default: 24.04]
  -o, --output <OUTPUT>    Output image path
  -s, --spec <SPEC>        Image specification file
```

### `deploy`
Deploy an image to a target machine.

```bash
ubuntu-autoinstall-agent deploy [OPTIONS] --target <TARGET> --config <CONFIG>

Options:
  -t, --target <TARGET>    Target machine hostname/IP
  -c, --config <CONFIG>    Target configuration file
      --via-ssh            Deploy via SSH
      --dry-run            Show what would be done without executing
```

### `validate`
Validate image integrity.

```bash
ubuntu-autoinstall-agent validate --image <IMAGE>
```

### `list-images`
List available golden images.

```bash
ubuntu-autoinstall-agent list-images [OPTIONS]

Options:
  -f, --filter-arch <ARCH>  Filter by architecture
  -j, --json               Output in JSON format
```

### `cleanup`
Remove old images to free disk space.

```bash
ubuntu-autoinstall-agent cleanup [OPTIONS]

Options:
      --older-than-days <DAYS>  Remove images older than N days [default: 30]
      --dry-run                 Show what would be deleted
```

## Configuration

### Target Configuration

Create a YAML file defining your target server configuration:

```yaml
# examples/configs/basic-server.yaml
hostname: my-server
architecture: amd64
disk_device: /dev/sda
timezone: UTC

network:
  interface: eth0
  dhcp: true
  dns_servers:
    - 1.1.1.1
    - 1.0.0.1

users:
  - name: admin
    sudo: true
    shell: /bin/bash
    ssh_keys:
      - "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC7..."

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

### Image Specification

Define how your golden images should be built:

```yaml
# examples/specs/ubuntu-24.04-minimal.yaml
ubuntu_version: "24.04"
architecture: amd64

base_packages:
  - openssh-server
  - curl
  - wget
  - htop
  - vim

vm_config:
  memory_mb: 2048
  disk_size_gb: 20
  cpu_cores: 2

custom_scripts: []
```

## Security

### LUKS Encryption

All deployments use LUKS full disk encryption by default:

- **Cipher**: AES-XTS-Plain64 (configurable)
- **Key Size**: 512 bits (configurable)
- **Hash**: SHA256 (configurable)
- **Passphrase**: Environment variable substitution prevents secrets in configs

### SSH Security

- Key-based authentication only
- No password authentication supported
- Secure file permissions (600 for keys, 644 for configs)
- Input validation on all user-provided data

## Development

### Prerequisites

- Rust 1.70+
- QEMU/KVM (for image building)
- SSH keys configured for target access
- ISO creation tools (one of: genisoimage, mkisofs, or xorriso)
- tar (for extracting netboot tarballs)

#### Installing ISO Creation Tools

**Ubuntu/Debian:**
```bash
sudo apt install genisoimage
```

**macOS:**
```bash
brew install cdrtools
```

**RHEL/CentOS/Fedora:**
```bash
sudo yum install genisoimage
# or on newer versions:
sudo dnf install genisoimage
```

**Arch Linux:**
```bash
sudo pacman -S cdrtools
```

#### Installing OVMF/UEFI Firmware

For VM image creation, you'll also need OVMF UEFI firmware:

**Ubuntu/Debian:**
```bash
sudo apt install ovmf
```

**macOS:**
```bash
brew install qemu  # Includes UEFI firmware
```

**RHEL/CentOS/Fedora:**
```bash
sudo dnf install edk2-ovmf
```

**Arch Linux:**
```bash
sudo pacman -S edk2-ovmf
```

### Optional: uutils/coreutils for Enhanced Reliability

For improved cross-platform compatibility and reliability, you can install [uutils/coreutils](https://github.com/uutils/coreutils):

```bash
# Install uutils coreutils via cargo
cargo install coreutils

# Or via package manager (if available)
# Ubuntu/Debian: apt install uutils-coreutils
# Arch: pacman -S uutils-coreutils
```

The agent automatically detects and prefers uutils implementations when available, falling back to system commands otherwise. uutils provides:

- Cross-platform compatibility
- Consistent behavior across different systems
- Memory-safe implementations
- Better error handling and diagnostics

### Building from Source

```bash
git clone https://github.com/jdfalk/ubuntu-autoinstall-agent
cd ubuntu-autoinstall-agent
cargo build --release
```

### Running Tests

```bash
# Unit tests
cargo test

# Integration tests
cargo test --test integration_test

# All tests
cargo test --all
```

## License

This project is licensed under the MIT License.

---

**⚡ Replace complex shell scripts with reliable, memory-safe Ubuntu deployment automation.**
