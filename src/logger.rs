//! Tiny logger: stderr (when attached to a terminal) + a size-capped log file.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

struct Logger {
    file: Mutex<Option<File>>,
    console: bool,
    level: log::LevelFilter,
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= self.level && (m.target().starts_with("openflow") || m.level() <= log::Level::Warn)
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let line = format!("{} {:<5} {}\n", chrono::Local::now().format("%H:%M:%S%.3f"), r.level(), r.args());
        if self.console {
            let _ = std::io::stderr().write_all(line.as_bytes());
        }
        if let Ok(mut f) = self.file.lock() {
            if let Some(f) = f.as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }
    fn flush(&self) {}
}

pub fn init(console: bool) {
    let dir = crate::config::data_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("openflow.log");
    if std::fs::metadata(&path).map(|m| m.len() > 5_000_000).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("openflow.old.log"));
    }
    let file = OpenOptions::new().create(true).append(true).open(&path).ok();
    let level = if std::env::var("OPENFLOW_DEBUG").is_ok() { log::LevelFilter::Debug } else { log::LevelFilter::Info };
    let logger = Box::leak(Box::new(Logger { file: Mutex::new(file), console, level }));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(level);
    }
}
