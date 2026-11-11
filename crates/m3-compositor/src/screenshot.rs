//! Screenshots: render an output offscreen and write a PNG.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, ExportMem, ImportAll, ImportMem, Offscreen, Renderer, TextureMapping, damage::OutputDamageTracker,
            gles::GlesTexture,
        },
    },
    output::Output,
    utils::Rectangle,
};

use crate::{
    render::output_elements,
    state::{M3, State},
};

/// An RGBA8 image, rows top to bottom.
pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// `~/Pictures/Screenshots/Screenshot <date time>.png`.
pub fn default_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join("Pictures").join("Screenshots");
    std::fs::create_dir_all(&dir).ok()?;
    // Same naming as the shell's region screenshots.
    let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S");
    let mut p = dir.join(format!("Screenshot {stamp}.png"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("Screenshot {stamp} ({n}).png"));
        n += 1;
    }
    Some(p)
}

/// Render `output` into a fresh offscreen buffer and read it back.
pub fn capture<R>(renderer: &mut R, m3: &mut M3, output: &Output, include_cursor: bool) -> anyhow::Result<Capture>
where
    R: Renderer + ImportAll + ImportMem + Offscreen<GlesTexture> + Bind<GlesTexture> + ExportMem,
    R::TextureId: Clone + Send + 'static,
    R::Error: Send + Sync + 'static,
{
    let mode = output.current_mode().context("output has no mode")?;
    let size = output.current_transform().transform_size(mode.size);
    let mut texture: GlesTexture =
        renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into()).context("allocating capture buffer")?;
    let clear = crate::render::color32(m3.theme.colors.surface_dim);
    let elements = output_elements(m3, renderer, output, include_cursor);
    let mut tracker =
        OutputDamageTracker::new(size, output.current_scale().fractional_scale(), output.current_transform());
    let mut fb = renderer.bind(&mut texture).context("binding capture buffer")?;
    tracker
        .render_output(renderer, &mut fb, 0, &elements, clear)
        .map_err(|e| anyhow::anyhow!("rendering capture: {e:?}"))?;
    let mapping = renderer
        .copy_framebuffer(&fb, Rectangle::from_size((size.w, size.h).into()), Fourcc::Abgr8888)
        .context("reading back capture")?;
    // `flipped()` reports whether rows are already top-down relative to
    // GL's lower-left origin; only bottom-up data needs reversing.
    let bottom_up = !mapping.flipped();
    let bytes = renderer.map_texture(&mapping).context("mapping capture")?.to_vec();
    drop(fb);

    let (width, height) = (size.w as u32, size.h as u32);
    let stride = width as usize * 4;
    let rgba = if bottom_up { bytes.chunks_exact(stride).rev().flatten().copied().collect() } else { bytes };
    Ok(Capture { width, height, rgba })
}

impl State {
    /// Capture `output` (or the first output) to a PNG at `path`.
    pub fn screenshot(
        &mut self,
        output: Option<&str>,
        path: &Path,
        region: Option<[i32; 4]>,
    ) -> anyhow::Result<(u32, u32)> {
        if !path.is_absolute() {
            bail!("screenshot path must be absolute");
        }
        let output: Output = match output {
            Some(name) => {
                self.m3.shell.output_by_name(name).cloned().with_context(|| format!("no output named `{name}`"))?
            }
            None => self.m3.shell.outputs.first().cloned().context("no outputs")?,
        };
        // Region captures leave the pointer out.
        let c = self.backend.capture(&mut self.m3, &output, region.is_none())?;
        let c = match region {
            None => c,
            Some([x, y, w, h]) => {
                let size = smithay::utils::Size::from((c.width as i32, c.height as i32));
                let r = crate::screencopy::region_to_physical(
                    (x, y, w, h),
                    output.current_scale().fractional_scale(),
                    size,
                )
                .context("the region is outside the output")?;
                crop(&c, r)
            }
        };
        write_png(path, c.width, c.height, &c.rgba)?;
        Ok((c.width, c.height))
    }
}

/// Cut `r` out of a capture.
pub fn crop(c: &Capture, r: Rectangle<i32, smithay::utils::Physical>) -> Capture {
    let (x, y, w, h) = (r.loc.x as usize, r.loc.y as usize, r.size.w as usize, r.size.h as usize);
    let stride = c.width as usize * 4;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in y..y + h {
        rgba.extend_from_slice(&c.rgba[row * stride + x * 4..row * stride + (x + w) * 4]);
    }
    Capture { width: w as u32, height: h as u32, rgba }
}

/// Write an RGBA PNG with owner-only permissions, refusing to follow a
/// symlink at the destination.
pub fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header()?;
        writer.write_image_data(rgba)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    f.write_all(&buf)?;
    Ok(())
}
