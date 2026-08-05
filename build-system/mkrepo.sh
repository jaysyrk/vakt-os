#!/bin/bash
set -e

# ==============================================================================
# Vakt OS - Package Repository Builder
# ==============================================================================
# Stages compiled tools into package roots, signs them with zrpkg pack, and
# writes the resulting .zrp archives + index.json into tools/repo/ where
# zrpkg-server can serve them to the VM.
# ==============================================================================

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
REPO_DIR="$PROJECT_ROOT/tools/repo"
KEY_DIR="$PROJECT_ROOT/build-system/keys"
KEY_FILE="$KEY_DIR/repo.key"
PUB_FILE="$KEY_DIR/repo.pub"
STAGE="/tmp/vakt-pkgstage"
ZRPKG="$PROJECT_ROOT/pkg-manager/target/release/zrpkg"

echo "========================================"
echo "     Vakt OS Repository Builder         "
echo "========================================"

if [ ! -x "$ZRPKG" ]; then
    echo "[+] zrpkg not built yet, building it..."
    (cd "$PROJECT_ROOT/pkg-manager" && cargo build --release)
fi

# --- Signing key -------------------------------------------------------------
mkdir -p "$KEY_DIR"
if [ ! -f "$KEY_FILE" ]; then
    echo "[+] Generating repository signing key..."
    openssl rand -hex 32 > "$KEY_FILE"
    chmod 600 "$KEY_FILE"
fi
PRIV_KEY=$(cat "$KEY_FILE")

# --- Package definitions -----------------------------------------------------
# Format: <package name>|<path to binary>|<version>|<description>
PACKAGES=(
    "vakt-audit|$PROJECT_ROOT/tools/bin/vakt-audit|1.0.0|CIS-style security compliance auditor."
    "vakt-ids|$PROJECT_ROOT/tools/bin/vakt-ids|1.0.0|Filesystem integrity intrusion detection daemon."
    "vakt-compositor|$PROJECT_ROOT/vakt-compositor/target/release/vakt-compositor|0.1.0|Raw framebuffer graphical compositor."
    "vakt-ai|$PROJECT_ROOT/tools/bin/vakt-ai|0.1.0|Vakt OS AI assistant."
)

rm -rf "$STAGE"
mkdir -p "$STAGE" "$REPO_DIR"
rm -f "$REPO_DIR"/*.zrp "$REPO_DIR"/*.json

PUB_KEY=""
INDEX_ENTRIES=()

for entry in "${PACKAGES[@]}"; do
    IFS='|' read -r name binary version description <<< "$entry"

    if [ ! -f "$binary" ]; then
        echo "[-] Skipping $name (not built: $binary)"
        continue
    fi

    echo ""
    echo "[+] Staging $name..."
    mkdir -p "$STAGE/$name/usr/bin"
    cp "$binary" "$STAGE/$name/usr/bin/$name"
    chmod +x "$STAGE/$name/usr/bin/$name"

    OUTPUT=$("$ZRPKG" pack "$STAGE/$name" "$PRIV_KEY" \
        --out-dir "$REPO_DIR" \
        --version "$version" \
        --description "$description")
    echo "$OUTPUT" | sed 's/^/    /'

    # Every package is signed with the same key, so capture it once.
    if [ -z "$PUB_KEY" ]; then
        PUB_KEY=$(echo "$OUTPUT" | awk '/^Public key:/ {print $3}')
    fi

    INDEX_ENTRIES+=("{\"name\":\"$name\",\"version\":\"$version\",\"description\":\"$description\"}")
done

if [ ${#INDEX_ENTRIES[@]} -eq 0 ]; then
    echo "[!] No packages were built. Run ./build.sh first."
    exit 1
fi

# --- Repository index --------------------------------------------------------
printf '{\n  "packages": [\n' > "$REPO_DIR/index.json"
for i in "${!INDEX_ENTRIES[@]}"; do
    sep=","
    [ "$i" -eq $(( ${#INDEX_ENTRIES[@]} - 1 )) ] && sep=""
    printf '    %s%s\n' "${INDEX_ENTRIES[$i]}" "$sep" >> "$REPO_DIR/index.json"
done
printf '  ]\n}\n' >> "$REPO_DIR/index.json"

echo "$PUB_KEY" > "$PUB_FILE"

echo ""
echo "========================================"
echo "  Repository ready: $REPO_DIR"
echo "  Packages:   ${#INDEX_ENTRIES[@]}"
echo "  Public key: $PUB_KEY"
echo "========================================"
echo ""
echo "Serve it to the VM with:"
echo "    $PROJECT_ROOT/tools/bin/zrpkg-server -dir $REPO_DIR"
echo ""
echo "Then inside Vakt OS:"
echo "    zrpkg update"
echo "    zrpkg install vakt-audit"
