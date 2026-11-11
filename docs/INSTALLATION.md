# Installation

m3 needs a Linux system with systemd-logind or seatd, a GPU with a
GBM/EGL driver (Intel, AMD, virtio-gpu; NVIDIA through its GBM support) and
Wayland libraries. It never runs as root.

> The DRM/KMS session path is implemented but not yet verified on hardware —
> please report problems. See [ROADMAP.md](ROADMAP.md).

## Arch Linux

```sh
sudo pacman -S --needed base-devel rust clang gtk4 gtk4-layer-shell libinput \
    libxkbcommon seatd mesa libdisplay-info pam libpulse
cd packaging/arch
makepkg -si
```

Recommended runtime extras: `xorg-xwayland networkmanager bluez upower
power-profiles-daemon pipewire-pulse wireplumber xdg-desktop-portal-gtk
inter-font foot libnotify`.

## Other distributions

Install the equivalents of the packages above (development headers
included; see `packaging/debian` and `packaging/fedora`), Rust ≥ 1.85, then:

```sh
cargo build --release --workspace
sudo PREFIX=/usr scripts/install.sh
```

The script installs the binaries, the `m3` session for display managers,
the `m3-session.target` systemd user unit, the Settings desktop entry, the
icon font, an xdg-desktop-portal configuration, and `/etc/pam.d/m3-lock`
(only if that file does not exist yet). It never touches home directories.

## Starting m3

* **Display manager** (GDM, SDDM, greetd, LightDM…): choose **m3**.
* **From a text console**: run `m3-session`.
* **Inside another desktop** (for trying it out):
  `m3 --backend winit -- m3-shell`

Your existing desktop configuration is not modified. m3 creates
`~/.config/m3/` only when you change a setting.

## Uninstalling

`sudo PREFIX=/usr scripts/uninstall.sh` (or `pacman -R m3-desktop`).
`~/.config/m3` and `~/.local/state/m3` remain; delete them to reset.

## Troubleshooting

* Logs: `journalctl --user -b` (or the display manager's session log).
  Set `M3_LOG=debug` for more detail.
* The compositor refuses to start as root, and if libseat cannot open a
  session (no logind/seatd, or not on a VT) it says so.
* `m3ctl version` shows whether the compositor is reachable.
* A crashed lock screen restarts automatically; the session stays locked.
