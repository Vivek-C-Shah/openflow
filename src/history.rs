//! Local history: nothing you say is ever lost, even if pasting fails.

use crate::config;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Entry {
    pub time: String,
    pub app: String,
    pub style: String,
    pub raw: String,
    pub text: String,
    pub stt: String,
    pub ai: String,
    pub ms: u64,
    pub audio: Option<String>,
    pub pasted: bool,
    pub error: Option<String>,
}

pub struct History {
    keep: AtomicUsize,
    lock: Mutex<()>,
}

fn dir() -> PathBuf {
    config::data_dir().join("history")
}
fn file() -> PathBuf {
    dir().join("history.jsonl")
}

impl History {
    pub fn new(keep: usize) -> History {
        History { keep: AtomicUsize::new(keep), lock: Mutex::new(()) }
    }

    pub fn set_keep(&self, keep: usize) {
        self.keep.store(keep, Ordering::Relaxed);
    }

    fn keep(&self) -> usize {
        self.keep.load(Ordering::Relaxed)
    }

    /// Save the audio first, so even a crash mid-processing loses nothing.
    pub fn save_audio(&self, wav: &[u8]) -> Option<String> {
        if self.keep() == 0 {
            return None;
        }
        let d = dir().join("audio");
        std::fs::create_dir_all(&d).ok()?;
        let name = format!("{}.wav", chrono::Local::now().format("%Y%m%d-%H%M%S%.3f"));
        let p = d.join(&name);
        std::fs::write(&p, wav).ok()?;
        self.prune_audio(&d);
        Some(p.to_string_lossy().to_string())
    }

    fn prune_audio(&self, d: &std::path::Path) {
        let mut files: Vec<PathBuf> = match std::fs::read_dir(d) {
            Ok(r) => r.flatten().map(|e| e.path()).collect(),
            Err(_) => return,
        };
        let keep = self.keep().min(50); // audio is big; keep fewer
        if files.len() > keep {
            files.sort();
            for f in &files[..files.len() - keep] {
                let _ = std::fs::remove_file(f);
            }
        }
    }

    pub fn append(&self, e: &Entry) {
        if self.keep() == 0 {
            return;
        }
        let _g = self.lock.lock().unwrap();
        let _ = std::fs::create_dir_all(dir());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file()) {
            if let Ok(line) = serde_json::to_string(e) {
                let _ = writeln!(f, "{line}");
            }
        }
        // compact occasionally
        if let Ok(s) = std::fs::read_to_string(file()) {
            let lines: Vec<&str> = s.lines().collect();
            if lines.len() > self.keep() * 2 {
                let kept = lines[lines.len() - self.keep()..].join("\n") + "\n";
                let _ = std::fs::write(file(), kept);
            }
        }
    }

    pub fn recent(n: usize) -> Vec<Entry> {
        let s = std::fs::read_to_string(file()).unwrap_or_default();
        let mut v: Vec<Entry> = s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let start = v.len().saturating_sub(n);
        v.drain(..start);
        v
    }

    pub fn last_text() -> Option<String> {
        Self::recent(20).into_iter().rev().find(|e| !e.text.is_empty()).map(|e| e.text)
    }
}
