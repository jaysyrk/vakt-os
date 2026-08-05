#!/bin/bash
set -e

echo "[+] Starting Vakt OS LFS Bootstrap..."

LFS=/mnt/lfs
mkdir -pv $LFS
mkdir -pv $LFS/{bin,etc,lib,sbin,usr,var}

echo "[+] Setting up cross-compiler toolchain..."
# Skeleton: Downloading binutils, gcc, linux headers, glibc
# wget https://...
# tar xf ...

echo "[+] Building kernel..."
# cd linux-*
# make mrproper
# cp ../kernel.config .config
# make -j$(nproc)

echo "[+] Replacing coreutils with uutils (Rust)..."
# cargo install coreutils --features unix

echo "[+] Setting up fastfetch logo and configuration..."
mkdir -pv $LFS/etc/fastfetch
cp ../build-system/fastfetch/vakt_logo.txt $LFS/etc/fastfetch/
cp ../build-system/fastfetch/config.jsonc $LFS/etc/fastfetch/

echo "[+] Bootstrap complete."
