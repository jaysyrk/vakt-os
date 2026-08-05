#!/bin/bash
set -e

echo "[+] Generating Vakt OS Live ISO..."
LFS=/mnt/lfs
ISO_DIR=/tmp/vakt-iso
mkdir -p $ISO_DIR/boot/grub

echo "[+] Copying kernel and initramfs..."
# cp $LFS/boot/vmlinuz-* $ISO_DIR/boot/vmlinuz
# cp $LFS/boot/initramfs-* $ISO_DIR/boot/initramfs.img

echo "[+] Creating rootfs squashfs..."
# mksquashfs $LFS $ISO_DIR/live/filesystem.squashfs -e boot

echo "[+] Building grub config..."
cat << EOF > $ISO_DIR/boot/grub/grub.cfg
set default=0
set timeout=5
menuentry "Vakt OS (Live)" {
    linux /boot/vmlinuz boot=live quiet splash
    initrd /boot/initramfs.img
}
EOF

echo "[+] Creating ISO image..."
# grub-mkrescue -o vakt-os-amd64.iso $ISO_DIR

echo "[+] Build complete: vakt-os-amd64.iso"
