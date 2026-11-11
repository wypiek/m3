# Development

## Layout

See [ARCHITECTURE.md](ARCHITECTURE.md). Crates live in `crates/`; data files
(session entry, systemd unit, PAM, portals) in `data/`; the patched Smithay in
`vendor/`.

## Building and testing

```sh
cargo build --workspace
cargo test --workspace             # unit + compositor integration tests
cargo clippy --workspace --all-targets
cargo fmt --all
```

The compositor integration tests (`crates/m3-compositor/tests`) start real
`m3 --backend headless --test-mode` instances, connect real clients and drive
them over IPC. They need `XDG_RUNTIME_DIR`, Mesa (for the EGL device) and,
for the client tests, `weston-simple-shm` and `foot`; missing programs make
those tests skip with a message.

## Running a session for development

```sh
# Inside an existing Wayland/X11 session, as a window:
cargo run -p m3-compositor -- --backend winit -- target/debug/m3-shell

# Headless, scriptable (screenshots via m3ctl):
source scripts/headless-session.sh 1600x1000
m3-shell &                 # PATH points at the debug binaries
m3ctl screenshot /tmp/shot.png
m3_stop
```

`scripts/screenshots.sh OUT_DIR` captures every shell surface, several
Settings pages and the lock screen on a headless session — use it to review
styling changes.

`M3_SLOW_ANIMATIONS=10` slows the compositor's animations down tenfold
(also honoured by `scripts/headless-session.sh`), which makes them possible
to inspect in screenshots.

`--test-mode` enables `m3ctl test-input` (synthetic pointer/keyboard) for
automation. `scripts/lock-test.sh` runs the end-to-end lock-screen test (as
root inside a throwaway VM; it creates/uses a `m3test` user).

## Testing a real session (DRM/KMS) in QEMU

```sh
cd packaging/arch && makepkg -f            # build the package
sudo scripts/qemu/build-image.sh m3-desktop-*.pkg.tar.* m3.qcow2   # on aarch64 Arch Linux ARM
scripts/qemu/run.sh m3.qcow2 state/        # virtio-gpu; KVM on Linux, HVF on macOS
scripts/qemu/qmp.py state/qmp.sock screendump shot.png
scripts/qemu/qmp.py state/qmp.sock key meta_l ret     # Super+Return
```

The image boots straight into m3 through greetd (user `m3test`, password
`m3test`); the serial console is in `state/serial.log`.

## Developing on macOS

m3 only builds on Linux. The project uses an OrbStack Arch Linux machine
(`m3-dev`) with the repository mounted; `scripts/vm.sh <cmd>` runs a command
inside it with a VM-local `CARGO_TARGET_DIR`:

```sh
scripts/vm.sh cargo test --workspace
```

## Conventions

* No `unwrap()` on production paths; errors are logged with context or shown
  to the user.
* Never run a command through a shell; spawn argv.
* Controls in the UI must be backed by a working implementation — if a
  feature is missing, hide the control and list it in `ROADMAP.md`.
* Every configuration change goes through `m3_core::config` (validation,
  atomic write, comment preservation).
