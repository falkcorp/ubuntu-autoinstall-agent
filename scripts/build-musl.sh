#!/usr/bin/env bash
# file: scripts/build-musl.sh
# version: 1.0.0
# guid: 8c5e1a37-4f92-4d6b-b8a0-6d3c9e2f7a14
# last-edited: 2026-07-09
#
# Build the static x86_64 musl release binary of the agent on a Linux box
# (e.g. the server 172.16.2.30 or any amd64 Ubuntu host). This is the binary
# the USB auto-bootstrap curls at boot:
#
#   http://172.16.2.30/uaa/uaa-amd64   (UAA_AGENT_URL default in
#                                       installer-image/nocloud/uaa-usb-bootstrap.sh)
#
# DEPLOY (human step) after building:
#   sudo install -D -m 0755 \
#     target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent \
#     /var/www/html/uaa/uaa-amd64
#
# CI builds the same artifact: .github/workflows/musl-build.yml -> `uaa-amd64`.
#
# Requires: rustup, musl-tools (apt install musl-tools), perl, make
# (the vendored-openssl in the ssh2 crate builds with musl-gcc).

set -euo pipefail

if [[ "$OSTYPE" != "linux-gnu"* ]]; then
    echo "ERROR: musl static builds must run on Linux (use the server or CI)" >&2
    exit 1
fi

command -v musl-gcc >/dev/null 2>&1 || { echo "ERROR: musl-gcc not found (apt install musl-tools)" >&2; exit 1; }
rustup target list --installed | grep -q x86_64-unknown-linux-musl \
    || rustup target add x86_64-unknown-linux-musl

export CC_x86_64_unknown_linux_musl=musl-gcc
cargo build --release --target x86_64-unknown-linux-musl

BIN=target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent
echo
file "$BIN" || true
if ldd "$BIN" 2>&1 | grep -vq "not a dynamic executable\|statically linked"; then
    echo "ERROR: binary is not static" >&2
    exit 1
fi
echo "OK: static binary at $BIN"
echo "Deploy (human): sudo install -D -m 0755 '$BIN' /var/www/html/uaa/uaa-amd64"
