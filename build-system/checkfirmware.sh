#!/usr/bin/env bash
#
# Does the image carry the firmware its drivers will ask for?
#
#   ./build-system/checkfirmware.sh <firmware-dir> [module...]
#
# A driver that loads without its firmware leaves an interface that exists,
# enumerates, and never works - which looks like a configuration problem and
# is not one. This turns that into a build-time list of missing files.
#
# With no modules named, every currently-loaded module that declares firmware
# is checked, which on the build machine means the hardware it is running on.
#
# Only meaningful for VAKT_KERNEL=host: a monolithic kernel has no modinfo to
# ask, and carries no firmware tree at all.
#
# VAKT_MODINFO overrides the modinfo command, for testing.

set -eu

FIRMWARE_DIR="${1:?usage: checkfirmware.sh <firmware-dir> [module...]}"
shift || true

MODINFO="${VAKT_MODINFO:-modinfo}"

if [ ! -d "$FIRMWARE_DIR" ]; then
    echo "[!] No firmware directory at $FIRMWARE_DIR" >&2
    exit 2
fi

modules() {
    if [ "$#" -gt 0 ]; then
        printf '%s\n' "$@"
        return
    fi
    # Column 1 of /proc/modules is the module name.
    [ -r /proc/modules ] && awk '{print $1}' /proc/modules
}

# Arch ships firmware compressed, and the kernel decompresses on demand, so a
# request for foo.ucode is satisfied by foo.ucode.zst.
present() {
    for candidate in "$1" "$1.zst" "$1.xz" "$1.gz"; do
        [ -e "$FIRMWARE_DIR/$candidate" ] && return 0
    done
    return 1
}

missing=0
checked=0

for module in $(modules "$@"); do
    # A module with no firmware= entries prints nothing, which is not an error.
    for firmware in $("$MODINFO" -F firmware "$module" 2>/dev/null); do
        checked=$((checked + 1))
        if ! present "$firmware"; then
            echo "[!] $module wants $firmware, which is not in the image" >&2
            missing=$((missing + 1))
        fi
    done
done

if [ "$checked" -eq 0 ]; then
    echo "[+] No loaded module declares firmware; nothing to check."
    exit 0
fi

if [ "$missing" -ne 0 ]; then
    echo "[!] $missing of $checked firmware file(s) missing from $FIRMWARE_DIR" >&2
    echo "[!] Hardware needing them will enumerate and never work." >&2
    exit 1
fi

echo "[+] All $checked firmware file(s) the loaded modules ask for are present."
