use anyhow::Result;
use rand::prelude::*;
use std::io::Write;
use std::path::Path;

/// Stats returned from the prepare step
#[derive(Debug, serde::Serialize)]
pub struct PrepareStats {
    pub correction_examples: usize,
    pub identity_examples: usize,
    pub train_count: usize,
    pub valid_count: usize,
    pub test_count: usize,
}

pub struct PrepareConfig {
    pub input: String,
    pub output: String,
    pub identity_count: usize,
    pub claude_history: String,
    pub codex_history: String,
    pub train_ratio: f64,
}

impl Default for PrepareConfig {
    fn default() -> Self {
        Self {
            input: "data/corpus_5k.jsonl".into(),
            output: "training/data".into(),
            identity_count: 95000,
            claude_history: "~/.claude/history.jsonl".into(),
            codex_history: "~/.codex/history.jsonl".into(),
            train_ratio: 0.8,
        }
    }
}

pub struct TrainConfig {
    pub data: String,
    pub adapters: String,
    pub model: String,
    pub iters: usize,
    pub batch_size: usize,
    pub num_layers: usize,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self {
            data: "training/data".into(),
            adapters: "training/adapters".into(),
            model: "Qwen/Qwen2.5-0.5B".into(),
            iters: 1000,
            batch_size: 1,
            num_layers: 4,
        }
    }
}

/// Prepare training data from corpus JSONL → MLX-LM completions format
pub fn prepare(config: &PrepareConfig, mut on_status: impl FnMut(&str)) -> Result<PrepareStats> {
    let content = std::fs::read_to_string(&config.input)?;
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    on_status(&format!("Read {} corpus entries from {}", lines.len(), config.input));

    let mut examples = Vec::new();
    let mut rng = rand::rng();

    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line)?;
        let original = v["original"].as_str().unwrap_or("");
        let parakeet = v["parakeet"].as_str().unwrap_or("");
        let qwen = v["qwen"].as_str().unwrap_or("");

        if original.is_empty() {
            continue;
        }

        let prompt = format!("<parakeet> {} <qwen> {} <correct>", parakeet, qwen);
        examples.push(serde_json::json!({
            "prompt": prompt,
            "completion": format!(" {}<|endoftext|>", original),
        }));
    }

    let n_corrections = examples.len();

    // Build markov chain from history + blog posts for identity examples
    let mut chain = synth_corrupt::markov::MarkovChain::new();
    let mut raw_texts: Vec<String> = Vec::new();

    for path in [&config.claude_history, &config.codex_history] {
        let expanded = shellexpand::tilde(path).to_string();
        if let Ok(content) = std::fs::read_to_string(&expanded) {
            for line in content.lines() {
                if let Ok(d) = serde_json::from_str::<serde_json::Value>(line) {
                    let text = d["display"]
                        .as_str()
                        .or_else(|| d["text"].as_str())
                        .unwrap_or("");
                    if text.len() >= 20
                        && text.len() <= 200
                        && !text.contains("[Pasted")
                        && !text.contains("[Image")
                        && !text.starts_with('/')
                    {
                        chain.feed(text);
                        raw_texts.push(text.to_string());
                    }
                }
            }
        }
    }

    // Blog posts from ~/bearcove/fasterthanli.me
    let blog_dir = shellexpand::tilde("~/bearcove/fasterthanli.me").to_string();
    let mut blog_paragraphs = 0usize;
    if let Ok(entries) = glob_md(&blog_dir) {
        for path in entries {
            if let Ok(content) = std::fs::read_to_string(&path) {
                let mut in_code_block = false;
                let mut current_para = String::new();

                let mut opts = pulldown_cmark::Options::empty();
                opts.insert(pulldown_cmark::Options::ENABLE_TABLES);
                for event in pulldown_cmark::Parser::new_ext(&content, opts) {
                    match event {
                        pulldown_cmark::Event::Start(pulldown_cmark::Tag::CodeBlock(_)) => {
                            in_code_block = true;
                        }
                        pulldown_cmark::Event::End(pulldown_cmark::TagEnd::CodeBlock) => {
                            in_code_block = false;
                        }
                        pulldown_cmark::Event::Text(text) if !in_code_block => {
                            current_para.push_str(&text);
                        }
                        pulldown_cmark::Event::SoftBreak
                        | pulldown_cmark::Event::HardBreak
                            if !in_code_block =>
                        {
                            current_para.push(' ');
                        }
                        // Table cells: add space between cells
                        pulldown_cmark::Event::End(pulldown_cmark::TagEnd::TableCell) => {
                            current_para.push(' ');
                        }
                        // Table rows: flush like paragraphs
                        pulldown_cmark::Event::End(
                            pulldown_cmark::TagEnd::TableHead | pulldown_cmark::TagEnd::TableRow,
                        ) => {
                            let clean = current_para.trim().to_string();
                            if clean.len() >= 30 && clean.len() <= 300 {
                                chain.feed(&clean);
                                blog_paragraphs += 1;
                            }
                            current_para.clear();
                        }
                        pulldown_cmark::Event::End(pulldown_cmark::TagEnd::Paragraph) => {
                            let clean = current_para.trim().to_string();
                            if clean.len() >= 30 && clean.len() <= 300 {
                                chain.feed(&clean);
                                blog_paragraphs += 1;
                            }
                            current_para.clear();
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    on_status(&format!("Blog: {} paragraphs from fasterthanli.me", blog_paragraphs));
    on_status(&format!(
        "Markov chain: {} transitions, {} raw texts",
        chain.transition_count(),
        raw_texts.len()
    ));

    let mut identity_generated = 0;
    for _ in 0..config.identity_count {
        let text = if !raw_texts.is_empty() && rng.random_bool(0.5) {
            raw_texts[rng.random_range(0..raw_texts.len())].clone()
        } else {
            let target_len: usize = 10 + rng.random_range(0..10);
            match chain.generate(&mut rng, target_len) {
                Some(t) => t,
                None => continue,
            }
        };

        let prompt = format!("<parakeet> {} <qwen> {} <correct>", text, text);
        examples.push(serde_json::json!({
            "prompt": prompt,
            "completion": format!(" {}<|endoftext|>", text),
        }));
        identity_generated += 1;
    }
    on_status(&format!("Generated {} identity examples", identity_generated));

    examples.shuffle(&mut rng);

    let n = examples.len();
    let n_train = (n as f64 * config.train_ratio) as usize;
    let n_remaining = n - n_train;
    let n_valid = n_remaining / 2;

    let train = &examples[..n_train];
    let valid = &examples[n_train..n_train + n_valid];
    let test = &examples[n_train + n_valid..];

    std::fs::create_dir_all(&config.output)?;
    write_jsonl(&format!("{}/train.jsonl", config.output), train)?;
    write_jsonl(&format!("{}/valid.jsonl", config.output), valid)?;
    write_jsonl(&format!("{}/test.jsonl", config.output), test)?;

    let stats = PrepareStats {
        correction_examples: n_corrections,
        identity_examples: identity_generated,
        train_count: train.len(),
        valid_count: valid.len(),
        test_count: test.len(),
    };

    on_status(&format!(
        "Wrote {} train, {} valid, {} test to {}",
        stats.train_count, stats.valid_count, stats.test_count, config.output
    ));

    Ok(stats)
}

/// Run MLX-LM LoRA training (wraps uvx)
pub fn train(config: &TrainConfig) -> Result<std::process::ExitStatus> {
    let status = std::process::Command::new("uvx")
        .args([
            "--from",
            "mlx-lm",
            "mlx_lm.lora",
            "--model",
            &config.model,
            "--data",
            &config.data,
            "--train",
            "--iters",
            &config.iters.to_string(),
            "--batch-size",
            &config.batch_size.to_string(),
            "--num-layers",
            &config.num_layers.to_string(),
            "--adapter-path",
            &config.adapters,
            "--mask-prompt",
        ])
        .status()?;
    Ok(status)
}

pub fn glob_md(root: &str) -> Result<Vec<std::path::PathBuf>> {
    let mut results = Vec::new();
    fn walk(dir: &Path, results: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, results);
                } else if path.extension().is_some_and(|e| e == "md") {
                    results.push(path);
                }
            }
        }
    }
    walk(Path::new(root), &mut results);
    Ok(results)
}

pub fn write_jsonl(path: &str, entries: &[serde_json::Value]) -> Result<()> {
    let mut f = std::fs::File::create(path)?;
    for entry in entries {
        serde_json::to_writer(&mut f, entry)?;
        f.write_all(b"\n")?;
    }
    f.flush()?;
    Ok(())
}
