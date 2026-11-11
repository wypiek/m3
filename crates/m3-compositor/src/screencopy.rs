//! `wlr-screencopy-unstable-v1` (version 3): lets trusted clients capture
//! outputs — screenshot tools such as `grim`, and screen sharing through
//! `xdg-desktop-portal-wlr`. Sandboxed clients (security-context) don't get
//! the global; they go through the portal instead.
//!
//! Buffers are shared memory (XRGB8888/ARGB8888). `copy` is served right
//! away; `copy_with_damage` waits until the output renders a new frame, so a
//! still screen doesn't make screen sharing spin.

use std::sync::Mutex;

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::GlobalId,
            protocol::{wl_buffer::WlBuffer, wl_shm},
        },
    },
    utils::{Physical, Rectangle, Size},
    wayland::shm::{self, BufferData},
};
use tracing::{debug, warn};

use crate::state::{ClientState, State};

const VERSION: u32 = 3;

pub struct ScreencopyState {
    _global: GlobalId,
}

/// Global data: which clients may bind.
pub struct ScreencopyGlobalData {
    filter: Box<dyn Fn(&Client) -> bool + Send + Sync>,
}

impl ScreencopyState {
    pub fn new(dh: &DisplayHandle) -> Self {
        let data = ScreencopyGlobalData {
            filter: Box::new(|client| client.get_data::<ClientState>().is_none_or(|c| c.security_context.is_none())),
        };
        let global = dh.create_global::<State, ZwlrScreencopyManagerV1, _>(VERSION, data);
        Self { _global: global }
    }
}

/// What a frame captures.
pub struct FrameData {
    output: Output,
    /// Region in physical output pixels.
    region: Rectangle<i32, Physical>,
    overlay_cursor: bool,
    used: Mutex<bool>,
}

/// A copy waiting for the next rendered frame of its output.
pub struct PendingCopy {
    frame: ZwlrScreencopyFrameV1,
    buffer: WlBuffer,
    output: Output,
    /// Output frame counter when the copy was requested.
    after_frame: u64,
    queued_at: std::time::Instant,
}

/// A still screen is still sent once a second (screen sharing keep-alive).
const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// Physical size of `output` in its current mode and transform.
fn physical_size(output: &Output) -> Option<Size<i32, Physical>> {
    let mode = output.current_mode()?;
    Some(output.current_transform().transform_size(mode.size))
}

/// Clamp a logical region of an output to physical pixels (pure; tested).
pub fn region_to_physical(
    (x, y, w, h): (i32, i32, i32, i32),
    scale: f64,
    size: Size<i32, Physical>,
) -> Option<Rectangle<i32, Physical>> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let px = |v: i32| (f64::from(v) * scale).round() as i32;
    let (x0, y0) = (px(x).clamp(0, size.w), px(y).clamp(0, size.h));
    let (x1, y1) = (px(x + w).clamp(0, size.w), px(y + h).clamp(0, size.h));
    (x1 > x0 && y1 > y0).then(|| Rectangle::new((x0, y0).into(), (x1 - x0, y1 - y0).into()))
}

/// Copy an RGBA image region into an XRGB/ARGB8888 (little-endian BGRA)
/// buffer with the given stride (pure; tested).
pub fn write_bgra(
    rgba: &[u8],
    src_width: u32,
    region: Rectangle<i32, Physical>,
    dst: &mut [u8],
    dst_stride: usize,
) -> bool {
    let (rx, ry, rw, rh) =
        (region.loc.x as usize, region.loc.y as usize, region.size.w as usize, region.size.h as usize);
    let src_stride = src_width as usize * 4;
    if dst.len() < dst_stride * rh || rgba.len() < src_stride * (ry + rh) || rx + rw > src_width as usize {
        return false;
    }
    for row in 0..rh {
        let src = &rgba[(ry + row) * src_stride + rx * 4..][..rw * 4];
        let out = &mut dst[row * dst_stride..][..rw * 4];
        for (s, d) in src.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
    }
    true
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ScreencopyGlobalData> for State {
    fn bind(
        _state: &mut State,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _data: &ScreencopyGlobalData,
        init: &mut DataInit<'_, State>,
    ) {
        init.init(resource, ());
    }

    fn can_view(client: Client, data: &ScreencopyGlobalData) -> bool {
        (data.filter)(&client)
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for State {
    fn request(
        _state: &mut State,
        _client: &Client,
        _manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        init: &mut DataInit<'_, State>,
    ) {
        let (frame, overlay_cursor, wl_output, region) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput { frame, overlay_cursor, output } => {
                (frame, overlay_cursor, output, None)
            }
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => (frame, overlay_cursor, output, Some((x, y, width, height))),
            zwlr_screencopy_manager_v1::Request::Destroy => return,
            _ => return,
        };
        let output = Output::from_resource(&wl_output);
        let size = output.as_ref().and_then(physical_size);
        let (Some(output), Some(size)) = (output, size) else {
            // The output is gone: the frame fails immediately.
            let f = init.init(frame, FrameUserData(None));
            f.failed();
            return;
        };
        let full = Rectangle::from_size(size);
        let region = match region {
            None => Some(full),
            Some(r) => region_to_physical(r, output.current_scale().fractional_scale(), size),
        };
        let Some(region) = region else {
            let f = init.init(frame, FrameUserData(None));
            f.failed();
            return;
        };
        let data = FrameData { output, region, overlay_cursor: overlay_cursor != 0, used: Mutex::new(false) };
        let f = init.init(frame, FrameUserData(Some(data)));
        let (w, h) = (region.size.w as u32, region.size.h as u32);
        f.buffer(wl_shm::Format::Xrgb8888, w, h, w * 4);
        if f.version() >= 3 {
            f.buffer_done();
        }
    }
}

/// Frame user data (`None` for frames that failed at creation).
pub struct FrameUserData(Option<FrameData>);

impl Dispatch<ZwlrScreencopyFrameV1, FrameUserData> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameUserData,
        _dh: &DisplayHandle,
        _init: &mut DataInit<'_, State>,
    ) {
        let (buffer, with_damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            zwlr_screencopy_frame_v1::Request::Destroy => return,
            _ => return,
        };
        let Some(fd) = &data.0 else { return };
        {
            let Ok(mut used) = fd.used.lock() else { return };
            if *used {
                frame.post_error(zwlr_screencopy_frame_v1::Error::AlreadyUsed, "frame already used");
                return;
            }
            *used = true;
        }
        // Validate the buffer: shared memory of exactly the advertised size.
        let ok = shm::with_buffer_contents(&buffer, |_, _, d: BufferData| {
            d.width == fd.region.size.w
                && d.height == fd.region.size.h
                && d.stride >= fd.region.size.w * 4
                && matches!(d.format, wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888)
        });
        if !matches!(ok, Ok(true)) {
            frame.post_error(zwlr_screencopy_frame_v1::Error::InvalidBuffer, "invalid buffer");
            return;
        }
        if with_damage {
            let after_frame = state.m3.outputs.get(&fd.output).map_or(0, |s| s.frames);
            state.m3.pending_screencopies.push(PendingCopy {
                frame: frame.clone(),
                buffer,
                output: fd.output.clone(),
                after_frame,
                queued_at: std::time::Instant::now(),
            });
            // Make sure the loop wakes for the keep-alive even if nothing
            // else happens.
            use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
            let _ = state.m3.loop_handle.insert_source(Timer::from_duration(MAX_WAIT), |_, _, state| {
                state.flush_screencopies();
                TimeoutAction::Drop
            });
        } else {
            state.serve_screencopy(frame, &buffer, fd, false);
        }
    }
}

impl State {
    /// Render the frame's output and copy it into `buffer`.
    fn serve_screencopy(&mut self, frame: &ZwlrScreencopyFrameV1, buffer: &WlBuffer, fd: &FrameData, damage: bool) {
        let capture = match self.backend.capture(&mut self.m3, &fd.output, fd.overlay_cursor) {
            Ok(c) => c,
            Err(e) => {
                warn!("screencopy failed: {e:#}");
                frame.failed();
                return;
            }
        };
        let written = shm::with_buffer_contents_mut(buffer, |ptr, len, d| {
            // SAFETY: smithay hands us the mapped pool of this live buffer;
            // `len` covers it and we only write within `offset..len`.
            let pool = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
            let start = d.offset as usize;
            pool.get_mut(start..)
                .is_some_and(|dst| write_bgra(&capture.rgba, capture.width, fd.region, dst, d.stride as usize))
        });
        if !matches!(written, Ok(true)) {
            frame.failed();
            return;
        }
        frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
        if damage {
            frame.damage(0, 0, fd.region.size.w as u32, fd.region.size.h as u32);
        }
        let ts = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
        let secs = ts.tv_sec as u64;
        frame.ready((secs >> 32) as u32, secs as u32, ts.tv_nsec as u32);
        debug!(output = fd.output.name(), "screencopy frame served");
    }

    /// Serve `copy_with_damage` requests whose output rendered a new frame.
    pub fn flush_screencopies(&mut self) {
        if self.m3.pending_screencopies.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.m3.pending_screencopies);
        for p in pending {
            if !p.frame.is_alive() || !p.buffer.is_alive() {
                continue;
            }
            let current = self.m3.outputs.get(&p.output).map(|s| s.frames);
            match current {
                None => p.frame.failed(),
                Some(n) if n > p.after_frame || p.queued_at.elapsed() >= MAX_WAIT => {
                    if let Some(fd) = p.frame.data::<FrameUserData>().and_then(|d| d.0.as_ref()) {
                        let fd = FrameData {
                            output: fd.output.clone(),
                            region: fd.region,
                            overlay_cursor: fd.overlay_cursor,
                            used: Mutex::new(true),
                        };
                        self.serve_screencopy(&p.frame, &p.buffer, &fd, true);
                    }
                }
                Some(_) => self.m3.pending_screencopies.push(p),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_scaling_and_clamping() {
        let size: Size<i32, Physical> = (2560, 1600).into();
        let r = region_to_physical((10, 20, 100, 50), 2.0, size).expect("region");
        assert_eq!((r.loc.x, r.loc.y, r.size.w, r.size.h), (20, 40, 200, 100));
        let r = region_to_physical((1200, 700, 500, 500), 2.0, size).expect("clamped");
        assert_eq!((r.loc.x, r.loc.y, r.size.w, r.size.h), (2400, 1400, 160, 200));
        assert!(region_to_physical((0, 0, 0, 10), 1.0, size).is_none());
        assert!(region_to_physical((3000, 0, 10, 10), 1.0, size).is_none());
    }

    #[test]
    fn rgba_to_bgra_region() {
        // 2×2 image: red, green / blue, white.
        let rgba = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
        let mut dst = vec![0u8; 16];
        let region = Rectangle::new((1, 0).into(), (1, 2).into());
        assert!(write_bgra(&rgba, 2, region, &mut dst, 8));
        assert_eq!(&dst[0..4], &[0, 255, 0, 255]); // green
        assert_eq!(&dst[8..12], &[255, 255, 255, 255]); // white
        assert!(!write_bgra(&rgba, 2, Rectangle::new((1, 0).into(), (2, 2).into()), &mut dst, 8));
    }
}
