# Contributing to m3

Thank you for helping! Please read [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)
first.

* **Discuss bigger changes first** in an issue, especially protocol, IPC or
  configuration-schema changes.
* **Keep it working end to end.** A UI control must do something real.
  Unfinished features stay hidden and are listed in `docs/ROADMAP.md`.
* **Test what you change.** Compositor behaviour belongs in
  `crates/m3-compositor/tests`; pure logic gets unit tests. Run
  `cargo fmt --all`, `cargo clippy --workspace --all-targets` and
  `cargo test --workspace` before sending a change.
* **Configuration compatibility.** Changing a config schema means bumping its
  `SCHEMA_VERSION` and adding a `migrate_step`.
* **IPC compatibility.** Additive changes are fine; incompatible ones bump
  `m3_protocol::PROTOCOL_VERSION`.
* **Security.** Never log secrets, never execute through a shell, keep the
  lock screen fail-closed. Report vulnerabilities privately to the
  maintainers rather than in a public issue.
* **Design.** Use the m3-theme tokens and component classes; do not hard-code
  colours, radii or durations.

By contributing you agree that your contributions are licensed under the
GPL-3.0-or-later.
