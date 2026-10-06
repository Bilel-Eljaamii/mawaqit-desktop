#!/bin/sh
# tauri's pinned linuxdeploy (github.com/tauri-apps/binary-releases, built
# 2024-07-26) bundles a `strip` that cannot parse DT_RELR packed relocations
# — which every modern library ships — so AppImage bundling fails with
# "failed to run linuxdeploy" on current Arch/Manjaro (and Ubuntu 24.04).
# The continuous linuxdeploy build carries binutils that handle RELR.
#
# The bundler downloads its pinned copy only when the cache is empty, so
# planting the continuous build here sticks until the cache is wiped; this
# script re-plants it whenever the broken build is (back) in the cache.
set -eu

CACHE="${HOME}/.cache/tauri"
ARCH="$(uname -m)"
DEST="$CACHE/linuxdeploy-$ARCH.AppImage"
URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$ARCH.AppImage"

mkdir -p "$CACHE"

# The pinned build identifies itself by its build date; anything else that
# runs is left alone.
if [ -x "$DEST" ] \
    && ! APPIMAGE_EXTRACT_AND_RUN=1 "$DEST" --version 2>/dev/null | grep -q "2024-07-26"; then
    exit 0
fi

echo "installing continuous linuxdeploy into $DEST (pinned build cannot strip RELR libs)"
curl -fsSL "$URL" -o "$DEST"
chmod +x "$DEST"
