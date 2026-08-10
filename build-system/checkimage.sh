#!/usr/bin/env bash
#
# Inspect a packed initramfs without booting it. Every check here is something
# that shipped broken once and that CI cannot see, because CI only ever runs
# the image as root.
#
#   ./build-system/checkimage.sh /tmp/vakt-initramfs.cpio.gz

set -eu

ARCHIVE="${1:-/tmp/vakt-initramfs.cpio.gz}"

if [ ! -f "$ARCHIVE" ]; then
  echo "[!] No such archive: $ARCHIVE" >&2
  exit 2
fi

LISTING=$(gzip -dc "$ARCHIVE" | cpio -itv 2>/dev/null)
if [ -z "$LISTING" ]; then
  echo "[!] $ARCHIVE listed as empty; not a cpio archive?" >&2
  exit 2
fi

failures=0

fail() {
  echo "[!] $*" >&2
  failures=$((failures + 1))
}

# The mode cpio prints for an entry, or empty if absent. A symlink is listed as
# `bin/sh -> busybox`, so the name is not simply the last field.
mode_of() {
  awk -v want="$1" '
    { line = $0; sub(/ -> .*$/, "", line)
      n = split(line, field, " ")
      if (field[n] == want) { print field[1]; exit } }
  ' <<<"$LISTING"
}

exists() {
  [ -n "$(mode_of "$1")" ]
}

# Whether a mode string grants execute (traverse, for a directory) to "other".
# A trailing `.` or `+` is an SELinux/ACL marker, not a permission bit.
other_exec() {
  local mode="${1%[.+]}"
  case "$mode" in
    l*) return 0 ;; # a symlink's own mode is not what gets checked
    ?????????x) return 0 ;;
    *) return 1 ;;
  esac
}

# Shipped 0700 once: `mktemp -d`'s mode reached the cpio's `.` entry, the
# kernel chmods the real `/` to match, and nothing unprivileged could exec.
root_mode=$(mode_of ".")
if [ "$root_mode" != "drwxr-xr-x" ]; then
  fail "the image root is $root_mode, not drwxr-xr-x - the panel's user will not be able to traverse /"
fi

# The kernel reports "No working init found" both when /init is missing and
# when it is present but unrunnable, so check the mode too.
init_mode=$(mode_of "init")
if [ -z "$init_mode" ]; then
  fail "there is no /init - the kernel will panic with 'No working init found'"
else
  case "${init_mode%[.+]}" in
    -??x*) ;;
    *) fail "/init is $init_mode - the kernel cannot execute it" ;;
  esac
fi

# A missing interpreter fails execve with ENOENT, which reads as no working
# init while naming a file that is plainly there.
if ! grep -q 'ld-linux\|ld-musl' <<<"$LISTING"; then
  fail "no dynamic loader (ld-linux/ld-musl) in the image - a dynamically linked /init cannot start"
fi

# Anything the panel's user has to walk through or run.
for dir in usr usr/bin bin lib usr/lib etc sbin usr/sbin; do
  exists "$dir" || continue
  other_exec "$(mode_of "$dir")" ||
    fail "/$dir is $(mode_of "$dir") - not traversable by the panel's user"
done

for binary in usr/bin/vakt-panel bin/sh; do
  exists "$binary" || { fail "/$binary is missing from the image"; continue; }
  other_exec "$(mode_of "$binary")" ||
    fail "/$binary is $(mode_of "$binary") - not executable by the panel's user"
done

# Only meaningful for a modular kernel (VAKT_KERNEL=host); a monolithic build
# ships no /lib/modules at all.
if exists "lib/modules"; then
  # busybox's applet cannot read .ko.zst, and /bin precedes /usr/bin in PATH.
  if exists "bin/modprobe"; then
    fail "/bin/modprobe is present alongside /lib/modules - the busybox applet will shadow real kmod"
  fi
  if ! exists "usr/bin/modprobe"; then
    fail "/lib/modules exists but there is no /usr/bin/modprobe to load them with"
  fi
fi

if [ "$failures" -ne 0 ]; then
  echo "[!] $failures problem(s) in $ARCHIVE" >&2
  exit 1
fi

echo "[+] $ARCHIVE looks sane: /init runnable, root 0755, panel path traversable, \
modprobe not shadowed."
