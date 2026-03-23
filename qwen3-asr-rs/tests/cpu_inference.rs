//! Integration tests for CPU inference with BF16 safetensors weights.
//!
//! These tests verify that BF16 weights are automatically converted to F32 on
//! CPU, fixing the "unsupported dtype BF16 for op matmul" error.
//!
//! Requires a downloaded Qwen3-ASR model. Run with:
//!
//!   QWEN3_ASR_MODEL_DIR=/path/to/model cargo test --no-default-features --test cpu_inference -- --ignored

use std::path::PathBuf;

fn model_dir() -> PathBuf {
    std::env::var("QWEN3_ASR_MODEL_DIR")
        .map(PathBuf::from)
        .expect("Set QWEN3_ASR_MODEL_DIR to the model directory")
}

#[test]
#[ignore]
fn test_cpu_batch_inference() {
    let model_dir = model_dir();

    // Force CPU device — this is the scenario that fails without the fix
    let engine = qwen3_asr::AsrInference::load(&model_dir, candle_core::Device::Cpu)
        .expect("CPU load should succeed with BF16 weights");

    // 2 seconds of silence
    let samples = vec![0.0f32; 32000];
    let result = engine
        .transcribe_samples(&samples, qwen3_asr::TranscribeOptions::default())
        .expect("CPU inference should not fail");

    assert!(result.raw_output.len() <= 10000);
}

#[test]
#[ignore]
fn test_cpu_streaming_inference() {
    let model_dir = model_dir();

    let engine = qwen3_asr::AsrInference::load(&model_dir, candle_core::Device::Cpu)
        .expect("CPU load should succeed");

    let mut state = engine.init_streaming(qwen3_asr::StreamingOptions::default());
    let chunk = vec![0.0f32; 32000]; // 2 seconds
    let _ = engine
        .feed_audio(&mut state, &chunk)
        .expect("feed should not fail on CPU");
    let result = engine
        .finish_streaming(&mut state)
        .expect("finish should not fail on CPU");
    assert!(result.raw_output.len() <= 10000);
}
