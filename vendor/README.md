# Vendored dependencies

## smithay 0.7.0 (patched)

Unmodified crates.io release of Smithay 0.7.0 (MIT) except for one change in
`src/wayland/shell/wlr_layer/handlers.rs`:

The layer-surface pre-commit hook validates `size`/`anchor` on every commit of
the underlying `wl_surface`. When a client destroys the `zwlr_layer_surface_v1`
object, Smithay resets that cached state to defaults (size 0×0, no anchors)
but keeps the hook. The next commit on the `wl_surface` — GTK always commits a
null buffer after destroying the role object to unmap it — then fails
validation, and Smithay posts a protocol error on the already-destroyed object,
disconnecting the client. Any layer-shell client that hides a surface which is
not anchored to opposite edges (popovers, notification popups, OSDs) is killed.

The patch skips validation once the role object is no longer alive. It should
be proposed upstream; drop this vendored copy once a fixed release exists.

## Patch 2: popup grab drops the motion that ends it

`src/desktop/wayland/popup/grab.rs`, `PopupPointerGrab::motion`: when the
popup grab has ended, the grab was unset but the motion event itself was
dropped, leaving pointer focus on the destroyed popup — the next click
without moving the pointer went nowhere. The motion is now forwarded after
`unset_grab`, exactly as `button()` already does.
