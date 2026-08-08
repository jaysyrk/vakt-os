#!/bin/bash
set -euo pipefail

# ==============================================================================
# Vakt OS - Update Bundle Builder
# ==============================================================================
# Builds and signs slot B for the A/B update mechanism (docs/OS_UPDATES.md),
# reusing build.sh for the actual image build. Unvalidated on real hardware.
#
#   sudo ./build-system/mkupdate.sh [version]
# ==============================================================================

if [ "$EUID" -ne 0 ]; then
    echo "[!] Please run as root (with sudo) - this builds a full image, same as build.sh."
    exit 1
fi

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
KEY_FILE="$PROJECT_ROOT/build-system/keys/repo.key"
REPO_DIR="$PROJECT_ROOT/tools/repo"
ZRPKG="$PROJECT_ROOT/pkg-manager/target/release/zrpkg"
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
