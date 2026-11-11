#!/usr/bin/env bash
# Start m3 headless (test mode) in the background and print its environment.
# Usage (inside Linux): source scripts/headless-session.sh [outputs]
#   then use m3ctl / clients; `m3_stop` to shut down.
set -u
T=${CARGO_TARGET_DIR:-target}/debug
export M3_TEST_DIR=$(mktemp -d)
export M3_CONFIG_DIR=$M3_TEST_DIR/config
mkdir -p "$M3_CONFIG_DIR"
M3_LOG=${M3_LOG:-info,smithay=warn} M3_SLOW_ANIMATIONS=${M3_SLOW_ANIMATIONS:-1} "$T/m3" --backend headless --test-mode --no-xwayland \
    --config-dir "$M3_CONFIG_DIR" --headless-outputs "${1:-1280x800}" >"$M3_TEST_DIR/m3.log" 2>&1 &
export M3_PID=$!
for _ in $(seq 100); do
    sock=$(grep -o 'IPC listening on [^ ]*' "$M3_TEST_DIR/m3.log" | awk '{print $4}')
    [ -n "$sock" ] && [ -S "$sock" ] && break
    sleep 0.05
done
export M3_SOCKET=$sock
export WAYLAND_DISPLAY=$(basename "$sock" .sock)
export PATH=$T:$PATH
m3_stop() { kill "$M3_PID" 2>/dev/null; wait "$M3_PID" 2>/dev/null; }
echo "m3 pid=$M3_PID WAYLAND_DISPLAY=$WAYLAND_DISPLAY M3_SOCKET=$M3_SOCKET log=$M3_TEST_DIR/m3.log"
