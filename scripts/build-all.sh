#!/usr/bin/env bash
# file: scripts/build-all.sh
# version: 2.0.0
# guid: a1b2c3d4-e5f6-7a8b-9c0d-e1f2a3b4c5d6
# last-edited: 2026-06-21
#
# Build ubuntu-autoinstall-agent for all four release targets:
#   linux-amd64   — fully static musl binary (runs anywhere on x86-64 Linux)
#   linux-arm64   — fully static musl binary (runs anywhere on aarch64 Linux)
#   darwin-arm64  — native macOS binary (Apple Silicon)
#   darwin-amd64  — native macOS binary (Intel)
#
# Linux builds use Docker + Alpine (no host tools required beyond Docker).
# macOS builds compile natively — must be run on macOS.
#
# Usage:
#   scripts/build-all.sh              # build everything
#   scripts/build-all.sh --no-cache   # force full Docker rebuild
#   DEPLOY=1 scripts/build-all.sh     # also SCP linux-amd64 to 172.16.2.30

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="$REPO_ROOT/dist"
BINARY="ubuntu-autoinstall-agent"
DEPLOY_HOST="172.16.2.30"
DEPLOY_USER="jdfalk"
DEPLOY_PATH="/usr/local/bin/$BINARY"

NO_CACHE=""
if [[ "${1:-}" == "--no-cache" ]]; then
    NO_CACHE="--no-cache"
fi

mkdir -p "$DIST_DIR"

build_linux() {
    local arch="$1"           # amd64 or arm64
    local platform="linux/$arch"
    local tag="$BINARY-build:$arch"
    local out="$DIST_DIR/$BINARY-linux-$arch"

    echo "==> Building linux-$arch (static musl) ..."
    docker build $NO_CACHE \
        --platform "$platform" \
        -f "$REPO_ROOT/Dockerfile.build" \
        -t "$tag" \
        "$REPO_ROOT"

    local ctr
    ctr=$(docker create --platform "$platform" "$tag" /ubuntu-autoinstall-agent)
    docker cp "$ctr:/ubuntu-autoinstall-agent" "$out"
    docker rm "$ctr"
    chmod +x "$out"
    printf "    -> %s\n" "$out"
    file "$out"
}

# ── Linux (Docker + Alpine musl) ──────────────────────────────────────────────
build_linux amd64
build_linux arm64

# ── macOS (native cross-compile) ─────────────────────────────────────────────
if [[ "$(uname)" != "Darwin" ]]; then
    echo "==> Skipping macOS builds (not running on macOS)"
else
    rustup target add aarch64-apple-darwin x86_64-apple-darwin 2>/dev/null || true

    for darwin_arch in arm64 amd64; do
        rust_target="aarch64-apple-darwin"
        [[ "$darwin_arch" == "amd64" ]] && rust_target="x86_64-apple-darwin"

        echo "==> Building darwin-$darwin_arch ..."
        cargo build --release \
            --target "$rust_target" \
            --manifest-path "$REPO_ROOT/Cargo.toml"

        src="$REPO_ROOT/target/$rust_target/release/$BINARY"
        out="$DIST_DIR/$BINARY-darwin-$darwin_arch"
        cp "$src" "$out"
        printf "    -> %s\n" "$out"
        file "$out"
    done
fi

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
echo "==> Artifacts:"
ls -lh "$DIST_DIR/$BINARY"-* 2>/dev/null

# ── Deploy (optional) ─────────────────────────────────────────────────────────
if [[ "${DEPLOY:-0}" == "1" ]]; then
    echo ""
    echo "==> Deploying linux-amd64 to $DEPLOY_USER@$DEPLOY_HOST ..."
    scp "$DIST_DIR/$BINARY-linux-amd64" "$DEPLOY_USER@$DEPLOY_HOST:/tmp/$BINARY"
    ssh "$DEPLOY_USER@$DEPLOY_HOST" "sudo mv /tmp/$BINARY $DEPLOY_PATH && sudo chmod +x $DEPLOY_PATH"
    echo "==> Deployed: $DEPLOY_PATH"
fi
