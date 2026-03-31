use anyhow::Result;
use qwen3_asr::{AsrInference, TranscribeOptions};
use std::path::Path;
use std::time::Instant;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: bench <model_dir> <audio.wav>");
        std::process::exit(1);
    }
    let model_dir = Path::new(&args[1]);
    let wav_path = &args[2];

    let device = qwen3_asr::best_device();
    eprintln!("Device: {device:?}");

    let t0 = Instant::now();
    let engine = AsrInference::load(model_dir, device)?;
    eprintln!("Model loaded in {:.0}ms", t0.elapsed().as_millis());

    for run in 0..3 {
        let t0 = Instant::now();
        let result = engine.transcribe(wav_path, TranscribeOptions::default())?;
        let ms = t0.elapsed().as_millis();
        eprintln!("Run {}: {:.0}ms — {}", run + 1, ms, result.text);
    }

    Ok(())
}
