#!/bin/bash
set -euo pipefail

# ==============================================================================
# Vakt OS - Kernel Builder
# ==============================================================================
# Downloads a kernel tarball, configures it from build-system/kernel.config, and
# builds a monolithic bzImage.
#
# The configuration is a seed rather than a full .config: `make allnoconfig`
# starts every symbol at "no" and KCONFIG_ALLCONFIG turns on only what the seed
# asks for and what those symbols pull in. That is what "stripping out unused
# drivers" means here - nothing is stripped, because nothing unasked-for was
# ever enabled.
#
# Because a seed can silently lose a symbol (a dependency that is off, a name
# that changed between releases), the resulting .config is verified afterwards.
# A missing required symbol fails the build; a missing optional one is reported.
#
#   ./build-system/mkkernel.sh [output-path]
#
# Environment:
#   KERNEL_VERSION   kernel to build (default below)
#   KERNEL_SHA256    expected checksum of the tarball; verified when set
#   KERNEL_SRC_DIR   where to unpack and build (default /tmp/vakt-kernel)
#   JOBS             parallel make jobs (default: nproc)
# ==============================================================================

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
CONFIG_SEED="$PROJECT_ROOT/build-system/kernel.config"
OUT_IMAGE="${1:-$PROJECT_ROOT/build-system/out/vmlinuz}"

KERNEL_VERSION="${KERNEL_VERSION:-6.12.41}"
KERNEL_SHA256="${KERNEL_SHA256:-}"
SRC_DIR="${KERNEL_SRC_DIR:-/tmp/vakt-kernel}"
JOBS="${JOBS:-$(nproc)}"

SERIES="v${KERNEL_VERSION%%.*}.x"
TARBALL="linux-$KERNEL_VERSION.tar.xz"
URL="https://cdn.kernel.org/pub/linux/kernel/$SERIES/$TARBALL"
TREE="$SRC_DIR/linux-$KERNEL_VERSION"

# Unlike build.sh's other work directories, this one is a deliberate cache
# kept at a fixed path across runs (CI restores/saves it by that exact path
# via actions/cache) rather than a fresh mktemp -d each time, so it can't be
# closed the same way - refuse instead if the path was pre-planted as a
# symlink before this, typically root, script gets to it.
if [ -L "$SRC_DIR" ]; then
    echo "[!] Refusing to use $SRC_DIR: it is a symlink." >&2
    exit 1
fi

echo "========================================"
echo "     Vakt OS Kernel Builder             "
echo "========================================"
echo "Version: $KERNEL_VERSION"
echo "Source:  $TREE"
echo "Output:  $OUT_IMAGE"
echo ""

# --- Fetch -------------------------------------------------------------------
mkdir -p "$SRC_DIR"
if [ ! -d "$TREE" ]; then
    if [ ! -f "$SRC_DIR/$TARBALL" ]; then
        echo "[+] Downloading $TARBALL..."
        curl -fSL --retry 3 -o "$SRC_DIR/$TARBALL.part" "$URL"
        mv "$SRC_DIR/$TARBALL.part" "$SRC_DIR/$TARBALL"
    fi

    if [ -n "$KERNEL_SHA256" ]; then
        echo "[+] Verifying tarball checksum..."
        echo "$KERNEL_SHA256  $SRC_DIR/$TARBALL" | sha256sum -c -
    else
        echo "[!] KERNEL_SHA256 is unset; the tarball is not being verified."
    fi

    echo "[+] Unpacking (this takes a minute)..."
    tar -xf "$SRC_DIR/$TARBALL" -C "$SRC_DIR"
fi

# --- Configure ---------------------------------------------------------------
# The whole seed goes in, optional symbols included. The OPTIONAL marker only
# decides how hard the verification below reacts to one going missing.
echo "[+] Configuring from $(basename "$CONFIG_SEED")..."
make -C "$TREE" mrproper >/dev/null
make -C "$TREE" allnoconfig KCONFIG_ALLCONFIG="$CONFIG_SEED" >/dev/null

# --- Verify ------------------------------------------------------------------
# `make allnoconfig` drops a requested symbol without complaint when its
# dependencies are off, so the config it produced is checked against what was
# asked for. This is the difference between finding out now and finding out
# when the image will not boot.
echo "[+] Checking the generated configuration..."
missing_required=()
missing_optional=()
in_optional=0

while IFS= read -r line; do
    if [[ "$line" == "# OPTIONAL" ]]; then
        in_optional=1
        continue
    fi
    [[ "$line" =~ ^CONFIG_[A-Z0-9_]+= ]] || continue
    symbol="${line%%=*}"

    if grep -qx -- "$line" "$TREE/.config"; then
        continue
    fi
    if [ "$in_optional" -eq 1 ]; then
        missing_optional+=("$symbol")
    else
        missing_required+=("$symbol")
    fi
done < "$CONFIG_SEED"

if [ ${#missing_optional[@]} -gt 0 ]; then
    echo "[!] Hardening options this kernel does not have (or spells differently):"
    printf '      %s\n' "${missing_optional[@]}"
fi

if [ ${#missing_required[@]} -gt 0 ]; then
    echo ""
    echo "[!] These required symbols did not survive configuration:"
    printf '      %s\n' "${missing_required[@]}"
    echo ""
    echo "    Linux $KERNEL_VERSION either renamed them or has them behind a"
    echo "    dependency the seed does not enable. Fix build-system/kernel.config"
    echo "    before shipping this kernel - the image will not work without them."
    exit 1
fi
echo "[+] All required symbols are present."

# A monolithic kernel is the whole point; nothing in the image loads modules.
if grep -qx "CONFIG_MODULES=y" "$TREE/.config"; then
    echo "[!] CONFIG_MODULES came out enabled; the kernel is not monolithic."
    exit 1
fi

# --- Build -------------------------------------------------------------------
echo "[+] Building with $JOBS job(s). This is the slow part."
make -C "$TREE" -j"$JOBS" bzImage

mkdir -p "$(dirname "$OUT_IMAGE")"
cp "$TREE/arch/x86/boot/bzImage" "$OUT_IMAGE"

echo ""
echo "========================================"
echo "  Kernel ready: $OUT_IMAGE"
echo "  Size: $(du -h "$OUT_IMAGE" | cut -f1)"
echo "========================================"
