//! Speech-to-text via local whisper-server or Groq, with automatic fallback.

use crate::app::{provider_order, App, Provider};
use crate::config::Config;
use crate::engines::Kind;
use anyhow::{anyhow, bail, Result};
use std::time::{Duration, Instant};

pub struct Transcript {
    pub text: String,
    pub via: Provider,
}

/// Vocabulary hint for Whisper (helps names and jargon).
pub fn prompt(cfg: &Config) -> String {
    let mut p = cfg.dictionary.words.iter().take(40).cloned().collect::<Vec<_>>().join(", ");
    if p.len() > 400 {
        p.truncate(400);
    }
    p
}

pub fn transcribe(app: &App, cfg: &Config, wav: &[u8]) -> Result<Transcript> {
    let key = cfg.groq_key();
    let prompt = prompt(cfg);
    let mut errors = Vec::new();
    for p in provider_order(&cfg.speech.provider, key.is_some()) {
        let t = Instant::now();
        let r = match p {
            Provider::Local => local(app, cfg, wav, &prompt),
            Provider::Cloud => groq(cfg, key.as_deref().unwrap(), wav, &prompt),
        };
        match r {
            Ok(text) => {
                log::info!("stt via {} in {:?}", p.name(), t.elapsed());
                return Ok(Transcript { text, via: p });
            }
            Err(e) => {
                log::warn!("stt via {} failed: {e:#}", p.name());
                errors.push(format!("{}: {e:#}", p.name()));
            }
        }
    }
    bail!("speech recognition failed ({})", errors.join("; "))
}

/// First inference after start is ~2 s slower (GPU kernel/buffer setup). Pay
/// that cost while the user is still talking.
pub fn warmup(app: &App, cfg: &Config) {
    if !app.engines.claim_warmup(Kind::Whisper) {
        return;
    }
    let t = Instant::now();
    let wav = crate::audio::wav_bytes(&vec![0.0f32; crate::audio::RATE as usize / 2]);
    match local(app, cfg, &wav, "") {
        Ok(_) => log::info!("whisper warmed up in {:?}", t.elapsed()),
        Err(e) => log::debug!("whisper warm-up failed: {e:#}"),
    }
}

fn local(app: &App, cfg: &Config, wav: &[u8], prompt: &str) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    let base = app.engines.wait_ready(Kind::Whisper, cfg, Duration::from_secs(45))?;
    let mut fields = vec![("response_format", "json"), ("temperature", "0.0")];
    if !prompt.is_empty() {
        fields.push(("prompt", prompt));
    }
    let (ct, body) = multipart(&fields, wav);
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(60)).build();
    loop {
        match agent.post(&format!("{base}/inference")).set("Content-Type", &ct).send_bytes(&body) {
            Ok(resp) => {
                let v: serde_json::Value = resp.into_json()?;
                if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
                    bail!("whisper-server: {err}");
                }
                return Ok(v["text"].as_str().unwrap_or_default().trim().to_string());
            }
            // still loading: retry
            Err(ureq::Error::Status(503, _)) | Err(ureq::Error::Transport(_)) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(anyhow!("whisper-server request failed: {e}")),
        }
    }
}

fn groq(cfg: &Config, key: &str, wav: &[u8], prompt: &str) -> Result<String> {
    let mut fields = vec![("model", cfg.groq.stt_model.as_str()), ("response_format", "json"), ("temperature", "0")];
    let lang = cfg.speech.language.trim();
    if !lang.is_empty() && lang != "auto" {
        fields.push(("language", lang));
    }
    if !prompt.is_empty() {
        fields.push(("prompt", prompt));
    }
    let (ct, body) = multipart(&fields, wav);
    let resp = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build()
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", &ct)
        .send_bytes(&body)
        .map_err(|e| match e {
            ureq::Error::Status(code, r) => anyhow!("groq {code}: {}", r.into_string().unwrap_or_default()),
            e => anyhow!("groq: {e}"),
        })?;
    let v: serde_json::Value = resp.into_json()?;
    Ok(v["text"].as_str().unwrap_or_default().trim().to_string())
}

fn multipart(fields: &[(&str, &str)], wav: &[u8]) -> (String, Vec<u8>) {
    let boundary = format!("openflow{:x}", Instant::now().elapsed().as_nanos() ^ 0x5eed_f00d_cafe);
    let mut b = Vec::with_capacity(wav.len() + 1024);
    for (k, v) in fields {
        b.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    b.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    b.extend(wav);
    b.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), b)
}
