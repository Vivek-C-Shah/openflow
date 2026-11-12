//! Global trigger-key listener. Event-driven (the OS calls us on key events;
//! nothing polls), so idle CPU is zero. Keys always pass through untouched.
//!
//! - Windows: native low-level keyboard hook (can ignore our own injected keys).
//! - macOS / Linux (X11): `rdev`.

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    TriggerDown,
    TriggerUp,
    Esc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKey {
    CapsLock,
    RightAlt,
    RightCtrl,
    RightShift,
    ScrollLock,
    Pause,
    F(u8),
}

impl TriggerKey {
    pub fn parse(s: &str) -> Result<TriggerKey> {
        let k = s.trim().to_ascii_lowercase().replace([' ', '_', '-'], "");
        Ok(match k.as_str() {
            "capslock" | "caps" => TriggerKey::CapsLock,
            "rightalt" | "altgr" | "rightoption" | "ralt" => TriggerKey::RightAlt,
            "rightctrl" | "rightcontrol" | "rctrl" => TriggerKey::RightCtrl,
            "rightshift" | "rshift" => TriggerKey::RightShift,
            "scrolllock" => TriggerKey::ScrollLock,
            "pause" => TriggerKey::Pause,
            f if f.starts_with('f') => match f[1..].parse::<u8>() {
                Ok(n) if (13..=24).contains(&n) => TriggerKey::F(n),
                _ => bail!("unknown trigger key '{s}'"),
            },
            _ => bail!("unknown trigger key '{s}'"),
        })
    }

    /// Keys whose press flips OS state, which we undo after a session.
    pub fn is_toggle(self) -> bool {
        matches!(self, TriggerKey::CapsLock | TriggerKey::ScrollLock)
    }
}

pub type Callback = Box<dyn Fn(KeyEvent) + Send + Sync + 'static>;

// ───────────────────────────── Windows ─────────────────────────────
#[cfg(windows)]
mod imp {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    static CALLBACK: OnceLock<Callback> = OnceLock::new();
    static VK: AtomicU32 = AtomicU32::new(0x14);
    const VK_ESCAPE: u32 = 0x1B;

    pub fn vk(key: TriggerKey) -> u32 {
        match key {
            TriggerKey::CapsLock => 0x14,
            TriggerKey::RightAlt => 0xA5,
            TriggerKey::RightCtrl => 0xA3,
            TriggerKey::RightShift => 0xA1,
            TriggerKey::ScrollLock => 0x91,
            TriggerKey::Pause => 0x13,
            TriggerKey::F(n) => 0x7C + (n as u32 - 13),
        }
    }

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
            // Ignore keys we inject ourselves (paste, Caps Lock restore).
            if kb.flags & LLKHF_INJECTED == 0 {
                let msg = wparam as u32;
                let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
                let up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
                if let Some(cb) = CALLBACK.get() {
                    if kb.vkCode == VK.load(Ordering::Relaxed) {
                        if down {
                            cb(KeyEvent::TriggerDown);
                        } else if up {
                            cb(KeyEvent::TriggerUp);
                        }
                    } else if kb.vkCode == VK_ESCAPE && down {
                        cb(KeyEvent::Esc);
                    }
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    pub fn set_key(key: TriggerKey) {
        VK.store(vk(key), Ordering::Relaxed);
    }

    /// Blocks forever running the hook's message loop.
    pub fn listen(key: TriggerKey, cb: Callback) -> Result<()> {
        set_key(key);
        if CALLBACK.set(cb).is_err() {
            bail!("keyboard listener already running");
        }
        unsafe {
            let h = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), GetModuleHandleW(std::ptr::null()), 0);
            if h.is_null() {
                bail!("could not install keyboard hook");
            }
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }

    pub fn note_synthetic_toggle() {}
}

// ─────────────────────────── macOS / Linux ──────────────────────────
#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::Mutex;

    static KEY: Mutex<Option<TriggerKey>> = Mutex::new(None);
    /// Synthetic toggle presses/releases still to be ignored.
    static IGNORE: AtomicI32 = AtomicI32::new(0);

    fn matches(k: &rdev::Key, key: TriggerKey) -> bool {
        use rdev::Key::*;
        matches!(
            (k, key),
            (CapsLock, TriggerKey::CapsLock)
                | (AltGr, TriggerKey::RightAlt)
                | (ControlRight, TriggerKey::RightCtrl)
                | (ShiftRight, TriggerKey::RightShift)
                | (ScrollLock, TriggerKey::ScrollLock)
                | (Pause, TriggerKey::Pause)
        )
    }

    pub fn set_key(key: TriggerKey) {
        *KEY.lock().unwrap() = Some(key);
    }

    pub fn listen(key: TriggerKey, cb: Callback) -> Result<()> {
        if let TriggerKey::F(_) = key {
            bail!("F13-F24 trigger keys are only supported on Windows");
        }
        set_key(key);
        rdev::listen(move |ev| {
            let key = match *KEY.lock().unwrap() {
                Some(k) => k,
                None => return,
            };
            match ev.event_type {
                rdev::EventType::KeyPress(k) | rdev::EventType::KeyRelease(k) if matches(&k, key) => {
                    if IGNORE.load(Ordering::SeqCst) > 0 {
                        IGNORE.fetch_sub(1, Ordering::SeqCst);
                        return;
                    }
                    if matches!(ev.event_type, rdev::EventType::KeyPress(_)) {
                        cb(KeyEvent::TriggerDown)
                    } else {
                        cb(KeyEvent::TriggerUp)
                    }
                }
                rdev::EventType::KeyPress(rdev::Key::Escape) => cb(KeyEvent::Esc),
                _ => {}
            }
        })
        .map_err(|e| anyhow::anyhow!("keyboard listener failed: {e:?} (macOS: grant Accessibility permission; Linux: X11 required)"))
    }

    pub fn note_synthetic_toggle() {
        IGNORE.fetch_add(2, Ordering::SeqCst);
    }
}

pub use imp::{listen, set_key};

/// Tap the trigger key once (used to undo a Caps Lock flip).
pub fn tap_toggle(key: TriggerKey) {
    use enigo::{Direction, Enigo, Keyboard, Settings};
    let k = match key {
        TriggerKey::CapsLock => enigo::Key::CapsLock,
        #[cfg(windows)]
        TriggerKey::ScrollLock => enigo::Key::Other(0x91),
        _ => return,
    };
    imp::note_synthetic_toggle();
    match Enigo::new(&Settings::default()) {
        Ok(mut e) => {
            if let Err(err) = e.key(k, Direction::Click) {
                log::warn!("could not restore {key:?}: {err}");
            }
        }
        Err(err) => log::warn!("could not restore {key:?}: {err}"),
    }
}
