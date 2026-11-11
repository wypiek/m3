# Roadmap and status

Status legend: **✅ done & tested** (automated test or verified end-to-end in a
session) · **🟡 implemented, not verified** (compiles, but not yet exercised
on the target environment) · **⬜ not started**.

Everything marked ✅ was verified in the development VM (Arch Linux) on the
headless backend with real clients (`foot`, `weston-simple-shm`, GTK 4) and
real system services (NetworkManager, UPower, power-profiles-daemon,
PipeWire/WirePlumber, PAM).

## Milestone 1 — Core compositor

| Feature | Status |
|---|---|
| Smithay compositor, Wayland socket, xdg-shell, popups + grabs | ✅ |
| Keyboard (xkb layouts, repeat) and pointer input | ✅ |
| Headless backend (GLES on an EGL device; llvmpipe works) | ✅ |
| Nested backend (winit) | 🟡 |
| DRM/KMS backend: libseat session, udev, vblank pacing, libinput, software-rendering fallback | ✅ in QEMU (virtio-gpu, software rendering) via greetd |
| DRM/KMS on hardware GPUs (Intel/AMD/NVIDIA), multi-GPU, monitor hot-plug, mode changes, VT switching | 🟡 |
| Floating placement, focus, raise, click-to-focus, focus-follows-mouse | ✅ |
| Move/resize (client and Super+drag), edge snapping, drag-to-edge snap/maximize | 🟡 (logic unit-tested; not driven by a test) |
| Maximize, fullscreen, minimize, snapping halves | ✅ |
| Tiling (master/stack) and monocle layouts, per-output | ✅ |
| Workspaces: switch, move windows, names, count, wrap | ✅ |
| Alt+Tab focus cycling | ✅ |
| Window rules (app id/title globs: float, workspace, size, opacity, …) | 🟡 |
| Keybindings from `keybindings.toml` | ✅ |
| Layer shell (panels, exclusive zones, keyboard interactivity) | ✅ |
| Protocols: data-device, primary selection, data-control, xdg-decoration, xdg-activation, xdg-foreign, viewporter, fractional-scale, presentation-time, pointer constraints, relative pointer, pointer gestures, text-input/input-method, virtual keyboard, keyboard-shortcuts-inhibit, idle-notify, idle-inhibit, ext-foreign-toplevel-list, cursor-shape, single-pixel-buffer, security-context, linux-dmabuf | 🟡 (advertised and implemented through Smithay; only some exercised by tests) |
| XWayland (X11 apps, clipboard bridging) | 🟡 |
| IPC socket + `m3ctl` | ✅ |
| Screenshots: Print (full), Shift+Print / Super+Shift+S (region selector), notification with Open / Show in folder | ✅ |
| Server-side window decorations (Material title bars) | ⬜ — clients are asked to draw their own |
| Window open animation (spring scale + fade; off with reduced motion) | ✅ (unit-tested curve; seen mid-animation with `M3_SLOW_ANIMATIONS`) |
| Window close / workspace-switch animations | ⬜ |
| wlr-screencopy v3 (grim, xdg-desktop-portal-wlr screen sharing; damage-driven) | ✅ with `grim` (full + region); portal screen sharing 🟡 |
| Picture-in-picture mode | ⬜ |
| Per-monitor independent workspaces | ⬜ (workspaces span all monitors) |

## Milestone 2 — Desktop shell

| Feature | Status |
|---|---|
| Wallpaper per monitor, fit modes, crossfade, built-in theme-coloured wallpaper | ✅ |
| Slideshow | ✅ |
| Panel: launcher button, workspaces, tasks (pinned + running), clock & calendar, status, notifications | ✅ |
| Panel positions (bottom by default; sheets and popovers follow the panel), floating style, autohide/intelligent hide | ✅ top + bottom; left/right and autohide 🟡 |
| Launcher: search, categories, recent, history, recent files, keyboard launch | ✅ |
| Overview: workspaces + windows, search, drag between workspaces, hot corner | ✅ (window thumbnails need screencopy ⬜) |
| System tray (StatusNotifierItem + dbusmenu) | ✅ with libayatana-appindicator; Electron/Qt apps 🟡 |
| Desktop widgets: two-tone clock, “Good” greeting with date cookies, day flower, shortcuts + search, now playing (MPRIS), countdown, storage, RAM/CPU, weather (Open-Meteo, opt-in) | ✅ (screenshots; weather fetch checked through Settings place search only) |
| Island panel style, power menu and search modules | ✅ (screenshots) |
| Default look: warm “Wine” seed, Content scheme variant, organic default wallpaper | ✅ |

## Milestone 3 — Design system

| Feature | Status |
|---|---|
| Material colour engine (Spec 2025): all roles, 9 variants, contrast, accent/surface controls | ✅ (incl. WCAG contrast tests) |
| Wallpaper colour extraction with cache | ✅ |
| Light/dark/automatic, presets, custom seed — live everywhere | ✅ |
| Tokens: Expressive shape scale, emphasized type scale, spring motion, state layers; GTK component stylesheet | ✅ |
| Expressive components: connected button groups, grouped lists, tonal pill menus/dropdowns, expressive switch and slider | ✅ (screenshots via `scripts/screenshots.sh`) |
| Expressive shape library + spring shape morphing, shape icons, loading indicator (`m3-widgets`) | ✅ (shape/spring maths unit-tested; morphing checked by hand only) |
| Wavy linear/circular progress indicators and wavy slider (`m3-widgets`) | ✅ (geometry unit-tested; used by m3-music and m3-browser) |
| Shell motion: staggered entrance animations (quick settings, launcher, overview, notifications, OSD), spring press feedback | ✅ (screenshots) |
| Google Sans Flex (variable: weight, width, roundness) as the default font | ✅ |
| Material Symbols icons | ✅ |

## Milestone 4 — Desktop services

| Feature | Status |
|---|---|
| Notification server (freedesktop 1.2), popups, actions, history, grouping, DnD, schedule, per-app prefs | ✅ |
| Quick settings: Wi-Fi, Bluetooth, DnD, dark mode, power mode, airplane, volume, mic, brightness, output device, battery, session actions | ✅ with the services available in the VM; Wi-Fi/Bluetooth/brightness 🟡 (no hardware in the VM) |
| OSDs: volume, brightness, microphone, workspace | ✅ |
| Media keys via MPRIS | 🟡 |
| Night light (DRM gamma, schedule, smooth transition, quick settings tile, Settings) | 🟡 (not yet verified on a GPU) |
| Low-battery warnings (20/10/5 %) | ✅ (thresholds unit-tested) |
| Screen recording, keyboard-backlight OSD | ⬜ |

## Milestone 5 — Settings

| Feature | Status |
|---|---|
| Appearance, Display, Desktop (workspaces, windows, panel, shortcuts), Input, Network, Bluetooth, Sound, Notifications, Power, Applications, Privacy & Security, About | ✅ pages render with live data; 🟡 individual controls are not covered by automated tests |
| Live restyling and persistence through `m3_core::config` | ✅ |

## Milestone 6 — Security & session

| Feature | Status |
|---|---|
| ext-session-lock in the compositor (opaque fallback, takeover only if the lock client died) | ✅ |
| `m3-lock`: PAM, zeroized password, failure handling, caps-lock hint | ✅ (wrong + right password, kill -9 recovery) |
| Automatic relaunch of a crashed lock screen | ✅ |
| Idle lock, idle suspend, idle inhibitors | ✅ |
| Lock before suspend (logind delay inhibitor) | 🟡 |
| Lock screen in a real DRM session (PAM file from the package) | ✅ (QEMU) |
| `m3-session`, `m3-session.target`, session `.desktop` file, env export, XDG autostart, journal logging | ✅ started by greetd in QEMU |
| Shell supervision with crash-loop protection, `m3ctl restart` | ✅ |
| Screen power-off when idle (DPMS) + `displays_off` action | 🟡 (not yet verified on a GPU) |

## Milestone 7 — Hardening

| Feature | Status |
|---|---|
| Arch PKGBUILD (build, check, package), install script | ✅ (`makepkg` + installed into the QEMU image) |
| Uninstall script | 🟡 |
| Fedora / Debian packaging | ⬜ (notes in `packaging/`) |
| CI (fmt, clippy, build, tests) | 🟡 (workflow written, not run on a hosted runner yet) |
| QEMU virtio-gpu test image (`scripts/qemu/`) | ✅ |
| Real-hardware testing | ⬜ next |
| Application compatibility matrix (Firefox, Chromium/Electron, Qt, SDL, Vulkan games, X11) | ⬜ |

## Next steps

1. Test on real Intel/AMD hardware (hardware rendering, multiple monitors,
   hot-plug, suspend/resume).
2. Material server-side decorations and window animations.
3. Screen capture protocol + portal backend (screenshots, screen sharing,
   overview thumbnails).
4. Night light, DPMS, system tray.
