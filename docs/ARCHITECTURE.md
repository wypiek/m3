# m3 architecture

m3 is a set of cooperating processes, not a monolith. The compositor is the
only component that must stay up; everything else can crash and restart
without taking the session down.

```
                    ┌──────────────────────── user session (systemd --user) ───────────────────────┐
                    │                                                                                │
  logind/seatd ───► │  m3 (compositor, Smithay)                                                      │
  DRM/KMS, libinput │   ├─ Wayland socket  ─────────────► apps (GTK, Qt, Electron, SDL, …)           │
                    │   ├─ XWayland ────────────────────► X11 apps                                   │
                    │   └─ IPC socket $XDG_RUNTIME_DIR/m3/<display>.sock (JSON lines)                │
                    │          ▲             ▲                 ▲                                     │
                    │          │             │                 │                                     │
                    │      m3-shell       m3ctl          m3-settings                                 │
                    │  (GTK4 + layer shell: panel, launcher, overview, quick settings, OSD,          │
                    │   notifications, wallpaper)                                                    │
                    │          │                                                                     │
                    │          └── D-Bus ──► NetworkManager, BlueZ, UPower, power-profiles-daemon,   │
                    │                        PipeWire/WirePlumber, logind, org.freedesktop.Notifications│
                    │      m3-lock (ext-session-lock-v1 client, PAM)                                 │
                    └────────────────────────────────────────────────────────────────────────────────┘
```

## Crates

| Crate | Kind | Responsibility |
|---|---|---|
| `m3-protocol` | lib | IPC message types, socket path rules, blocking client. No Smithay/GTK dependency. |
| `m3-core` | lib | Configuration schemas (`~/.config/m3/*.toml`), validation, migration, atomic comment-preserving writes, XDG paths. |
| `m3-compositor` | bin `m3` | Wayland compositor: backends, protocols, window management, rendering, IPC server. |
| `m3ctl` | bin | Command-line control over IPC and configuration files. |
| `m3-theme` | lib | Material 3 colour engine (tonal palettes, schemes, wallpaper extraction) and design tokens. |
| `m3-widgets` | lib | GTK 4 widgets CSS cannot express: the M3 Expressive shape set with spring shape morphing, shape icons, the loading indicator, wavy progress/slider, a shared MPRIS client and `AppStyle` (live theming for m3 apps). The component stylesheet lives in `m3-theme`. |
| `m3-services` | lib | D-Bus integrations: notifications server, NetworkManager, BlueZ, UPower, power profiles, logind, MPRIS media keys, StatusNotifierItem tray, PipeWire/PulseAudio, Open-Meteo weather. |
| `m3-shell` | bin | Panel, launcher, overview, quick settings, notification server + centre, OSDs, wallpaper. |
| `m3-settings` | bin | Settings application. |
| `m3-lock` | bin | Lock screen (ext-session-lock-v1 + PAM). |
| `m3-session` | bin | Session entry point used by the display manager. |
| `m3-terminal` | bin | Terminal emulator (VTE 4). |
| `m3-music` | bin | Music player (GStreamer, MPRIS server). |
| `m3-files` | bin | File manager (GIO, org.freedesktop.FileManager1). |
| `m3-browser` | bin | Web browser (WebKitGTK 6) — unfinished, excluded from packages. |

(Crates beyond `m3ctl` are added milestone by milestone; see `ROADMAP.md`.)

## Compositor

* **State split** — `State { backend, m3 }`. `M3` holds all protocol state and
  the window model; the backend owns renderers and devices. This split lets
  rendering borrow the renderer mutably while reading the scene.
* **Backends** — `udev` (DRM/KMS + libinput via libseat, multi-GPU through
  `GpuManager`, vblank-paced frames, hot-plug), `winit` (nested window for
  development) and `headless` (offscreen GLES on an EGL device; Mesa's
  llvmpipe works without a GPU). All backends render with the same GLES code
  path, so automated tests exercise real rendering.
* **Window model** — global workspaces, each a Smithay `Space` with every
  output mapped at its global position. Windows carry metadata (`WindowData`:
  id, floating/maximized/fullscreen/minimized, home output, restore geometry).
  Layouts (`floating`, master/stack `tiling`, `monocle`) are pure functions in
  `shell/layout.rs` and unit-tested.
* **Mapping** — toplevels stay in `pending` until their first buffer, then get
  placed (rules → layout → centred/cascaded) and focused. A null-buffer commit
  unmaps them back to `pending`.
* **Focus** — keyboard focus is re-validated every loop iteration
  (`ensure_focus`): the lock surface wins while locked, then layer surfaces
  asking for exclusive keyboard input (launcher), then the current focus if
  still visible, then the workspace's most recently used window.
* **Redraw** — nothing renders unless something queued a redraw (commits,
  pointer motion, layout changes). Idle sessions do not wake the GPU.
* **Process spawning** — argv only, never a shell; children get their own
  process group and are reaped via pidfd event sources (no polling, no
  zombies).

### Security-relevant decisions

* The compositor refuses to run as root; device access comes from logind or
  seatd through libseat.
* **Session lock** (`handlers/session_lock.rs`): while locked only lock
  surfaces are drawn, over an opaque fallback, and only they get input. `locked`
  is sent to the client after every output rendered a locked frame. If the lock
  client dies the session stays locked; a new lock client may take over only
  when the previous one is dead.
* Sandboxed clients (security-context-v1, e.g. Flatpak) cannot see the session
  lock, data-control, input-method, virtual-keyboard or foreign-toplevel
  globals.
* XWayland clipboard reads are allowed only for the focused X11 client.
* Screenshots are written with mode 0600 and `O_NOFOLLOW`.

## IPC: why a socket and not D-Bus for the compositor

The compositor exposes newline-delimited JSON on
`$XDG_RUNTIME_DIR/m3/<wayland-display>.sock`:

* it works before (and without) a session bus — useful in early session
  startup, in headless tests and on minimal systems;
* it integrates directly with the compositor's calloop event loop without an
  async runtime;
* it is trivially scriptable (`m3ctl --json`, `socat`).

The socket directory is 0700, the socket 0600, and every peer's UID is checked
with `SO_PEERCRED`. Slow subscribers are disconnected after 4 MiB of backlog;
oversized requests are rejected. Desktop *services* (notifications, theme
changes) use D-Bus where freedesktop specifications require it.

## Configuration

`m3-core::config` defines one `ConfigFile` type per file. Loading never writes;
invalid files are reported with every problem listed and the previous (or
default) configuration stays active. Saving validates first, refuses to
downgrade files written by a newer m3, backs up files before schema upgrades,
and merges into the existing TOML document so comments survive.

## Testing

* Unit tests: config parsing/validation/migration, layout geometry, IPC wire
  format, key-combo parsing.
* Integration tests (`crates/m3-compositor/tests`): start a real
  `m3 --backend headless --test-mode`, connect real clients (`foot`,
  `weston-simple-shm`), drive synthetic input through IPC and assert on window
  state and screenshots.
* Development happens in an Arch Linux VM (`scripts/vm.sh`); DRM/KMS sessions
  are tested in a QEMU VM with virtio-gpu (see `DEVELOPMENT.md`).
