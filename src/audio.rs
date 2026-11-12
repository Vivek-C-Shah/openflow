//! Microphone capture (only open while recording), resampling to 16 kHz,
//! voice-activity check, WAV encoding, and audio cues.

use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const RATE: u32 = 16_000;

pub struct Recording {
    pub samples: Vec<f32>,
    pub rate: u32,
}

/// Holds the mic stream. The stream (and the OS mic indicator) only exists
/// between `start` and `stop`.
pub struct Recorder {
    stream: Option<cpal::Stream>,
    buf: Arc<Mutex<Vec<f32>>>,
    rate: u32,
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder {
    pub fn new() -> Self {
        Recorder { stream: None, buf: Arc::new(Mutex::new(Vec::new())), rate: RATE }
    }

    pub fn device_name() -> Option<String> {
        cpal::default_host().default_input_device().and_then(|d| d.name().ok())
    }

    pub fn start(&mut self) -> Result<()> {
        self.stream = None;
        let host = cpal::default_host();
        let dev = host.default_input_device().ok_or_else(|| anyhow!("no microphone found"))?;
        let cfg = dev.default_input_config().context("microphone has no usable config")?;
        self.rate = cfg.sample_rate().0;
        let channels = cfg.channels() as usize;
        let buf = Arc::new(Mutex::new(Vec::with_capacity(self.rate as usize * 30)));
        self.buf = buf.clone();
        let sc: cpal::StreamConfig = cfg.clone().into();
        let stream = match cfg.sample_format() {
            SampleFormat::F32 => build::<f32>(&dev, &sc, channels, buf, |s| s),
            SampleFormat::I16 => build::<i16>(&dev, &sc, channels, buf, |s| s as f32 / 32768.0),
            SampleFormat::U16 => build::<u16>(&dev, &sc, channels, buf, |s| (s as f32 - 32768.0) / 32768.0),
            SampleFormat::I32 => build::<i32>(&dev, &sc, channels, buf, |s| s as f32 / 2_147_483_648.0),
            f => bail!("unsupported microphone sample format {f:?}"),
        }?;
        stream.play().context("could not start microphone")?;
        self.stream = Some(stream);
        Ok(())
    }

    pub fn stop(&mut self) -> Recording {
        self.stream = None; // drops the stream -> mic released
        let samples = std::mem::take(&mut *self.buf.lock().unwrap());
        Recording { samples, rate: self.rate }
    }
}

fn build<T: SizedSample + Copy + Send + 'static>(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    channels: usize,
    buf: Arc<Mutex<Vec<f32>>>,
    conv: fn(T) -> f32,
) -> Result<cpal::Stream> {
    let channels = channels.max(1);
    Ok(dev.build_input_stream(
        cfg,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            if let Ok(mut b) = buf.lock() {
                for frame in data.chunks(channels) {
                    let sum: f32 = frame.iter().map(|&s| conv(s)).sum();
                    b.push(sum / channels as f32);
                }
            }
        },
        |e| log::error!("microphone error: {e}"),
        None,
    )?)
}

/// Box-filtered linear resampling (good enough for speech recognition).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let n = (input.len() as f64 / ratio) as usize;
    let half = (ratio / 2.0).max(0.5);
    (0..n)
        .map(|i| {
            let center = i as f64 * ratio;
            let lo = ((center - half).floor().max(0.0)) as usize;
            let hi = ((center + half).ceil() as usize).min(input.len() - 1);
            let slice = &input[lo..=hi];
            slice.iter().sum::<f32>() / slice.len() as f32
        })
        .collect()
}

pub struct Speech {
    /// Trimmed + normalized 16 kHz audio.
    pub samples: Vec<f32>,
    pub speech_ms: u64,
}

/// Energy-based voice detection. Returns `None` if nobody spoke, otherwise
/// the audio with leading/trailing silence trimmed and quiet speech boosted.
pub fn detect_speech(pcm: &[f32]) -> Option<Speech> {
    const FRAME: usize = 480; // 30 ms at 16 kHz
    if pcm.len() < FRAME * 4 {
        return None;
    }
    let rms: Vec<f32> = pcm
        .chunks(FRAME)
        .map(|f| (f.iter().map(|s| s * s).sum::<f32>() / f.len() as f32).sqrt())
        .collect();
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let noise = sorted[sorted.len() / 5];
    let threshold = (noise * 3.0).max(0.008);
    let voiced: Vec<usize> = rms.iter().enumerate().filter(|(_, &r)| r > threshold).map(|(i, _)| i).collect();
    if voiced.len() < 8 {
        return None; // < 240 ms of speech
    }
    let pad = 10; // keep 300 ms around speech
    let first = voiced[0].saturating_sub(pad);
    let last = (voiced[voiced.len() - 1] + pad + 1).min(rms.len());
    let mut samples = pcm[first * FRAME..(last * FRAME).min(pcm.len())].to_vec();
    let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    if peak > 0.0 && peak < 0.5 {
        let gain = (0.9 / peak).min(10.0);
        samples.iter_mut().for_each(|s| *s *= gain);
    }
    Some(Speech { samples, speech_ms: voiced.len() as u64 * 30 })
}

pub fn wav_bytes(pcm: &[f32]) -> Vec<u8> {
    let spec = hound::WavSpec { channels: 1, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut cur = std::io::Cursor::new(Vec::with_capacity(pcm.len() * 2 + 44));
    {
        let mut w = hound::WavWriter::new(&mut cur, spec).expect("wav header");
        for &s in pcm {
            w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16).expect("wav write");
        }
        w.finalize().expect("wav finalize");
    }
    cur.into_inner()
}

/// Read a WAV file into mono f32 + its sample rate.
pub fn read_wav(path: &std::path::Path) -> Result<Recording> {
    let mut r = hound::WavReader::open(path)?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.map(|v| v as f32 / max)).collect::<Result<_, _>>()?
        }
    };
    let samples = raw.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    Ok(Recording { samples, rate: spec.sample_rate })
}

#[derive(Debug, Clone, Copy)]
pub enum Cue {
    Start,
    Stop,
    Cancel,
    Error,
}

/// Fire-and-forget audio cue. Opens the speaker only for the cue's duration.
pub fn cue(kind: Cue, enabled: bool) {
    if !enabled {
        return;
    }
    std::thread::spawn(move || {
        if let Err(e) = play(kind) {
            log::debug!("cue failed: {e}");
        }
    });
}

fn play(kind: Cue) -> Result<()> {
    // (frequency Hz, duration ms) sequence
    let notes: &[(f32, u32)] = match kind {
        Cue::Start => &[(660.0, 60), (990.0, 80)],
        Cue::Stop => &[(990.0, 60), (660.0, 80)],
        Cue::Cancel => &[(440.0, 90)],
        Cue::Error => &[(220.0, 140), (0.0, 60), (220.0, 140)],
    };
    let dev = cpal::default_host().default_output_device().ok_or_else(|| anyhow!("no speaker"))?;
    let cfg = dev.default_output_config()?;
    let rate = cfg.sample_rate().0 as f32;
    let channels = cfg.channels() as usize;
    let mut wave: Vec<f32> = Vec::new();
    for &(f, ms) in notes {
        let n = (rate * ms as f32 / 1000.0) as usize;
        let fade = (rate * 0.008) as usize;
        for i in 0..n {
            let env = (i.min(n - i) as f32 / fade as f32).min(1.0);
            let v = if f > 0.0 { (2.0 * std::f32::consts::PI * f * i as f32 / rate).sin() * 0.12 * env } else { 0.0 };
            wave.push(v);
        }
    }
    let total_ms: u32 = notes.iter().map(|n| n.1).sum();
    let wave = Arc::new(wave);
    let pos = Arc::new(Mutex::new(0usize));
    let sc: cpal::StreamConfig = cfg.clone().into();
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => out_stream::<f32>(&dev, &sc, channels, wave, pos, |v| v),
        SampleFormat::I16 => out_stream::<i16>(&dev, &sc, channels, wave, pos, |v| (v * 32767.0) as i16),
        SampleFormat::U16 => out_stream::<u16>(&dev, &sc, channels, wave, pos, |v| ((v + 1.0) * 32767.5) as u16),
        f => bail!("unsupported output format {f:?}"),
    }?;
    stream.play()?;
    std::thread::sleep(Duration::from_millis(total_ms as u64 + 60));
    Ok(())
}

fn out_stream<T: SizedSample + Send + 'static>(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    channels: usize,
    wave: Arc<Vec<f32>>,
    pos: Arc<Mutex<usize>>,
    conv: fn(f32) -> T,
) -> Result<cpal::Stream> {
    let channels = channels.max(1);
    Ok(dev.build_output_stream(
        cfg,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mut p = pos.lock().unwrap();
            for frame in data.chunks_mut(channels) {
                let v = wave.get(*p).copied().unwrap_or(0.0);
                *p += 1;
                for s in frame.iter_mut() {
                    *s = conv(v);
                }
            }
        },
        |e| log::debug!("speaker error: {e}"),
        None,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_length() {
        let x = vec![0.0f32; 48_000];
        assert_eq!(resample(&x, 48_000, 16_000).len(), 16_000);
    }

    #[test]
    fn silence_is_not_speech() {
        assert!(detect_speech(&vec![0.0005f32; 32_000]).is_none());
    }

    #[test]
    fn tone_is_speech_and_trimmed() {
        let mut x = vec![0.0f32; 16_000];
        x.extend((0..16_000).map(|i| (i as f32 * 0.05).sin() * 0.3));
        x.extend(vec![0.0f32; 16_000]);
        let s = detect_speech(&x).unwrap();
        assert!(s.samples.len() < 30_000 && s.samples.len() > 16_000);
        assert!(s.speech_ms >= 900);
    }
}
