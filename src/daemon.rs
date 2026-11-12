//! The always-on part. Main thread: keyboard hook. Controller thread: trigger
//! state machine + mic. Each dictation is processed on its own thread so the
//! hotkey never lags.

use crate::app::App;
use crate::audio::{self, Cue, Recorder};
use crate::config::Config;
use crate::context::{self, Target};
use crate::engines::{self, Hold, Kind};
use crate::keyboard::{self, KeyEvent, TriggerKey};
use crate::trigger::{Action, Input, Mode, Params, Trigger};
use crate::{control, logger, paste, pipeline};
use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

enum Msg {
    Key(KeyEvent),
    Reload,
    Quit,
}

struct Session {
    target: Target,
    hold: Hold,
    started: Instant,
}

fn params(cfg: &Config, key: TriggerKey) -> Params {
    Params {
        double_tap: Duration::from_millis(cfg.trigger.double_tap_ms),
        hold: Duration::from_millis(cfg.trigger.hold_ms),
        max: Duration::from_secs(cfg.trigger.max_seconds.max(5)),
        toggle_key: key.is_toggle(),
        esc_cancels: cfg.trigger.esc_cancels,
    }
}

pub fn run(console: bool) -> Result<()> {
    logger::init(console);
    let listener = control::bind().ok_or_else(|| anyhow!("OpenFlow is already running"))?;
    engines::cleanup_orphans();
    let cfg = Config::load()?;
    let key = TriggerKey::parse(&cfg.trigger.key)?;
    let app = App::new(cfg);
    let (tx, rx) = mpsc::channel::<Msg>();

    let ctl = tx.clone();
    control::serve(listener, move |cmd| match cmd {
        "ping" => "pong".into(),
        "reload" => {
            let _ = ctl.send(Msg::Reload);
            "reloaded".into()
        }
        "quit" => {
            let _ = ctl.send(Msg::Quit);
            "stopping".into()
        }
        _ => "unknown command".into(),
    });

    let a = app.clone();
    std::thread::Builder::new().name("controller".into()).spawn(move || controller(a, key, rx))?;

    log::info!("OpenFlow ready: double-tap {key:?} for hands-free, hold it for push-to-talk. Idle footprint: keyboard hook only.");
    keyboard::listen(key, Box::new(move |e| {
        let _ = tx.send(Msg::Key(e));
    }))
}

fn controller(app: Arc<App>, mut key: TriggerKey, rx: Receiver<Msg>) {
    let mut trig = Trigger::new(params(&app.cfg(), key));
    let mut rec = Recorder::new();
    let mut session: Option<Session> = None;

    loop {
        // Block with no timeout when idle: zero CPU until a key event.
        let msg = match trig.deadline() {
            Some(d) => match rx.recv_timeout(d.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            },
        };
        let now = Instant::now();
        let mut queue: VecDeque<Action> = match msg {
            None => trig.tick(now).into(),
            Some(Msg::Key(e)) => {
                let input = match e {
                    KeyEvent::TriggerDown => Input::Down,
                    KeyEvent::TriggerUp => Input::Up,
                    KeyEvent::Esc => Input::Esc,
                };
                trig.input(input, now).into()
            }
            Some(Msg::Reload) => {
                match Config::load().and_then(|c| TriggerKey::parse(&c.trigger.key).map(|k| (c, k))) {
                    Ok((c, k)) => {
                        key = k;
                        keyboard::set_key(k);
                        trig.set_params(params(&c, k));
                        app.set_cfg(c);
                        log::info!("config reloaded");
                    }
                    Err(e) => log::error!("config reload failed, keeping old config: {e:#}"),
                }
                VecDeque::new()
            }
            Some(Msg::Quit) => {
                if session.is_some() {
                    rec.stop();
                }
                app.engines.shutdown();
                log::info!("bye");
                std::process::exit(0);
            }
        };

        while let Some(action) = queue.pop_front() {
            let cfg = app.cfg();
            let sounds = cfg.output.sounds;
            match action {
                Action::Start(mode) => match rec.start() {
                    Ok(()) => {
                        audio::cue(Cue::Start, sounds);
                        let target = context::capture();
                        let hold = app.engines.hold();
                        prewarm(&app, &cfg);
                        log::info!("listening ({}) in {:?}", if mode == Mode::HandsFree { "hands-free" } else { "push-to-talk" }, target.app);
                        session = Some(Session { target, hold, started: now });
                    }
                    Err(e) => {
                        log::error!("microphone: {e:#}");
                        audio::cue(Cue::Error, sounds);
                        queue.extend(trig.abort());
                    }
                },
                Action::Stop => {
                    if let Some(s) = session.take() {
                        let recording = rec.stop();
                        audio::cue(Cue::Stop, sounds);
                        log::info!("recorded {:.1}s, processing", s.started.elapsed().as_secs_f32());
                        let app = app.clone();
                        std::thread::spawn(move || finish(app, recording, s));
                    }
                }
                Action::Cancel => {
                    if session.take().is_some() {
                        rec.stop();
                        audio::cue(Cue::Cancel, sounds);
                        log::info!("cancelled");
                    }
                }
                Action::RestoreToggle => {
                    let k = key;
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(40));
                        keyboard::tap_toggle(k);
                    });
                }
            }
        }
    }
}

/// Start engines in the background so they load while the user speaks.
fn prewarm(app: &App, cfg: &Config) {
    let has_key = cfg.groq_key().is_some();
    let local_stt = !cfg.speech.provider.eq_ignore_ascii_case("cloud") || !has_key;
    let local_ai = cfg.intelligence.enabled && (!cfg.intelligence.provider.eq_ignore_ascii_case("cloud") || !has_key);
    if local_stt {
        if let Err(e) = app.engines.prewarm(Kind::Whisper, cfg) {
            log::warn!("{e:#}");
        }
    }
    if local_ai {
        if let Err(e) = app.engines.prewarm(Kind::Llama, cfg) {
            log::warn!("{e:#}");
        }
    }
}

fn finish(app: Arc<App>, recording: audio::Recording, s: Session) {
    let _hold = s.hold; // keep engines loaded until we're done
    let cfg = app.cfg();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pipeline::process(&app, recording, &s.target)));
    match result {
        Ok(Ok(Some(mut o))) => {
            match paste::insert(&o.text, &s.target, &cfg) {
                Ok(()) => o.entry.pasted = true,
                Err(e) => {
                    log::error!("paste failed: {e:#}");
                    o.entry.error = Some(format!("paste: {e:#}"));
                    let _ = paste::copy(&o.text); // at least it's on the clipboard
                    audio::cue(Cue::Error, cfg.output.sounds);
                }
            }
            app.history.append(&o.entry);
        }
        Ok(Ok(None)) => audio::cue(Cue::Cancel, cfg.output.sounds),
        Ok(Err(e)) => {
            log::error!("{e:#}");
            audio::cue(Cue::Error, cfg.output.sounds);
        }
        Err(_) => {
            log::error!("internal error while processing (panic); audio saved in history");
            audio::cue(Cue::Error, cfg.output.sounds);
        }
    }
}
