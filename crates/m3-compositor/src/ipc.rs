//! The m3 IPC server (see the `m3-protocol` crate for the wire format).
//!
//! Security: the socket lives in `$XDG_RUNTIME_DIR/m3/` (mode 0700), is
//! itself mode 0600, and every connection's peer UID is checked against ours.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use anyhow::Context;
use m3_protocol::{
    Event, OutputInfo, PROTOCOL_VERSION, Rect, Reply, Request, Response, WindowInfo, WorkspaceInfo, encode,
};
use smithay::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};
use tracing::{debug, info, warn};

use crate::{
    shell::WindowExt,
    state::{M3, State},
};

/// Drop subscribers that fall this far behind.
const MAX_BACKLOG: usize = 4 * 1024 * 1024;
/// Reject absurdly long request lines.
const MAX_LINE: usize = 64 * 1024;

struct Conn {
    stream: UnixStream,
    out: Vec<u8>,
    subscribed: bool,
    dead: bool,
}

pub struct IpcServer {
    path: PathBuf,
    conns: HashMap<u64, Conn>,
    next_id: u64,
    last_workspaces: Vec<WorkspaceInfo>,
    last_windows: Vec<WindowInfo>,
    last_focus: Option<u64>,
}

impl IpcServer {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn start(m3: &mut M3) -> anyhow::Result<Self> {
        let runtime = m3_core::paths::runtime_dir().context("$XDG_RUNTIME_DIR is not set")?;
        let display = m3.socket_name.to_string_lossy().into_owned();
        let path = m3_protocol::socket_path_for(&runtime, &display);
        let dir = path.parent().context("socket path has no parent")?;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        if path.exists() {
            if UnixStream::connect(&path).is_ok() {
                anyhow::bail!("another m3 is already listening on {}", path.display());
            }
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;

        m3.loop_handle
            .insert_source(Generic::new(listener, Interest::READ, Mode::Level), |_, listener, state| {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => state.m3.ipc_accept(stream),
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) => {
                            warn!("IPC accept failed: {e}");
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            })
            .map_err(|e| anyhow::anyhow!("inserting IPC listener: {e}"))?;
        info!("IPC listening on {}", path.display());
        Ok(Self {
            path,
            conns: HashMap::new(),
            next_id: 1,
            last_workspaces: Vec::new(),
            last_windows: Vec::new(),
            last_focus: None,
        })
    }

    pub fn remove_socket(&self) {
        let _ = std::fs::remove_file(&self.path);
    }

    fn queue(&mut self, id: u64, bytes: &[u8]) {
        if let Some(c) = self.conns.get_mut(&id) {
            c.out.extend_from_slice(bytes);
            if c.out.len() > MAX_BACKLOG {
                warn!("IPC client {id} is not reading; disconnecting it");
                c.dead = true;
            }
        }
    }

    fn broadcast(&mut self, event: &Event) {
        let Ok(bytes) = encode(event) else { return };
        let ids: Vec<u64> = self.conns.iter().filter(|(_, c)| c.subscribed).map(|(i, _)| *i).collect();
        for id in ids {
            self.queue(id, &bytes);
        }
    }

    /// Write out buffered data without blocking.
    pub fn flush_writes(&mut self) {
        for c in self.conns.values_mut() {
            while !c.out.is_empty() && !c.dead {
                match c.stream.write(&c.out) {
                    Ok(0) => c.dead = true,
                    Ok(n) => {
                        c.out.drain(..n);
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => c.dead = true,
                }
            }
        }
        self.conns.retain(|_, c| !c.dead);
    }
}

fn peer_is_us(stream: &UnixStream) -> bool {
    match rustix::net::sockopt::socket_peercred(stream) {
        Ok(cred) => cred.uid == rustix::process::getuid(),
        Err(_) => false,
    }
}

impl M3 {
    fn ipc_accept(&mut self, stream: UnixStream) {
        if !peer_is_us(&stream) {
            warn!("rejected IPC connection from another user");
            return;
        }
        let Some(ipc) = self.ipc.as_mut() else { return };
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        let Ok(writer) = stream.try_clone() else { return };
        let id = ipc.next_id;
        ipc.next_id += 1;
        ipc.conns.insert(id, Conn { stream: writer, out: Vec::new(), subscribed: false, dead: false });
        let mut inbuf: Vec<u8> = Vec::new();
        let r = self.loop_handle.insert_source(
            Generic::new(stream, Interest::READ, Mode::Level),
            move |_, stream, state| {
                let mut buf = [0u8; 4096];
                loop {
                    // SAFETY: we only read from the stream; the source keeps it alive.
                    match unsafe { stream.get_mut() }.read(&mut buf) {
                        Ok(0) => {
                            if let Some(ipc) = state.m3.ipc.as_mut() {
                                ipc.conns.remove(&id);
                            }
                            return Ok(PostAction::Remove);
                        }
                        Ok(n) => inbuf.extend_from_slice(&buf[..n]),
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                        Err(_) => {
                            if let Some(ipc) = state.m3.ipc.as_mut() {
                                ipc.conns.remove(&id);
                            }
                            return Ok(PostAction::Remove);
                        }
                    }
                }
                while let Some(pos) = inbuf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = inbuf.drain(..=pos).collect();
                    let reply: Reply = match serde_json::from_slice::<Request>(&line) {
                        Ok(req) => state.handle_ipc(id, req),
                        Err(e) => Err(format!("invalid request: {e}")),
                    };
                    if matches!(&reply, Err(e) if e == SKIP_REPLY) {
                        continue;
                    }
                    if let (Some(ipc), Ok(bytes)) = (state.m3.ipc.as_mut(), encode(&reply)) {
                        ipc.queue(id, &bytes);
                    }
                }
                if inbuf.len() > MAX_LINE {
                    warn!("IPC client {id} sent an oversized request; disconnecting");
                    if let Some(ipc) = state.m3.ipc.as_mut() {
                        ipc.conns.remove(&id);
                    }
                    return Ok(PostAction::Remove);
                }
                if let Some(ipc) = state.m3.ipc.as_mut() {
                    ipc.flush_writes();
                    if !ipc.conns.contains_key(&id) {
                        return Ok(PostAction::Remove);
                    }
                }
                Ok(PostAction::Continue)
            },
        );
        if let Err(e) = r {
            warn!("failed to register IPC client: {e}");
        }
        debug!(id, "IPC client connected");
    }

    pub fn ipc_event(&mut self, event: Event) {
        if let Some(ipc) = self.ipc.as_mut() {
            ipc.broadcast(&event);
        }
    }

    pub fn ipc_outputs_changed(&mut self) {
        let outputs = self.output_infos();
        self.ipc_event(Event::OutputsChanged { outputs });
    }

    /// Emit change events for anything that differs from the last snapshot.
    pub fn ipc_refresh(&mut self) {
        if self.ipc.as_ref().is_none_or(|i| !i.conns.values().any(|c| c.subscribed)) {
            self.shell.dirty = false;
            return;
        }
        let focus = self.focused_window_id();
        let dirty = std::mem::take(&mut self.shell.dirty);
        let (workspaces, windows) =
            if dirty { (self.workspace_infos(), self.window_infos()) } else { (Vec::new(), Vec::new()) };
        let Some(ipc) = self.ipc.as_mut() else { return };
        if dirty && workspaces != ipc.last_workspaces {
            ipc.broadcast(&Event::WorkspacesChanged { workspaces: workspaces.clone() });
            ipc.last_workspaces = workspaces;
        }
        if dirty && windows != ipc.last_windows {
            ipc.broadcast(&Event::WindowsChanged { windows: windows.clone() });
            ipc.last_windows = windows;
        }
        if focus != ipc.last_focus {
            ipc.broadcast(&Event::FocusChanged { window: focus });
            ipc.last_focus = focus;
        }
    }

    fn focused_window_id(&self) -> Option<u64> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        match focus {
            crate::focus::KeyboardFocusTarget::Window(w) => Some(w.id()),
            _ => None,
        }
    }

    pub fn workspace_infos(&self) -> Vec<WorkspaceInfo> {
        self.shell
            .workspaces
            .iter()
            .enumerate()
            .map(|(i, ws)| WorkspaceInfo {
                index: i + 1,
                name: ws.name.clone(),
                active: i == self.shell.active,
                window_count: ws.windows.len(),
                output: None,
            })
            .collect()
    }

    pub fn window_infos(&self) -> Vec<WindowInfo> {
        let focus = self.focused_window_id();
        let mut out = Vec::new();
        for (i, ws) in self.shell.workspaces.iter().enumerate() {
            for w in &ws.windows {
                let st = w.st();
                let geo = ws.space.element_geometry(w).unwrap_or_default();
                out.push(WindowInfo {
                    id: w.id(),
                    title: w.title(),
                    app_id: w.app_id(),
                    workspace: i + 1,
                    focused: focus == Some(w.id()),
                    minimized: st.minimized,
                    maximized: st.maximized,
                    fullscreen: st.fullscreen,
                    floating: st.floating || ws.layout == m3_core::config::LayoutKind::Floating,
                    geometry: Rect { x: geo.loc.x, y: geo.loc.y, width: geo.size.w, height: geo.size.h },
                    xwayland: w.is_x11(),
                });
            }
        }
        out.sort_by_key(|w| w.id);
        out
    }

    pub fn output_infos(&self) -> Vec<OutputInfo> {
        self.shell
            .outputs
            .iter()
            .map(|o| {
                let props = o.physical_properties();
                let geo = self.shell.output_geometry(o).unwrap_or_default();
                let current = o.current_mode();
                let preferred = o.preferred_mode();
                OutputInfo {
                    name: o.name(),
                    make: props.make,
                    model: props.model,
                    enabled: true,
                    current_mode: current.map(|m| m3_protocol::Mode {
                        width: m.size.w,
                        height: m.size.h,
                        refresh_mhz: m.refresh,
                        preferred: Some(m) == preferred,
                    }),
                    modes: o
                        .modes()
                        .into_iter()
                        .map(|m| m3_protocol::Mode {
                            width: m.size.w,
                            height: m.size.h,
                            refresh_mhz: m.refresh,
                            preferred: Some(m) == preferred,
                        })
                        .collect(),
                    scale: o.current_scale().fractional_scale(),
                    transform: crate::backend::from_smithay_transform(o.current_transform()),
                    logical: Rect { x: geo.loc.x, y: geo.loc.y, width: geo.size.w, height: geo.size.h },
                    physical_size_mm: (props.size.w, props.size.h),
                }
            })
            .collect()
    }
}

impl State {
    fn handle_ipc(&mut self, conn: u64, req: Request) -> Reply {
        match req {
            Request::Version => {
                Ok(Response::Version { compositor: format!("m3 {}", m3_core::VERSION), protocol: PROTOCOL_VERSION })
            }
            Request::Reload => {
                self.reload_config();
                Ok(Response::Handled)
            }
            Request::Subscribe => {
                let initial = [
                    Event::WorkspacesChanged { workspaces: self.m3.workspace_infos() },
                    Event::WindowsChanged { windows: self.m3.window_infos() },
                    Event::OutputsChanged { outputs: self.m3.output_infos() },
                    Event::FocusChanged { window: self.m3.focused_window_id() },
                    Event::LockChanged { locked: self.m3.lock.is_locked() },
                ];
                if let Some(ipc) = self.m3.ipc.as_mut() {
                    // The Ok reply must come first; queue it, then the snapshot.
                    if let Ok(bytes) = encode(&Ok::<Response, String>(Response::Handled)) {
                        ipc.queue(conn, &bytes);
                    }
                    for e in &initial {
                        if let Ok(bytes) = encode(e) {
                            ipc.queue(conn, &bytes);
                        }
                    }
                    if let Some(c) = ipc.conns.get_mut(&conn) {
                        c.subscribed = true;
                    }
                }
                // The reply and snapshot were queued above, in order.
                Err(SKIP_REPLY.to_owned())
            }
            Request::Workspaces => Ok(Response::Workspaces { workspaces: self.m3.workspace_infos() }),
            Request::Windows => Ok(Response::Windows { windows: self.m3.window_infos() }),
            Request::Outputs => Ok(Response::Outputs { outputs: self.m3.output_infos() }),
            Request::FocusedWindow => {
                let id = self.m3.focused_window_id();
                Ok(Response::FocusedWindow { window: self.m3.window_infos().into_iter().find(|w| Some(w.id) == id) })
            }
            Request::Action { action } => {
                self.do_action(action);
                Ok(Response::Handled)
            }
            Request::ConfigureOutput { output, change } => {
                let o = self
                    .m3
                    .shell
                    .output_by_name(&output)
                    .cloned()
                    .ok_or_else(|| format!("no output named `{output}`"))?;
                self.backend.configure_output(&mut self.m3, &o, &change)?;
                for ws in 0..self.m3.shell.workspaces.len() {
                    self.arrange(ws);
                }
                self.m3.ipc_outputs_changed();
                Ok(Response::Handled)
            }
            Request::Screenshot { output, path, region, notify } => {
                self.screenshot(output.as_deref(), &path, region).map_err(|e| format!("{e:#}"))?;
                if notify {
                    self.m3.ipc_event(Event::ScreenshotSaved { path });
                }
                Ok(Response::Handled)
            }
            Request::TestInput { input } => {
                if !self.m3.test_mode {
                    return Err("test input requires m3 --test-mode".into());
                }
                self.inject_test_input(input);
                Ok(Response::Handled)
            }
        }
    }
}

/// Marker used internally when a reply was already queued.
pub const SKIP_REPLY: &str = "\u{0}skip";
