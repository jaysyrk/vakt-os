#!/usr/bin/env bash
#
# Stage the "it refuses a tampered package" demo, so filming it is repeatable
# instead of improvised.
#
# Adds a `tampered-demo` package to the local repository: a real signed
# manifest beside an archive whose bytes were altered after signing. Installing
# it fails signature verification, which is the one claim in this project that
# sounds like marketing until someone watches it happen.
#
#   ./build-system/demo.sh          # stage it and print the shot list
#   ./build-system/demo.sh --serve  # stage it, then serve on :8080
#
# Re-runnable. mkrepo.sh wipes the repository, so run this after it.

set -eu

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
REPO_DIR="$PROJECT_ROOT/tools/repo"
SERVER="$PROJECT_ROOT/tools/bin/zrpkg-server"

# The package copied to make the tampered one. Small, and its name reads well
# on screen next to the failure.
SOURCE_PKG="vakt-audit"
DEMO_PKG="tampered-demo"

if [ ! -f "$REPO_DIR/$SOURCE_PKG.zrp" ]; then
    echo "[!] No repository yet. Run ./build-system/mkrepo.sh first" >&2
    exit 1
fi

echo "[+] Staging $DEMO_PKG..."

# The signature covers the archive, not the manifest, so the copied manifest
# stays valid - and that is the point. The failure has to come from the bytes
# being altered, not from a mismatched name or a missing signature, or the
# demo would be proving something weaker than it claims.
sed "s/\"name\": \"$SOURCE_PKG\"/\"name\": \"$DEMO_PKG\"/" \
    "$REPO_DIR/$SOURCE_PKG.json" > "$REPO_DIR/$DEMO_PKG.json"

cp "$REPO_DIR/$SOURCE_PKG.zrp" "$REPO_DIR/$DEMO_PKG.zrp"
printf 'tampered-by-demo.sh' >> "$REPO_DIR/$DEMO_PKG.zrp"

# Advertise it, unless a previous run already did.
if ! grep -q "\"$DEMO_PKG\"" "$REPO_DIR/index.json"; then
    entry="    {\"name\":\"$DEMO_PKG\",\"version\":\"1.0.0\",\"description\":\"Deliberately corrupted, for the demo.\",\"dependencies\":[]},"
    sed -i "/\"packages\": \[/a\\$entry" "$REPO_DIR/index.json"
fi

echo "[+] $DEMO_PKG.zrp is $(stat -c%s "$REPO_DIR/$DEMO_PKG.zrp") bytes, \
$(( $(stat -c%s "$REPO_DIR/$DEMO_PKG.zrp") - $(stat -c%s "$REPO_DIR/$SOURCE_PKG.zrp") )) more than the archive it was signed as."

cat <<'SHOTS'

----------------------------------------------------------------------
  Shot list - type these on the appliance, in this order
----------------------------------------------------------------------

  zrpkg update                    # tampered-demo is offered like any other

  zrpkg install vakt-audit        # fetch, verify, install. "Signature OK."

  zrpkg install tampered-demo     # <- the shot. Refused, not warned about.

  zrpkg list                      # vakt-audit is there. The other never was.

The last two are the video. Everything before them is setup.

If you are filming zrpkg on the build machine rather than the appliance,
unset RUST_BACKTRACE first - with it set, the refusal is followed by a stack
trace and the shot is ruined.

----------------------------------------------------------------------
SHOTS

if [ "${1:-}" = "--serve" ]; then
    [ -x "$SERVER" ] || {
        echo "[!] $SERVER is missing. Build it with:" >&2
        echo "    go build -C tools -o bin/zrpkg-server ./cmd/zrpkg-server/" >&2
        exit 1
    }
    echo "[+] Serving $REPO_DIR on :8080 - Ctrl-C to stop."
    exec "$SERVER" -dir "$REPO_DIR"
fi

echo "Serve it with:"
echo "    $SERVER -dir $REPO_DIR"
