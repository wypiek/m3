#!/usr/bin/env bash
# Manual end-to-end lock screen test (run as root inside the dev VM):
# locks a headless session as the `m3test` user, tries a wrong password,
# kills the lock screen, then unlocks with the right password.
set -u
BIN=/tmp/m3bin; OUT=${1:-/tmp/lock-shots}
mkdir -p $BIN $OUT; cp $CARGO_TARGET_DIR/debug/{m3,m3-lock,m3ctl} $BIN/; chmod 755 $BIN/*; chmod 777 $OUT
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p /usr/share/fonts/m3 && cp "$ROOT/assets/fonts/MaterialSymbolsRounded.ttf" /usr/share/fonts/m3/ && fc-cache -f >/dev/null
su m3test -s /bin/bash -c "
export XDG_RUNTIME_DIR=/tmp/m3test-rt; rm -rf \$XDG_RUNTIME_DIR; mkdir -m700 -p \$XDG_RUNTIME_DIR
export PATH=$BIN:\$PATH M3_CONFIG_DIR=/tmp/m3test-cfg; mkdir -p \$M3_CONFIG_DIR
m3 --backend headless --test-mode --no-xwayland --headless-outputs 1280x800 > /tmp/m3test.log 2>&1 &
sleep 2; export WAYLAND_DISPLAY=\$(ls \$XDG_RUNTIME_DIR | grep -m1 '^wayland-[0-9]*\$'); export M3_SOCKET=\$XDG_RUNTIME_DIR/m3/\$WAYLAND_DISPLAY.sock
t() { for k in \"\$@\"; do m3ctl test-input tap \$k; done; }
foot >/dev/null 2>&1 & sleep 1.5
m3ctl screenshot $OUT/0-unlocked.png >/dev/null
m3-lock > /tmp/m3lock.log 2>&1 & sleep 2.5
m3ctl screenshot $OUT/1-locked.png >/dev/null
t 49 24 25 18 28; sleep 4
m3ctl screenshot $OUT/2-wrong.png >/dev/null
pkill -9 -x m3-lock; sleep 0.3
m3ctl screenshot $OUT/3-crashed.png >/dev/null
sleep 3
m3ctl screenshot $OUT/4-relaunched.png >/dev/null
t 46 24 19 19 18 46 20 12 35 24 19 31 18 12 5 3 28; sleep 4
m3ctl screenshot $OUT/5-unlocked-again.png >/dev/null
m3ctl window list
pkill -x m3; pkill -x foot
"
grep -i 'lock' /tmp/m3test.log | tail -6
