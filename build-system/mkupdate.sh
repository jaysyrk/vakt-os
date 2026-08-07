#!/bin/bash
set -euo pipefail

# ==============================================================================
# Vakt OS - Update Bundle Builder
# ==============================================================================
# Builds slot B for the A/B update mechanism: the exact same image build.sh
# produces for slot A, marked /etc/vakt-slot=B, packaged as a signed
# zrpkg-style bundle (vakt-update.zrp/.json) instead of an ISO.
#
# Reuses build.sh entirely for the rootfs/kernel/initramfs assembly rather
# than a second implementation of it - VAKT_SLOT=B and VAKT_UPDATE_OUT are
# the only difference from an ordinary build.sh run. Signs with the same
# build-system/keys/repo.key mkrepo.sh uses, so the appliance's existing
# trust anchor (/etc/vakt/trusted.key) verifies it with no separate key
# management for updates.
#
# UNVALIDATED ON REAL HARDWARE as of this writing - see docs/OS_UPDATES.md.
#
#   sudo ./build-system/mkupdate.sh [version]
#
# VAKT_KERNEL is honored the same as build.sh; VAKT_SLOT and VAKT_UPDATE_OUT
# are set by this script and should not be overridden.
# ==============================================================================

if [ "$EUID" -ne 0 ]; then
    echo "[!] Please run as root (with sudo) - this builds a full image, same as build.sh."
    exit 1
fi

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
KEY_FILE="$PROJECT_ROOT/build-system/keys/repo.key"
REPO_DIR="$PROJECT_ROOT/tools/repo"
ZRPKG="$PROJECT_ROOT/pkg-manager/target/release/zrpkg"
# mktemp -d rather than a fixed name for the same reason build.sh and
# mkrepo.sh do: this stages files about to be signed, as root.
STAGE=$(mktemp -d /tmp/vakt-updatestage.XXXXXXXX)
VERSION="${1:-$(date -u +%Y%m%d%H%M%S)}"

if [ ! -f "$KEY_FILE" ]; then
    echo "[!] No repository signing key at $KEY_FILE."
    echo "    Run build-system/mkrepo.sh (or ./build.sh, which calls it) at least once first -"
    echo "    an update bundle is signed with the same key ordinary packages are."
    exit 1
fi

echo "========================================"
echo "     Vakt OS Update Bundle Builder      "
echo "========================================"
echo "Version: $VERSION"
echo ""

BUNDLE_DIR="$STAGE/vakt-update"
mkdir -p "$BUNDLE_DIR"

echo "[+] Building slot B image (VAKT_SLOT=B)..."
VAKT_SLOT=B VAKT_UPDATE_OUT="$BUNDLE_DIR" "$PROJECT_ROOT/build.sh"

echo "$VERSION" > "$BUNDLE_DIR/version"

if [ ! -x "$ZRPKG" ]; then
    echo "[+] zrpkg not built yet, building it..."
    (cd "$PROJECT_ROOT/pkg-manager" && cargo build --release)
fi

echo ""
echo "[+] Signing update bundle..."
mkdir -p "$REPO_DIR"
rm -f "$REPO_DIR/vakt-update.zrp" "$REPO_DIR/vakt-update.json"
PRIV_KEY=$(cat "$KEY_FILE")
OUTPUT=$("$ZRPKG" pack "$BUNDLE_DIR" "$PRIV_KEY" -o "$REPO_DIR" \
    --version "$VERSION" --description "Vakt OS image update (slot B).")
echo "$OUTPUT" | sed 's/^/    /'

rm -rf "$STAGE"

echo ""
echo "========================================"
echo "  Update bundle ready: $REPO_DIR/vakt-update.zrp"
echo "  Version: $VERSION"
echo "========================================"
echo ""
echo "Serve it from the same repository as packages:"
echo "    $PROJECT_ROOT/tools/bin/zrpkg-server -dir $REPO_DIR"
echo ""
echo "On the appliance:"
echo "    vakt-update check"
echo "    vakt-update apply --reboot"
