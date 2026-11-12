//! One-command setup: detect hardware, fetch prebuilt engines and the right
//! models, write config, enable start-at-login, start OpenFlow.

use crate::config::{self, Config};
use crate::engines::{self, Kind};
use anyhow::{anyhow, bail, Context, Result};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Opts {
    pub cpu: bool,
    pub gpu: bool,
    pub cloud: bool,
    pub no_autostart: bool,
    pub force: bool,
}

struct ModelFile {
    name: &'static str,
    url: &'static str,
}

const WHISPER_GPU: ModelFile = ModelFile {
    name: "ggml-large-v3-turbo-q5_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
};
const WHISPER_CPU: ModelFile = ModelFile {
    name: "ggml-small-q5_1.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin",
};
const LLM_GPU: ModelFile = ModelFile {
    name: "qwen2.5-3b-instruct-q4_k_m.gguf",
    url: "https://huggingface.co/Qwen/Qwen2.5-3B-Instruct-GGUF/resolve/main/qwen2.5-3b-instruct-q4_k_m.gguf",
};
const LLM_CPU: ModelFile = ModelFile {
    name: "qwen2.5-1.5b-instruct-q4_k_m.gguf",
    url: "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf",
};

pub fn run(o: Opts) -> Result<()> {
    println!("\n  OpenFlow setup\n  ──────────────");
    let cfg = Config::load()?;
    println!("  config: {}", config::config_path().display());
    println!("  data:   {}", config::data_dir().display());

    let nvidia = has_nvidia();
    let apple_gpu = cfg!(all(target_os = "macos", target_arch = "aarch64"));
    let gpu = !o.cpu && (o.gpu || nvidia || apple_gpu);
    println!(
        "  hardware: {}{}",
        if nvidia { "NVIDIA GPU detected" } else if apple_gpu { "Apple Silicon (Metal)" } else { "no supported GPU, using CPU" },
        if o.cpu { " (forced CPU)" } else { "" }
    );

    if o.cloud {
        println!("\n  Cloud mode: skipping local engine and model downloads.");
        set_value("speech", "provider", "\"cloud\"")?;
        set_value("intelligence", "provider", "\"cloud\"")?;
    } else {
        install_engines(&cfg, gpu, o.force)?;
        let (w, l) = if gpu { (WHISPER_GPU, LLM_GPU) } else { (WHISPER_CPU, LLM_CPU) };
        println!("\n  Models (downloaded once, then fully offline):");
        download_model(&w)?;
        download_model(&l)?;
        set_value("engines", "gpu", if gpu { "true" } else { "false" })?;
    }

    let cfg = Config::load()?;
    if cfg.groq_key().is_none() && std::io::stdin().is_terminal() {
        println!("\n  Optional: a free Groq API key gives a smarter cloud AI and an automatic backup.");
        println!("  Get one at https://console.groq.com/keys");
        print!("  Paste key (or press Enter to stay 100% local): ");
        std::io::stdout().flush()?;
        let mut key = String::new();
        std::io::stdin().read_line(&mut key)?;
        let key = key.trim();
        if !key.is_empty() {
            set_value("groq", "api_key", &format!("{key:?}"))?;
            println!("  Saved.");
        }
    }
    if o.cloud && Config::load()?.groq_key().is_none() {
        bail!("cloud mode needs a Groq API key: add it to {} under [groq]", config::config_path().display());
    }

    if !o.no_autostart {
        match set_autostart(true) {
            Ok(()) => println!("\n  Start at login: enabled"),
            Err(e) => println!("\n  Start at login: could not enable ({e:#})"),
        }
    }

    if crate::control::is_running() {
        let _ = crate::control::send("reload");
        println!("  OpenFlow is running (config reloaded).");
    } else {
        start_daemon()?;
        println!("  OpenFlow started.");
    }
    let key = Config::load()?.trigger.key;
    println!("\n  Ready! Double-tap {key} and talk; tap it again to finish.");
    println!("  Or hold {key} while talking (push-to-talk). Esc cancels.");
    #[cfg(target_os = "macos")]
    println!("\n  macOS: allow OpenFlow in System Settings > Privacy & Security > Accessibility and Microphone.");
    println!();
    Ok(())
}

// ───────────────────────────── engines ─────────────────────────────

fn install_engines(cfg: &Config, gpu: bool, force: bool) -> Result<()> {
    let have_w = engines::find_engine(Kind::Whisper, cfg).is_some();
    let have_l = engines::find_engine(Kind::Llama, cfg).is_some();
    if have_w && have_l && !force {
        println!("\n  Engines: already installed");
        return Ok(());
    }
    println!("\n  Engines:");
    platform_engines(gpu)
}

#[cfg(windows)]
fn platform_engines(gpu: bool) -> Result<()> {
    let edir = config::engines_dir();
    let llama_dir = edir.join("llama");
    let whisper_dir = edir.join("whisper");
    if gpu {
        let (rel, llama) = find_asset("ggml-org/llama.cpp", |n| {
            n.starts_with("llama-") && n.contains("-bin-win-cuda-12") && n.ends_with("-x64.zip")
        })?;
        let cudart = rel
            .iter()
            .find(|(n, _)| n.starts_with("cudart-llama-bin-win-cuda-12") && n.ends_with("-x64.zip"))
            .cloned()
            .ok_or_else(|| anyhow!("CUDA runtime package not found in llama.cpp release"))?;
        let (_, whisper) = find_asset("ggml-org/whisper.cpp", |n| {
            (n.starts_with("whisper-bin-win-cuda-12") || n.starts_with("whisper-cublas-12")) && n.ends_with("x64.zip")
        })?;
        fetch_and_extract(&llama, &llama_dir, true)?;
        fetch_and_extract(&whisper, &whisper_dir, true)?;
        // One copy of the CUDA 12 runtime, next to llama-server; whisper-server
        // finds it through PATH (see engines::spawn).
        let cuda = download_tmp(&cudart)?;
        let l_exe = engines::find_engine(Kind::Llama, &Config::default()).context("llama-server missing after extract")?;
        extract(&cuda, l_exe.parent().unwrap())?;
        let _ = std::fs::remove_file(cuda);
    } else {
        let (_, llama) = find_asset("ggml-org/llama.cpp", |n| n.starts_with("llama-") && n.ends_with("-bin-win-cpu-x64.zip"))?;
        let (_, whisper) = find_asset("ggml-org/whisper.cpp", |n| n == "whisper-bin-x64.zip")?;
        fetch_and_extract(&llama, &llama_dir, true)?;
        fetch_and_extract(&whisper, &whisper_dir, true)?;
    }
    verify_engines()
}

#[cfg(target_os = "linux")]
fn platform_engines(_gpu: bool) -> Result<()> {
    let edir = config::engines_dir();
    let arm = cfg!(target_arch = "aarch64");
    let vulkan = Command::new("vulkaninfo").arg("--summary").output().map(|o| o.status.success()).unwrap_or(false);
    let (_, llama) = find_asset("ggml-org/llama.cpp", |n| {
        n.starts_with("llama-")
            && n.ends_with(".tar.gz")
            && if arm {
                n.ends_with("-bin-ubuntu-arm64.tar.gz")
            } else if vulkan {
                n.ends_with("-bin-ubuntu-vulkan-x64.tar.gz")
            } else {
                n.ends_with("-bin-ubuntu-x64.tar.gz")
            }
    })?;
    let want = if arm { "whisper-bin-ubuntu-arm64.tar.gz" } else { "whisper-bin-ubuntu-x64.tar.gz" };
    let (_, whisper) = find_asset("ggml-org/whisper.cpp", |n| n == want)?;
    fetch_and_extract(&llama, &edir.join("llama"), true)?;
    fetch_and_extract(&whisper, &edir.join("whisper"), true)?;
    verify_engines()
}

#[cfg(target_os = "macos")]
fn platform_engines(_gpu: bool) -> Result<()> {
    let brew = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"].into_iter().find(|p| Path::new(p).exists());
    let Some(brew) = brew else {
        bail!("Homebrew is required on macOS. Install it from https://brew.sh then run `openflow setup` again.");
    };
    println!("    installing whisper-cpp and llama.cpp with Homebrew (Metal-accelerated)...");
    let ok = Command::new(brew).args(["install", "whisper-cpp", "llama.cpp"]).status()?.success();
    if !ok {
        bail!("brew install failed");
    }
    verify_engines()
}

fn verify_engines() -> Result<()> {
    let c = Config::default();
    for k in [Kind::Whisper, Kind::Llama] {
        let p = engines::find_engine(k, &c).ok_or_else(|| anyhow!("{} engine not found after install", k.name()))?;
        println!("    {} -> {}", k.name(), p.display());
    }
    Ok(())
}

type Asset = (String, String); // (name, url)

/// Find the newest release that has an asset matching `pred`.
/// Returns all assets of that release plus the match.
fn find_asset(repo: &str, pred: impl Fn(&str) -> bool) -> Result<(Vec<Asset>, Asset)> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=15");
    let v: serde_json::Value = agent(30)
        .get(&url)
        .set("User-Agent", "openflow-setup")
        .set("Accept", "application/vnd.github+json")
        .call()
        .with_context(|| format!("querying {repo} releases"))?
        .into_json()?;
    for rel in v.as_array().into_iter().flatten() {
        let assets: Vec<Asset> = rel["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| Some((a["name"].as_str()?.to_string(), a["browser_download_url"].as_str()?.to_string())))
            .collect();
        if let Some(hit) = assets.iter().find(|(n, _)| pred(n)).cloned() {
            return Ok((assets, hit));
        }
    }
    bail!("no suitable prebuilt package found in {repo} releases")
}

fn fetch_and_extract(asset: &Asset, dir: &Path, clean: bool) -> Result<()> {
    let tmp = download_tmp(asset)?;
    if clean && dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("removing old {} (is OpenFlow running? `openflow stop`)", dir.display()))?;
    }
    extract(&tmp, dir)?;
    let _ = std::fs::remove_file(tmp);
    Ok(())
}

fn download_tmp(asset: &Asset) -> Result<PathBuf> {
    let dir = config::data_dir().join("downloads");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(&asset.0);
    download(&asset.1, &path, &asset.0)?;
    Ok(path)
}

fn extract(archive: &Path, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let name = archive.to_string_lossy().to_string();
    if name.ends_with(".zip") {
        let mut z = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
        for i in 0..z.len() {
            let mut f = z.by_index(i)?;
            let Some(rel) = f.enclosed_name() else { continue };
            let out = dir.join(rel);
            if f.is_dir() {
                std::fs::create_dir_all(&out)?;
                continue;
            }
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut w = std::fs::File::create(&out)?;
            std::io::copy(&mut f, &mut w)?;
            #[cfg(unix)]
            if let Some(mode) = f.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode))?;
            }
        }
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let gz = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
        tar::Archive::new(gz).unpack(dir)?;
    } else {
        bail!("unknown archive type: {name}");
    }
    Ok(())
}

// ───────────────────────────── downloads ─────────────────────────────

fn agent(timeout_s: u64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(timeout_s))
        .timeout_read(Duration::from_secs(timeout_s))
        .build()
}

fn download_model(m: &ModelFile) -> Result<()> {
    let dir = config::models_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(m.name);
    if path.exists() {
        println!("    {} (already downloaded)", m.name);
        return Ok(());
    }
    download(m.url, &path, m.name)
}

/// Resumable download with progress; writes to `.part` and renames on success.
fn download(url: &str, dest: &Path, label: &str) -> Result<()> {
    let part = dest.with_extension(format!(
        "{}.part",
        dest.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default()
    ));
    let mut attempt = 0;
    loop {
        attempt += 1;
        match try_download(url, &part, label) {
            Ok(()) => break,
            Err(e) if attempt < 5 => {
                println!("\n    download interrupted ({e:#}); retrying...");
                std::thread::sleep(Duration::from_secs(2 * attempt));
            }
            Err(e) => return Err(e.context(format!("downloading {label}"))),
        }
    }
    std::fs::rename(&part, dest)?;
    Ok(())
}

fn try_download(url: &str, part: &Path, label: &str) -> Result<()> {
    let have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let mut req = agent(60).get(url).set("User-Agent", "openflow-setup");
    if have > 0 {
        req = req.set("Range", &format!("bytes={have}-"));
    }
    let resp = req.call()?;
    let resumed = resp.status() == 206;
    let remaining: Option<u64> = resp.header("Content-Length").and_then(|s| s.parse().ok());
    let start = if resumed { have } else { 0 };
    let total = remaining.map(|r| r + start);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(part)?;
    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = start;
    let mut last = Instant::now() - Duration::from_secs(1);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if last.elapsed() > Duration::from_millis(250) {
            last = Instant::now();
            progress(label, done, total);
        }
    }
    file.flush()?;
    progress(label, done, total);
    println!();
    if let Some(t) = total {
        if done != t {
            bail!("incomplete download ({done} of {t} bytes)");
        }
    }
    Ok(())
}

fn progress(label: &str, done: u64, total: Option<u64>) {
    let mb = |b: u64| b as f64 / 1_048_576.0;
    match total {
        Some(t) if t > 0 => print!("\r    {label}  {:>6.1} / {:.1} MB  ({:>3.0}%)", mb(done), mb(t), done as f64 * 100.0 / t as f64),
        _ => print!("\r    {label}  {:>6.1} MB", mb(done)),
    }
    let _ = std::io::stdout().flush();
}

// ───────────────────────────── system ─────────────────────────────

fn has_nvidia() -> bool {
    Command::new("nvidia-smi")
        .arg("-L")
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("GPU"))
        .unwrap_or(false)
}

pub fn daemon_exe() -> PathBuf {
    let me = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("openflow"));
    let d = me.with_file_name(format!("openflow-daemon{}", std::env::consts::EXE_SUFFIX));
    if d.exists() {
        d
    } else {
        me
    }
}

pub fn start_daemon() -> Result<()> {
    let exe = daemon_exe();
    let mut cmd = Command::new(&exe);
    if exe.file_stem().is_some_and(|s| s == "openflow") {
        cmd.arg("run");
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().with_context(|| format!("starting {}", exe.display()))?;
    for _ in 0..30 {
        if crate::control::is_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("OpenFlow did not start; see {}", config::data_dir().join("logs").join("openflow.log").display())
}

pub fn set_autostart(on: bool) -> Result<()> {
    let exe = daemon_exe();
    let path = exe.to_string_lossy().to_string();
    let args: &[&str] = if exe.file_stem().is_some_and(|s| s == "openflow") { &["run"] } else { &[] };
    let al = auto_launch::AutoLaunchBuilder::new()
        .set_app_name("OpenFlow")
        .set_app_path(&path)
        .set_use_launch_agent(true)
        .set_args(args)
        .build()
        .map_err(|e| anyhow!("{e}"))?;
    if on {
        al.enable().map_err(|e| anyhow!("{e}"))?;
    } else if al.is_enabled().unwrap_or(false) {
        al.disable().map_err(|e| anyhow!("{e}"))?;
    }
    Ok(())
}

/// Minimal in-place config edit that keeps the user's comments.
pub fn set_value(section: &str, key: &str, literal: &str) -> Result<()> {
    let path = config::config_path();
    let _ = Config::load()?; // ensure file exists
    let text = std::fs::read_to_string(&path)?;
    let mut out = Vec::new();
    let mut in_section = false;
    let mut done = false;
    let header = format!("[{section}]");
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if in_section && !done {
                out.push(format!("{key} = {literal}"));
                done = true;
            }
            in_section = t == header;
        } else if in_section && !done {
            if let Some(rest) = t.strip_prefix(key) {
                if rest.trim_start().starts_with('=') {
                    let comment = line.find(" #").map(|i| line[i..].to_string()).unwrap_or_default();
                    out.push(format!("{key} = {literal}{comment}"));
                    done = true;
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    if !done {
        if !in_section {
            out.push(header);
        }
        out.push(format!("{key} = {literal}"));
    }
    let new = out.join("\n") + "\n";
    Config::from_user_str(&new).context("refusing to write an invalid config")?;
    std::fs::write(&path, new)?;
    Ok(())
}
