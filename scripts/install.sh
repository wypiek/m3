#!/usr/bin/env bash
# Install m3 from a release build. Distribution packages use the same layout.
#
#   cargo build --release --workspace
#   sudo scripts/install.sh            # PREFIX=/usr/local by default
#   sudo PREFIX=/usr scripts/install.sh
#   DESTDIR=/tmp/pkg PREFIX=/usr scripts/install.sh   # staging (packaging)
#
# Never touches files in users' home directories.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${PREFIX:-/usr/local}"
DESTDIR="${DESTDIR:-}"
SYSCONFDIR="${SYSCONFDIR:-/etc}"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}/release"

for bin in m3 m3-shell m3-settings m3-lock m3-session m3ctl; do
    [ -x "$TARGET/$bin" ] || { echo "missing $TARGET/$bin — run: cargo build --release --workspace" >&2; exit 1; }
    install -Dm755 "$TARGET/$bin" "$DESTDIR$PREFIX/bin/$bin"
done
# m3's apps: installed when they were built.
apps=(Settings)
for pair in m3-terminal:Terminal m3-music:Music m3-files:Files m3-browser:Browser; do
    bin=${pair%%:*}
    if [ -x "$TARGET/$bin" ]; then
        install -Dm755 "$TARGET/$bin" "$DESTDIR$PREFIX/bin/$bin"
        apps+=("${pair##*:}")
    fi
done
install -Dm644 "$ROOT/data/wayland-sessions/m3.desktop" "$DESTDIR$PREFIX/share/wayland-sessions/m3.desktop"
if [ -x "$TARGET/m3-files" ]; then
    # "Show in folder" from browsers and other apps opens m3-files. Our own
    # file name, so it never collides with other file managers' activation
    # files (e.g. Nautilus ships org.freedesktop.FileManager1.service); a
    # running m3-files owns the bus name either way.
    install -d "$DESTDIR$PREFIX/share/dbus-1/services"
    sed "s|/usr/bin/m3-files|$PREFIX/bin/m3-files|" "$ROOT/data/dbus-1/services/org.freedesktop.FileManager1.service" \
        > "$DESTDIR$PREFIX/share/dbus-1/services/org.m3.Files.FileManager1.service"
    chmod 644 "$DESTDIR$PREFIX/share/dbus-1/services/org.m3.Files.FileManager1.service"
fi
for app in "${apps[@]}"; do
    install -Dm644 "$ROOT/data/applications/org.m3.$app.desktop" "$DESTDIR$PREFIX/share/applications/org.m3.$app.desktop"
done
install -Dm644 "$ROOT/data/systemd/user/m3-session.target" "$DESTDIR$PREFIX/lib/systemd/user/m3-session.target"
install -Dm644 "$ROOT/data/xdg-desktop-portal/m3-portals.conf" "$DESTDIR$PREFIX/share/xdg-desktop-portal/m3-portals.conf"
install -Dm644 "$ROOT/assets/fonts/MaterialSymbolsRounded.ttf" "$DESTDIR$PREFIX/share/fonts/m3/MaterialSymbolsRounded.ttf"
install -Dm644 "$ROOT/assets/fonts/LICENSE-MaterialSymbols.txt" "$DESTDIR$PREFIX/share/licenses/m3/LICENSE-MaterialSymbols.txt"
install -Dm644 "$ROOT/assets/fonts/GoogleSansFlex.ttf" "$DESTDIR$PREFIX/share/fonts/m3/GoogleSansFlex.ttf"
install -Dm644 "$ROOT/assets/fonts/LICENSE-GoogleSansFlex-OFL.txt" "$DESTDIR$PREFIX/share/licenses/m3/LICENSE-GoogleSansFlex-OFL.txt"
install -Dm644 "$ROOT/assets/fonts/TRADEMARKS-GoogleSansFlex.md" "$DESTDIR$PREFIX/share/licenses/m3/TRADEMARKS-GoogleSansFlex.md"
install -Dm644 "$ROOT/LICENSE" "$DESTDIR$PREFIX/share/licenses/m3/LICENSE"
for doc in README.md docs/*.md; do
    install -Dm644 "$ROOT/$doc" "$DESTDIR$PREFIX/share/doc/m3/$(basename "$doc")"
done
# PAM service for the lock screen: never overwrite an administrator's copy.
if [ ! -e "$DESTDIR$SYSCONFDIR/pam.d/m3-lock" ]; then
    install -Dm644 "$ROOT/data/pam.d/m3-lock" "$DESTDIR$SYSCONFDIR/pam.d/m3-lock"
else
    echo "keeping existing $SYSCONFDIR/pam.d/m3-lock"
fi
if [ -z "$DESTDIR" ] && command -v fc-cache >/dev/null; then fc-cache -f >/dev/null || true; fi
echo "m3 installed to $DESTDIR$PREFIX. Choose \"m3\" in your display manager's session list."
