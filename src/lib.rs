//! OpenFlow: free, local, private voice dictation.
//!
//! Idle footprint is a keyboard hook and nothing else. Speech and AI engines
//! are separate processes that start the moment you trigger dictation (so they
//! load while you talk) and are killed after an idle period.

pub mod app;
pub mod audio;
pub mod config;
pub mod context;
pub mod control;
pub mod daemon;
pub mod engines;
pub mod history;
pub mod intelligence;
pub mod keyboard;
pub mod logger;
pub mod paste;
pub mod pipeline;
pub mod setup;
pub mod stt;
pub mod text;
pub mod trigger;
pub mod ui;
