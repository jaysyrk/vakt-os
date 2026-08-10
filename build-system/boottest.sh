#!/usr/bin/env bash
#
# Boot the image under QEMU with no display and check it comes up.
#
# CI builds the image and, until this existed, never ran it - which is how a
# kernel panic, a root directory nothing could traverse, and a panel crash loop
# all shipped. Everything here is text over a serial port, so it works headless
# and the whole boot ends up in a log you can read.
#
#   ./build-system/boottest.sh <vmlinuz> <initramfs> [disk.img]
#
# Needs no KVM: plain emulation is slower but this image boots in seconds.

set -eu

KERNEL="${1:?usage: boottest.sh <vmlinuz> <initramfs> [disk.img]}"
INITRAMFS="${2:?usage: boottest.sh <vmlinuz> <initramfs> [disk.img]}"
DISK="${3:-}"

BOOT_TIMEOUT="${BOOT_TIMEOUT:-180}"
LOG="${BOOT_LOG:-/tmp/vakt-boottest.log}"

for f in "$KERNEL" "$INITRAMFS"; do
    [ -f "$f" ] || { echo "[!] no such file: $f" >&2; exit 2; }
done
command -v qemu-system-x86_64 >/dev/null ||
    { echo "[!] qemu-system-x86_64 is not installed" >&2; exit 2; }

WORK=$(mktemp -d)
FIFO="$WORK/console-in"
mkfifo "$FIFO"
: > "$LOG"

cleanup() {
    exec 3>&- 2>/dev/null || true
    [ -n "${QEMU_PID:-}" ] && kill "$QEMU_PID" 2>/dev/null
    wait "${QEMU_PID:-}" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

disk_args=()
[ -n "$DISK" ] && disk_args=(-drive "file=$DISK,format=raw,index=0,media=disk")

# `-display none -serial stdio` rather than -nographic: the latter multiplexes
# the QEMU monitor onto the same stream, and scripted input lands in the
# monitor instead of the guest.
#
# The fifo is held open on fd 3 for the whole run. A plain redirect would hand
# QEMU an immediate EOF, and anything written before the shell exists is
# dropped on the floor. Opened read-write: opening a fifo write-only blocks
# until a reader appears, and the reader is the QEMU started just below.
exec 3<>"$FIFO"
qemu-system-x86_64 \
    -m 2G -no-reboot -display none -serial stdio \
    -kernel "$KERNEL" -initrd "$INITRAMFS" \
    -append "ro lsm=landlock,yama console=ttyS0,115200 vakt.rootshell" \
    "${disk_args[@]}" \
    < "$FIFO" > "$LOG" 2>&1 &
QEMU_PID=$!

# Wait for `pattern` to appear in the log, or give up.
wait_for() {
    local pattern="$1" label="$2" waited=0
    while [ "$waited" -lt "$BOOT_TIMEOUT" ]; do
        grep -aq "$pattern" "$LOG" && return 0
        kill -0 "$QEMU_PID" 2>/dev/null || break
        sleep 1
        waited=$((waited + 1))
    done
    echo "[!] timed out after ${BOOT_TIMEOUT}s waiting for $label" >&2
    return 1
}

fail() {
    echo "[!] $*" >&2
    echo "--- last 40 lines of $LOG ---" >&2
    tail -40 "$LOG" | tr -d '\000' >&2
    exit 1
}

echo "[+] Booting $(basename "$KERNEL") with $(basename "$INITRAMFS")..."
wait_for '\[Vakt-OS\]' "a shell prompt" || fail "the image never reached a shell"

# The recovery shell is a real shell on the serial console, so the checks are
# just commands and their output lands in the same log.
#
# Every marker is assembled by printf at runtime rather than written out
# whole. A serial console echoes what it is sent, so a marker that appeared
# literally in a command would match its own echo and pass no matter what the
# command did.
{
    echo 'ls -ld /'
    echo 'cat /run/services.status'
    echo 'touch /nope 2>&1 | head -1'
    echo 'su vakt -c '"'"'printf "UNPRIV_%s_OK\n" EXEC'"'"' 2>&1 | head -1'
    echo 'printf "BOOT%s\n" TEST_DONE'
} >&3

wait_for 'BOOTTEST_DONE' "the checks to finish" || fail "the shell stopped responding"

# Belt and braces: drop the echoed command lines, which are the ones carrying
# the shell prompt, and check only what the guest actually said back.
RESPONSES="$WORK/responses"
grep -av '\[Vakt-OS\]' "$LOG" > "$RESPONSES" || true

echo "[+] Checking what it reported..."
problems=0
check() {
    if grep -aq "$1" "$RESPONSES"; then
        echo "    ok    $2"
    else
        echo "    FAIL  $2" >&2
        problems=$((problems + 1))
    fi
}
refute() {
    if grep -aq "$1" "$RESPONSES"; then
        echo "    FAIL  $2" >&2
        problems=$((problems + 1))
    else
        echo "    ok    $2"
    fi
}

refute 'Kernel panic'                     "the kernel did not panic"
check  'Root filesystem is read-only'     "the root was sealed"
check  'Read-only file system'            "and writing to it really fails"
check  'Panel will run as vakt'           "an unprivileged account was found"
check  "Service 'vakt-ids' is ready"      "vakt-ids reported ready"
check  "Service 'vakt-net' is ready"      "vakt-net reported ready"
check  'All services reported ready'      "boot did not time out waiting"
check  'UNPRIV_EXEC_OK'                   "the unprivileged user can exec"
refute 'Could not run'                    "nothing failed to launch"
refute 'crashed .* times'                 "no service crash-looped"

if [ "$problems" -ne 0 ]; then
    fail "$problems check(s) failed"
fi

echo "[+] Boot test passed. Full log: $LOG"
