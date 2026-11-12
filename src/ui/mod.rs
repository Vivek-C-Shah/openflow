//! Overlay pill + tray icon.
//!
//! Windows: native layered window and notification-area icon, driven by the
//! same message loop as the keyboard hook. The overlay animates only while
//! something changes; idle it is a static bitmap (zero CPU).
//! macOS / Linux: not implemented yet (sound cues only); see the issue tracker.

pub mod png;
pub mod render;
#[cfg(windows)]
mod win;

use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Listening { hands_free: bool },
    Processing,
    Error,
}

/// Things the user can do from the overlay or tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Stop,
    Cancel,
    CopyLast,
    OpenSettings,
    Reload,
    OpenHistory,
    OpenLogs,
    ToggleAutostart,
    ToggleIdleBar,
    Quit,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub overlay: bool,
    pub idle_bar: bool,
    pub tray: bool,
}

pub type CommandSink = Box<dyn Fn(Command) + Send + Sync + 'static>;

static LEVEL: AtomicU32 = AtomicU32::new(0);

/// Current microphone loudness (RMS), set from the audio callback.
pub fn set_level(rms: f32) {
    LEVEL.store(rms.to_bits(), Ordering::Relaxed);
}

pub fn level() -> f32 {
    f32::from_bits(LEVEL.load(Ordering::Relaxed))
}

/// Create the overlay and tray on the *current* thread, which must then run
/// the message loop (the keyboard listener does).
pub fn start(opts: Options, sink: CommandSink) {
    #[cfg(windows)]
    win::start(opts, sink);
    #[cfg(not(windows))]
    let _ = (opts, sink);
}

pub fn set_status(s: Status) {
    #[cfg(windows)]
    win::set_status(s);
    #[cfg(not(windows))]
    let _ = s;
}

pub fn set_options(opts: Options) {
    #[cfg(windows)]
    win::set_options(opts);
    #[cfg(not(windows))]
    let _ = opts;
}

/// Remove the tray icon before exiting.
pub fn shutdown() {
    #[cfg(windows)]
    win::shutdown();
}

/// Write PNG previews of every overlay state (for the README / design checks).
pub fn render_previews(dir: &std::path::Path) -> anyhow::Result<()> {
    use render::*;
    std::fs::create_dir_all(dir)?;
    let mut live = Look::idle(true);
    live.width = 104.0;
    live.height = 34.0;
    live.expand = 1.0;
    live.bars = [0.25, 0.5, 0.8, 0.6, 1.0, 0.7, 0.45, 0.6, 0.3];
    let mut hands_free = live;
    hands_free.width = 156.0;
    hands_free.controls = 1.0;
    let mut thinking = live;
    thinking.processing = 1.0;
    thinking.time = 0.6;
    let mut error = thinking;
    error.error = 1.0;
    let states = [
        ("idle", Look::idle(true)),
        ("push-to-talk", live),
        ("hands-free", hands_free),
        ("processing", thinking),
        ("error", error),
    ];
    for (name, look) in states {
        let c = draw_overlay(&look, 2.0);
        // preview on a desktop-like backdrop so the translucent pill is visible
        let mut bg = render::Canvas::new(c.w, c.h);
        bg.round_rect(c.w as f32 / 2.0, c.h as f32 / 2.0, c.w as f32 * 2.0, c.h as f32 * 2.0, 0.0, rgba(0x6b7a8f, 1.0));
        let img = composite(&bg.to_rgba(), &c.to_rgba());
        std::fs::write(dir.join(format!("overlay-{name}.png")), png::encode(c.w, c.h, &img))?;
    }
    for rec in [false, true] {
        let c = draw_icon(64, rec);
        std::fs::write(dir.join(if rec { "icon-recording.png" } else { "icon.png" }), png::encode(64, 64, &c.to_rgba()))?;
    }
    Ok(())
}

fn composite(bg: &[u8], fg: &[u8]) -> Vec<u8> {
    bg.chunks(4)
        .zip(fg.chunks(4))
        .flat_map(|(b, f)| {
            let a = f[3] as f32 / 255.0;
            let m = |i: usize| (f[i] as f32 * a + b[i] as f32 * (1.0 - a)) as u8;
            [m(0), m(1), m(2), 255]
        })
        .collect()
}
