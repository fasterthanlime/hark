//! Streaming transcription state machine for Qwen3-ASR.
//!
//! Ports the chunked audio buffering, VAD gating, prefix rollback, and
//! incremental encoder caching from qwen3-asr-rs. All knobs are preserved.

use mlx_rs::error::Exception;
use mlx_rs::Array;

use crate::encoder::EncoderCache;
use crate::generate;
use crate::mel::MelExtractor;
use crate::model::Qwen3ASRModel;

// ── VAD constants ───────────────────────────────────────────────────────

const VAD_WINDOW_SIZE: usize = 160; // 10ms at 16kHz
const VAD_SPEECH_RMS_THRESHOLD: f32 = 0.01; // ~-40dBFS
const POST_SPEECH_SILENCE_RMS_THRESHOLD: f32 = 0.006; // ~-44dBFS

// ── StreamingOptions ────────────────────────────────────────────────────

/// Options for streaming transcription.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct StreamingOptions {
    /// Audio chunk size in seconds. Default: 2.0
    pub chunk_size_sec: f32,
    /// Number of initial chunks before prefix conditioning kicks in. Default: 2
    pub unfixed_chunk_num: usize,
    /// Number of tokens to roll back from the end when building prefix. Default: 5
    pub unfixed_token_num: usize,
    /// Maximum new tokens per streaming step. Default: 32
    pub max_new_tokens_streaming: usize,
    /// Maximum new tokens for the final flush. Default: 512
    pub max_new_tokens_final: usize,
    /// Force a specific language (e.g., "english"). None for auto-detect.
    pub language: Option<String>,
    /// Context text from previous session for cold-start vocabulary consistency.
    pub initial_text: Option<String>,
}

impl Default for StreamingOptions {
    fn default() -> Self {
        Self {
            chunk_size_sec: 2.0,
            unfixed_chunk_num: 2,
            unfixed_token_num: 5,
            max_new_tokens_streaming: 32,
            max_new_tokens_final: 512,
            language: None,
            initial_text: None,
        }
    }
}

impl StreamingOptions {
    pub fn with_chunk_size_sec(mut self, v: f32) -> Self { self.chunk_size_sec = v; self }
    pub fn with_unfixed_chunk_num(mut self, v: usize) -> Self { self.unfixed_chunk_num = v; self }
    pub fn with_unfixed_token_num(mut self, v: usize) -> Self { self.unfixed_token_num = v; self }
    pub fn with_max_new_tokens_streaming(mut self, v: usize) -> Self { self.max_new_tokens_streaming = v; self }
    pub fn with_max_new_tokens_final(mut self, v: usize) -> Self { self.max_new_tokens_final = v; self }
    pub fn with_language(mut self, v: impl Into<String>) -> Self { self.language = Some(v.into()); self }
    pub fn with_initial_text(mut self, v: impl Into<String>) -> Self { self.initial_text = Some(v.into()); self }
}

// ── StreamingState ──────────────────────────────────────────────────────

pub struct StreamingState {
    /// Unconsumed audio samples (partial chunk).
    buffer: Vec<f32>,
    /// All audio samples accumulated from the start.
    audio_accum: Vec<f32>,
    /// Number of samples per chunk.
    chunk_size_samples: usize,
    /// Number of chunks processed so far.
    pub chunk_id: usize,
    /// Raw generated token IDs from last generation (full sequence including prefix).
    raw_token_ids: Vec<u32>,
    /// Streaming configuration.
    pub options: StreamingOptions,
    /// Current detected language.
    pub language: String,
    /// Current best transcription text.
    pub text: String,
    /// Encoder cache for incremental encoding.
    encoder_cache: EncoderCache,
    /// Whether speech has been detected (VAD gate).
    speech_detected: bool,
    /// Tokenizer for decoding output tokens.
    tokenizer: tokenizers::Tokenizer,
    /// Mel extractor.
    mel_extractor: MelExtractor,
    /// Pre-tokenized "language {lang}<asr_text>" tokens.
    language_tokens: Vec<i32>,
    asr_text_tokens: Vec<i32>,
}

// ── Public API ──────────────────────────────────────────────────────────

impl StreamingState {
    pub fn new(options: StreamingOptions, tokenizer: tokenizers::Tokenizer) -> Self {
        let chunk_size_samples = (options.chunk_size_sec * 16000.0) as usize;
        let language = options.language.clone().unwrap_or_else(|| "english".to_string());

        // Pre-tokenize the language header
        let lang_header = format!("language {language}");
        let language_tokens = tokenize_to_i32(&tokenizer, &lang_header);
        let asr_text_tokens = tokenize_to_i32(&tokenizer, "<asr_text>");

        Self {
            buffer: Vec::new(),
            audio_accum: Vec::new(),
            chunk_size_samples,
            chunk_id: 0,
            raw_token_ids: Vec::new(),
            options,
            language,
            text: String::new(),
            encoder_cache: EncoderCache::new(),
            speech_detected: false,
            tokenizer,
            mel_extractor: MelExtractor::new(400, 160, 128, 16000),
            language_tokens,
            asr_text_tokens,
        }
    }
}

fn tokenize_to_i32(tokenizer: &tokenizers::Tokenizer, text: &str) -> Vec<i32> {
    tokenizer
        .encode(text, false)
        .map(|enc| enc.get_ids().iter().map(|&id| id as i32).collect())
        .unwrap_or_default()
}

/// Feed audio samples (16 kHz f32) into the streaming state.
/// Returns Some(text) when a chunk boundary is crossed and inference runs.
pub fn feed_audio(
    model: &mut Qwen3ASRModel,
    state: &mut StreamingState,
    samples: &[f32],
) -> Result<Option<String>, Exception> {
    feed_audio_inner(model, state, samples, false)
}

/// Feed audio while finalizing — does NOT drop low-energy post-speech chunks.
pub fn feed_audio_finalizing(
    model: &mut Qwen3ASRModel,
    state: &mut StreamingState,
    samples: &[f32],
) -> Result<Option<String>, Exception> {
    feed_audio_inner(model, state, samples, true)
}

/// Finalize streaming: flush buffer and run final inference with high token budget.
pub fn finish_streaming(
    model: &mut Qwen3ASRModel,
    state: &mut StreamingState,
) -> Result<String, Exception> {
    flush_remaining_buffer(state);

    if state.audio_accum.is_empty() {
        return Ok(state.text.clone());
    }

    run_streaming_step(model, state, state.options.max_new_tokens_final)?;
    Ok(state.text.clone())
}

// ── Internals ───────────────────────────────────────────────────────────

fn feed_audio_inner(
    model: &mut Qwen3ASRModel,
    state: &mut StreamingState,
    samples: &[f32],
    finalizing: bool,
) -> Result<Option<String>, Exception> {
    // VAD gate
    if !state.speech_detected {
        if let Some(onset) = detect_speech_onset(samples) {
            state.speech_detected = true;
            state.buffer.extend_from_slice(&samples[onset..]);
        } else {
            return Ok(None);
        }
    } else {
        state.buffer.extend_from_slice(samples);
    }

    // Try to drain a chunk
    if !try_drain_chunk(state) {
        return Ok(None);
    }

    // Drop silent post-speech chunks (unless finalizing)
    if !finalizing && state.chunk_id > 1 {
        let chunk_start = state.audio_accum.len() - state.chunk_size_samples;
        let chunk_samples = &state.audio_accum[chunk_start..];
        let rms = compute_rms(chunk_samples);
        if rms < POST_SPEECH_SILENCE_RMS_THRESHOLD {
            return Ok(None);
        }
    }

    run_streaming_step(model, state, state.options.max_new_tokens_streaming)?;
    Ok(Some(state.text.clone()))
}

fn run_streaming_step(
    model: &mut Qwen3ASRModel,
    state: &mut StreamingState,
    max_new_tokens: usize,
) -> Result<(), Exception> {
    // Encode audio incrementally
    let (mel_data, n_mels, n_frames) = state.mel_extractor.extract(&state.audio_accum)
        .map_err(|e| Exception::custom(format!("mel: {e}")))?;
    let mel = Array::from_slice(&mel_data, &[n_mels as i32, n_frames as i32]);
    let audio_features = model.encode_incremental(&mel, &mut state.encoder_cache)?;
    let audio_features = mlx_rs::ops::expand_dims(&audio_features, 0)?;

    // Build prefix
    let prefix_ids = compute_prefix_ids(state);

    // Generate
    let generated = generate::generate_streaming(
        model,
        &audio_features,
        prefix_ids,
        &state.language_tokens,
        &state.asr_text_tokens,
        max_new_tokens,
    )?;

    // Combine prefix + generated
    let all_ids = combine_prefix_and_generated(state, &generated);

    // Decode
    let ids_u32: Vec<u32> = all_ids.iter().map(|&t| t as u32).collect();
    let text = state.tokenizer
        .decode(&ids_u32, true)
        .unwrap_or_default();

    state.raw_token_ids = ids_u32;
    state.text = text;

    Ok(())
}

fn compute_prefix_ids(state: &StreamingState) -> Option<&[u32]> {
    if state.chunk_id <= state.options.unfixed_chunk_num {
        return None; // cold start
    }
    if state.raw_token_ids.is_empty() {
        return None;
    }
    let keep = state.raw_token_ids.len().saturating_sub(state.options.unfixed_token_num);
    if keep == 0 {
        return None;
    }
    Some(&state.raw_token_ids[..keep])
}

fn combine_prefix_and_generated(state: &StreamingState, generated: &[i32]) -> Vec<i32> {
    if state.raw_token_ids.is_empty() || state.chunk_id <= state.options.unfixed_chunk_num {
        return generated.to_vec();
    }
    let keep = state.raw_token_ids.len().saturating_sub(state.options.unfixed_token_num);
    if keep == 0 {
        return generated.to_vec();
    }
    let mut combined: Vec<i32> = state.raw_token_ids[..keep].iter().map(|&t| t as i32).collect();
    combined.extend_from_slice(generated);
    combined
}

fn try_drain_chunk(state: &mut StreamingState) -> bool {
    if state.buffer.len() < state.chunk_size_samples {
        return false;
    }
    let chunk: Vec<f32> = state.buffer.drain(..state.chunk_size_samples).collect();
    state.audio_accum.extend_from_slice(&chunk);
    state.chunk_id += 1;
    true
}

fn flush_remaining_buffer(state: &mut StreamingState) {
    if !state.buffer.is_empty() {
        state.audio_accum.extend(state.buffer.drain(..));
        state.chunk_id += 1;
    }
}

// ── VAD ─────────────────────────────────────────────────────────────────

fn detect_speech_onset(samples: &[f32]) -> Option<usize> {
    let mut consecutive_speech = 0;
    let mut first_speech_idx = 0;

    for (i, window) in samples.chunks(VAD_WINDOW_SIZE).enumerate() {
        let rms = compute_rms(window);
        if rms >= VAD_SPEECH_RMS_THRESHOLD {
            if consecutive_speech == 0 {
                first_speech_idx = i * VAD_WINDOW_SIZE;
            }
            consecutive_speech += 1;
            if consecutive_speech >= 2 {
                return Some(first_speech_idx);
            }
        } else {
            consecutive_speech = 0;
        }
    }
    None
}

fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}
