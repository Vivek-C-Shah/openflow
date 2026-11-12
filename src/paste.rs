//! Insert text into the focused app: clipboard + paste shortcut (fast and
//! works everywhere), then restore whatever was on the clipboard before.

use crate::config::Config;
use crate::context::{self, Target};
use anyhow::{Context, Result};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::sync::Mutex;
use std::time::Duration;

static LOCK: Mutex<()> = Mutex::new(());

enum Saved {
    Text(String),
    Image(arboard::ImageData<'static>),
    Nothing,
}

pub fn insert(text: &str, target: &Target, cfg: &Config) -> Result<()> {
    let _g = LOCK.lock().unwrap();
    let mut cb = arboard::Clipboard::new().context("clipboard unavailable")?;
    let saved = if cfg.output.restore_clipboard {
        if let Ok(t) = cb.get_text() {
            Saved::Text(t)
        } else if let Ok(i) = cb.get_image() {
            Saved::Image(i.to_owned_img())
        } else {
            Saved::Nothing
        }
    } else {
        Saved::Nothing
    };

    set_text(&mut cb, text)?;
    std::thread::sleep(Duration::from_millis(30));
    send_paste(target)?;

    if cfg.output.restore_clipboard && !matches!(saved, Saved::Nothing) {
        std::thread::sleep(Duration::from_millis(cfg.output.restore_delay_ms));
        // Only restore if nobody copied something else in the meantime.
        if cb.get_text().ok().as_deref() == Some(text) {
            match saved {
                Saved::Text(t) => {
                    let _ = set_text(&mut cb, &t);
                }
                Saved::Image(i) => {
                    let _ = cb.set_image(i);
                }
                Saved::Nothing => {}
            }
        }
    }
    Ok(())
}

/// Put text on the clipboard (on Windows, hidden from Win+V history).
pub fn set_text(cb: &mut arboard::Clipboard, text: &str) -> Result<()> {
    #[cfg(windows)]
    {
        use arboard::SetExtWindows;
        cb.set().exclude_from_history().text(text.to_string())?;
    }
    #[cfg(not(windows))]
    cb.set_text(text.to_string())?;
    Ok(())
}

fn send_paste(target: &Target) -> Result<()> {
    let mut e = Enigo::new(&Settings::default()).context("cannot send keystrokes (macOS: grant Accessibility)")?;
    #[cfg(target_os = "macos")]
    let (modifier, v) = (Key::Meta, Key::Other(9)); // kVK_ANSI_V
    #[cfg(windows)]
    let (modifier, v) = (Key::Control, Key::Other(0x56)); // VK_V
    #[cfg(all(unix, not(target_os = "macos")))]
    let (modifier, v) = (Key::Control, Key::Unicode('v'));

    let shift = cfg!(all(unix, not(target_os = "macos"))) && context::is_terminal(target);
    e.key(modifier, Direction::Press)?;
    if shift {
        e.key(Key::Shift, Direction::Press)?;
    }
    let r = e.key(v, Direction::Click);
    if shift {
        let _ = e.key(Key::Shift, Direction::Release);
    }
    e.key(modifier, Direction::Release)?;
    r?;
    Ok(())
}

/// Copy text to the clipboard without pasting (used by `openflow last`).
pub fn copy(text: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new()?;
    set_text(&mut cb, text)
}
