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
use crate::history::History;
use crate::ui::{self, Command, Status};
use crate::{control, logger, paste, pipeline, setup};
use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum Msg {
    Key(KeyEvent),
    Ui(Command),
    Reload,
    Quit,
}

fn ui_options(cfg: &Config) -> ui::Options {
    ui::Options { overlay: cfg.output.overlay, idle_bar: cfg.output.idle_bar, tray: cfg.output.tray }
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
    let opts = ui_options(&cfg);
    let app = App::new(cfg);
    let (tx, rx) = mpsc::channel::<Msg>();

    let ctl = tx.clone();
    control::serve(listener, move |cmd| match cmd {
        "ping" => "pong".into(),
        "reload" => {
            let _ = ctl.send(Msg::Reload);
            "reloaded".into()
        }
        "demo" => {
            // Walk the overlay through its states (for screenshots / QA).
            std::thread::spawn(|| {
                ui::set_status(Status::Listening { hands_free: true });
                for i in 0..120 {
                    ui::set_level(0.04 + 0.05 * ((i as f32) * 0.35).sin().abs());
                    std::thread::sleep(Duration::from_millis(25));
                }
                ui::set_level(0.0);
                ui::set_status(Status::Processing);
                std::thread::sleep(Duration::from_millis(2500));
                ui::set_status(Status::Idle);
            });
            "demo".into()
        }
        "quit" => {
            let _ = ctl.send(Msg::Quit);
            "stopping".into()
        }
        _ => "unknown command".into(),
    });

    let a = app.clone();
    let ctl = tx.clone();
    std::thread::Builder::new().name("controller".into()).spawn(move || controller(a, key, rx, ctl))?;

    // Overlay + tray live on this thread; the keyboard listener pumps their messages.
    let ui_tx = Mutex::new(tx.clone());
    ui::start(opts, Box::new(move |c| {
        let _ = ui_tx.lock().unwrap().send(Msg::Ui(c));
    }));

    log::info!("OpenFlow ready: double-tap {key:?} for hands-free, hold it for push-to-talk. Idle footprint: keyboard hook only.");
    keyboard::listen(key, Box::new(move |e| {
        let _ = tx.send(Msg::Key(e));
    }))
}

fn reload(app: &App, trig: &mut Trigger, key: &mut TriggerKey) {
    match Config::load().and_then(|c| TriggerKey::parse(&c.trigger.key).map(|k| (c, k))) {
        Ok((c, k)) => {
            *key = k;
            keyboard::set_key(k);
            trig.set_params(params(&c, k));
            ui::set_options(ui_options(&c));
            app.set_cfg(c);
            log::info!("config reloaded");
        }
        Err(e) => log::error!("config reload failed, keeping old config: {e:#}"),
    }
}

fn quit(app: &App, rec: &mut Recorder) -> ! {
    rec.stop();
    app.engines.shutdown();
    ui::shutdown();
    log::info!("bye");
    std::process::exit(0);
}

/// Tray-menu actions that don't touch the recording state.
fn ui_action(c: Command, tx: Sender<Msg>) {
    let r: anyhow::Result<()> = (|| {
        match c {
            Command::CopyLast => {
                if let Some(t) = History::last_text() {
                    paste::copy(&t)?;
                }
            }
            Command::OpenSettings => setup::open_path(&crate::config::config_path())?,
            Command::OpenHistory => {
                let path = crate::config::data_dir().join("history").join("history.txt");
                let mut s = String::from("OpenFlow history (newest first)\n\n");
                for e in History::recent(200).into_iter().rev() {
                    s.push_str(&format!("[{}] {} ({})\n  said:  {}\n  typed: {}\n\n", e.time, e.app, e.style, e.raw, e.text.trim_end()));
                }
                std::fs::create_dir_all(path.parent().unwrap())?;
                std::fs::write(&path, s)?;
                setup::open_path(&path)?;
            }
            Command::OpenLogs => setup::open_path(&crate::config::data_dir().join("logs"))?,
            Command::ToggleAutostart => setup::set_autostart(!setup::autostart_enabled())?,
            Command::ToggleIdleBar => {
                let on = Config::load()?.output.idle_bar;
                setup::set_value("output", "idle_bar", if on { "false" } else { "true" })?;
                let _ = tx.send(Msg::Reload);
            }
            _ => {}
        }
        Ok(())
    })();
    if let Err(e) = r {
        log::error!("{c:?}: {e:#}");
    }
}

fn controller(app: Arc<App>, mut key: TriggerKey, rx: Receiver<Msg>, tx: Sender<Msg>) {
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
            Some(Msg::Reload) | Some(Msg::Ui(Command::Reload)) => {
                reload(&app, &mut trig, &mut key);
                VecDeque::new()
            }
            Some(Msg::Quit) | Some(Msg::Ui(Command::Quit)) => quit(&app, &mut rec),
            Some(Msg::Ui(Command::Stop)) => trig.stop_now().into(),
            Some(Msg::Ui(Command::Cancel)) => trig.cancel_now().into(),
            Some(Msg::Ui(c)) => {
                let tx = tx.clone();
                std::thread::spawn(move || ui_action(c, tx));
                VecDeque::new()
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
                        ui::set_status(Status::Listening { hands_free: mode == Mode::HandsFree });
                        log::info!("listening ({}) in {:?}", if mode == Mode::HandsFree { "hands-free" } else { "push-to-talk" }, target.app);
                        session = Some(Session { target, hold, started: now });
                    }
                    Err(e) => {
                        log::error!("microphone: {e:#}");
                        audio::cue(Cue::Error, sounds);
                        ui::set_status(Status::Error);
                        queue.extend(trig.abort());
                    }
                },
                Action::Stop => {
                    if let Some(s) = session.take() {
                        let recording = rec.stop();
                        audio::cue(Cue::Stop, sounds);
                        ui::set_status(Status::Processing);
                        log::info!("recorded {:.1}s, processing", s.started.elapsed().as_secs_f32());
                        let app = app.clone();
                        std::thread::spawn(move || finish(app, recording, s));
                    }
                }
                Action::Cancel => {
                    if session.take().is_some() {
                        rec.stop();
                        audio::cue(Cue::Cancel, sounds);
                        ui::set_status(Status::Idle);
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

/// Start engines in the background so they load (and warm up) while the user speaks.
pub fn prewarm(app: &Arc<App>, cfg: &Arc<Config>) {
    let has_key = cfg.groq_key().is_some();
    let local_stt = !cfg.speech.provider.eq_ignore_ascii_case("cloud") || !has_key;
    let local_ai = cfg.intelligence.enabled && (!cfg.intelligence.provider.eq_ignore_ascii_case("cloud") || !has_key);
    if local_stt {
        match app.engines.prewarm(Kind::Whisper, cfg) {
            Ok(()) => {
                let (a, c) = (app.clone(), cfg.clone());
                std::thread::spawn(move || crate::stt::warmup(&a, &c));
            }
            Err(e) => log::warn!("{e:#}"),
        }
    }
    if local_ai {
        match app.engines.prewarm(Kind::Llama, cfg) {
            Ok(()) => {
                let (a, c) = (app.clone(), cfg.clone());
                std::thread::spawn(move || crate::intelligence::warmup(&a, &c));
            }
            Err(e) => log::warn!("{e:#}"),
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
                Ok(()) => {
                    o.entry.pasted = true;
                    ui::set_status(Status::Idle);
                }
                Err(e) => {
                    log::error!("paste failed: {e:#}");
                    o.entry.error = Some(format!("paste: {e:#}"));
                    let _ = paste::copy(&o.text); // at least it's on the clipboard
                    audio::cue(Cue::Error, cfg.output.sounds);
                    ui::set_status(Status::Error);
                }
            }
            app.history.append(&o.entry);
        }
        Ok(Ok(None)) => {
            audio::cue(Cue::Cancel, cfg.output.sounds);
            ui::set_status(Status::Idle);
        }
        Ok(Err(e)) => {
            log::error!("{e:#}");
            audio::cue(Cue::Error, cfg.output.sounds);
            ui::set_status(Status::Error);
        }
        Err(_) => {
            log::error!("internal error while processing (panic); audio saved in history");
            audio::cue(Cue::Error, cfg.output.sounds);
            ui::set_status(Status::Error);
        }
    }
}
