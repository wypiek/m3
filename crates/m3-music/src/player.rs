//! Audio playback through GStreamer `playbin`.

use std::cell::RefCell;
use std::path::Path;

use gstreamer as gst;
use gstreamer::prelude::*;
use gtk::glib;

/// Events from the pipeline, delivered on the GTK main loop.
pub enum PlayerEvent {
    /// The current track finished.
    Eos,
    Error(String),
    /// Playing (`true`) or paused/stopped (`false`).
    State(bool),
    /// The duration became known.
    Duration,
}

pub struct Player {
    playbin: gst::Element,
    _watch: RefCell<Option<gst::bus::BusWatchGuard>>,
}

impl Player {
    pub fn new(on_event: impl Fn(PlayerEvent) + 'static) -> Result<Self, String> {
        let playbin = gst::ElementFactory::make("playbin")
            .build()
            .map_err(|e| format!("GStreamer playbin is not available: {e}"))?;
        // Audio only: no video window for files with artwork streams.
        let flags = playbin.property_value("flags");
        if let Some(flags_class) = glib::FlagsClass::with_type(flags.type_())
            && let Some(b) = flags_class.builder_with_value(flags)
            && let Some(v) = b.unset_by_nick("video").unset_by_nick("text").build()
        {
            playbin.set_property_from_value("flags", &v);
        }
        let bus = playbin.bus().ok_or("the pipeline has no bus")?;
        let pb = playbin.downgrade();
        let watch = bus
            .add_watch_local(move |_, msg| {
                use gst::MessageView;
                match msg.view() {
                    MessageView::Eos(_) => on_event(PlayerEvent::Eos),
                    MessageView::Error(e) => {
                        let src = e.src().map(|s| s.name().to_string()).unwrap_or_default();
                        tracing::warn!("playback error from {src}: {} ({:?})", e.error(), e.debug());
                        on_event(PlayerEvent::Error(e.error().to_string()));
                    }
                    MessageView::StateChanged(s) => {
                        let from_playbin = pb.upgrade().is_some_and(|p| s.src().is_some_and(|src| *src == p));
                        if from_playbin {
                            on_event(PlayerEvent::State(s.current() == gst::State::Playing));
                        }
                    }
                    MessageView::DurationChanged(_) | MessageView::AsyncDone(_) => on_event(PlayerEvent::Duration),
                    _ => {}
                }
                glib::ControlFlow::Continue
            })
            .map_err(|e| format!("cannot watch the pipeline: {e}"))?;
        Ok(Self { playbin, _watch: RefCell::new(Some(watch)) })
    }

    /// Load `path` and start playing (or stay paused, ready to play).
    pub fn load(&self, path: &Path, play: bool) -> Result<(), String> {
        let _ = self.playbin.set_state(gst::State::Null);
        let uri = gtk::prelude::FileExt::uri(&gtk::gio::File::for_path(path));
        self.playbin.set_property("uri", uri.as_str());
        let state = if play { gst::State::Playing } else { gst::State::Paused };
        self.playbin.set_state(state).map(|_| ()).map_err(|_| format!("cannot play {}", path.display()))
    }

    pub fn play(&self) {
        if self.playbin.set_state(gst::State::Playing).is_err() {
            tracing::warn!("cannot resume playback");
        }
    }

    pub fn pause(&self) {
        let _ = self.playbin.set_state(gst::State::Paused);
    }

    pub fn stop(&self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }

    pub fn is_playing(&self) -> bool {
        self.playbin.current_state() == gst::State::Playing
    }

    pub fn position_ms(&self) -> Option<u64> {
        self.playbin.query_position::<gst::ClockTime>().map(|t| t.mseconds())
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.playbin.query_duration::<gst::ClockTime>().map(|t| t.mseconds())
    }

    pub fn seek_ms(&self, ms: u64) {
        if let Err(e) = self
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, gst::ClockTime::from_mseconds(ms))
        {
            tracing::debug!("seek failed: {e}");
        }
    }

    /// Linear volume 0..=1.
    pub fn volume(&self) -> f64 {
        self.playbin.property::<f64>("volume")
    }

    pub fn set_volume(&self, v: f64) {
        self.playbin.set_property("volume", v.clamp(0.0, 1.0));
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}
