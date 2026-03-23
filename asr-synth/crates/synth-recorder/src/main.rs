use anyhow::{Context, Result};
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Parser)]
#[command(about = "Record eval sentences with mic profile support")]
struct Args {
    /// Mic profile name (e.g. "desk-sm7b", "laptop-bed", "airpods")
    #[arg(short, long)]
    profile: String,

    /// Path to eval sentences JSONL
    #[arg(short, long, default_value = "data/eval_sentences.jsonl")]
    sentences: String,

    /// Output directory for recordings
    #[arg(short, long, default_value = "data/eval_recordings")]
    output: String,

    /// Start from sentence N (0-indexed, for resuming)
    #[arg(long, default_value = "0")]
    start: usize,
}

#[derive(serde::Deserialize)]
struct EvalSentence {
    text: String,
    spoken: String,
    vocab_terms: Vec<String>,
}

#[derive(serde::Serialize)]
struct RecordingManifest {
    sentence_index: usize,
    text: String,
    spoken: String,
    vocab_terms: Vec<String>,
    profile: String,
    wav_path: String,
    sample_rate: u32,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Load sentences
    let sentences: Vec<EvalSentence> = std::fs::read_to_string(&args.sentences)
        .context("reading sentences file")?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).context("parsing sentence"))
        .collect::<Result<Vec<_>>>()?;

    println!("Loaded {} sentences", sentences.len());
    if args.start >= sentences.len() {
        println!("Nothing to record (start={} >= total={})", args.start, sentences.len());
        return Ok(());
    }

    // Create output directory
    let profile_dir = PathBuf::from(&args.output).join(&args.profile);
    std::fs::create_dir_all(&profile_dir)?;

    // Open manifest file (append mode for resuming)
    let manifest_path = profile_dir.join("manifest.jsonl");
    let mut manifest = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&manifest_path)?;

    // Set up audio
    let host = cpal::default_host();
    let device = host.default_input_device()
        .context("no input device found")?;
    let device_name = device.name().unwrap_or_else(|_| "unknown".to_string());
    let config = device.default_input_config()?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;

    println!("Profile:     {}", args.profile);
    println!("Input:       {} ({} Hz, {} ch)", device_name, sample_rate, channels);
    println!("Output:      {}", profile_dir.display());
    println!("Sentences:   {}-{} of {}", args.start, sentences.len() - 1, sentences.len());
    println!();
    println!("Controls:");
    println!("  Space/Enter  = start/stop recording");
    println!("  S            = skip sentence");
    println!("  R            = re-record last");
    println!("  Q/Esc        = quit");
    println!();

    terminal::enable_raw_mode()?;
    let _cleanup = RawModeGuard;

    let mut i = args.start;
    while i < sentences.len() {
        let sentence = &sentences[i];
        let wav_name = format!("{:04}.wav", i);
        let wav_path = profile_dir.join(&wav_name);

        // Show the sentence
        print!("\r\x1b[2J\x1b[H"); // clear screen
        println!("┌─ Sentence {}/{} ─────────────────────────────────────", i + 1, sentences.len());
        println!("│");
        println!("│  \x1b[1;37m{}\x1b[0m", sentence.text);
        println!("│");
        println!("│  \x1b[2m(say: {})\x1b[0m", sentence.spoken);
        println!("│");
        println!("│  vocab: {}", sentence.vocab_terms.join(", "));
        println!("│");
        println!("└─ Press \x1b[1mSpace\x1b[0m to start recording, \x1b[1mS\x1b[0m to skip, \x1b[1mQ\x1b[0m to quit");
        std::io::stdout().flush()?;

        // Wait for start
        match wait_for_key()? {
            KeyAction::Record => {}
            KeyAction::Skip => { i += 1; continue; }
            KeyAction::Quit => break,
            KeyAction::Redo => continue,
        }

        // Record
        let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
        let buf_clone = buffer.clone();

        let stream = device.build_input_stream(
            &config.clone().into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let mut buf = buf_clone.lock().unwrap();
                // Mix to mono if multi-channel
                if channels == 1 {
                    buf.extend_from_slice(data);
                } else {
                    for chunk in data.chunks(channels) {
                        let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                        buf.push(mono);
                    }
                }
            },
            |err| eprintln!("Audio error: {err}"),
            None,
        )?;

        stream.play()?;
        println!("\r\x1b[K  \x1b[1;31m● RECORDING\x1b[0m — speak now, press \x1b[1mSpace\x1b[0m when done");
        std::io::stdout().flush()?;

        // Wait for stop
        match wait_for_key()? {
            KeyAction::Quit => {
                drop(stream);
                break;
            }
            _ => {}
        }

        drop(stream);

        let samples = buffer.lock().unwrap();
        let duration = samples.len() as f32 / sample_rate as f32;

        if samples.is_empty() || duration < 0.3 {
            println!("  \x1b[33mToo short ({:.1}s), skipping\x1b[0m", duration);
            continue;
        }

        // Write WAV (16kHz mono for ASR compatibility)
        write_wav_16k(&wav_path, &samples, sample_rate)?;
        println!("  \x1b[32m✓\x1b[0m Saved {:.1}s → {}", duration, wav_name);

        // Write manifest entry
        let entry = RecordingManifest {
            sentence_index: i,
            text: sentence.text.clone(),
            spoken: sentence.spoken.clone(),
            vocab_terms: sentence.vocab_terms.clone(),
            profile: args.profile.clone(),
            wav_path: wav_name.clone(),
            sample_rate: 16000,
        };
        serde_json::to_writer(&mut manifest, &entry)?;
        manifest.write_all(b"\n")?;
        manifest.flush()?;

        println!("  Press \x1b[1mSpace\x1b[0m for next, \x1b[1mR\x1b[0m to redo, \x1b[1mQ\x1b[0m to quit");
        std::io::stdout().flush()?;

        match wait_for_key()? {
            KeyAction::Redo => continue, // re-record same sentence
            KeyAction::Quit => break,
            _ => { i += 1; }
        }
    }

    println!("\n\x1b[1mDone!\x1b[0m Recordings in {}", profile_dir.display());
    println!("Manifest: {}", manifest_path.display());
    Ok(())
}

enum KeyAction {
    Record, // Space/Enter — start or stop
    Skip,
    Redo,
    Quit,
}

fn wait_for_key() -> Result<KeyAction> {
    loop {
        if let Event::Key(KeyEvent { code, modifiers, .. }) = event::read()? {
            match code {
                KeyCode::Char(' ') | KeyCode::Enter => return Ok(KeyAction::Record),
                KeyCode::Char('s') | KeyCode::Char('S') => return Ok(KeyAction::Skip),
                KeyCode::Char('r') | KeyCode::Char('R') => return Ok(KeyAction::Redo),
                KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => return Ok(KeyAction::Quit),
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => return Ok(KeyAction::Quit),
                _ => {}
            }
        }
    }
}

/// Write samples to a 16kHz mono WAV, resampling if needed.
fn write_wav_16k(path: &Path, samples: &[f32], source_rate: u32) -> Result<()> {
    let samples_16k = if source_rate == 16000 {
        samples.to_vec()
    } else {
        // Simple linear resampling for recording (quality is fine for eval)
        let ratio = 16000.0 / source_rate as f64;
        let out_len = (samples.len() as f64 * ratio) as usize;
        let mut out = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let src_pos = i as f64 / ratio;
            let idx = src_pos as usize;
            let frac = src_pos - idx as f64;
            let s0 = samples.get(idx).copied().unwrap_or(0.0);
            let s1 = samples.get(idx + 1).copied().unwrap_or(s0);
            out.push(s0 + (s1 - s0) * frac as f32);
        }
        out
    };

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in &samples_16k {
        writer.write_sample((s * 32767.0f32).clamp(-32768.0, 32767.0) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

/// RAII guard to restore terminal on exit/panic.
struct RawModeGuard;
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}
