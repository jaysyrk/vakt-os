#!/usr/bin/env bash
#
# Wi-Fi association against a virtual radio.
#
# mac80211_hwsim gives the guest two 802.11 radios with no hardware. wlan1 is
# turned into a WPA2 access point with a DHCP server behind it; wlan0 is left
# for vakt-net, which is the code actually under test - supplicant config,
# association, the carrier wait, and DHCP.
#
#   ./build-system/wifitest.sh <vmlinuz> <initramfs> <disk.img>
#
# NEEDS A KERNEL BUILT WITH CONFIG_MAC80211_HWSIM=y, which the shipped
# kernel.config deliberately does not set - an appliance has no use for fake
# radios. Build one for testing without disturbing the seed:
#
#   cp build-system/kernel.config /tmp/seed.config
#   echo CONFIG_MAC80211_HWSIM=y >> build-system/kernel.config
#   ./build-system/mkkernel.sh /tmp/vmlinuz-hwsim
#   git checkout build-system/kernel.config
#
# Not in CI: it costs a second kernel build. Run it when touching anything in
# vakt-net's connect path.
#
# What it does not test is any real chipset. A driver that needs firmware the
# image does not carry fails on hardware and passes here.

set -eu

KERNEL="$1"; INITRAMFS="$2"; DISK="$3"
LOG="${BOOT_LOG:-/tmp/vakt-wifi.log}"
BOOT_TIMEOUT="${BOOT_TIMEOUT:-240}"

# WPA2 by default; set MODE=wpa3 for an SAE-only access point.
if [ "${MODE:-wpa2}" = "wpa3" ]; then
    AP_KEY_MGMT="SAE"; AP_PMF=2
else
    AP_KEY_MGMT="WPA-PSK"; AP_PMF=1
fi

WORK=$(mktemp -d); FIFO="$WORK/console-in"; mkfifo "$FIFO"; : > "$LOG"
cleanup() {
    exec 3>&- 2>/dev/null || true
    [ -n "${QEMU_PID:-}" ] && kill "$QEMU_PID" 2>/dev/null
    wait "${QEMU_PID:-}" 2>/dev/null; rm -rf "$WORK"
}
trap cleanup EXIT

exec 3<>"$FIFO"
qemu-system-x86_64 -m 2G -no-reboot -display none -serial stdio \
    -kernel "$KERNEL" -initrd "$INITRAMFS" \
    -append "ro lsm=landlock,yama console=ttyS0,115200 vakt.rootshell" \
    -drive "file=$DISK,format=raw,index=0,media=disk" \
    < "$FIFO" > "$LOG" 2>&1 &
QEMU_PID=$!

wait_for() {
    local pattern="$1" limit="$2" waited=0
    while [ "$waited" -lt "$limit" ]; do
        grep -aq "$pattern" "$LOG" && return 0
        kill -0 "$QEMU_PID" 2>/dev/null || { echo "[!] qemu exited"; return 1; }
        sleep 2; waited=$((waited + 2))
    done
    return 1
}

echo "[+] Booting with virtual radios..."
wait_for '\[Vakt-OS\]' "$BOOT_TIMEOUT" || { tail -30 "$LOG" | tr -d '\000'; exit 1; }

echo "[+] Radios present:"
{
    echo 'printf "MARK%s\n" _RADIOS'
    echo 'ls /sys/class/ieee80211/'
    echo 'ip -o link show | cut -d: -f2'
} >&3
wait_for 'MARK_RADIOS' 60 || true
sleep 5

echo "[+] Building an access point on wlan1..."
{
    echo 'ip link set wlan1 up'
    # A WPA2 AP. mode=2 is wpa_supplicant's own AP mode, so no hostapd needed.
    echo "printf 'network={\n ssid=\"VaktTest\"\n psk=\"testpassword123\"\n mode=2\n frequency=2412\n key_mgmt=$AP_KEY_MGMT\n proto=RSN\n pairwise=CCMP\n group=CCMP\n ieee80211w=$AP_PMF\n}\n' > /run/ap.conf"
    echo 'wpa_supplicant -B -i wlan1 -c /run/ap.conf -P /run/ap.pid'
    echo 'sleep 6'
    echo 'ip addr add 10.9.0.1/24 dev wlan1'
    echo 'printf "start 10.9.0.100\nend 10.9.0.200\ninterface wlan1\nopt subnet 255.255.255.0\nopt router 10.9.0.1\nlease_file /run/udhcpd.leases\npidfile /run/udhcpd.pid\n" > /run/udhcpd.conf'
    echo 'touch /run/udhcpd.leases'
    echo 'udhcpd /run/udhcpd.conf'
    echo 'sleep 2'
    echo 'printf "MARK%s\n" _AP_UP'
    echo 'iw dev wlan1 info 2>/dev/null || ip addr show wlan1'
} >&3
wait_for 'MARK_AP_UP' 90 || echo "[!] AP setup did not finish"
sleep 3

echo "[+] Pointing vakt-net at it (in place - Landlock keys on the inode)..."
{
    echo 'printf "ssid=VaktTest\npsk=testpassword123\ninterface=wlan0\n" > /persistent/etc/vakt-net.conf'
    echo 'printf "MARK%s\n" _CONF_WRITTEN'
} >&3
wait_for 'MARK_CONF_WRITTEN' 60 || true

echo "[+] Waiting for vakt-net to notice and connect (up to 90s)..."
sleep 90

{
    echo 'printf "MARK%s\n" _RESULT'
    echo 'cat /run/vakt-net.status'
    echo 'printf "MARK%s\n" _NETLOG'
    echo 'tail -25 /run/vakt-net.log'
    echo 'printf "MARK%s\n" _ADDR'
    echo 'ip addr show wlan0 | grep inet'
    echo 'printf "MARK%s\n" _DONE'
} >&3
wait_for 'MARK_DONE' 90 || true

echo "================================================================"
tr -d '\000' < "$LOG" | sed -n '/MARK_RADIOS/,$p' | grep -av '^\[Vakt-OS\]' | tail -60
echo "================================================================"
