use anyhow::Result;
use clap::Parser;
use rand::prelude::*;
use std::io::Write;
use std::path::Path;

#[derive(Parser)]
#[command(about = "Prepare training data from corrupted corpus + run MLX-LM LoRA training")]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Convert corpus JSONL → MLX-LM completions format (train/valid/test splits)
    Prepare {
        /// Input corpus JSONL (from synth-corrupt generate)
        #[arg(short, long, default_value = "data/corpus_5k.jsonl")]
        input: String,

        /// Output directory for train.jsonl / valid.jsonl / test.jsonl
        #[arg(short, long, default_value = "training/data")]
        output: String,

        /// Number of identity (no-change) examples
        #[arg(long, default_value = "95000")]
        identity_count: usize,

        /// Claude history for identity examples
        #[arg(long, default_value = "~/.claude/history.jsonl")]
        claude_history: String,

        /// Codex history for identity examples
        #[arg(long, default_value = "~/.codex/history.jsonl")]
        codex_history: String,

        /// Train/valid/test split ratios
        #[arg(long, default_value = "0.8")]
        train_ratio: f64,
    },
    /// Run MLX-LM LoRA training (wraps uvx)
    Train {
        /// Training data directory
        #[arg(long, default_value = "training/data")]
        data: String,

        /// Adapter output directory
        #[arg(long, default_value = "training/adapters")]
        adapters: String,

        /// Base model
        #[arg(long, default_value = "Qwen/Qwen2.5-0.5B")]
        model: String,

        /// Number of iterations
        #[arg(long, default_value = "1000")]
        iters: usize,

        /// Batch size
        #[arg(long, default_value = "1")]
        batch_size: usize,

        /// Number of LoRA layers
        #[arg(long, default_value = "4")]
        num_layers: usize,
    },
}

fn main() -> Result<()> {
    match Args::parse().cmd {
        Cmd::Prepare { input, output, identity_count, claude_history, codex_history, train_ratio } => {
            prepare(&input, &output, identity_count, &claude_history, &codex_history, train_ratio)
        }
        Cmd::Train { data, adapters, model, iters, batch_size, num_layers } => {
            train(&data, &adapters, &model, iters, batch_size, num_layers)
        }
    }
}

fn prepare(input: &str, output: &str, identity_count: usize, claude_history: &str, codex_history: &str, train_ratio: f64) -> Result<()> {
    let content = std::fs::read_to_string(input)?;
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    eprintln!("Read {} corpus entries from {}", lines.len(), input);

    let mut examples = Vec::new();
    let mut rng = rand::rng();

    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line)?;
        let original = v["original"].as_str().unwrap_or("");
        let corrupted = v["corrupted"].as_str().unwrap_or("");
        let corruptions = v["corruptions_applied"].as_array();

        if original.is_empty() || corrupted.is_empty() {
            continue;
        }

        let vocab: Vec<String> = corruptions
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| c["term"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let vocab_str = vocab.join(" ");

        let prompt = format!(
            "<vocab> {} <noisy> {} <correct>",
            vocab_str, corrupted
        );
        examples.push(serde_json::json!({
            "prompt": prompt,
            "completion": format!(" {}<|endoftext|>", original),
        }));
    }

    let n_corrections = examples.len();

    // Load identity examples from conversation history (real clean text)
    let mut identity_pool: Vec<String> = Vec::new();
    for path in [claude_history, codex_history] {
        let expanded = shellexpand::tilde(path).to_string();
        if let Ok(content) = std::fs::read_to_string(&expanded) {
            for line in content.lines() {
                if let Ok(d) = serde_json::from_str::<serde_json::Value>(line) {
                    let text = d["display"].as_str()
                        .or_else(|| d["text"].as_str())
                        .unwrap_or("");
                    if text.len() >= 20 && text.len() <= 200
                        && !text.contains("[Pasted")
                        && !text.contains("[Image")
                        && !text.starts_with('/')
                    {
                        identity_pool.push(text.to_string());
                    }
                }
            }
        }
    }
    eprintln!("Identity pool: {} messages from history", identity_pool.len());

    // Sample identity examples
    let n_identity = identity_count.min(identity_pool.len());
    for text in identity_pool.choose_multiple(&mut rng, n_identity) {
        let prompt = format!("<vocab>  <noisy> {} <correct>", text);
        examples.push(serde_json::json!({
            "prompt": prompt,
            "completion": format!(" {}<|endoftext|>", text),
        }));
    }

    // Shuffle
    examples.shuffle(&mut rng);

    // Split
    let n = examples.len();
    let n_train = (n as f64 * train_ratio) as usize;
    let n_remaining = n - n_train;
    let n_valid = n_remaining / 2;
    let n_test = n_remaining - n_valid;

    let train = &examples[..n_train];
    let valid = &examples[n_train..n_train + n_valid];
    let test = &examples[n_train + n_valid..];

    // Write
    std::fs::create_dir_all(output)?;
    write_jsonl(&format!("{}/train.jsonl", output), train)?;
    write_jsonl(&format!("{}/valid.jsonl", output), valid)?;
    write_jsonl(&format!("{}/test.jsonl", output), test)?;

    eprintln!(
        "Wrote {} train, {} valid, {} test to {}",
        train.len(), valid.len(), test.len(), output
    );
    eprintln!(
        "({} correction + {} identity examples)",
        examples.len() - n_identity.min(lines.len()),
        n_identity.min(lines.len())
    );

    Ok(())
}

fn write_jsonl(path: &str, entries: &[serde_json::Value]) -> Result<()> {
    let mut f = std::fs::File::create(path)?;
    for entry in entries {
        serde_json::to_writer(&mut f, entry)?;
        f.write_all(b"\n")?;
    }
    f.flush()?;
    Ok(())
}

fn train(data: &str, adapters: &str, model: &str, iters: usize, batch_size: usize, num_layers: usize) -> Result<()> {
    eprintln!("=== ASR Correction Model Training ===");
    eprintln!("Model:    {model}");
    eprintln!("Data:     {data}");
    eprintln!("Adapters: {adapters}");
    eprintln!("Iters:    {iters}");

    let status = std::process::Command::new("uvx")
        .args([
            "--from", "mlx-lm",
            "mlx_lm.lora",
            "--model", model,
            "--data", data,
            "--train",
            "--iters", &iters.to_string(),
            "--batch-size", &batch_size.to_string(),
            "--num-layers", &num_layers.to_string(),
            "--adapter-path", adapters,
            "--mask-prompt",
        ])
        .status()?;

    if !status.success() {
        anyhow::bail!("Training failed with exit code: {:?}", status.code());
    }

    eprintln!("Training complete. Adapters saved to {adapters}");
    Ok(())
}
