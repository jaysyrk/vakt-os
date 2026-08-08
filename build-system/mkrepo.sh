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
# mktemp -d rather than a fixed name: this stages the very files that are
# about to be signed, and a predictable /tmp path a build-host user could
# pre-plant as a symlink before this (typically root) script runs is a
# classic local privilege-escalation primitive.
STAGE=$(mktemp -d /tmp/vakt-pkgstage.XXXXXXXX)
ZRPKG="$PROJECT_ROOT/pkg-manager/target/release/zrpkg"
# Independent second opinion on every signature this script produces - see
# vakt-verify/src/main.zig for why a from-scratch, separately-implemented
# verifier is worth running here rather than trusting zrpkg to check its own
# work. Optional: if it was never built, packaging proceeds without it.
VAKT_VERIFY="$PROJECT_ROOT/vakt-verify/zig-out/bin/vakt-verify"

echo "========================================"
echo "     Vakt OS Repository Builder         "
echo "========================================"

if [ ! -x "$ZRPKG" ]; then
    echo "[+] zrpkg not built yet, building it..."
    (cd "$PROJECT_ROOT/pkg-manager" && cargo build --release)
fi

# --- Signing key -------------------------------------------------------------
mkdir -p "$KEY_DIR"
# The directory too, not just the key inside it: this is the one secret in
# the project whose loss means an attacker can sign packages every appliance
# already trusts.
chmod 700 "$KEY_DIR"
if [ ! -f "$KEY_FILE" ]; then
    echo "[+] Generating repository signing key..."
    # Written under a restrictive umask in a subshell rather than chmod'd
    # afterwards. A plain redirect creates the file 0644, so the private key
    # would sit world-readable on the build machine for the instant between
    # the redirect and the chmod.
    (umask 077 && openssl rand -hex 32 > "$KEY_FILE")
fi
PRIV_KEY=$(cat "$KEY_FILE")

# --- Package definitions -----------------------------------------------------
# Format: <package name>|<path to binary>|<version>|<description>|<comma-separated dependencies>
# The dependency field may be empty. zrpkg resolves the graph itself, so listing
# a package's direct dependencies here is enough - transitive ones are found.
PACKAGES=(
    "vakt-audit|$PROJECT_ROOT/tools/bin/vakt-audit|1.0.0|CIS-style security compliance auditor.|"
    "vakt-ids|$PROJECT_ROOT/tools/bin/vakt-ids|1.0.0|Filesystem integrity intrusion detection daemon.|"
    "vakt-compositor|$PROJECT_ROOT/vakt-compositor/target/release/vakt-compositor|0.1.0|Raw framebuffer graphical compositor.|"
    "vakt-verify|$PROJECT_ROOT/vakt-verify/zig-out/bin/vakt-verify|0.1.0|Independent Ed25519 package signature verifier.|"
)

mkdir -p "$REPO_DIR"
rm -f "$REPO_DIR"/*.zrp "$REPO_DIR"/*.json

PUB_KEY=""
INDEX_ENTRIES=()

for entry in "${PACKAGES[@]}"; do
    IFS='|' read -r name binary version description depends <<< "$entry"

    if [ ! -f "$binary" ]; then
        echo "[-] Skipping $name (not built: $binary)"
        continue
    fi

    echo ""
    echo "[+] Staging $name..."
    mkdir -p "$STAGE/$name/usr/bin"
    cp "$binary" "$STAGE/$name/usr/bin/$name"
    chmod +x "$STAGE/$name/usr/bin/$name"

    PACK_ARGS=(pack "$STAGE/$name" "$PRIV_KEY"
        --out-dir "$REPO_DIR"
        --version "$version"
        --description "$description")
    if [ -n "$depends" ]; then
        PACK_ARGS+=(--depends "$depends")
    fi

    OUTPUT=$("$ZRPKG" "${PACK_ARGS[@]}")
    echo "$OUTPUT" | sed 's/^/    /'

    # Every package is signed with the same key, so capture it once.
    if [ -z "$PUB_KEY" ]; then
        PUB_KEY=$(echo "$OUTPUT" | awk '/^Public key:/ {print $3}')
    fi

    # A second, independently-implemented verifier checking what zrpkg just
    # signed. Disagreement here means a bug in one of the two implementations,
    # and it is cheap to catch it now rather than after the repository is
    # published.
    #
    # The exit status has to come from vakt-verify itself, not from `sed` -
    # piping straight into `sed 's/^/    /'` would make `if ! cmd | sed` test
    # sed's (always zero) exit code and silently ignore a real failure here.
    # Capturing the output as the condition of the `if` sidesteps that, and
    # also keeps `set -e` from aborting the script before the diagnostic below
    # gets a chance to print.
    if [ -x "$VAKT_VERIFY" ]; then
        if VERIFY_OUTPUT=$("$VAKT_VERIFY" "$REPO_DIR/$name.zrp" "$REPO_DIR/$name.json" --pubkey "$PUB_KEY" 2>&1); then
            echo "$VERIFY_OUTPUT" | sed 's/^/    /'
        else
            echo "$VERIFY_OUTPUT" | sed 's/^/    /'
            echo "[!] vakt-verify disagrees with zrpkg's own signature for $name; refusing to publish."
            exit 1
        fi
    fi

    # Render the dependency list as a JSON array: "a,b" -> ["a","b"], "" -> [].
    if [ -n "$depends" ]; then
        DEPS_JSON=$(printf '%s' "$depends" | awk -F, '{
            for (i = 1; i <= NF; i++) printf "%s\"%s\"", (i > 1 ? "," : ""), $i
        }')
    else
        DEPS_JSON=""
    fi

    INDEX_ENTRIES+=("{\"name\":\"$name\",\"version\":\"$version\",\"description\":\"$description\",\"dependencies\":[$DEPS_JSON]}")
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
