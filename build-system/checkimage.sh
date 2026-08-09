#!/usr/bin/env bash
#
# Inspect a packed initramfs without booting it.
#
# Everything checked here is something that already shipped broken once and
# that CI could not see, because CI builds the image and never boots it as
# anyone but root. Modes inside the archive are the actual on-disk modes after
# the kernel extracts it, so they can be asserted from outside.
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

# The mode string cpio prints for an entry, or empty if the entry is absent.
#
# A symlink is listed as `bin/sh -> busybox`, so the name is not simply the
# last field - most of this image's /bin is busybox symlinks, and reading the
# last field finds the target instead and reports every applet as missing.
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

# The whole image root shipped as 0700 once, because the staging directory came
# from `mktemp -d` and `find .` records that mode for `.`. The kernel's
# initramfs extractor chmods the real `/` to match, so the unprivileged panel
# user could not traverse `/` and every exec as that user died with EACCES -
# which presents as vakt-init looping the panel open and shut.
root_mode=$(mode_of ".")
if [ "$root_mode" != "drwxr-xr-x" ]; then
  fail "the image root is $root_mode, not drwxr-xr-x - the panel's user will not be able to traverse /"
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
  # busybox's modprobe applet cannot decompress the .ko.zst files a real
  # kernel package ships, and /bin precedes /usr/bin in vakt-init's PATH, so
  # leaving the applet in place makes every module fail to load with "invalid
  # ELF header magic" - and then no storage driver, and then no data disk.
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

echo "[+] $ARCHIVE looks sane: root 0755, panel path traversable, modprobe not shadowed."
