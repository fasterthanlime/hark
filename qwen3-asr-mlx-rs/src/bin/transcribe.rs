use std::path::Path;
use std::time::Instant;

use mlx_rs::module::ModuleParametersExt;
use mlx_rs::ops;
use mlx_rs::ops::indexing::IndexOp;
use mlx_rs::Array;

use qwen3_asr_mlx::config::AsrConfig;
use qwen3_asr_mlx::generate;
use qwen3_asr_mlx::mel::{load_audio_wav, MelExtractor};
use qwen3_asr_mlx::model::{
    Qwen3ASRModel, AUDIO_END_TOKEN_ID, AUDIO_PAD_TOKEN_ID, AUDIO_START_TOKEN_ID,
};

// Chat template token IDs
const TOK_IM_START: i32 = 151644;
const TOK_IM_END: i32 = 151645;
const TOK_SYSTEM: i32 = 8948;
const TOK_USER: i32 = 872;
const TOK_ASSISTANT: i32 = 77091;
const TOK_NEWLINE: i32 = 198;

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: transcribe <model_dir> <audio.wav>");
        std::process::exit(1);
    }

    let model_dir = Path::new(&args[1]);
    let audio_path = &args[2];

    // 1. Load config
    let t0 = Instant::now();
    let config_path = model_dir.join("config.json");
    let config_str = std::fs::read_to_string(&config_path)?;
    let config: AsrConfig = serde_json::from_str(&config_str)?;
    let thinker = &config.thinker_config;
    println!("Config loaded in {:.0}ms", t0.elapsed().as_millis());
    println!(
        "  encoder: {} layers, d_model={}",
        thinker.audio_config.encoder_layers, thinker.audio_config.d_model
    );
    println!(
        "  decoder: {} layers, hidden_size={}",
        thinker.text_config.num_hidden_layers, thinker.text_config.hidden_size
    );

    // 2. Create model (random weights)
    let t0 = Instant::now();
    let mut model = Qwen3ASRModel::new(thinker)?;
    println!("Model created in {:.0}ms", t0.elapsed().as_millis());

    // 3. Load weights from safetensors
    let t0 = Instant::now();
    // Find all safetensors files
    let mut st_files: Vec<_> = std::fs::read_dir(model_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "safetensors"))
        .map(|e| e.path())
        .collect();
    st_files.sort();

    if st_files.is_empty() {
        anyhow::bail!("No .safetensors files found in {}", model_dir.display());
    }

    // Load weights with key remapping (strip thinker. prefix, transpose conv2d)
    let mut all_weights = std::collections::HashMap::new();
    for f in &st_files {
        println!("  Loading weights from {}", f.display());
        let tensors = Array::load_safetensors(f)?;
        all_weights.extend(tensors);
    }

    // Remap keys
    let mut remapped = std::collections::HashMap::new();
    for (key, value) in all_weights {
        let mut new_key = key.clone();
        let had_thinker = new_key.starts_with("thinker.");
        if had_thinker {
            new_key = new_key["thinker.".len()..].to_string();
        }
        // Transpose conv2d weights: PyTorch (out,in,kH,kW) → MLX (out,kH,kW,in)
        let value = if had_thinker
            && new_key.contains("conv2d")
            && new_key.ends_with(".weight")
            && value.ndim() == 4
        {
            value.transpose_axes(&[0, 2, 3, 1])?
        } else {
            value
        };
        remapped.insert(new_key, value);
    }

    println!("  Remapped {} weight keys", remapped.len());

    // Load into model using the flattened parameter tree
    use mlx_rs::module::ModuleParameters;
    let mut params = model.parameters_mut().flatten();
    let mut loaded = 0;
    let mut skipped = Vec::new();
    for (key, value) in &remapped {
        if let Some(param) = params.get_mut(&**key) {
            **param = value.clone();
            loaded += 1;
        } else {
            skipped.push(key.clone());
        }
    }
    println!("  Loaded {} params, skipped {} keys", loaded, skipped.len());
    if !skipped.is_empty() {
        for k in skipped.iter().take(10) {
            println!("    skipped: {}", k);
        }
        if skipped.len() > 10 {
            println!("    ... and {} more", skipped.len() - 10);
        }
    }
    // Eval after loading
    use mlx_rs::module::ModuleParametersExt;
    model.eval()?;
    println!("Weights loaded in {:.0}ms", t0.elapsed().as_millis());

    // 4. Load audio
    let t0 = Instant::now();
    let samples = load_audio_wav(audio_path, 16000)?;
    println!(
        "Audio loaded: {} samples ({:.1}s) in {:.0}ms",
        samples.len(),
        samples.len() as f64 / 16000.0,
        t0.elapsed().as_millis()
    );

    // 5. Compute mel spectrogram
    let t0 = Instant::now();
    let mel_extractor = MelExtractor::new(400, 160, 128, 16000);
    let (mel_data, n_mels, n_frames) = mel_extractor.extract(&samples)?;
    println!(
        "Mel: {}×{} frames in {:.0}ms",
        n_mels,
        n_frames,
        t0.elapsed().as_millis()
    );

    // Convert to MLX array: (n_mels, n_frames)
    let mel = Array::from_slice(&mel_data, &[n_mels as i32, n_frames as i32]);

    // 6. Encode audio
    let t0 = Instant::now();
    let audio_features = model.encode_audio(&mel)?;
    audio_features.eval()?;
    let n_audio_tokens = audio_features.shape()[0] as usize;
    let audio_dim = audio_features.shape()[1] as usize;
    println!(
        "Encoded: {} audio tokens × {} dim in {:.0}ms (expected dim={})",
        n_audio_tokens,
        audio_dim,
        t0.elapsed().as_millis(),
        thinker.text_config.hidden_size,
    );

    // Add batch dim: (1, n_audio_tokens, dim)
    let audio_features = mlx_rs::ops::expand_dims(&audio_features, 0)?;

    // 7. Build prompt
    let mut prompt_tokens: Vec<i32> = vec![
        TOK_IM_START,
        TOK_SYSTEM,
        TOK_NEWLINE,
        TOK_IM_END,
        TOK_NEWLINE,
        TOK_IM_START,
        TOK_USER,
        TOK_NEWLINE,
        AUDIO_START_TOKEN_ID,
    ];
    prompt_tokens.extend(std::iter::repeat_n(AUDIO_PAD_TOKEN_ID, n_audio_tokens));
    prompt_tokens.extend_from_slice(&[
        AUDIO_END_TOKEN_ID,
        TOK_IM_END,
        TOK_NEWLINE,
        TOK_IM_START,
        TOK_ASSISTANT,
        TOK_NEWLINE,
    ]);

    let seq_len = prompt_tokens.len();
    println!("Prompt: {} tokens ({} audio placeholders)", seq_len, n_audio_tokens);

    let input_ids = Array::from_slice(&prompt_tokens, &[1, seq_len as i32]);

    // Position IDs: (1, 3, seq_len) — all three dims use same positions
    let positions: Vec<i32> = (0..seq_len as i32).collect();
    let pos_arr = Array::from_slice(&positions, &[1, 1, seq_len as i32]);
    // Broadcast to (1, 3, seq_len)
    let position_ids = ops::broadcast_to(&pos_arr, &[1, 3, seq_len as i32])?;

    // 8. Generate
    let t0 = Instant::now();
    let output_tokens = generate::generate(
        &mut model,
        &input_ids,
        &audio_features,
        &position_ids,
        512,
    )?;
    let gen_ms = t0.elapsed().as_millis();
    println!(
        "Generated {} tokens in {:.0}ms ({:.1} tok/s)",
        output_tokens.len(),
        gen_ms,
        output_tokens.len() as f64 / (gen_ms as f64 / 1000.0)
    );

    // 9. Decode tokens with tokenizer
    let tokenizer_path = model_dir.join("tokenizer.json");
    if tokenizer_path.exists() {
        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("tokenizer: {e}"))?;
        let ids: Vec<u32> = output_tokens.iter().map(|&t| t as u32).collect();
        let text = tokenizer
            .decode(&ids, true)
            .map_err(|e| anyhow::anyhow!("decode: {e}"))?;
        println!("\nTranscription: {}", text);
    } else {
        println!("\nRaw tokens: {:?}", output_tokens);
        println!("(no tokenizer.json found at {})", tokenizer_path.display());
    }

    Ok(())
}
