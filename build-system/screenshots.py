#!/usr/bin/env python3
"""Photograph the panel from a real boot.

QEMU's QMP `screendump` writes the current display surface to a PPM, so these
are screenshots of the VGA console rather than photographs of a monitor, and
`send-key` drives the panel - the shots are of the actual TUI being used.

  ./build-system/screenshots.py <iso> <disk.img> <outdir> [boot|setup|lock]

  boot   the GRUB menu and the first screen after it
  setup  a fresh data disk: set a PIN, then every page
  lock   a disk that already has a PIN

Needs qemu-system-x86_64 and pnmtopng (netpbm). Regenerate docs/screenshots
with this after any change to the panel's layout.
"""
import json
import os
import socket
import subprocess
import sys
import time

ISO, DISK, OUTDIR = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(OUTDIR, exist_ok=True)
QMP = "/tmp/vakt-qmp.sock"

if os.path.exists(QMP):
    os.unlink(QMP)

qemu = subprocess.Popen([
    "qemu-system-x86_64", "-m", "2G", "-no-reboot",
    "-display", "none", "-vga", "std",
    "-cdrom", ISO,
    "-drive", f"file={DISK},format=raw,index=0,media=disk",
    "-qmp", f"unix:{QMP},server,nowait",
], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def connect(timeout=60):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            s = socket.socket(socket.AF_UNIX)
            s.connect(QMP)
            return s
        except OSError:
            time.sleep(0.5)
    raise SystemExit("[!] could not reach the QMP socket")


sock = connect()
f = sock.makefile("rw", encoding="utf-8", newline="\n")
f.readline()  # greeting


def cmd(name, **args):
    f.write(json.dumps({"execute": name, "arguments": args} if args
                       else {"execute": name}) + "\n")
    f.flush()
    while True:
        line = f.readline()
        if not line:
            raise SystemExit("[!] QMP closed")
        msg = json.loads(line)
        if "return" in msg or "error" in msg:
            return msg


cmd("qmp_capabilities")


def shot(name):
    ppm = f"/tmp/{name}.ppm"
    out = cmd("screendump", filename=ppm)
    if "error" in out:
        print(f"[!] {name}: {out['error']}")
        return
    png = os.path.join(OUTDIR, f"{name}.png")
    subprocess.run(["pnmtopng", ppm], stdout=open(png, "wb"),
                   stderr=subprocess.DEVNULL, check=False)
    os.unlink(ppm)
    size = os.path.getsize(png) if os.path.exists(png) else 0
    print(f"[+] {name}.png ({size} bytes)")


# tview draws on a text console; these are the keycodes QMP wants.
KEYS = {**{c: c for c in "abcdefghijklmnopqrstuvwxyz"},
        **{d: d for d in "0123456789"},
        "\t": "tab", "\r": "ret", " ": "spc"}


def typ(text, gap=0.25):
    for ch in text:
        key = KEYS.get(ch)
        if not key:
            continue
        cmd("send-key", keys=[{"type": "qcode", "data": key}])
        time.sleep(gap)


plan = sys.argv[4] if len(sys.argv) > 4 else "boot"

if plan == "boot":
    # GRUB menu, then the boot messages scrolling past, then the panel.
    for wait, name in [(6, "01-grub"), (25, "02-boot"), (30, "03-panel")]:
        time.sleep(wait)
        shot(name)
else:
    time.sleep(70)
    if plan == "setup":
        # Fresh disk: the setup screen wants the PIN twice.
        shot("10-setup")
        typ("8317\t8317\r")
    else:
        shot("10-lock")
        typ("8317\r")
    time.sleep(8)
    shot("11-menu")
    for key, name in [("n", "12-network"), ("i", "13-ids"),
                      ("s", "14-audit"), ("p", "15-packages"),
                      ("l", "16-panel-lock"), ("v", "17-services")]:
        cmd("send-key", keys=[{"type": "qcode", "data": key}])
        time.sleep(6)
        shot(name)
        cmd("send-key", keys=[{"type": "qcode", "data": "esc"}])
        time.sleep(2)

qemu.terminate()
try:
    qemu.wait(timeout=10)
except subprocess.TimeoutExpired:
    qemu.kill()
print("[+] done")
