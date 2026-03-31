use std::f32::consts::PI;

use mlx_rs::module::Module;
use mlx_rs::nn;
use mlx_rs::ops;
use mlx_rs::Array;

use crate::config::AudioEncoderConfig;
use crate::error::AsrError;
use crate::weights::Weights;

/// Windowed segment threshold — use per-window execution above this.
const WINDOWED_SEGMENT_MIN_WINDOWS: usize = 20;

/// Fixed sinusoidal position embeddings (not learned, not in weights).
///
/// Uses the Qwen3-ASR formula: `log_timescale_increment = log(10000) / (half_dim - 1)`
/// which differs from standard Transformer PE by using `half_dim - 1` in denominator.
struct SinusoidalPE {
    pe: Array, // (max_positions, embedding_dim)
}

impl SinusoidalPE {
    fn new(max_positions: usize, embedding_dim: usize) -> Self {
        let half_dim = embedding_dim / 2;
        let log_timescale_increment = (10000.0f32).ln() / (half_dim as f32 - 1.0);

        let inv_timescales: Vec<f32> = (0..half_dim)
            .map(|i| (-log_timescale_increment * i as f32).exp())
            .collect();
        let inv_t = Array::from_slice(&inv_timescales, &[1, half_dim as i32]);

        let positions: Vec<f32> = (0..max_positions).map(|i| i as f32).collect();
        let pos = Array::from_slice(&positions, &[max_positions as i32, 1]);

        // Outer product: (max_positions, half_dim)
        let scaled_time = ops::multiply(&pos, &inv_t).unwrap();
        let sin_part = ops::sin(&scaled_time).unwrap();
        let cos_part = ops::cos(&scaled_time).unwrap();

        // Concatenate sin and cos: (max_positions, embedding_dim)
        let pe = ops::concatenate(&[&sin_part, &cos_part], -1).unwrap();

        Self { pe }
    }

    /// Get PE for the first `len` positions.
    fn get(&self, len: usize) -> Array {
        self.pe.index((..len as i32, ..))
    }
}

/// Bidirectional multi-head attention for the audio encoder.
/// Uses bias on all projections. No causal mask, no RoPE.
struct AudioAttention {
    q_proj: nn::Linear,
    k_proj: nn::Linear,
    v_proj: nn::Linear,
    out_proj: nn::Linear,
    num_heads: usize,
    head_dim: usize,
}

impl AudioAttention {
    fn load(prefix: &str, d_model: usize, num_heads: usize, weights: &Weights) -> Result<Self, AsrError> {
        let head_dim = d_model / num_heads;

        let q_proj = load_linear_with_bias(weights, &format!("{prefix}.q_proj"), d_model, d_model)?;
        let k_proj = load_linear_with_bias(weights, &format!("{prefix}.k_proj"), d_model, d_model)?;
        let v_proj = load_linear_with_bias(weights, &format!("{prefix}.v_proj"), d_model, d_model)?;
        let out_proj = load_linear_with_bias(weights, &format!("{prefix}.out_proj"), d_model, d_model)?;

        Ok(Self {
            q_proj,
            k_proj,
            v_proj,
            out_proj,
            num_heads,
            head_dim,
        })
    }

    /// Forward pass. x: (B, L, D), mask: optional (1, 1, L, L)
    fn forward(&mut self, x: &Array, mask: Option<&Array>) -> Result<Array, AsrError> {
        let shape = x.shape();
        let b = shape[0];
        let l = shape[1];
        let h = self.num_heads as i32;
        let dh = self.head_dim as i32;

        let q = self.q_proj.forward(x).map_err(|e| AsrError::Inference(e.to_string()))?;
        let k = self.k_proj.forward(x).map_err(|e| AsrError::Inference(e.to_string()))?;
        let v = self.v_proj.forward(x).map_err(|e| AsrError::Inference(e.to_string()))?;

        // (B, L, D) → (B, L, H, Dh) → (B, H, L, Dh)
        let q = q.reshape(&[b, l, h, dh]).unwrap().transpose_axes(&[0, 2, 1, 3]).unwrap();
        let k = k.reshape(&[b, l, h, dh]).unwrap().transpose_axes(&[0, 2, 1, 3]).unwrap();
        let v = v.reshape(&[b, l, h, dh]).unwrap().transpose_axes(&[0, 2, 1, 3]).unwrap();

        // Scaled dot-product attention
        let scale = (self.head_dim as f32).sqrt();
        let attn = ops::multiply(&ops::matmul(&q, &k.transpose_axes(&[0, 1, 3, 2]).unwrap()).unwrap(),
            &Array::from_f32(1.0 / scale)).unwrap();

        let attn = if let Some(mask) = mask {
            ops::add(&attn, mask).unwrap()
        } else {
            attn
        };

        let attn = ops::softmax(&attn, &[-1][..]).unwrap();
        let out = ops::matmul(&attn, &v).unwrap();

        // (B, H, L, Dh) → (B, L, H, Dh) → (B, L, D)
        let d = (self.num_heads * self.head_dim) as i32;
        let out = out.transpose_axes(&[0, 2, 1, 3]).unwrap().reshape(&[b, l, d]).unwrap();

        self.out_proj.forward(&out).map_err(|e| AsrError::Inference(e.to_string()))
    }
}

/// Pre-norm transformer layer for the audio encoder.
struct AudioEncoderLayer {
    self_attn_layer_norm: nn::LayerNorm,
    self_attn: AudioAttention,
    final_layer_norm: nn::LayerNorm,
    fc1: nn::Linear,
    fc2: nn::Linear,
}

impl AudioEncoderLayer {
    fn load(
        prefix: &str,
        d_model: usize,
        num_heads: usize,
        ffn_dim: usize,
        weights: &Weights,
    ) -> Result<Self, AsrError> {
        let self_attn_layer_norm = load_layer_norm(weights, &format!("{prefix}.self_attn_layer_norm"), d_model)?;
        let self_attn = AudioAttention::load(&format!("{prefix}.self_attn"), d_model, num_heads, weights)?;
        let final_layer_norm = load_layer_norm(weights, &format!("{prefix}.final_layer_norm"), d_model)?;
        let fc1 = load_linear_with_bias(weights, &format!("{prefix}.fc1"), d_model, ffn_dim)?;
        let fc2 = load_linear_with_bias(weights, &format!("{prefix}.fc2"), ffn_dim, d_model)?;

        Ok(Self {
            self_attn_layer_norm,
            self_attn,
            final_layer_norm,
            fc1,
            fc2,
        })
    }

    fn forward(&mut self, x: &Array, mask: Option<&Array>) -> Result<Array, AsrError> {
        // Self-attention (pre-norm)
        let normed = self.self_attn_layer_norm.forward(x).map_err(|e| AsrError::Inference(e.to_string()))?;
        let attn_out = self.self_attn.forward(&normed, mask)?;
        let x = ops::add(x, &attn_out).unwrap();

        // FFN (pre-norm)
        let normed = self.final_layer_norm.forward(&x).map_err(|e| AsrError::Inference(e.to_string()))?;
        let fc1_out = self.fc1.forward(&normed).map_err(|e| AsrError::Inference(e.to_string()))?;
        let gelu_out = nn::gelu(&fc1_out);
        let fc2_out = self.fc2.forward(&gelu_out).map_err(|e| AsrError::Inference(e.to_string()))?;
        let x = ops::add(&x, &fc2_out).unwrap();

        Ok(x)
    }
}

/// Full audio encoder.
pub struct AudioEncoder {
    conv2d1: nn::Conv2d,
    conv2d2: nn::Conv2d,
    conv2d3: nn::Conv2d,
    conv_out: nn::Linear,
    embed_positions: SinusoidalPE,
    layers: Vec<AudioEncoderLayer>,
    ln_post: nn::LayerNorm,
    proj1: nn::Linear,
    proj2: nn::Linear,
    config: AudioEncoderConfig,
}

impl AudioEncoder {
    pub fn load(config: &AudioEncoderConfig, weights: &Weights) -> Result<Self, AsrError> {
        let prefix = "audio_tower";
        let dhs = config.downsample_hidden_size;
        let freq_after_conv = config.num_mel_bins / 8;

        let conv2d1 = load_conv2d(weights, &format!("{prefix}.conv2d1"), 1, dhs, 3, 2, 1)?;
        let conv2d2 = load_conv2d(weights, &format!("{prefix}.conv2d2"), dhs, dhs, 3, 2, 1)?;
        let conv2d3 = load_conv2d(weights, &format!("{prefix}.conv2d3"), dhs, dhs, 3, 2, 1)?;
        let conv_out = load_linear_no_bias(weights, &format!("{prefix}.conv_out"), dhs * freq_after_conv, config.d_model)?;

        let embed_positions = SinusoidalPE::new(config.max_source_positions, config.d_model);

        let mut layers = Vec::with_capacity(config.encoder_layers);
        for i in 0..config.encoder_layers {
            layers.push(AudioEncoderLayer::load(
                &format!("{prefix}.layers.{i}"),
                config.d_model,
                config.encoder_attention_heads,
                config.encoder_ffn_dim,
                weights,
            )?);
        }

        let ln_post = load_layer_norm(weights, &format!("{prefix}.ln_post"), config.d_model)?;
        let proj1 = load_linear_with_bias(weights, &format!("{prefix}.proj1"), config.d_model, config.d_model)?;
        let proj2 = load_linear_with_bias(weights, &format!("{prefix}.proj2"), config.d_model, config.output_dim)?;

        Ok(Self {
            conv2d1,
            conv2d2,
            conv2d3,
            conv_out,
            embed_positions,
            layers,
            ln_post,
            proj1,
            proj2,
            config: config.clone(),
        })
    }

    /// Encode a mel spectrogram. Returns (audio_features, n_tokens).
    /// mel shape: (n_mels, n_frames)
    pub fn encode(&mut self, mel: &Array) -> Result<Array, AsrError> {
        let total_frames = mel.shape()[1] as usize;
        let chunk_size = self.config.n_window * 2;
        let n_window_infer = self.config.n_window_infer;

        let n_full_chunks = total_frames / chunk_size;
        let mut chunk_token_lens: Vec<usize> = Vec::new();
        let mut chunk_conv_outputs: Vec<Array> = Vec::new();

        // Process full chunks
        if n_full_chunks > 0 {
            let full_frames = n_full_chunks * chunk_size;
            let full_mel = mel.index((.., ..full_frames as i32)); // (n_mels, full_frames)

            // Reshape to (n_full, n_mels, chunk_size) then add channel dim for NHWC
            let n_mels = mel.shape()[0];
            let full_mel = full_mel.reshape(&[n_mels, n_full_chunks as i32, chunk_size as i32]).unwrap()
                .transpose_axes(&[1, 0, 2]).unwrap(); // (n_full, n_mels, chunk_size)

            // NHWC: (n_full, H=n_mels, W=chunk_size, C=1)
            let x = ops::expand_dims(&full_mel, &[-1][..]).unwrap();
            let x = self.apply_conv_stem(&x)?;

            // (n_full, F', T', C) → (n_full, T', C, F') → (n_full, T', C*F')
            let sh = x.shape().to_vec();
            let (n, f_d, t_d, c_d) = (sh[0], sh[1], sh[2], sh[3]);
            let x = x.transpose_axes(&[0, 2, 3, 1]).unwrap()
                .reshape(&[n, t_d, c_d * f_d]).unwrap();

            // Flatten to (n_full * T', C*F')
            let x = x.reshape(&[n * t_d, c_d * f_d]).unwrap();
            chunk_conv_outputs.push(x);
            for _ in 0..n_full_chunks {
                chunk_token_lens.push(t_d as usize);
            }
        }

        // Process tail chunk
        let tail_start = n_full_chunks * chunk_size;
        if tail_start < total_frames {
            let tail_mel = mel.index((.., tail_start as i32..)); // (n_mels, tail_len)
            // NHWC: (1, n_mels, tail_len, 1)
            let x = ops::expand_dims(&tail_mel, &[0, -1][..]).unwrap();
            let x = self.apply_conv_stem(&x)?;

            let sh = x.shape().to_vec();
            let (f_d, t_d, c_d) = (sh[1], sh[2], sh[3]);
            let x = x.transpose_axes(&[0, 2, 3, 1]).unwrap()
                .reshape(&[1, t_d, c_d * f_d]).unwrap();
            chunk_token_lens.push(t_d as usize);
            chunk_conv_outputs.push(x.index((0, ..)));
        }

        if chunk_conv_outputs.is_empty() {
            return Err(AsrError::Inference("no audio frames".into()));
        }

        // Concatenate all chunks and project to d_model
        let refs: Vec<&Array> = chunk_conv_outputs.iter().collect();
        let x = ops::concatenate(&refs, 0).unwrap();
        let x = self.conv_out.forward(&x).map_err(|e| AsrError::Inference(e.to_string()))?;

        // Per-chunk sinusoidal PE (each chunk starts from position 0)
        let max_chunk_tokens = *chunk_token_lens.iter().max().unwrap();
        let pe = self.embed_positions.get(max_chunk_tokens);
        let mut pe_parts: Vec<Array> = Vec::new();
        for &ct in &chunk_token_lens {
            pe_parts.push(pe.index((..ct as i32, ..)));
        }
        let pe_refs: Vec<&Array> = pe_parts.iter().collect();
        let pe_full = ops::concatenate(&pe_refs, 0).unwrap();
        let x = ops::add(&x, &pe_full).unwrap();

        // Windowed attention
        let total_tokens = x.shape()[0] as usize;
        let tokens_per_full_chunk = chunk_token_lens[0];
        let tokens_per_window = tokens_per_full_chunk * (n_window_infer / chunk_size);

        let mut cu_seqlens: Vec<usize> = vec![0];
        let mut pos = 0;
        while pos < total_tokens {
            let window_end = (pos + tokens_per_window).min(total_tokens);
            cu_seqlens.push(window_end);
            pos = window_end;
        }

        let num_windows = cu_seqlens.len() - 1;
        // Add batch dim: (1, total_tokens, d_model)
        let mut x = ops::expand_dims(&x, &[0][..]).unwrap();

        if num_windows >= WINDOWED_SEGMENT_MIN_WINDOWS {
            // Per-window execution (avoids materializing dense mask)
            for layer in &mut self.layers {
                let mut parts: Vec<Array> = Vec::new();
                for w in 0..num_windows {
                    let s = cu_seqlens[w] as i32;
                    let e = cu_seqlens[w + 1] as i32;
                    let window = x.index((.., s..e, ..));
                    parts.push(layer.forward(&window, None)?);
                }
                let refs: Vec<&Array> = parts.iter().collect();
                x = ops::concatenate(&refs, 1).unwrap();
            }
        } else {
            let mask = create_windowed_mask(total_tokens, &cu_seqlens);
            for layer in &mut self.layers {
                x = layer.forward(&x, mask.as_ref())?;
            }
        }

        // Remove batch dim
        let x = x.index((0, ..));

        // Post-processing: LayerNorm → GELU(proj1) → proj2
        let x = self.ln_post.forward(&x).map_err(|e| AsrError::Inference(e.to_string()))?;
        let x = self.proj1.forward(&x).map_err(|e| AsrError::Inference(e.to_string()))?;
        let x = nn::gelu(&x);
        let x = self.proj2.forward(&x).map_err(|e| AsrError::Inference(e.to_string()))?;

        Ok(x) // (total_tokens, output_dim)
    }

    fn apply_conv_stem(&mut self, x: &Array) -> Result<Array, AsrError> {
        let x = self.conv2d1.forward(x).map_err(|e| AsrError::Inference(e.to_string()))?;
        let x = nn::gelu(&x);
        let x = self.conv2d2.forward(&x).map_err(|e| AsrError::Inference(e.to_string()))?;
        let x = nn::gelu(&x);
        let x = self.conv2d3.forward(&x).map_err(|e| AsrError::Inference(e.to_string()))?;
        let x = nn::gelu(&x);
        Ok(x)
    }
}

fn create_windowed_mask(seq_len: usize, cu_seqlens: &[usize]) -> Option<Array> {
    if cu_seqlens.len() <= 2 {
        return None;
    }

    // Build mask: 0.0 for same window, -1e9 for different window
    let mut mask_data = vec![0.0f32; seq_len * seq_len];
    for i in 0..seq_len {
        let win_i = cu_seqlens.windows(2).position(|w| i >= w[0] && i < w[1]).unwrap();
        for j in 0..seq_len {
            let win_j = cu_seqlens.windows(2).position(|w| j >= w[0] && j < w[1]).unwrap();
            if win_i != win_j {
                mask_data[i * seq_len + j] = -1e9;
            }
        }
    }

    let mask = Array::from_slice(&mask_data, &[seq_len as i32, seq_len as i32]);
    // (1, 1, L, L)
    Some(ops::expand_dims(&mask, &[0, 1][..]).unwrap())
}

// Helper: load nn::Linear with bias from weights
fn load_linear_with_bias(
    weights: &Weights,
    prefix: &str,
    _in_dim: usize,
    _out_dim: usize,
) -> Result<nn::Linear, AsrError> {
    let w = weights.get(&format!("{prefix}.weight"))?.clone();
    let b = weights.try_get(&format!("{prefix}.bias")).cloned();
    Ok(nn::Linear::new(w, b))
}

fn load_linear_no_bias(
    weights: &Weights,
    prefix: &str,
    _in_dim: usize,
    _out_dim: usize,
) -> Result<nn::Linear, AsrError> {
    let w = weights.get(&format!("{prefix}.weight"))?.clone();
    Ok(nn::Linear::new(w, None))
}

fn load_layer_norm(
    weights: &Weights,
    prefix: &str,
    _dim: usize,
) -> Result<nn::LayerNorm, AsrError> {
    let w = weights.get(&format!("{prefix}.weight"))?.clone();
    let b = weights.try_get(&format!("{prefix}.bias")).cloned();
    Ok(nn::LayerNorm::new(w, b, 1e-5))
}

fn load_conv2d(
    weights: &Weights,
    prefix: &str,
    _in_channels: usize,
    _out_channels: usize,
    _kernel_size: usize,
    stride: (usize, usize),
    padding: (usize, usize),
) -> Result<nn::Conv2d, AsrError> {
    let w = weights.get(&format!("{prefix}.weight"))?.clone();
    let b = weights.try_get(&format!("{prefix}.bias")).cloned();
    Ok(nn::Conv2d::new(w, b, stride, padding))
}
