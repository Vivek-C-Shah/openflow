//! Configuration. The commented template below is the single source of truth
//! for defaults: the user's file is deep-merged on top of it, so any key the
//! user deletes simply falls back to the default.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
const DEFAULT_KEY: &str = "RightAlt";
#[cfg(not(target_os = "macos"))]
const DEFAULT_KEY: &str = "CapsLock";

pub const TEMPLATE: &str = r#"# OpenFlow configuration.
# Edit this file, then run `openflow reload` (or restart OpenFlow).
# Anything you delete falls back to its default.

[trigger]
# Key that triggers dictation:
#   "CapsLock", "RightAlt" (Right Option on Mac), "RightCtrl", "RightShift",
#   "ScrollLock", "Pause", "F13" ... "F24" (F13+ Windows only)
# Double-tap = hands-free (tap again to finish). Hold = push-to-talk.
key = "__DEFAULT_KEY__"
double_tap_ms = 400   # max gap between the two taps for hands-free mode
hold_ms = 300         # hold the key at least this long for push-to-talk
max_seconds = 360     # safety stop for very long dictations
esc_cancels = true    # press Esc while recording to throw it away

[speech]
provider = "local"    # "local" (offline, private) or "cloud" (Groq; needs api_key)
language = "auto"     # "auto", "en", "hi", or any Whisper language code
script = "latin"      # "latin" = Hinglish in English letters, "native" = keep Devanagari
model = ""            # path to a whisper ggml model; empty = auto-detect
beam_size = 5         # 1 = fastest (greedy); 5 = more accurate (~+0.2s)

[intelligence]
enabled = true        # AI cleanup: fillers, self-corrections, punctuation, lists, tone
provider = "local"    # "local" or "cloud"
model = ""            # path to a .gguf model; empty = auto-detect
timeout_ms = 5000     # fall back to rule-based cleanup if the AI takes longer than this

[groq]
# Optional, free: https://console.groq.com/keys  (or set the GROQ_API_KEY env var)
# With a key, cloud is used as an automatic backup if local fails (and vice versa).
api_key = ""
stt_model = "whisper-large-v3-turbo"
llm_model = "llama-3.3-70b-versatile"

[engines]
idle_unload_seconds = 30  # free ALL AI memory this long after the last dictation (0 = immediately)
gpu = true
threads = 0               # 0 = auto
whisper_server = ""       # custom path to whisper-server; empty = auto
llama_server = ""         # custom path to llama-server; empty = auto

[output]
overlay = true           # floating pill at the bottom of the screen while dictating
idle_bar = true          # keep a tiny bar visible when idle (static image, no CPU)
tray = true              # notification-area icon with menu
sounds = true
append_space = true       # add a space after prose so consecutive dictations join nicely
restore_clipboard = true  # put your previous clipboard back after pasting
restore_delay_ms = 400
history_keep = 200        # dictations kept locally (0 = keep no history)

[dictionary]
# Words the AI must spell exactly: names, jargon, product names.
words = []
# words = ["OpenFlow", "Kubernetes", "Supabase"]

[snippets]
# Say the phrase, get the text.
# "my email" = "me@example.com"
# "my address" = "221B Baker Street, London"

[apps]
# Force a writing style for an app name or window title (substring match).
# Styles: "default", "casual", "email", "code"
# "notion" = "default"
# "linear" = "casual"
"#;

pub fn template() -> String {
    TEMPLATE.replace("__DEFAULT_KEY__", DEFAULT_KEY)
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub trigger: Trigger,
    pub speech: Speech,
    pub intelligence: Intelligence,
    pub groq: Groq,
    pub engines: Engines,
    pub output: Output,
    pub dictionary: Dictionary,
    pub snippets: BTreeMap<String, String>,
    pub apps: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Trigger {
    pub key: String,
    pub double_tap_ms: u64,
    pub hold_ms: u64,
    pub max_seconds: u64,
    pub esc_cancels: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Speech {
    pub provider: String,
    pub language: String,
    pub script: String,
    pub model: String,
    pub beam_size: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Intelligence {
    pub enabled: bool,
    pub provider: String,
    pub model: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Groq {
    pub api_key: String,
    pub stt_model: String,
    pub llm_model: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Engines {
    pub idle_unload_seconds: u64,
    pub gpu: bool,
    pub threads: usize,
    pub whisper_server: String,
    pub llama_server: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Output {
    pub overlay: bool,
    pub idle_bar: bool,
    pub tray: bool,
    pub sounds: bool,
    pub append_space: bool,
    pub restore_clipboard: bool,
    pub restore_delay_ms: u64,
    pub history_keep: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Dictionary {
    pub words: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        toml::from_str(&template()).expect("built-in config template must parse")
    }
}

impl Config {
    /// Load the user's config (creating it from the template if missing).
    pub fn load() -> Result<Config> {
        let path = config_path();
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, template())?;
        }
        let user = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::from_user_str(&user).with_context(|| format!("invalid config file {}", path.display()))
    }

    pub fn from_user_str(user: &str) -> Result<Config> {
        let mut base: toml::Value = toml::from_str(&template())?;
        let over: toml::Value = toml::from_str(user)?;
        merge(&mut base, over);
        Ok(base.try_into()?)
    }

    pub fn groq_key(&self) -> Option<String> {
        let k = self.groq.api_key.trim();
        if !k.is_empty() {
            return Some(k.to_string());
        }
        std::env::var("GROQ_API_KEY").ok().filter(|k| !k.trim().is_empty())
    }

    pub fn threads(&self) -> usize {
        if self.engines.threads > 0 {
            return self.engines.threads;
        }
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8)
    }
}

fn merge(base: &mut toml::Value, over: toml::Value) {
    match (base, over) {
        (toml::Value::Table(b), toml::Value::Table(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(bv) if bv.is_table() && v.is_table() => merge(bv, v),
                    _ => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("OPENFLOW_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("openflow").join("config.toml")
}

/// Where engines, models, history and logs live.
pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("OPENFLOW_HOME") {
        return PathBuf::from(p);
    }
    dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("openflow")
}

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

pub fn engines_dir() -> PathBuf {
    data_dir().join("engines")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses() {
        let c = Config::default();
        assert_eq!(c.trigger.double_tap_ms, 400);
        assert!(c.snippets.is_empty());
    }

    #[test]
    fn user_overrides_merge_onto_defaults() {
        let c = Config::from_user_str(
            "[trigger]\nhold_ms = 500\n[snippets]\n\"my email\" = \"a@b.c\"\n",
        )
        .unwrap();
        assert_eq!(c.trigger.hold_ms, 500);
        assert_eq!(c.trigger.double_tap_ms, 400); // untouched default
        assert_eq!(c.snippets["my email"], "a@b.c");
        assert!(c.intelligence.enabled);
    }
}
