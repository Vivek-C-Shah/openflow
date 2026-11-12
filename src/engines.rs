//! On-demand AI engines. whisper-server and llama-server run as child
//! processes that exist only around dictations: started the instant you
//! trigger (so they load while you talk) and killed `idle_unload_seconds`
//! after the last use, which frees 100% of their RAM/VRAM.

use crate::config::{self, Config};
use anyhow::{bail, Context, Result};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Whisper,
    Llama,
}

impl Kind {
    fn idx(self) -> usize {
        self as usize
    }
    pub fn name(self) -> &'static str {
        match self {
            Kind::Whisper => "whisper",
            Kind::Llama => "llama",
        }
    }
    fn exe(self) -> &'static str {
        match self {
            Kind::Whisper => "whisper-server",
            Kind::Llama => "llama-server",
        }
    }
}

struct Proc {
    child: Child,
    port: u16,
    ready: bool,
    warmed: bool,
    log: PathBuf,
}

struct Inner {
    procs: [Option<Proc>; 2],
    last_used: Instant,
    busy: usize,
    idle: Duration,
}

pub struct Engines {
    inner: Mutex<Inner>,
    cv: Condvar,
}

/// While alive, engines are never unloaded. Hold one across a whole session.
pub struct Hold(Arc<Engines>);

impl Drop for Hold {
    fn drop(&mut self) {
        let mut g = self.0.inner.lock().unwrap();
        g.busy = g.busy.saturating_sub(1);
        g.last_used = Instant::now();
        self.0.cv.notify_all();
    }
}

impl Engines {
    pub fn new(idle: Duration) -> Arc<Engines> {
        let e = Arc::new(Engines {
            inner: Mutex::new(Inner { procs: [None, None], last_used: Instant::now(), busy: 0, idle }),
            cv: Condvar::new(),
        });
        let r = e.clone();
        std::thread::Builder::new().name("engine-reaper".into()).spawn(move || r.reaper()).ok();
        e
    }

    pub fn set_idle(&self, idle: Duration) {
        self.inner.lock().unwrap().idle = idle;
        self.cv.notify_all();
    }

    pub fn hold(self: &Arc<Self>) -> Hold {
        self.inner.lock().unwrap().busy += 1;
        Hold(self.clone())
    }

    /// Start the engine in the background if it isn't running. Non-blocking.
    pub fn prewarm(&self, kind: Kind, cfg: &Config) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        self.ensure(&mut g, kind, cfg)
    }

    /// True exactly once per engine process: the caller should send a warm-up request.
    pub fn claim_warmup(&self, kind: Kind) -> bool {
        let mut g = self.inner.lock().unwrap();
        match g.procs[kind.idx()].as_mut() {
            Some(p) if !p.warmed => {
                p.warmed = true;
                true
            }
            _ => false,
        }
    }

    pub fn is_running(&self, kind: Kind) -> bool {
        let mut g = self.inner.lock().unwrap();
        match g.procs[kind.idx()].as_mut() {
            Some(p) => matches!(p.child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Ensure the engine is running and loaded; returns its base URL.
    pub fn wait_ready(&self, kind: Kind, cfg: &Config, timeout: Duration) -> Result<String> {
        let deadline = Instant::now() + timeout;
        loop {
            let port = {
                let mut g = self.inner.lock().unwrap();
                self.ensure(&mut g, kind, cfg)?;
                let p = g.procs[kind.idx()].as_mut().unwrap();
                if p.ready {
                    return Ok(format!("http://127.0.0.1:{}", p.port));
                }
                p.port
            };
            if probe(port) {
                let mut g = self.inner.lock().unwrap();
                if let Some(p) = g.procs[kind.idx()].as_mut() {
                    if p.port == port {
                        p.ready = true;
                        log::info!("{} engine ready", kind.name());
                    }
                }
                return Ok(format!("http://127.0.0.1:{port}"));
            }
            if Instant::now() >= deadline {
                bail!("{} engine did not become ready within {:?}", kind.name(), timeout);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn ensure(&self, g: &mut Inner, kind: Kind, cfg: &Config) -> Result<()> {
        if let Some(p) = g.procs[kind.idx()].as_mut() {
            match p.child.try_wait() {
                Ok(None) => return Ok(()),
                Ok(Some(status)) => {
                    let tail = log_tail(&p.log);
                    g.procs[kind.idx()] = None;
                    bail!("{} engine exited ({status}). Last log lines:\n{tail}", kind.name());
                }
                Err(e) => bail!("{} engine status error: {e}", kind.name()),
            }
        }
        let started = Instant::now();
        let proc = spawn(kind, cfg)?;
        log::info!("{} engine starting on port {} ({:?})", kind.name(), proc.port, started.elapsed());
        g.procs[kind.idx()] = Some(proc);
        g.last_used = Instant::now();
        self.cv.notify_all();
        Ok(())
    }

    pub fn shutdown(&self) {
        let mut g = self.inner.lock().unwrap();
        kill_all(&mut g);
    }

    fn reaper(self: Arc<Self>) {
        let mut g = self.inner.lock().unwrap();
        loop {
            let running = g.procs.iter().any(|p| p.is_some());
            if !running || g.busy > 0 {
                g = self.cv.wait(g).unwrap(); // sleep until something changes
                continue;
            }
            let deadline = g.last_used + g.idle;
            let now = Instant::now();
            if now >= deadline {
                log::info!("idle for {:?}: unloading engines", g.idle);
                kill_all(&mut g);
            } else {
                g = self.cv.wait_timeout(g, deadline - now).unwrap().0;
            }
        }
    }
}

fn kill_all(g: &mut Inner) {
    for p in g.procs.iter_mut() {
        if let Some(mut proc) = p.take() {
            let _ = proc.child.kill();
            let _ = proc.child.wait();
        }
    }
    #[cfg(unix)]
    let _ = std::fs::remove_file(pidfile());
}

fn probe(port: u16) -> bool {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_millis(400)).build();
    match agent.get(&format!("http://127.0.0.1:{port}/health")).call() {
        Ok(_) => true,
        Err(ureq::Error::Status(503, _)) => false, // still loading the model
        Err(ureq::Error::Status(_, _)) => true,    // older server without /health: it's listening
        Err(_) => false,
    }
}

fn free_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn log_tail(path: &Path) -> String {
    let s = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(8)..].join("\n")
}

fn spawn(kind: Kind, cfg: &Config) -> Result<Proc> {
    let exe = find_engine(kind, cfg)
        .with_context(|| format!("{} not found. Run `openflow setup`.", kind.exe()))?;
    let model = find_model(kind, cfg).with_context(|| format!("no {} model found. Run `openflow setup`.", kind.name()))?;
    let port = free_port()?;
    let threads = cfg.threads().to_string();
    let port_s = port.to_string();
    let model_s = model.to_string_lossy().to_string();
    let mut args: Vec<String> =
        vec!["-m".into(), model_s, "--host".into(), "127.0.0.1".into(), "--port".into(), port_s, "-t".into(), threads];
    match kind {
        Kind::Whisper => {
            args.push("-l".into());
            args.push(cfg.speech.language.clone());
            if cfg.speech.beam_size > 1 {
                args.extend(["-bs".into(), cfg.speech.beam_size.to_string(), "-bo".into(), cfg.speech.beam_size.to_string()]);
            }
            if !cfg.engines.gpu {
                args.push("-ng".into());
            }
        }
        Kind::Llama => {
            args.extend(["-c".into(), "4096".into(), "-ngl".into(), if cfg.engines.gpu { "99" } else { "0" }.into()]);
        }
    }

    let log_dir = config::data_dir().join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let log = log_dir.join(format!("{}.log", kind.name()));
    let out = std::fs::File::create(&log)?;
    let err = out.try_clone()?;

    let mut cmd = Command::new(&exe);
    cmd.args(&args).stdin(Stdio::null()).stdout(out).stderr(err);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir); // so bundled DLLs/dylibs are found
    }
    // Shared runtime libraries (e.g. CUDA DLLs live next to llama-server).
    let mut lib_dirs: Vec<PathBuf> = [Kind::Whisper, Kind::Llama]
        .iter()
        .filter_map(|k| find_engine(*k, cfg).and_then(|p| p.parent().map(|d| d.to_path_buf())))
        .collect();
    if let Some(p) = std::env::var_os("PATH") {
        lib_dirs.extend(std::env::split_paths(&p));
    }
    if let Ok(joined) = std::env::join_paths(lib_dirs) {
        cmd.env("PATH", joined);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    let child = cmd.spawn().with_context(|| format!("starting {}", exe.display()))?;
    os::adopt(&child);
    Ok(Proc { child, port, ready: false, warmed: false, log })
}

pub fn find_engine(kind: Kind, cfg: &Config) -> Option<PathBuf> {
    let custom = match kind {
        Kind::Whisper => &cfg.engines.whisper_server,
        Kind::Llama => &cfg.engines.llama_server,
    };
    if !custom.trim().is_empty() {
        return Some(PathBuf::from(custom.trim())).filter(|p| p.exists());
    }
    let file = format!("{}{}", kind.exe(), std::env::consts::EXE_SUFFIX);
    find_file(&config::engines_dir().join(kind.name()), &file, 4).or_else(|| which(&file))
}

pub fn find_model(kind: Kind, cfg: &Config) -> Option<PathBuf> {
    let custom = match kind {
        Kind::Whisper => &cfg.speech.model,
        Kind::Llama => &cfg.intelligence.model,
    };
    if !custom.trim().is_empty() {
        return Some(PathBuf::from(custom.trim())).filter(|p| p.exists());
    }
    let dir = config::models_dir();
    let preferred: &[&str] = match kind {
        Kind::Whisper => &[
            "ggml-large-v3-turbo-q5_0.bin",
            "ggml-large-v3-turbo-q8_0.bin",
            "ggml-large-v3-turbo.bin",
            "ggml-small-q5_1.bin",
            "ggml-small.bin",
            "ggml-base-q5_1.bin",
            "ggml-base.bin",
        ],
        Kind::Llama => &["qwen2.5-3b-instruct-q4_k_m.gguf", "qwen2.5-1.5b-instruct-q4_k_m.gguf"],
    };
    for name in preferred {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    let ext = match kind {
        Kind::Whisper => ".bin",
        Kind::Llama => ".gguf",
    };
    std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(ext) && !n.ends_with(".part")))
}

fn find_file(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    if depth == 0 {
        return None;
    }
    subdirs.into_iter().find_map(|d| find_file(&d, name, depth - 1))
}

fn which(file: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    #[allow(unused_mut)]
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    #[cfg(target_os = "macos")]
    dirs.extend([PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")]);
    dirs.into_iter().map(|d| d.join(file)).find(|p| p.is_file())
}

#[cfg(unix)]
fn pidfile() -> PathBuf {
    config::data_dir().join("engines.pids")
}

/// Make sure engines die with OpenFlow, even if it crashes.
mod os {
    use std::process::Child;

    #[cfg(windows)]
    pub fn adopt(child: &Child) {
        use std::os::windows::io::AsRawHandle;
        use std::sync::OnceLock;
        use windows_sys::Win32::System::JobObjects::*;
        static JOB: OnceLock<usize> = OnceLock::new();
        let job = *JOB.get_or_init(|| unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            job as usize
        });
        unsafe {
            AssignProcessToJobObject(job as _, child.as_raw_handle() as _);
        }
    }

    #[cfg(unix)]
    pub fn adopt(child: &Child) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(super::pidfile()) {
            let _ = writeln!(f, "{}", child.id());
        }
    }
}

/// Kill engines orphaned by a previous crash (Unix; Windows uses a job object).
pub fn cleanup_orphans() {
    #[cfg(unix)]
    if let Ok(s) = std::fs::read_to_string(pidfile()) {
        for pid in s.lines().filter_map(|l| l.trim().parse::<i32>().ok()) {
            // Only kill it if that PID is still one of our engines (PIDs get reused).
            let name = Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "comm="])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();
            if name.contains("whisper-server") || name.contains("llama-server") {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
        let _ = std::fs::remove_file(pidfile());
    }
}
