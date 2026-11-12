//! audio -> speech check -> transcript -> AI cleanup -> snippets -> text

use crate::app::App;
use crate::audio::{self, Recording};
use crate::context::{self, Target};
use crate::history::Entry;
use crate::text::{self, Style};
use crate::{intelligence, stt};
use anyhow::Result;
use std::time::Instant;

pub struct Outcome {
    pub raw: String,
    pub text: String,
    pub style: Style,
    pub entry: Entry,
}

/// Returns `Ok(None)` when there was nothing to type (silence / noise).
pub fn process(app: &App, rec: Recording, target: &Target) -> Result<Option<Outcome>> {
    let t0 = Instant::now();
    let cfg = app.cfg();
    let pcm = audio::resample(&rec.samples, rec.rate, audio::RATE);
    let speech = match audio::detect_speech(&pcm) {
        Some(s) => s,
        None => {
            log::info!("no speech detected ({:.1}s of audio)", pcm.len() as f32 / audio::RATE as f32);
            return Ok(None);
        }
    };
    let wav = audio::wav_bytes(&speech.samples);
    let audio_path = app.history.save_audio(&wav);
    let style = context::style_for(target, &cfg);

    let mut entry = Entry {
        time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        app: target.app.clone(),
        style: style.name().into(),
        audio: audio_path,
        ..Default::default()
    };

    let tr = match stt::transcribe(app, &cfg, &wav) {
        Ok(t) => t,
        Err(e) => {
            entry.error = Some(format!("{e:#}"));
            app.history.append(&entry);
            return Err(e);
        }
    };
    let raw = text::strip_hallucinations(&tr.text, speech.speech_ms, &stt::prompt(&cfg));
    entry.stt = tr.via.name().into();
    entry.raw = raw.clone();
    if raw.trim().is_empty() {
        log::info!("transcript empty after filtering");
        return Ok(None);
    }

    let polished = intelligence::polish(app, &cfg, &raw, style);
    let mut out = text::expand_snippets(&polished.text, &cfg.snippets);
    if out.trim().is_empty() {
        return Ok(None);
    }
    if cfg.output.append_space && style != Style::Code && !out.ends_with(['\n', ' ']) {
        out.push(' ');
    }
    entry.ai = polished.via.into();
    entry.text = out.clone();
    entry.ms = t0.elapsed().as_millis() as u64;
    log::info!(
        "done in {} ms [{} | stt {} | ai {}] {:?}",
        entry.ms,
        style.name(),
        entry.stt,
        entry.ai,
        out
    );
    Ok(Some(Outcome { raw, text: out, style, entry }))
}
