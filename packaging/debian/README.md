# Debian/Ubuntu packaging (planned)

Not provided yet. `debian/rules` will call `scripts/install.sh` with
`DESTDIR=$(CURDIR)/debian/m3-desktop PREFIX=/usr`. Build dependencies:
`libgtk-4-dev libgtk4-layer-shell-dev libinput-dev libxkbcommon-dev
libseat-dev libudev-dev libgbm-dev libdisplay-info-dev libpam0g-dev
libpulse-dev clang cargo rustc` (Rust ≥ 1.85 is required).
