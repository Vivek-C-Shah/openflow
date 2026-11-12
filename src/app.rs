//! Shared application state.

use crate::config::Config;
use crate::engines::Engines;
use crate::history::History;
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub struct App {
    cfg: RwLock<Arc<Config>>,
    pub engines: Arc<Engines>,
    pub history: History,
}

impl App {
    pub fn new(cfg: Config) -> Arc<App> {
        let engines = Engines::new(Duration::from_secs(cfg.engines.idle_unload_seconds));
        let history = History::new(cfg.output.history_keep);
        Arc::new(App { cfg: RwLock::new(Arc::new(cfg)), engines, history })
    }

    pub fn cfg(&self) -> Arc<Config> {
        self.cfg.read().unwrap().clone()
    }

    pub fn set_cfg(&self, cfg: Config) {
        self.engines.set_idle(Duration::from_secs(cfg.engines.idle_unload_seconds));
        self.history.set_keep(cfg.output.history_keep);
        *self.cfg.write().unwrap() = Arc::new(cfg);
    }
}

/// Which backend to try, in order. A configured Groq key gives automatic
/// cross-fallback (local -> cloud, or cloud -> local).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Local,
    Cloud,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::Local => "local",
            Provider::Cloud => "cloud",
        }
    }
}

pub fn provider_order(preferred: &str, has_key: bool) -> Vec<Provider> {
    match (preferred.trim().eq_ignore_ascii_case("cloud"), has_key) {
        (true, true) => vec![Provider::Cloud, Provider::Local],
        (false, true) => vec![Provider::Local, Provider::Cloud],
        (_, false) => vec![Provider::Local],
    }
}
