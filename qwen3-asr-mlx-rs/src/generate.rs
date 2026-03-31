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
