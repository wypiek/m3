#!/usr/bin/env bash
# Capture screenshots of every m3 surface on a headless session (inside Linux).
#   scripts/screenshots.sh OUT_DIR [WxH]
# Needs a debug build of the workspace.
set -u
OUT=${1:?usage: screenshots.sh OUT_DIR [WxH]}
mkdir -p "$OUT"
# shellcheck source=headless-session.sh
source "$(dirname "$0")/headless-session.sh" "${2:-1440x900}" >/dev/null
trap m3_stop EXIT
act() { m3ctl action "{\"action\":\"$1\"}" >/dev/null; sleep "${2:-0.8}"; }
shot() { m3ctl screenshot "$OUT/$1.png" >/dev/null && echo "$OUT/$1.png"; }

m3-shell >"$M3_TEST_DIR/shell.log" 2>&1 &
sleep 4
shot desktop
act toggle_quick_settings; shot quick-settings; act toggle_quick_settings 0.5
act toggle_launcher; shot launcher; act toggle_launcher 0.5
act toggle_overview; shot overview; act toggle_overview 0.5
command -v notify-send >/dev/null && {
    timeout 5 notify-send -a Mail "Lunch at noon?" "The usual place — I'll book a table." &
    sleep 1.2; shot notification-popup
    act toggle_notifications; shot notification-center; act toggle_notifications 0.5
}
act volume_up 0.5; shot osd
for page in appearance display desktop input sound power about; do
    m3-settings --page "$page" >/dev/null 2>&1 &
    sp=$!
    sleep 3
    shot "settings-$page"
    kill "$sp" 2>/dev/null; wait "$sp" 2>/dev/null
    sleep 0.3
done
# The lock screen last: it locks the session until m3 stops.
m3-lock >"$M3_TEST_DIR/lock.log" 2>&1 &
sleep 3
shot lock
