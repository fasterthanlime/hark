//! Live microphone streaming transcription demo.
//!
//! Captures audio from the default input device, resamples to 16 kHz mono,
//! and transcribes in real time using the streaming API.
//!
//! Every ~10 seconds of audio, the session is finalized and a new one starts,
//! carrying forward context via `initial_text`. This keeps processing bounded
//! instead of re-encoding the entire history on every step.
//!
//! Usage:
//!   cargo run --example live --release [-- [model_dir]]
//!
//! Press Ctrl-C to stop.

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use qwen3_asr::{AsrInference, StreamingOptions};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

const TARGET_SR: u32 = 16000;
/// How many 16kHz samples before we rotate the session (~10s).
const SESSION_SAMPLES: usize = TARGET_SR as usize * 10;

fn main() -> Result<()> {
    // ── Parse args ──────────────────────────────────────────────────────
    let args: Vec<String> = std::env::args().collect();
    let model_dir = args
        .get(1)
        .filter(|s| !s.starts_with('-'))
        .map(PathBuf::from)
        .or_else(|| std::env::var("MODEL_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("models"));

    // ── Load model ──────────────────────────────────────────────────────
    let device = qwen3_asr::best_device();
    eprintln!("Device   : {:?}", device);
    eprintln!("Model dir: {}", model_dir.display());
    eprintln!("Loading model...");
    let engine = AsrInference::load(&model_dir, device)?;
    eprintln!("Model loaded.\n");

    // ── Set up audio capture ────────────────────────────────────────────
    let host = cpal::default_host();
    let input_device = host
        .default_input_device()
        .context("no default input device found")?;
    eprintln!(
        "Input device: {}",
        input_device.name().unwrap_or_else(|_| "unknown".into())
    );

    let default_config = input_device.default_input_config()?;
    let native_sr = default_config.sample_rate().0;
    let native_channels = default_config.channels() as usize;
    eprintln!("Native config: {}Hz, {} ch", native_sr, native_channels);

    let config = cpal::StreamConfig {
        channels: native_channels as u16,
        sample_rate: cpal::SampleRate(native_sr),
        buffer_size: cpal::BufferSize::Default,
    };

    let (tx, rx) = mpsc::channel::<Vec<f32>>();

    let stream = input_device.build_input_stream(
        &config,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            let _ = tx.send(data.to_vec());
        },
        |err| {
            eprintln!("Audio stream error: {}", err);
        },
        None,
    )?;

    stream.play()?;

    // ── Ctrl-C handler ──────────────────────────────────────────────────
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })?;

    // ── Resampler setup ─────────────────────────────────────────────────
    let needs_resample = native_sr != TARGET_SR;
    let resample_chunk = (native_sr as usize) / 10; // ~100ms
    let mut resampler = if needs_resample {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        Some(SincFixedIn::<f32>::new(
            TARGET_SR as f64 / native_sr as f64,
            2.0,
            params,
            resample_chunk,
            1,
        )?)
    } else {
        None
    };
    let mut native_buf: Vec<f32> = Vec::new();

    // ── Session state ───────────────────────────────────────────────────
    let mut transcript = String::new(); // full accumulated transcript
    let mut session_samples: usize = 0; // 16kHz samples fed into current session
    let mut state = engine.init_streaming(StreamingOptions::default().with_chunk_size_sec(0.5));

    eprintln!("Listening... speak into your microphone. Press Ctrl-C to stop.\n");

    while running.load(Ordering::SeqCst) {
        let mut got_audio = false;
        while let Ok(raw) = rx.try_recv() {
            got_audio = true;

            // Downmix to mono.
            let mono: Vec<f32> = if native_channels == 1 {
                raw
            } else {
                raw.chunks_exact(native_channels)
                    .map(|frame| frame.iter().sum::<f32>() / native_channels as f32)
                    .collect()
            };

            // Resample to 16kHz.
            let samples_16k = if let Some(ref mut rs) = resampler {
                native_buf.extend_from_slice(&mono);
                let mut out = Vec::new();
                while native_buf.len() >= resample_chunk {
                    let chunk: Vec<f32> = native_buf.drain(..resample_chunk).collect();
                    let resampled = rs.process(&[chunk], None)?;
                    out.extend_from_slice(&resampled[0]);
                }
                out
            } else {
                mono
            };

            if samples_16k.is_empty() {
                continue;
            }

            session_samples += samples_16k.len();

            match engine.feed_audio(&mut state, &samples_16k) {
                Ok(Some(result)) => {
                    // Show committed transcript + current session text.
                    eprint!("\r\x1b[2K");
                    eprint!("{}{}", transcript, result.text);
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("\nTranscription error: {}", e);
                }
            }

            // Rotate session when we've accumulated enough audio.
            if session_samples >= SESSION_SAMPLES {
                let final_result = engine.finish_streaming(&mut state)?;
                if !final_result.text.is_empty() {
                    transcript.push_str(&final_result.text);
                    transcript.push(' ');
                }

                // Start a new session, seeding with trailing context.
                let context = tail_chars(&transcript, 200);
                state = engine.init_streaming(
                    StreamingOptions::default()
                        .with_chunk_size_sec(0.5)
                        .with_initial_text(context),
                );
                session_samples = 0;
            }
        }

        if !got_audio {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    // ── Finalize last session ───────────────────────────────────────────
    drop(stream);
    eprintln!("\n\nFinalizing...");
    let final_result = engine.finish_streaming(&mut state)?;
    if !final_result.text.is_empty() {
        transcript.push_str(&final_result.text);
    }
    let transcript = transcript.trim();
    eprintln!("Done.\n");
    println!("{}", transcript);

    Ok(())
}

/// Return the last `n` chars of `s` (or all of `s` if shorter).
fn tail_chars(s: &str, n: usize) -> &str {
    let byte_start = s
        .char_indices()
        .rev()
        .nth(n.saturating_sub(1))
        .map(|(i, _)| i)
        .unwrap_or(0);
    &s[byte_start..]
}
