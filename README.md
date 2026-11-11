# m3

**m3** is a Linux desktop environment built around its own Wayland compositor
and Google's Material 3 Expressive design language: dynamic colour from your
wallpaper, tonal surfaces, expressive shapes and motion — with the power and
openness of Linux.

It is not a theme or a window-manager configuration: m3 ships its own
compositor (Rust, [Smithay](https://github.com/Smithay/smithay)), desktop
shell (GTK 4 + layer shell) with desktop widgets, lock screen, settings app,
its own terminal, music player and file manager, system-service integration
and a command-line tool.

> **Status: early development (0.1).** Everything is tested in headless and
> virtual (QEMU) sessions, and m3 runs as a daily session on one real laptop
> (Arch Linux, NVIDIA GTX 1650 with nouveau). Other GPUs, multi-monitor
> setups and some features (night light, screen power-off) are not verified
> yet — see [docs/ROADMAP.md](docs/ROADMAP.md) for exactly what works.

## Components

| Program | What it is |
|---|---|
| `m3` | The Wayland compositor (window management, workspaces, tiling, input, rendering, XWayland, session lock, IPC). |
| `m3-shell` | Panel, launcher, overview, quick settings, notifications, OSDs, wallpaper. |
| `m3-settings` | The Settings app. |
| `m3-lock` | The lock screen (ext-session-lock + PAM). |
| `m3-session` | What your display manager starts. |
| `m3ctl` | Command-line control (`m3ctl --help`). |
| `m3-terminal` | Terminal (VTE): tabs, find, theme-derived colours. |
| `m3-music` | Music player (GStreamer): library, album-art colours, MPRIS. |
| `m3-files` | File manager (GIO): views, thumbnails, trash, drives, FileManager1. |
| `m3-browser` | Web browser (WebKitGTK) — **work in progress, not built or installed yet**. |

## Quick start

```sh
cargo build --release --workspace
sudo scripts/install.sh          # or: cd packaging/arch && makepkg -si
```

Then pick **m3** in your display manager. To try it inside an existing
Wayland/X11 session without logging out:

```sh
target/release/m3 --backend winit -- target/release/m3-shell
```

Default shortcuts: `Super+Space` launcher · `Super+Tab` overview ·
`Super+Return` terminal · `Super+Q` close · `Super+1…9` workspaces ·
`Super+Shift+1…9` move window · `Super+←/→/↑` snap/maximize ·
`Super+S` quick settings · `Super+N` notifications · `Super+L` lock ·
`Print` screenshot · `Shift+Print` or `Super+Shift+S` region screenshot ·
`Alt+Tab` switch windows. `Super`+drag moves, `Super`+right-drag resizes.

## Documentation

- [Installation](docs/INSTALLATION.md)
- [Configuration](docs/CONFIGURATION.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Development](docs/DEVELOPMENT.md)
- [Roadmap & status](docs/ROADMAP.md)
- [Contributing](CONTRIBUTING.md)

## License

m3 is GPL-3.0-or-later. It bundles Material Symbols (Apache-2.0), Google
Sans Flex (SIL OFL 1.1; see `assets/fonts/TRADEMARKS-GoogleSansFlex.md` for
the font's trademark terms) and a patched copy of Smithay 0.7.0 (MIT, see
`vendor/README.md`).
