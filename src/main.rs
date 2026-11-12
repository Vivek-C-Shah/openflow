//! `openflow` command-line interface.

use anyhow::{bail, Result};
use openflow::app::App;
use openflow::audio::{self, Recorder};
use openflow::config::{self, Config};
use openflow::context::Target;
use openflow::engines::{self, Kind};
use openflow::history::History;
use openflow::{control, daemon, logger, paste, pipeline, setup};
use std::time::{Duration, Instant};

const HELP: &str = "OpenFlow - free, local, private voice dictation

Usage: openflow <command>

  setup [--cpu|--gpu] [--cloud] [--no-autostart] [--force]
                    download engines + models, enable start-at-login, start
  start | stop | restart | status
  reload            re-read the config file
  config            open the config file (config path: print its location)
  test [seconds]    record from the mic and show the result (no pasting)
  transcribe <wav>  run a WAV file through the full pipeline
  last              copy your last dictation to the clipboard
  history [n]       show recent dictations
  autostart on|off
  doctor            check that everything is installed and working
  run               run in the foreground with live logs (for debugging)

Use: double-tap the trigger key (Caps Lock) to dictate hands-free, tap again
to finish. Or hold it while talking. Esc cancels.";

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    let rest = if args.len() > 1 { &args[1..] } else { &[] };
    let has = |f: &str| rest.iter().any(|a| a == f);

    match cmd {
        "setup" | "install" => setup::run(setup::Opts {
            cpu: has("--cpu"),
            gpu: has("--gpu"),
            cloud: has("--cloud"),
            no_autostart: has("--no-autostart"),
            force: has("--force"),
        }),
        "run" => daemon::run(true),
        "start" => {
            if control::is_running() {
                println!("OpenFlow is already running.");
                return Ok(());
            }
            setup::start_daemon()?;
            println!("OpenFlow started.");
            Ok(())
        }
        "stop" => {
            match control::send("quit") {
                Ok(_) => println!("OpenFlow stopped."),
                Err(_) => println!("OpenFlow is not running."),
            }
            Ok(())
        }
        "restart" => {
            let _ = control::send("quit");
            for _ in 0..30 {
                if !control::is_running() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            setup::start_daemon()?;
            println!("OpenFlow restarted.");
            Ok(())
        }
        "reload" => {
            Config::load()?; // validate first, with a clear error
            println!("{}", control::send("reload")?);
            Ok(())
        }
        "demo" => {
            println!("{}", control::send("demo")?);
            Ok(())
        }
        "status" => {
            println!("OpenFlow is {}", if control::is_running() { "running" } else { "not running" });
            Ok(())
        }
        "config" => {
            Config::load()?;
            let p = config::config_path();
            if rest.first().map(|s| s.as_str()) == Some("path") {
                println!("{}", p.display());
            } else {
                println!("Opening {}", p.display());
                setup::open_path(&p)?;
                println!("Run `openflow reload` after saving.");
            }
            Ok(())
        }
        "test" => {
            let secs: u64 = rest.first().and_then(|s| s.parse().ok()).unwrap_or(5);
            test_mic(secs)
        }
        "transcribe" => {
            if rest.is_empty() {
                bail!("usage: openflow transcribe <file.wav>...");
            }
            // --talk <ms>: wait this long before processing, like a user speaking
            let mut talk = 0u64;
            let mut files = Vec::new();
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                if a == "--talk" {
                    talk = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                } else {
                    files.push(a);
                }
            }
            let recs = files.iter().map(|p| audio::read_wav(std::path::Path::new(p.as_str()))).collect::<Result<Vec<_>>>()?;
            run_many(recs, talk)
        }
        "last" => match History::last_text() {
            Some(t) => {
                paste::copy(&t)?;
                println!("Copied to clipboard:\n{t}");
                Ok(())
            }
            None => {
                println!("No dictations yet.");
                Ok(())
            }
        },
        "history" => {
            let n = rest.first().and_then(|s| s.parse().ok()).unwrap_or(10);
            for e in History::recent(n) {
                println!("[{}] {} ({} | stt {} | ai {} | {} ms){}", e.time, e.app, e.style, e.stt, e.ai, e.ms,
                    e.error.as_ref().map(|x| format!(" ERROR: {x}")).unwrap_or_default());
                println!("  said:  {}", e.raw);
                println!("  typed: {}\n", e.text.trim_end());
            }
            Ok(())
        }
        "autostart" => {
            let on = match rest.first().map(|s| s.as_str()) {
                Some("on") => true,
                Some("off") => false,
                _ => bail!("usage: openflow autostart on|off"),
            };
            setup::set_autostart(on)?;
            println!("Start at login {}.", if on { "enabled" } else { "disabled" });
            Ok(())
        }
        "doctor" => doctor(),
        "render-preview" => {
            let dir = std::path::PathBuf::from(rest.first().map(|s| s.as_str()).unwrap_or("docs"));
            openflow::ui::render_previews(&dir)?;
            println!("Wrote overlay previews to {}", dir.display());
            Ok(())
        }
        "" | "help" | "-h" | "--help" => {
            println!("{HELP}");
            Ok(())
        }
        "--version" | "-V" | "version" => {
            println!("openflow {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => bail!("unknown command '{other}'. Run `openflow help`."),
    }
}

fn test_mic(secs: u64) -> Result<()> {
    let mut rec = Recorder::new();
    rec.start()?;
    println!("Recording for {secs}s from {:?}... speak now.", Recorder::device_name().unwrap_or_default());
    std::thread::sleep(Duration::from_secs(secs));
    let r = rec.stop();
    println!("Processing...");
    run_pipeline(r, Target::default())
}

fn run_pipeline(rec: audio::Recording, _target: Target) -> Result<()> {
    run_many(vec![rec], 0)
}

/// Process recordings in one session: the first includes engine cold start,
/// the rest show warm (engines already loaded) latency.
fn run_many(recs: Vec<audio::Recording>, talk_ms: u64) -> Result<()> {
    logger::init(true);
    let cfg = Config::load()?;
    let app = App::new(cfg.clone());
    // Start + warm engines exactly like the daemon does when you press the key.
    let hold = app.engines.hold();
    daemon::prewarm(&app, &app.cfg());
    std::thread::sleep(Duration::from_millis(talk_ms));
    let mut result = Ok(());
    for (i, rec) in recs.into_iter().enumerate() {
        let t = Instant::now();
        match pipeline::process(&app, rec, &Target::default()) {
            Ok(Some(o)) => {
                println!("\n  heard: {}", o.raw);
                println!("  typed: {}", o.text.trim_end());
                println!("  ({} style, stt {}, ai {}, {:?}{})", o.style.name(), o.entry.stt, o.entry.ai, t.elapsed(), if i == 0 { " incl. cold start" } else { " warm" });
            }
            Ok(None) => println!("\n  No speech detected."),
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    drop(hold);
    app.engines.shutdown();
    result
}

fn doctor() -> Result<()> {
    let ok = |b: bool| if b { "ok  " } else { "MISS" };
    let cfg = match Config::load() {
        Ok(c) => {
            println!("[ok  ] config      {}", config::config_path().display());
            c
        }
        Err(e) => {
            println!("[FAIL] config      {e:#}");
            return Ok(());
        }
    };
    let key = openflow::keyboard::TriggerKey::parse(&cfg.trigger.key);
    println!("[{}] trigger     {}", ok(key.is_ok()), cfg.trigger.key);
    let mic = Recorder::device_name();
    println!("[{}] microphone  {}", ok(mic.is_some()), mic.unwrap_or_else(|| "none found".into()));
    for k in [Kind::Whisper, Kind::Llama] {
        let e = engines::find_engine(k, &cfg);
        let m = engines::find_model(k, &cfg);
        println!("[{}] {:<11} {}", ok(e.is_some()), format!("{} exe", k.name()), e.map(|p| p.display().to_string()).unwrap_or_else(|| "not installed".into()));
        println!("[{}] {:<11} {}", ok(m.is_some()), format!("{} model", k.name()), m.map(|p| p.display().to_string()).unwrap_or_else(|| "not downloaded".into()));
    }
    println!("[{}] groq key    {}", if cfg.groq_key().is_some() { "ok  " } else { "--  " }, if cfg.groq_key().is_some() { "set (cloud backup on)" } else { "not set (local only)" });
    println!("[{}] daemon      {}", ok(control::is_running()), if control::is_running() { "running" } else { "not running (openflow start)" });
    println!("\nspeech: {} ({}), ai: {} ({}), idle unload: {}s",
        cfg.speech.provider, cfg.speech.language, if cfg.intelligence.enabled { cfg.intelligence.provider.as_str() } else { "off" },
        if cfg.engines.gpu { "gpu" } else { "cpu" }, cfg.engines.idle_unload_seconds);
    println!("logs: {}", config::data_dir().join("logs").display());
    Ok(())
}
