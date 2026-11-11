//! Building the list of render elements for an output and post-frame work.
//!
//! Stacking (front to back): cursor, drag icon, then either the lock screen
//! (while locked) or: overlay layers, X11 override-redirect windows, top
//! layers (hidden under fullscreen windows), windows, bottom layers,
//! background layers, and finally the clear colour.

use std::time::Duration;

use smithay::{
    backend::renderer::{
        Color32F, ImportAll, ImportMem, Renderer,
        element::{
            AsRenderElements, Id, Kind,
            memory::MemoryRenderBufferRenderElement,
            solid::SolidColorRenderElement,
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::RescaleRenderElement,
        },
        utils::CommitCounter,
    },
    desktop::{
        layer_map_for_output,
        utils::{
            OutputPresentationFeedback, surface_presentation_feedback_flags_from_states,
            surface_primary_scanout_output, update_surface_primary_scanout_output,
        },
    },
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    output::Output,
    render_elements,
    utils::{IsAlive, Physical, Point, Rectangle, Scale},
    wayland::{compositor::with_states, shell::wlr_layer::Layer},
};

use crate::state::M3;

render_elements! {
    pub OutputElement<R> where R: ImportAll + ImportMem;
    Surface = WaylandSurfaceRenderElement<R>,
    Rescaled = RescaleRenderElement<WaylandSurfaceRenderElement<R>>,
    Solid = SolidColorRenderElement,
    Memory = MemoryRenderBufferRenderElement<R>,
}

/// Convert a theme colour for the renderer.
pub fn color32(c: m3_theme::Rgb) -> Color32F {
    let [r, g, b] = c.to_f32();
    Color32F::new(r, g, b, 1.0)
}

/// Duration of the window open animation.
const OPEN_ANIMATION: Duration = Duration::from_millis(450);

/// Damped spring from 0 to 1 at `t` seconds (M3 Expressive spatial spring:
/// damping ratio `zeta`, stiffness `k`); overshoots slightly.
pub fn spring(t: f64, zeta: f64, k: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    let w = k.sqrt();
    let wd = w * (1.0 - zeta * zeta).sqrt();
    1.0 - (-zeta * w * t).exp() * ((wd * t).cos() + (zeta * w / wd) * (wd * t).sin())
}

/// `M3_SLOW_ANIMATIONS=<factor>` slows compositor animations down (for
/// development and screenshots), like Android's animator duration scale.
fn slowdown() -> f64 {
    static F: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *F.get_or_init(|| {
        std::env::var("M3_SLOW_ANIMATIONS")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|f| *f >= 1.0)
            .unwrap_or(1.0)
    })
}

/// Scale and opacity of a window `elapsed` after it appeared, or `None`
/// once the animation is over.
pub fn open_animation(elapsed: Duration) -> Option<(f64, f32)> {
    let elapsed = elapsed.div_f64(slowdown());
    if elapsed >= OPEN_ANIMATION {
        return None;
    }
    let t = elapsed.as_secs_f64();
    let scale = 0.86 + 0.14 * spring(t, 0.72, 420.0);
    let alpha = (t / 0.14).min(1.0) as f32;
    Some((scale, alpha))
}

/// A stable id for the locked-session fallback rectangle.
fn locked_fallback_id() -> &'static Id {
    static ID: std::sync::OnceLock<Id> = std::sync::OnceLock::new();
    ID.get_or_init(Id::new)
}

pub fn output_elements<R>(m3: &mut M3, renderer: &mut R, output: &Output, include_cursor: bool) -> Vec<OutputElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    let mut out: Vec<OutputElement<R>> = Vec::new();
    let Some(og) = m3.shell.output_geometry(output) else { return out };
    let scale = Scale::from(output.current_scale().fractional_scale());
    let to_phys = |p: Point<i32, smithay::utils::Logical>| (p - og.loc).to_physical_precise_round(scale);

    if include_cursor {
        cursor_elements(m3, renderer, output, &mut out);
    }

    if m3.lock.is_locked() {
        if let Some(lock) = m3.lock.surface_for(output) {
            out.extend(render_elements_from_surface_tree(
                renderer,
                lock.wl_surface(),
                Point::<i32, Physical>::from((0, 0)),
                scale,
                1.0,
                Kind::Unspecified,
            ));
        }
        // Always put an opaque layer behind the lock surface so nothing can
        // show through, even if the lock client uses transparency or died.
        let size = og.size.to_physical_precise_round(scale);
        out.push(OutputElement::Solid(SolidColorRenderElement::new(
            locked_fallback_id().clone(),
            Rectangle::from_size(size),
            CommitCounter::default(),
            color32(m3.theme.colors.surface_container_lowest),
            Kind::Unspecified,
        )));
        return out;
    }

    let map = layer_map_for_output(output);
    let layer_elems = |renderer: &mut R, layer: Layer, out: &mut Vec<OutputElement<R>>| {
        for surface in map.layers_on(layer).rev() {
            if let Some(geo) = map.layer_geometry(surface) {
                let loc = geo.loc.to_physical_precise_round(scale);
                out.extend(
                    AsRenderElements::<R>::render_elements::<WaylandSurfaceRenderElement<R>>(
                        surface, renderer, loc, scale, 1.0,
                    )
                    .into_iter()
                    .map(OutputElement::Surface),
                );
            }
        }
    };

    layer_elems(renderer, Layer::Overlay, &mut out);

    for w in m3.shell.override_redirect.iter().rev() {
        if let Some(x) = w.x11_surface() {
            let loc = to_phys(x.geometry().loc);
            out.extend(
                AsRenderElements::<R>::render_elements::<WaylandSurfaceRenderElement<R>>(w, renderer, loc, scale, 1.0)
                    .into_iter()
                    .map(OutputElement::Surface),
            );
        }
    }

    let fullscreen = m3.shell.fullscreen_on(output);
    if fullscreen.is_none() {
        layer_elems(renderer, Layer::Top, &mut out);
    }

    let anim_cfg = &m3.config.compositor.animations;
    let animate = anim_cfg.enabled && !m3.config.theme.reduced_motion;
    let speed = anim_cfg.speed.clamp(0.1, 5.0);
    let mut animating = false;
    let ws = m3.shell.active_ws();
    for w in ws.stacking_top_first() {
        let Some(loc) = ws.space.element_location(&w) else { continue };
        let render_loc = loc - w.geometry().loc;
        // Skip windows that are entirely off this output.
        let bbox = Rectangle::new(render_loc, w.bbox().size);
        if !bbox.overlaps(og) {
            continue;
        }
        let (alpha, opened_at) = {
            let st = crate::shell::WindowExt::st(&w);
            (st.opacity.clamp(0.1, 1.0), st.opened_at)
        };
        // Expressive open animation: grow from 86 % with a spring and fade in.
        let anim = opened_at.filter(|_| animate).and_then(|t| open_animation(t.elapsed().mul_f64(speed)));
        let elems = AsRenderElements::<R>::render_elements::<WaylandSurfaceRenderElement<R>>(
            &w,
            renderer,
            to_phys(render_loc),
            scale,
            alpha * anim.map_or(1.0, |a| a.1),
        );
        match anim {
            Some((s, _)) => {
                animating = true;
                let geo = Rectangle::new(loc, w.geometry().size);
                let center = to_phys(geo.loc + geo.size.downscale(2).to_point());
                out.extend(
                    elems
                        .into_iter()
                        .map(|e| OutputElement::Rescaled(RescaleRenderElement::from_element(e, center, s))),
                );
            }
            None => out.extend(elems.into_iter().map(OutputElement::Surface)),
        }
        if fullscreen.as_ref() == Some(&w) {
            // Nothing below a fullscreen window is visible.
            if animating {
                m3.schedule_animation_frame();
            }
            return out;
        }
    }

    if fullscreen.is_none() {
        layer_elems(renderer, Layer::Bottom, &mut out);
        layer_elems(renderer, Layer::Background, &mut out);
    }
    if animating {
        m3.schedule_animation_frame();
    }
    out
}

fn cursor_elements<R>(m3: &mut M3, renderer: &mut R, output: &Output, out: &mut Vec<OutputElement<R>>)
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Clone + Send + 'static,
{
    let Some(og) = m3.shell.output_geometry(output) else { return };
    let Some(pointer) = m3.seat.get_pointer() else { return };
    let pos = pointer.current_location();
    if !og.to_f64().contains(pos) {
        return;
    }
    let scale = Scale::from(output.current_scale().fractional_scale());
    let rel = pos - og.loc.to_f64();

    if let Some(icon) = m3.dnd_icon.clone().filter(|s| s.alive()) {
        let loc = rel.to_physical(scale).to_i32_round();
        out.extend(render_elements_from_surface_tree(renderer, &icon, loc, scale, 1.0, Kind::Unspecified));
    }

    if let CursorImageStatus::Surface(s) = &m3.cursor_status {
        if !s.alive() {
            m3.cursor_status = CursorImageStatus::default_named();
        }
    }
    match m3.cursor_status.clone() {
        CursorImageStatus::Hidden => {}
        CursorImageStatus::Surface(surface) => {
            let hotspot = with_states(&surface, |s| {
                s.data_map
                    .get::<CursorImageSurfaceData>()
                    .and_then(|d| d.lock().ok().map(|d| d.hotspot))
                    .unwrap_or_default()
            });
            let loc = (rel - hotspot.to_f64()).to_physical(scale).to_i32_round();
            out.extend(render_elements_from_surface_tree(renderer, &surface, loc, scale, 1.0, Kind::Cursor));
        }
        CursorImageStatus::Named(icon) => {
            let int_scale = output.current_scale().integer_scale().max(1) as u32;
            let time = m3.uptime().as_millis() as u32;
            let (buffer, hotspot) = m3.cursor.buffer(icon, int_scale, time);
            let loc = rel.to_physical(scale) - hotspot.to_f64().upscale(scale.x / int_scale as f64);
            match MemoryRenderBufferRenderElement::from_buffer(renderer, loc, &buffer, None, None, None, Kind::Cursor) {
                Ok(e) => out.push(OutputElement::Memory(e)),
                Err(e) => tracing::warn!("failed to upload cursor: {e:?}"),
            }
            if icon != CursorIcon::Default || m3.cursor.is_animated(icon) {
                // Animated cursors need continuous frames; cheap to ask.
            }
        }
    }
}

impl M3 {
    /// After a frame: update primary-output tracking, send frame callbacks
    /// and presentation feedback.
    pub fn post_render(
        &mut self,
        output: &Output,
        states: &smithay::backend::renderer::element::RenderElementStates,
    ) -> OutputPresentationFeedback {
        let time = self.clock.now();
        let throttle = Some(Duration::from_secs(1));
        let ws = self.shell.active_ws();
        let mut feedback = OutputPresentationFeedback::new(output);

        for w in ws.space.elements().chain(self.shell.override_redirect.iter()) {
            w.with_surfaces(|surface, s| {
                update_surface_primary_scanout_output(surface, output, s, states, |o, _, _, _| o);
            });
            w.send_frame(output, time, throttle, surface_primary_scanout_output);
            w.take_presentation_feedback(&mut feedback, surface_primary_scanout_output, |surface, _| {
                surface_presentation_feedback_flags_from_states(surface, states)
            });
        }
        let map = layer_map_for_output(output);
        for l in map.layers() {
            l.with_surfaces(|surface, s| {
                update_surface_primary_scanout_output(surface, output, s, states, |o, _, _, _| o);
            });
            l.send_frame(output, time, throttle, surface_primary_scanout_output);
            l.take_presentation_feedback(&mut feedback, surface_primary_scanout_output, |surface, _| {
                surface_presentation_feedback_flags_from_states(surface, states)
            });
        }
        drop(map);
        if let Some(lock) = self.lock.surface_for(output) {
            smithay::desktop::utils::send_frames_surface_tree(lock.wl_surface(), output, time, throttle, |_, _| {
                Some(output.clone())
            });
        }
        if let CursorImageStatus::Surface(s) = &self.cursor_status {
            smithay::desktop::utils::send_frames_surface_tree(s, output, time, throttle, |_, _| Some(output.clone()));
        }
        if let Some(icon) = &self.dnd_icon {
            smithay::desktop::utils::send_frames_surface_tree(icon, output, time, throttle, |_, _| {
                Some(output.clone())
            });
        }
        let outputs: Vec<Output> = self.shell.outputs.clone();
        self.lock.output_rendered(output, &outputs);
        if let Some(s) = self.outputs.get_mut(output) {
            s.frames += 1;
            s.last_frame = Some(std::time::Instant::now());
        }
        feedback
    }
}

/// Mark presentation feedback as presented now (backends without real
/// vblank timing: nested and headless).
pub fn present_now(
    mut feedback: OutputPresentationFeedback,
    output: &Output,
    clock: &smithay::utils::Clock<smithay::utils::Monotonic>,
) {
    let refresh = output
        .current_mode()
        .map(|m| {
            smithay::wayland::presentation::Refresh::fixed(Duration::from_secs_f64(1000.0 / m.refresh.max(1) as f64))
        })
        .unwrap_or(smithay::wayland::presentation::Refresh::Unknown);
    feedback.presented::<_, smithay::utils::Monotonic>(
        clock.now(),
        refresh,
        0,
        smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::empty(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_animation_springs_and_ends() {
        let (s0, a0) = open_animation(Duration::ZERO).expect("animating");
        assert!((s0 - 0.86).abs() < 1e-9 && a0 == 0.0);
        // The spring overshoots past full size before settling.
        let peak =
            (1..45).filter_map(|i| open_animation(Duration::from_millis(i * 10))).map(|a| a.0).fold(0.0, f64::max);
        assert!(peak > 1.0 && peak < 1.05, "peak {peak}");
        assert!(open_animation(OPEN_ANIMATION).is_none());
    }
}
