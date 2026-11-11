# Fedora packaging (planned)

Not provided yet. A spec file will use `scripts/install.sh` with
`DESTDIR=%{buildroot} PREFIX=%{_prefix}`. Build requirements map to:
`gtk4-devel gtk4-layer-shell-devel libinput-devel libxkbcommon-devel
libseat-devel systemd-devel mesa-libgbm-devel libdisplay-info-devel
pam-devel pulseaudio-libs-devel clang rust cargo`.
