//! Windows overlay (per-pixel-alpha layered window) + tray icon.
//! Everything here runs on the keyboard-hook thread's message loop.

use super::render::{self, Look, BARS};
use super::{Command, CommandSink, Options, Status};
use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_STATE: u32 = WM_APP + 1;
const WM_TRAY: u32 = WM_APP + 2;
const WM_SHUTDOWN: u32 = WM_APP + 3;
const TIMER_ID: usize = 1;
const TRAY_ID: u32 = 1;

static HWND_UI: AtomicIsize = AtomicIsize::new(0);
static SINK: OnceLock<CommandSink> = OnceLock::new();
static SHARED: Mutex<Shared> = Mutex::new(Shared { status: Status::Idle, opts: None });

struct Shared {
    status: Status,
    opts: Option<Options>,
}

struct Ui {
    hwnd: HWND,
    opts: Options,
    status: Status,
    look: Look,
    started: Instant,
    last_tick: Instant,
    error_until: Option<Instant>,
    timer_on: bool,
    visible: bool,
    scale: f32,
    origin: (i32, i32),
    level: f32,
    bar_phase: [f32; BARS],
    icon_idle: HICON,
    icon_rec: HICON,
    taskbar_created: u32,
    tray_added: bool,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn send(cmd: Command) {
    if let Some(s) = SINK.get() {
        s(cmd);
    }
}

pub fn start(opts: Options, sink: CommandSink) {
    let _ = SINK.set(sink);
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide("OpenFlowOverlay");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_HAND),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let title = wide("OpenFlow");
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            log::warn!("could not create overlay window");
            return;
        }
        HWND_UI.store(hwnd as isize, Ordering::SeqCst);
        let mut ui = Ui {
            hwnd,
            opts,
            status: Status::Idle,
            look: Look::idle(opts.overlay && opts.idle_bar),
            started: Instant::now(),
            last_tick: Instant::now(),
            error_until: None,
            timer_on: false,
            visible: false,
            scale: 1.0,
            origin: (0, 0),
            level: 0.0,
            bar_phase: [0.0, 1.7, 0.6, 2.4, 1.1, 2.9, 0.3, 2.0, 1.4],
            icon_idle: make_icon(false),
            icon_rec: make_icon(true),
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            tray_added: false,
        };
        ui.place();
        ui.present();
        if opts.tray {
            ui.tray(NIM_ADD);
        }
        UI.with(|u| *u.borrow_mut() = Some(ui));
    }
}

fn post(msg: u32) {
    let h = HWND_UI.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            PostMessageW(h as HWND, msg, 0, 0);
        }
    }
}

pub fn set_status(s: Status) {
    SHARED.lock().unwrap().status = s;
    post(WM_STATE);
}

pub fn set_options(o: Options) {
    SHARED.lock().unwrap().opts = Some(o);
    post(WM_STATE);
}

pub fn shutdown() {
    let h = HWND_UI.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            SendMessageW(h as HWND, WM_SHUTDOWN, 0, 0);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let handled = UI.with(|u| {
        let mut b = u.try_borrow_mut().ok()?;
        let ui = b.as_mut()?;
        match msg {
            WM_STATE => {
                ui.sync();
                Some(0)
            }
            WM_TIMER => {
                ui.tick();
                Some(0)
            }
            WM_MOUSEACTIVATE => Some(MA_NOACTIVATE as LRESULT),
            WM_LBUTTONUP => {
                let x = (lp & 0xffff) as i16 as f32 / ui.scale;
                let y = ((lp >> 16) & 0xffff) as i16 as f32 / ui.scale;
                ui.click(x, y);
                Some(0)
            }
            WM_TRAY => {
                let event = (lp & 0xffff) as u32;
                if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
                    drop(b);
                    show_menu(hwnd);
                }
                Some(0)
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
                ui.place();
                ui.present();
                Some(0)
            }
            WM_SHUTDOWN => {
                ui.tray(NIM_DELETE);
                Some(0)
            }
            m if m == ui.taskbar_created && m != 0 => {
                ui.tray_added = false; // explorer restarted
                if ui.opts.tray {
                    ui.tray(NIM_ADD);
                }
                Some(0)
            }
            _ => None,
        }
    });
    match handled {
        Some(r) => r,
        None => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

impl Ui {
    /// Apply status/options changed from other threads.
    fn sync(&mut self) {
        let (status, opts) = {
            let mut s = SHARED.lock().unwrap();
            (s.status, s.opts.take())
        };
        if let Some(o) = opts {
            if o.tray != self.opts.tray {
                self.tray(if o.tray { NIM_ADD } else { NIM_DELETE });
            }
            self.opts = o;
        }
        if status != self.status {
            let was_idle = self.status == Status::Idle;
            self.status = status;
            self.error_until = (status == Status::Error).then(|| Instant::now() + Duration::from_millis(1400));
            if was_idle && status != Status::Idle {
                self.place(); // follow the monitor you're working on
            }
            if self.opts.tray {
                self.tray(NIM_MODIFY);
            }
        }
        self.start_timer();
    }

    fn target(&self) -> (f32, f32, f32, f32, f32, f32, f32) {
        // width, height, opacity, expand, controls, processing, error
        let show = self.opts.overlay;
        match self.status {
            Status::Idle => (44.0, 8.0, if show && self.opts.idle_bar { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0, 0.0),
            Status::Listening { hands_free: true } => (156.0, 34.0, show as u8 as f32, 1.0, 1.0, 0.0, 0.0),
            Status::Listening { hands_free: false } => (104.0, 34.0, show as u8 as f32, 1.0, 0.0, 0.0, 0.0),
            Status::Processing => (104.0, 34.0, show as u8 as f32, 1.0, 0.0, 1.0, 0.0),
            Status::Error => (104.0, 34.0, show as u8 as f32, 1.0, 0.0, 1.0, 1.0),
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_tick).as_secs_f32().min(0.05);
        self.last_tick = now;
        if let Some(t) = self.error_until {
            if now >= t {
                self.error_until = None;
                self.status = Status::Idle;
                SHARED.lock().unwrap().status = Status::Idle;
                if self.opts.tray {
                    self.tray(NIM_MODIFY);
                }
            }
        }
        let (w, h, op, ex, ctl, pr, er) = self.target();
        let k = 1.0 - (-dt * 16.0).exp(); // spring-ish ease
        let l = &mut self.look;
        let mut settled = true;
        for (v, t) in [
            (&mut l.width, w),
            (&mut l.height, h),
            (&mut l.opacity, op),
            (&mut l.expand, ex),
            (&mut l.controls, ctl),
            (&mut l.processing, pr),
            (&mut l.error, er),
        ] {
            *v += (t - *v) * k;
            if (t - *v).abs() > 0.004 {
                settled = false;
            } else {
                *v = t;
            }
        }
        l.time = (now - self.started).as_secs_f32();

        // live voice bars: fast attack, slower release, per-bar wobble
        let listening = matches!(self.status, Status::Listening { .. });
        let raw = if listening { (super::level() * 9.0).sqrt().min(1.0) } else { 0.0 };
        let rate = if raw > self.level { 0.6 } else { 0.18 };
        self.level += (raw - self.level) * rate;
        for i in 0..BARS {
            let center = 1.0 - ((i as f32 - (BARS as f32 - 1.0) / 2.0).abs() / (BARS as f32 / 2.0)) * 0.55;
            let wobble = 0.65 + 0.35 * (l.time * (5.0 + i as f32 * 0.9) + self.bar_phase[i]).sin();
            let t = self.level * center * wobble;
            l.bars[i] += (t - l.bars[i]) * 0.5;
            if l.bars[i] > 0.01 {
                settled = false;
            }
        }
        self.present();
        let animating = !settled || self.status != Status::Idle || self.error_until.is_some();
        if !animating {
            self.stop_timer();
        }
    }

    fn start_timer(&mut self) {
        if !self.timer_on {
            self.last_tick = Instant::now();
            unsafe {
                SetTimer(self.hwnd, TIMER_ID, 16, None);
            }
            self.timer_on = true;
        }
    }

    fn stop_timer(&mut self) {
        if self.timer_on {
            unsafe {
                KillTimer(self.hwnd, TIMER_ID);
            }
            self.timer_on = false;
        }
    }

    fn click(&mut self, x: f32, y: f32) {
        if self.look.controls < 0.5 {
            return;
        }
        let g = render::layout(&self.look);
        let hit = |c: (f32, f32)| ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt() <= g.button_r + 3.0;
        if hit(g.stop) {
            send(Command::Stop);
        } else if hit(g.cancel) {
            send(Command::Cancel);
        }
    }

    /// Bottom-center of the monitor you're using, above the taskbar.
    fn place(&mut self) {
        unsafe {
            let fg = GetForegroundWindow();
            let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTOPRIMARY);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            GetMonitorInfoW(mon, &mut mi);
            let (mut dx, mut dy) = (96u32, 96u32);
            GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            self.scale = dx as f32 / 96.0;
            let w = (render::CANVAS_W * self.scale).ceil() as i32;
            let h = (render::CANVAS_H * self.scale).ceil() as i32;
            let work = mi.rcWork;
            self.origin = (work.left + (work.right - work.left - w) / 2, work.bottom - h - (6.0 * self.scale) as i32);
        }
    }

    fn present(&mut self) {
        let visible = self.look.opacity > 0.003;
        unsafe {
            if !visible {
                if self.visible {
                    ShowWindow(self.hwnd, SW_HIDE);
                    self.visible = false;
                }
                return;
            }
            let canvas = render::draw_overlay(&self.look, self.scale);
            let (w, h) = (canvas.w as i32, canvas.h as i32);
            let pixels = canvas.to_bgra_premultiplied();
            let screen = GetDC(std::ptr::null_mut());
            let mem = CreateCompatibleDC(screen);
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h; // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if !dib.is_null() && !bits.is_null() {
                std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
                let old = SelectObject(mem, dib);
                let dst = POINT { x: self.origin.0, y: self.origin.1 };
                let size = SIZE { cx: w, cy: h };
                let src = POINT { x: 0, y: 0 };
                let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
                UpdateLayeredWindow(self.hwnd, screen, &dst, &size, mem, &src, 0, &blend, ULW_ALPHA);
                SelectObject(mem, old);
                DeleteObject(dib);
            }
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            if !self.visible {
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                self.visible = true;
            }
        }
    }

    fn tray(&mut self, op: u32) {
        unsafe {
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = self.hwnd;
            nid.uID = TRAY_ID;
            nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
            nid.uCallbackMessage = WM_TRAY;
            let recording = matches!(self.status, Status::Listening { .. });
            nid.hIcon = if recording { self.icon_rec } else { self.icon_idle };
            let tip = match self.status {
                Status::Idle => "OpenFlow - double-tap Caps Lock to dictate",
                Status::Listening { .. } => "OpenFlow - listening...",
                Status::Processing => "OpenFlow - writing...",
                Status::Error => "OpenFlow - something went wrong (see logs)",
            };
            for (i, c) in tip.encode_utf16().take(127).enumerate() {
                nid.szTip[i] = c;
            }
            match op {
                NIM_ADD if !self.tray_added => {
                    self.tray_added = Shell_NotifyIconW(NIM_ADD, &nid) != 0;
                }
                NIM_MODIFY if self.tray_added => {
                    Shell_NotifyIconW(NIM_MODIFY, &nid);
                }
                NIM_DELETE if self.tray_added => {
                    Shell_NotifyIconW(NIM_DELETE, &nid);
                    self.tray_added = false;
                }
                _ => {}
            }
        }
    }
}

fn make_icon(recording: bool) -> HICON {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON).max(16) } as usize * 2; // crisp at 200%
    let canvas = render::draw_icon(size, recording);
    let px = canvas.to_bgra_premultiplied();
    unsafe {
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = size as i32;
        bmi.bmiHeader.biHeight = -(size as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let screen = GetDC(std::ptr::null_mut());
        let color = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        ReleaseDC(std::ptr::null_mut(), screen);
        if color.is_null() || bits.is_null() {
            return std::ptr::null_mut();
        }
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, std::ptr::null());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(mask);
        DeleteObject(color);
        icon
    }
}

const ID_STATUS: usize = 1;
const ID_COPY_LAST: usize = 2;
const ID_HISTORY: usize = 3;
const ID_SETTINGS: usize = 4;
const ID_RELOAD: usize = 5;
const ID_AUTOSTART: usize = 6;
const ID_IDLE_BAR: usize = 7;
const ID_LOGS: usize = 8;
const ID_QUIT: usize = 9;

fn show_menu(hwnd: HWND) {
    let (status, idle_bar) = UI.with(|u| u.borrow().as_ref().map(|u| (u.status, u.opts.idle_bar)).unwrap_or((Status::Idle, true)));
    let autostart = crate::setup::autostart_enabled();
    let key = crate::config::Config::load().map(|c| c.trigger.key).unwrap_or_else(|_| "Caps Lock".into());
    let headline = match status {
        Status::Idle => format!("OpenFlow  -  double-tap {key} to dictate"),
        Status::Listening { .. } => "OpenFlow  -  listening...".into(),
        Status::Processing => "OpenFlow  -  writing...".into(),
        Status::Error => "OpenFlow  -  last dictation failed".into(),
    };
    unsafe {
        let m = CreatePopupMenu();
        let add = |flags: u32, id: usize, text: &str| {
            let w = wide(text);
            AppendMenuW(m, flags, id, w.as_ptr());
        };
        add(MF_STRING | MF_GRAYED, ID_STATUS, &headline);
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        add(MF_STRING, ID_COPY_LAST, "Copy last dictation");
        add(MF_STRING, ID_HISTORY, "History...");
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        add(MF_STRING, ID_SETTINGS, "Settings...");
        add(MF_STRING, ID_RELOAD, "Reload settings");
        add(MF_STRING | if autostart { MF_CHECKED } else { 0 }, ID_AUTOSTART, "Start at login");
        add(MF_STRING | if idle_bar { MF_CHECKED } else { 0 }, ID_IDLE_BAR, "Show bar when idle");
        add(MF_STRING, ID_LOGS, "Open logs folder");
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        add(MF_STRING, ID_QUIT, "Quit OpenFlow");

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd); // required so the menu closes when clicking elsewhere
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, std::ptr::null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(m);
        let c = match cmd as usize {
            ID_COPY_LAST => Some(Command::CopyLast),
            ID_HISTORY => Some(Command::OpenHistory),
            ID_SETTINGS => Some(Command::OpenSettings),
            ID_RELOAD => Some(Command::Reload),
            ID_AUTOSTART => Some(Command::ToggleAutostart),
            ID_IDLE_BAR => Some(Command::ToggleIdleBar),
            ID_LOGS => Some(Command::OpenLogs),
            ID_QUIT => Some(Command::Quit),
            _ => None,
        };
        if let Some(c) = c {
            send(c);
        }
    }
}
