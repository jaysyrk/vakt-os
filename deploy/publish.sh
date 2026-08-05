#!/bin/bash
set -eu

# ==============================================================================
# Vakt OS - Publish the package repository to a server
# ==============================================================================
# Builds and signs the repository locally, then copies the result to a host you
# rent. The signing key never leaves this machine: signing happens here, and
# what goes over the wire is only the signed output.
#
#   ./deploy/publish.sh user@vps.example.com
#   ZRPKG_REMOTE_DIR=/srv/vakt ./deploy/publish.sh user@vps.example.com
#
# Environment:
#   ZRPKG_REMOTE_DIR   destination directory (default /var/lib/zrpkg/repo)
#   SKIP_BUILD=1       publish what is already in tools/repo
# ==============================================================================

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
LOCAL_REPO="$PROJECT_ROOT/tools/repo"
REMOTE_DIR="${ZRPKG_REMOTE_DIR:-/var/lib/zrpkg/repo}"
TARGET="${1:-}"

if [ -z "$TARGET" ]; then
    echo "usage: $0 <user@host>" >&2
    echo "" >&2
    echo "Copies the signed repository to <host>:$REMOTE_DIR." >&2
    exit 1
fi

if [ "${SKIP_BUILD:-0}" != "1" ]; then
    echo "[+] Building and signing the repository..."
    "$PROJECT_ROOT/build-system/mkrepo.sh"
fi

if [ ! -f "$LOCAL_REPO/index.json" ]; then
    echo "[!] No index.json in $LOCAL_REPO. Run build-system/mkrepo.sh first." >&2
    exit 1
fi

# Refuse to ship a private key even if one has somehow ended up in the
# repository directory. This is the one mistake in this script that would
# actually matter.
if find "$LOCAL_REPO" -name '*.key' -print -quit | grep -q .; then
    echo "[!] There is a .key file in $LOCAL_REPO. Remove it before publishing." >&2
    exit 1
fi

echo "[+] Publishing to $TARGET:$REMOTE_DIR"
ssh "$TARGET" "mkdir -p '$REMOTE_DIR'"

# --delete so a package removed locally stops being served, and the index is
# copied last so clients never see it advertise an archive that is not there
# yet.
rsync -av --delete \
    --exclude 'index.json' \
    --include '*.zrp' --include '*.json' \
    --exclude '*' \
    "$LOCAL_REPO/" "$TARGET:$REMOTE_DIR/"
rsync -av "$LOCAL_REPO/index.json" "$TARGET:$REMOTE_DIR/index.json"

PUB_FILE="$PROJECT_ROOT/build-system/keys/repo.pub"
echo ""
echo "========================================"
echo "  Published to $TARGET:$REMOTE_DIR"
echo "========================================"
echo ""
echo "On the appliance:"
echo "    zrpkg repo https://<your-host>"
echo "    zrpkg update"
echo ""
if [ -f "$PUB_FILE" ]; then
    echo "Images must carry this public key at /etc/vakt/trusted.key or they"
    echo "will refuse everything this repository serves:"
    echo "    $(cat "$PUB_FILE")"
fi
