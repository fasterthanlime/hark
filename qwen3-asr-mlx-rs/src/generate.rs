//! Autoregressive text generation for Qwen3-ASR.

use mlx_rs::error::Exception;
use mlx_rs::ops;
use mlx_rs::ops::indexing::{self, IndexOp};
use mlx_rs::Array;

use crate::decoder::KVCache;
use crate::model::{Qwen3ASRModel, EOS_TOKEN_IDS};

const REPETITION_THRESHOLD: usize = 20;

/// Greedy autoregressive generation.
///
/// 1. Prefill: process prompt + audio, populate KV cache
/// 2. Decode: generate one token at a time until EOS or max_tokens
pub fn generate(
    model: &mut Qwen3ASRModel,
    input_ids: &Array,
    audio_features: &Array,
    position_ids: &Array,
    max_new_tokens: usize,
) -> Result<Vec<i32>, Exception> {
    let mut cache = Some(model.create_cache());

    // Prefill
    let logits = model.prefill(input_ids, audio_features, position_ids, &mut cache)?;
    let mut token = argmax(&logits)?;
    let mut generated = vec![token];

    if is_eos(token) || max_new_tokens <= 1 {
        return Ok(strip_eos(generated));
    }

    let seq_len = input_ids.shape()[1];

    // Autoregressive decode
    for step in 1..max_new_tokens {
        let next_ids = Array::from_slice(&[token], &[1, 1]);
        let pos = (seq_len as i32) + (step as i32);
        let pos_arr = Array::from_slice(&[pos, pos, pos], &[1, 3, 1]);

        let logits = model.step(&next_ids, &pos_arr, &mut cache)?;
        token = argmax(&logits)?;
        generated.push(token);

        if is_eos(token) {
            break;
        }

        if detect_repetition(&generated) {
            break;
        }

        // Periodically materialize to avoid unbounded graph growth
        if step % 8 == 0 {
            logits.eval()?;
        }
    }

    Ok(strip_eos(generated))
}

fn argmax(logits: &Array) -> Result<i32, Exception> {
    let flat = logits.reshape(&[-1])?;
    let idx = indexing::argmax(&flat, None)?;
    Ok(idx.item::<i32>())
}

fn is_eos(token: i32) -> bool {
    EOS_TOKEN_IDS.contains(&token)
}

fn strip_eos(mut tokens: Vec<i32>) -> Vec<i32> {
    if let Some(&last) = tokens.last() {
        if is_eos(last) {
            tokens.pop();
        }
    }
    tokens
}

fn detect_repetition(tokens: &[i32]) -> bool {
    if tokens.len() < REPETITION_THRESHOLD {
        return false;
    }
    let last = tokens[tokens.len() - 1];
    tokens[tokens.len() - REPETITION_THRESHOLD..]
        .iter()
        .all(|&t| t == last)
}

// Chat template token IDs
const TOK_IM_START: i32 = 151644;
const TOK_IM_END: i32 = 151645;
const TOK_SYSTEM: i32 = 8948;
const TOK_USER: i32 = 872;
const TOK_ASSISTANT: i32 = 77091;
const TOK_NEWLINE: i32 = 198;

use crate::model::{AUDIO_START_TOKEN_ID, AUDIO_PAD_TOKEN_ID, AUDIO_END_TOKEN_ID};

/// Generate for streaming: builds prompt internally, supports prefix token injection.
///
/// Returns (all_token_ids, generated_token_ids) where all_token_ids includes
/// any prefix tokens prepended.
pub fn generate_streaming(
    model: &mut Qwen3ASRModel,
    audio_features: &Array,
    prefix_ids: Option<&[u32]>,
    language_tokens: &[i32],
    asr_text_tokens: &[i32],
    max_new_tokens: usize,
) -> Result<Vec<i32>, Exception> {
    let n_audio_tokens = audio_features.shape()[1] as usize;

    // Build prompt: <|im_start|>system\n<|im_end|>\n<|im_start|>user\n<|audio_start|>{pads}<|audio_end|><|im_end|>\n<|im_start|>assistant\n
    let mut prompt: Vec<i32> = vec![
        TOK_IM_START, TOK_SYSTEM, TOK_NEWLINE,
        TOK_IM_END, TOK_NEWLINE,
        TOK_IM_START, TOK_USER, TOK_NEWLINE,
        AUDIO_START_TOKEN_ID,
    ];
    prompt.extend(std::iter::repeat_n(AUDIO_PAD_TOKEN_ID, n_audio_tokens));
    prompt.extend_from_slice(&[
        AUDIO_END_TOKEN_ID, TOK_IM_END, TOK_NEWLINE,
        TOK_IM_START, TOK_ASSISTANT, TOK_NEWLINE,
    ]);
    // Append language and <asr_text> tokens
    prompt.extend_from_slice(language_tokens);
    prompt.extend_from_slice(asr_text_tokens);

    // Append prefix tokens if any
    if let Some(prefix) = prefix_ids {
        prompt.extend(prefix.iter().map(|&t| t as i32));
    }

    let seq_len = prompt.len();
    let input_ids = Array::from_slice(&prompt, &[1, seq_len as i32]);

    let positions: Vec<i32> = (0..seq_len as i32).collect();
    let pos_arr = Array::from_slice(&positions, &[1, 1, seq_len as i32]);
    let position_ids = ops::broadcast_to(&pos_arr, &[1, 3, seq_len as i32])?;

    let mut cache = Some(model.create_cache());

    // Prefill
    let logits = model.prefill(&input_ids, audio_features, &position_ids, &mut cache)?;
    let mut token = argmax(&logits)?;
    let mut generated = vec![token];

    if is_eos(token) || max_new_tokens <= 1 {
        return Ok(strip_eos(generated));
    }

    // Decode
    for step in 1..max_new_tokens {
        let next_ids = Array::from_slice(&[token], &[1, 1]);
        let pos = (seq_len as i32) + (step as i32);
        let pos_arr = Array::from_slice(&[pos, pos, pos], &[1, 3, 1]);

        let logits = model.step(&next_ids, &pos_arr, &mut cache)?;
        token = argmax(&logits)?;
        generated.push(token);

        if is_eos(token) {
            break;
        }
        if detect_repetition(&generated) {
            break;
        }
        if step % 8 == 0 {
            logits.eval()?;
        }
    }

    Ok(strip_eos(generated))
}
