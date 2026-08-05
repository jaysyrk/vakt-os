#!/bin/bash
set -e

# ==============================================================================
# Vakt OS - Pure LFS ISO Builder (Path A: Hardcore Networking)
# ==============================================================================

if [ "$EUID" -ne 0 ]; then
  echo "[!] Please run as root (with sudo) to build the OS."
  exit 1
fi

PROJECT_ROOT=$(pwd)
ROOTFS="/tmp/vakt-rootfs"
ISO_DIR="/tmp/vakt-iso"
OUT_ISO="$PROJECT_ROOT/vakt-os.iso"

echo "========================================"
echo "    Starting Vakt OS Pure LFS Build     "
echo "========================================"

# Helper function to extract dependencies of a binary and copy them to ROOTFS
copy_deps() {
    local bin=$1
    local dest=$2
    if [ -f "$bin" ]; then
        cp $bin $dest/usr/bin/
        ldd $bin 2>/dev/null | grep -v "vdso" | awk '{print $(NF-1)}' | grep "^/" | while read -r lib; do
            if [ -f "$lib" ]; then
                dir=$(dirname $lib)
                mkdir -p $dest$dir
                cp -n $lib $dest$lib
            fi
        done
    fi
}

echo "[+] Installing build dependencies..."
pacman -S --needed --noconfirm grub xorriso cpio gzip mtools wget wpa_supplicant kmod

echo ""
echo "[Step 1/3] Compiling Custom OS Software..."
echo "----------------------------------------"
cd $PROJECT_ROOT/pkg-manager && sudo -u $SUDO_USER cargo build --release
cd $PROJECT_ROOT/tools
sudo -u $SUDO_USER go build -o bin/vakt-audit ./cmd/vakt-audit/
sudo -u $SUDO_USER go build -o bin/vakt-ids ./cmd/vakt-ids/
sudo -u $SUDO_USER go build -o bin/vakt-panel ./cmd/vakt-panel/
sudo -u $SUDO_USER go build -o bin/zrpkg-server ./cmd/zrpkg-server/
cd $PROJECT_ROOT/vakt-init && sudo -u $SUDO_USER cargo build --release
cd $PROJECT_ROOT/vakt-net && sudo -u $SUDO_USER cargo build --release
cd $PROJECT_ROOT/vakt-compositor && sudo -u $SUDO_USER cargo build --release

echo ""
echo "[+] Building signed package repository..."
cd $PROJECT_ROOT && sudo -u $SUDO_USER ./build-system/mkrepo.sh
echo ""
echo "[Step 2/3] Constructing Pure LFS RootFS..."
echo "----------------------------------------"
rm -rf $ROOTFS $ISO_DIR
# /tmp is where zrpkg stages downloads; /run holds pid, log and status files;
# /persistent is the mount point for the data disk.
mkdir -p $ROOTFS/{bin,sbin,etc,etc/vakt,proc,sys,dev,tmp,usr/bin,usr/sbin,usr/share/udhcpc,run,persistent,usr/lib}
chmod 1777 $ROOTFS/tmp
ln -s usr/lib $ROOTFS/lib
ln -s usr/lib $ROOTFS/lib64
ln -s lib $ROOTFS/usr/lib64

# Static Busybox
echo "[+] Downloading statically linked busybox..."
wget -q -O $ROOTFS/bin/busybox https://www.busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox
chmod +x $ROOTFS/bin/busybox
for applet in sh ls cp mv rm cat echo mkdir rmdir pwd mount umount vi ps kill grep find awk sed ip mdev udhcpc sleep poweroff reboot halt ping setsid cttyhack; do
    ln -s busybox $ROOTFS/bin/$applet
done

# Setup basic UNIX authentication files
echo "root:x:0:0:root:/root:/bin/sh" > $ROOTFS/etc/passwd
echo "root::19000:0:99999:7:::" > $ROOTFS/etc/shadow
chmod 600 $ROOTFS/etc/shadow
# Inject Custom Tools
echo "[+] Injecting zrpkg, vakt-audit, vakt-panel, vakt-net..."
copy_deps $PROJECT_ROOT/pkg-manager/target/release/zrpkg $ROOTFS
copy_deps $PROJECT_ROOT/tools/bin/vakt-audit $ROOTFS
copy_deps $PROJECT_ROOT/tools/bin/vakt-ids $ROOTFS
copy_deps $PROJECT_ROOT/tools/bin/vakt-panel $ROOTFS
copy_deps $PROJECT_ROOT/vakt-net/target/release/vakt-net $ROOTFS
copy_deps $PROJECT_ROOT/vakt-compositor/target/release/vakt-compositor $ROOTFS
cp $PROJECT_ROOT/build-system/fastfetch/vakt_logo.txt $ROOTFS/etc/vakt_logo.txt

# Trust anchor for zrpkg: without this, installs proceed unverified.
if [ -f $PROJECT_ROOT/build-system/keys/repo.pub ]; then
    echo "[+] Installing repository trust anchor..."
    cp $PROJECT_ROOT/build-system/keys/repo.pub $ROOTFS/etc/vakt/trusted.key
else
    echo "[!] No repo public key found; zrpkg installs will be unverified."
fi

# Extract Wi-Fi stack & CA Certs
echo "[+] Extracting Networking Stack (wpa_supplicant, modprobe)..."
copy_deps $(which wpa_supplicant) $ROOTFS
copy_deps $(which wpa_passphrase) $ROOTFS
copy_deps $(which modprobe) $ROOTFS
mkdir -p $ROOTFS/etc/ssl/certs
cp -L -r /etc/ssl/certs/* $ROOTFS/etc/ssl/certs/ 2>/dev/null || true
mkdir -p $ROOTFS/etc/ca-certificates/extracted
cp -L /etc/ca-certificates/extracted/tls-ca-bundle.pem $ROOTFS/etc/ca-certificates/extracted/tls-ca-bundle.pem 2>/dev/null || true
ln -sf /etc/ca-certificates/extracted/tls-ca-bundle.pem $ROOTFS/etc/ssl/certs/ca-certificates.crt

# Copy Kernel Modules & Firmware (WARNING: This makes the initramfs huge)
echo "[+] Copying Kernel Modules and Firmware (This may take a minute)..."
mkdir -p $ROOTFS/lib/modules
cp -r /lib/modules/$(uname -r) $ROOTFS/lib/modules/
mkdir -p $ROOTFS/lib/firmware
cp -r /lib/firmware/* $ROOTFS/lib/firmware/ 2>/dev/null || true

# Write udhcpc script
cat << 'EOF' > $ROOTFS/usr/share/udhcpc/default.script
#!/bin/sh
case "$1" in
    bound|renew)
        ip addr add $ip/$mask dev $interface
        ip route add default via $router
        > /etc/resolv.conf
        for i in $dns; do
            echo "nameserver $i" >> /etc/resolv.conf
        done
        ;;
esac
EOF
chmod +x $ROOTFS/usr/share/udhcpc/default.script

# Inject Custom Init System
echo "[+] Injecting Rust vakt-init..."
cp $PROJECT_ROOT/vakt-init/target/release/vakt-init $ROOTFS/init
chmod +x $ROOTFS/init
copy_deps $ROOTFS/init $ROOTFS

# Pack Initramfs
echo "[+] Packing Initramfs (cpio.gz)... (this will take time)"
cd $ROOTFS
find . -print0 | cpio --null -ov --format=newc 2>/dev/null | gzip -1 > /tmp/vakt-initramfs.cpio.gz

# 4. Build ISO and Persistent Data Disk
echo ""
echo "[Step 3/3] Generating Bootable ISO and Data Disk..."
echo "----------------------------------------"

# The data disk now holds Wi-Fi credentials, installed packages and the IDS
# baseline, so it must survive a rebuild. Delete it by hand to start fresh.
if [ -f $PROJECT_ROOT/vakt-data.img ]; then
    echo "[+] Keeping existing persistent data disk (vakt-data.img)."
else
    echo "[+] Generating persistent data disk (vakt-data.img)..."
    dd if=/dev/zero of=$PROJECT_ROOT/vakt-data.img bs=1M count=256 2>/dev/null
    mkfs.ext4 -F $PROJECT_ROOT/vakt-data.img >/dev/null
    chown $SUDO_USER:$SUDO_USER $PROJECT_ROOT/vakt-data.img
fi

mkdir -p $ISO_DIR/boot/grub
cp /boot/vmlinuz-linux $ISO_DIR/boot/vmlinuz
cp /tmp/vakt-initramfs.cpio.gz $ISO_DIR/boot/initramfs.img

# Write GRUB Config
cat << 'EOF' > $ISO_DIR/boot/grub/grub.cfg
set timeout=1
set default=0

menuentry "Vakt OS (Hardcore LFS)" {
    linux /boot/vmlinuz
    initrd /boot/initramfs.img
}
EOF

# Make ISO
echo "[+] Running grub-mkrescue..."
grub-mkrescue -o $OUT_ISO $ISO_DIR 2>/dev/null

echo "========================================"
echo "    Vakt OS Hardcore LFS Complete!      "
echo "========================================"
echo "SUCCESS! Bootable ISO generated at: $OUT_ISO"
echo ""
echo "Serve the package repository from this host first:"
echo "    $PROJECT_ROOT/tools/bin/zrpkg-server -dir $PROJECT_ROOT/tools/repo"
echo ""
echo "Then boot. The data disk must be the first drive so /dev/sda is the"
echo "persistent volume, and user networking puts this host at 10.0.2.2:"
echo "    qemu-system-x86_64 -m 2G \\"
echo "        -drive file=$PROJECT_ROOT/vakt-data.img,format=raw,index=0,media=disk \\"
echo "        -cdrom $OUT_ISO \\"
echo "        -netdev user,id=n0 -device e1000,netdev=n0"
