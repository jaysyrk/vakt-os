#!/bin/bash
# No `pipefail` here on purpose: copy_deps runs `ldd | grep`, and a statically
# linked binary - which every Go tool in this repository is - makes that grep
# find nothing and exit non-zero. That is the expected case, not a failure.
set -eu

# ==============================================================================
# Vakt OS - ISO Builder
# ==============================================================================
# Compiles everything in this repository, signs the package repository, builds
# the rootfs, and produces a bootable ISO plus a persistent data disk.
#
# Environment:
#   VAKT_KERNEL=custom   build a monolithic kernel from build-system/kernel.config
#                        (default). Small image, no modules, no firmware.
#   VAKT_KERNEL=host     reuse /boot/vmlinuz-linux and the host's modules and
#                        firmware. Much larger, but boots on hardware the custom
#                        kernel has no driver for.
#   VAKT_REPO_URL=...    bake a package repository URL into the image, for an
#                        appliance that should already know where to fetch from.
#                        Defaults to the QEMU host. Anything written to the data
#                        disk later overrides it. See deploy/README.md.
# ==============================================================================

if [ "$EUID" -ne 0 ]; then
  echo "[!] Please run as root (with sudo) to build the OS."
  exit 1
fi

PROJECT_ROOT=$(pwd)
ROOTFS="/tmp/vakt-rootfs"
ISO_DIR="/tmp/vakt-iso"
OUT_ISO="$PROJECT_ROOT/vakt-os.iso"
VAKT_KERNEL="${VAKT_KERNEL:-custom}"
# The QEMU host under user networking. Override to ship an image pointed at a
# repository on a server you run.
VAKT_REPO_URL="${VAKT_REPO_URL:-http://10.0.2.2:8080}"

# The unprivileged account the panel runs as. vakt-init drops to it before
# handing over the console, so nothing the user interacts with is root.
VAKT_UID=1000
VAKT_GID=1000
# On the read-only root there is nowhere writable to put a home directory, so
# it lives on the /run tmpfs and vakt-init creates it at boot.
VAKT_HOME="/run/vakt"

# Statically linked busybox, published by the busybox project. Pinned by
# checksum: this binary becomes every shell utility in the image, so fetching it
# unverified over the network would make the whole build trust whatever the
# mirror served that day.
BUSYBOX_URL="https://www.busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox"
BUSYBOX_SHA256="6e123e7f3202a8c1e9b1f94d8941580a25135382b99e8d3e34fb858bba311348"

# Applets to link if busybox cannot enumerate its own (a cross-architecture
# build host, say). The full list is preferred; this is the floor the system
# needs to boot at all.
ESSENTIAL_APPLETS="sh ls cp mv rm cat echo mkdir rmdir pwd mount umount vi ps kill
    grep find awk sed sort xargs ip mdev udhcpc sleep poweroff reboot halt ping
    setsid cttyhack modprobe chmod chown ln"

echo "========================================"
echo "    Building Vakt OS                    "
echo "========================================"
echo "Kernel:     $VAKT_KERNEL"
echo "Repository: $VAKT_REPO_URL"
echo ""

case "$VAKT_REPO_URL" in
    http://*|https://*) ;;
    *) echo "[!] VAKT_REPO_URL must start with http:// or https://"; exit 1 ;;
esac

# Copies a binary into the rootfs along with every shared library it needs.
copy_deps() {
    local bin=$1
    local dest=$2
    if [ -f "$bin" ]; then
        cp "$bin" "$dest/usr/bin/"
        ldd "$bin" 2>/dev/null | grep -v "vdso" | awk '{print $(NF-1)}' | grep "^/" | while read -r lib; do
            if [ -f "$lib" ]; then
                mkdir -p "$dest$(dirname "$lib")"
                cp -n "$lib" "$dest$lib"
            fi
        done
    fi
}

echo "[+] Installing build dependencies..."
pacman -S --needed --noconfirm grub xorriso cpio gzip mtools wget curl wpa_supplicant kmod
if [ "$VAKT_KERNEL" = "custom" ]; then
    pacman -S --needed --noconfirm base-devel bc flex bison openssl perl xz
fi

echo ""
echo "[Step 1/4] Compiling Vakt OS software"
echo "----------------------------------------"
build_as_user() { sudo -u "${SUDO_USER:-root}" "$@"; }

(cd "$PROJECT_ROOT/pkg-manager"      && build_as_user cargo build --release)
(cd "$PROJECT_ROOT/vakt-init"        && build_as_user cargo build --release)
(cd "$PROJECT_ROOT/vakt-net"         && build_as_user cargo build --release)
(cd "$PROJECT_ROOT/vakt-compositor"  && build_as_user cargo build --release)
(cd "$PROJECT_ROOT/tools" && for tool in vakt-audit vakt-ids vakt-panel zrpkg-server; do
    build_as_user go build -o "bin/$tool" "./cmd/$tool/"
done)

echo ""
echo "[+] Building signed package repository..."
(cd "$PROJECT_ROOT" && build_as_user ./build-system/mkrepo.sh)

echo ""
echo "[Step 2/4] Constructing the rootfs"
echo "----------------------------------------"
rm -rf "$ROOTFS" "$ISO_DIR"
# /tmp is where zrpkg stages downloads; /run holds pid, log, socket and status
# files; /persistent is the mount point for the data disk. All three are mount
# points for something writable, because the root itself is remounted read-only
# as soon as vakt-init has finished with it.
mkdir -p "$ROOTFS"/{bin,sbin,etc,etc/vakt,proc,sys,dev,dev/pts,tmp,usr/bin,usr/sbin,usr/share/udhcpc,run,persistent,usr/lib,root}
chmod 1777 "$ROOTFS/tmp"
ln -s usr/lib "$ROOTFS/lib"
ln -s usr/lib "$ROOTFS/lib64"
ln -s lib "$ROOTFS/usr/lib64"

# --- Busybox -----------------------------------------------------------------
echo "[+] Downloading statically linked busybox..."
curl -fSL --retry 3 -o "$ROOTFS/bin/busybox" "$BUSYBOX_URL"
echo "$BUSYBOX_SHA256  $ROOTFS/bin/busybox" | sha256sum -c -
chmod +x "$ROOTFS/bin/busybox"

echo "[+] Linking busybox applets..."
if APPLETS=$("$ROOTFS/bin/busybox" --list 2>/dev/null) && [ -n "$APPLETS" ]; then
    echo "    $(echo "$APPLETS" | wc -l) applets"
else
    echo "    busybox could not be run here; linking the essential set only"
    APPLETS="$ESSENTIAL_APPLETS"
fi
for applet in $APPLETS; do
    ln -sf busybox "$ROOTFS/bin/$applet"
done

# --- Accounts ----------------------------------------------------------------
# The panel and everything it launches runs as 'vakt'. Root keeps a shell for
# recovery (vakt.rootshell on the kernel command line), but nothing routine
# runs as it.
echo "[+] Creating system accounts..."
cat > "$ROOTFS/etc/passwd" <<EOF
root:x:0:0:root:/root:/bin/sh
vakt:x:$VAKT_UID:$VAKT_GID:Vakt appliance user:$VAKT_HOME:/bin/sh
EOF
cat > "$ROOTFS/etc/group" <<EOF
root:x:0:
tty:x:5:
vakt:x:$VAKT_GID:
EOF
# No password hash for either account: there is no login program in this image,
# and an empty field is a hash that matches nothing rather than an empty one.
cat > "$ROOTFS/etc/shadow" <<EOF
root:*:19000:0:99999:7:::
vakt:*:19000:0:99999:7:::
EOF
chmod 600 "$ROOTFS/etc/shadow"

# --- Read-only root accommodations -------------------------------------------
# Everything that expects to write to /etc gets pointed at a tmpfs instead.
# /etc/resolv.conf is rewritten by udhcpc on every lease, and busybox mount
# updates /etc/mtab unless it is already a symlink into procfs.
echo "[+] Redirecting writable /etc paths to /run..."
ln -sf /run/resolv.conf "$ROOTFS/etc/resolv.conf"
ln -sf /proc/self/mounts "$ROOTFS/etc/mtab"

# --- Vakt tools --------------------------------------------------------------
echo "[+] Injecting zrpkg, vakt-audit, vakt-ids, vakt-panel, vakt-net, vakt-compositor..."
copy_deps "$PROJECT_ROOT/pkg-manager/target/release/zrpkg" "$ROOTFS"
copy_deps "$PROJECT_ROOT/tools/bin/vakt-audit" "$ROOTFS"
copy_deps "$PROJECT_ROOT/tools/bin/vakt-ids" "$ROOTFS"
copy_deps "$PROJECT_ROOT/tools/bin/vakt-panel" "$ROOTFS"
copy_deps "$PROJECT_ROOT/vakt-net/target/release/vakt-net" "$ROOTFS"
copy_deps "$PROJECT_ROOT/vakt-compositor/target/release/vakt-compositor" "$ROOTFS"
cp "$PROJECT_ROOT/build-system/fastfetch/vakt_logo.txt" "$ROOTFS/etc/vakt_logo.txt"

# Trust anchor for zrpkg. Without it zrpkg refuses to install anything at all,
# which is the intended behaviour - an unverifiable package is not installable.
if [ -f "$PROJECT_ROOT/build-system/keys/repo.pub" ]; then
    echo "[+] Installing repository trust anchor..."
    cp "$PROJECT_ROOT/build-system/keys/repo.pub" "$ROOTFS/etc/vakt/trusted.key"
else
    echo "[!] No repository public key found; zrpkg will refuse to install packages."
fi

# Where to fetch from. This is deployment configuration, not a trust decision:
# packages are verified against the key above whatever server served them.
# /persistent/etc/zrpkg.conf on the data disk overrides this, which is what the
# panel and `zrpkg repo` write.
echo "[+] Pointing zrpkg at $VAKT_REPO_URL..."
cat > "$ROOTFS/etc/vakt/zrpkg.conf" <<EOF
# Vakt OS package repository, baked in at build time.
# Override on a running system with 'zrpkg repo <url>' or the panel's
# Packages page, which write /persistent/etc/zrpkg.conf.
repo_url=$VAKT_REPO_URL
EOF

# --- Networking stack --------------------------------------------------------
echo "[+] Extracting the Wi-Fi stack..."
copy_deps "$(command -v wpa_supplicant)" "$ROOTFS"
copy_deps "$(command -v wpa_passphrase)" "$ROOTFS"
mkdir -p "$ROOTFS/etc/ssl/certs" "$ROOTFS/etc/ca-certificates/extracted"
cp -L -r /etc/ssl/certs/* "$ROOTFS/etc/ssl/certs/" 2>/dev/null || true
cp -L /etc/ca-certificates/extracted/tls-ca-bundle.pem \
    "$ROOTFS/etc/ca-certificates/extracted/tls-ca-bundle.pem" 2>/dev/null || true
ln -sf /etc/ca-certificates/extracted/tls-ca-bundle.pem "$ROOTFS/etc/ssl/certs/ca-certificates.crt"

# udhcpc runs this on every lease. It writes the resolver configuration to /run,
# which /etc/resolv.conf points at, because /etc is read-only by then.
cat > "$ROOTFS/usr/share/udhcpc/default.script" <<'EOF'
#!/bin/sh
case "$1" in
    bound|renew)
        ip addr add $ip/$mask dev $interface
        ip route add default via $router
        : > /run/resolv.conf
        for i in $dns; do
            echo "nameserver $i" >> /run/resolv.conf
        done
        ;;
esac
EOF
chmod +x "$ROOTFS/usr/share/udhcpc/default.script"

# --- Init --------------------------------------------------------------------
echo "[+] Injecting vakt-init as PID 1..."
cp "$PROJECT_ROOT/vakt-init/target/release/vakt-init" "$ROOTFS/init"
chmod +x "$ROOTFS/init"
copy_deps "$ROOTFS/init" "$ROOTFS"

echo ""
echo "[Step 3/4] Kernel"
echo "----------------------------------------"
mkdir -p "$ISO_DIR/boot/grub"

if [ "$VAKT_KERNEL" = "host" ]; then
    echo "[+] Reusing the host kernel and its modules..."
    cp /boot/vmlinuz-linux "$ISO_DIR/boot/vmlinuz"
    mkdir -p "$ROOTFS/lib/modules" "$ROOTFS/lib/firmware"
    cp -r "/lib/modules/$(uname -r)" "$ROOTFS/lib/modules/"
    cp -r /lib/firmware/* "$ROOTFS/lib/firmware/" 2>/dev/null || true
    copy_deps "$(command -v modprobe)" "$ROOTFS"
else
    # A monolithic kernel has no modules, so there is no /lib/modules and no
    # firmware tree - which is most of why this image is small.
    #
    # Unlike the cargo and go builds this runs as root, because it writes the
    # image straight into the root-owned staging directory.
    echo "[+] Building a monolithic kernel from build-system/kernel.config..."
    "$PROJECT_ROOT/build-system/mkkernel.sh" "$ISO_DIR/boot/vmlinuz"
fi

echo ""
echo "[Step 4/4] Packing the image"
echo "----------------------------------------"
echo "[+] Packing initramfs..."
(cd "$ROOTFS" && find . -print0 | cpio --null -o --format=newc 2>/dev/null | gzip -1 > /tmp/vakt-initramfs.cpio.gz)
cp /tmp/vakt-initramfs.cpio.gz "$ISO_DIR/boot/initramfs.img"

# The data disk holds Wi-Fi credentials, installed packages and the IDS
# baseline, so it must survive a rebuild. Delete it by hand to start fresh.
if [ -f "$PROJECT_ROOT/vakt-data.img" ]; then
    echo "[+] Keeping the existing persistent data disk (vakt-data.img)."
else
    echo "[+] Generating persistent data disk (vakt-data.img)..."
    dd if=/dev/zero of="$PROJECT_ROOT/vakt-data.img" bs=1M count=256 2>/dev/null
    mkfs.ext4 -F "$PROJECT_ROOT/vakt-data.img" >/dev/null
    chown "${SUDO_USER:-root}:${SUDO_USER:-root}" "$PROJECT_ROOT/vakt-data.img"
fi

# `ro` asks the kernel to mount the root read-only; vakt-init remounts it that
# way itself as well, since an initramfs root is not covered by the flag.
# `lsm=` puts Landlock in the stack, without which the sandboxes in vakt-net and
# vakt-compositor silently do nothing. Add `vakt.rootshell` here to get a root
# recovery shell when the panel exits.
cat > "$ISO_DIR/boot/grub/grub.cfg" <<'EOF'
set timeout=1
set default=0

menuentry "Vakt OS" {
    linux /boot/vmlinuz ro lsm=landlock,yama
    initrd /boot/initramfs.img
}

menuentry "Vakt OS (root recovery shell)" {
    linux /boot/vmlinuz ro lsm=landlock,yama vakt.rootshell
    initrd /boot/initramfs.img
}
EOF

echo "[+] Running grub-mkrescue..."
grub-mkrescue -o "$OUT_ISO" "$ISO_DIR" 2>/dev/null
chown "${SUDO_USER:-root}:${SUDO_USER:-root}" "$OUT_ISO" 2>/dev/null || true

echo ""
echo "========================================"
echo "    Vakt OS build complete              "
echo "========================================"
echo "ISO:  $OUT_ISO ($(du -h "$OUT_ISO" | cut -f1))"
echo "Disk: $PROJECT_ROOT/vakt-data.img"
echo ""
echo "Serve the package repository from this host first:"
echo "    $PROJECT_ROOT/tools/bin/zrpkg-server -dir $PROJECT_ROOT/tools/repo"
echo ""
echo "Then boot. The data disk must be the first drive so it lands on /dev/sda,"
echo "and user networking puts this host at 10.0.2.2:"
echo "    qemu-system-x86_64 -m 2G \\"
echo "        -drive file=$PROJECT_ROOT/vakt-data.img,format=raw,index=0,media=disk \\"
echo "        -cdrom $OUT_ISO \\"
echo "        -netdev user,id=n0 -device e1000,netdev=n0"
