#!/usr/bin/env bash
# Screenshot of a UI executable on a 1440x900 virtual X display (Linux development aid).
#
# usage: scripts/xvfb-shot.sh <executable> <out.png> [app arguments...]
#
# Starts Xvfb on :99, runs the app with Slint's software renderer, waits 4 seconds,
# captures the whole screen into <out.png> and stops both. The app's output goes to
# <out.png>.log. Needs Xvfb and ImageMagick's `import`.
set -euo pipefail

if [ "$#" -lt 2 ]; then
    echo "usage: $0 <executable> <out.png> [app arguments...]" >&2
    exit 2
fi
exe=$1
out=$2
shift 2

display=:99
app=""
Xvfb "$display" -screen 0 1440x900x24 -nolisten tcp >/dev/null 2>&1 &
xvfb=$!

cleanup() {
    if [ -n "$app" ]; then
        kill "$app" 2>/dev/null || true
    fi
    kill "$xvfb" 2>/dev/null || true
    wait 2>/dev/null || true
}
trap cleanup EXIT

sleep 1
if ! kill -0 "$xvfb" 2>/dev/null; then
    echo "Xvfb did not start on $display (is another server using it?)" >&2
    exit 1
fi

DISPLAY=$display SLINT_BACKEND=winit-software "$exe" "$@" >"$out.log" 2>&1 &
app=$!
sleep 4
if ! kill -0 "$app" 2>/dev/null; then
    echo "the app exited early; see $out.log" >&2
    exit 1
fi

DISPLAY=$display import -window root "$out"
echo "$out"
