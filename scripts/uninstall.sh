#!/usr/bin/env bash
# Remove what scripts/install.sh installed. User configuration in
# ~/.config/m3 and state in ~/.local/state/m3 are left untouched.
set -euo pipefail
PREFIX="${PREFIX:-/usr/local}"
SYSCONFDIR="${SYSCONFDIR:-/etc}"
rm -f "$PREFIX"/bin/{m3,m3-shell,m3-settings,m3-lock,m3-session,m3ctl,m3-terminal,m3-music,m3-files,m3-browser}
rm -f "$PREFIX/share/wayland-sessions/m3.desktop" "$PREFIX"/share/applications/org.m3.{Settings,Terminal,Music,Files,Browser}.desktop "$PREFIX/share/dbus-1/services/org.m3.Files.FileManager1.service"
rm -f "$PREFIX/lib/systemd/user/m3-session.target" "$PREFIX/share/xdg-desktop-portal/m3-portals.conf"
rm -rf "$PREFIX/share/fonts/m3" "$PREFIX/share/licenses/m3" "$PREFIX/share/doc/m3"
echo "Removed m3 from $PREFIX. $SYSCONFDIR/pam.d/m3-lock was kept; delete it if no longer needed."
