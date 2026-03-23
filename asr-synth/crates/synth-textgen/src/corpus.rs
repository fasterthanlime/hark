use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize)]
pub struct VocabEntry {
    /// Written form (target for correction model): `SHA-256`, `--doc`, `.asm`
    pub term: String,
    /// How a human would say it aloud: `shaw two fifty six`, `dash dash doc`, `dot asm`
    pub spoken: String,
}

/// Extract interesting technical terms from markdown files in <root>/*/docs/**/*.md
pub fn extract_vocab(root: &str) -> Result<Vec<VocabEntry>> {
    let mut terms = HashSet::new();

    // Walk <root>/*/docs/ looking for markdown files
    let root = Path::new(root);
    let entries = std::fs::read_dir(root)?;
    for entry in entries.flatten() {
        let docs_dir = entry.path().join("docs");
        if docs_dir.is_dir() {
            walk_md(&docs_dir, &mut terms)?;
        }
    }

    // Also add some well-known terms that are commonly misrecognized
    for term in SEED_VOCAB {
        terms.insert(term.to_string());
    }

    let mut vocab: Vec<VocabEntry> = terms
        .into_iter()
        .map(|term| {
            let spoken = to_spoken(&term);
            VocabEntry { term, spoken }
        })
        .collect();
    vocab.sort_by(|a, b| a.term.to_lowercase().cmp(&b.term.to_lowercase()));
    Ok(vocab)
}

fn walk_md(dir: &Path, terms: &mut HashSet<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_md(&path, terms)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            extract_from_file(&path, terms);
        }
    }
    Ok(())
}

fn extract_from_file(path: &Path, terms: &mut HashSet<String>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };

    for word in content.split_whitespace() {
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
        if clean.is_empty() {
            continue;
        }

        // Backtick-wrapped code terms (most valuable)
        if word.starts_with('`') && word.ends_with('`') && word.len() > 2 {
            let inner = &word[1..word.len() - 1];
            let inner = inner.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
            if is_interesting_term(inner) {
                terms.insert(inner.to_string());
            }
            continue;
        }

        // CamelCase or mixed-case identifiers
        if is_interesting_term(clean) {
            terms.insert(clean.to_string());
        }
    }
}

fn is_interesting_term(s: &str) -> bool {
    if s.len() < 3 || s.len() > 30 {
        return false;
    }
    // Skip pure numbers
    if s.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    // Skip common English words
    if STOP_WORDS.contains(&s.to_lowercase().as_str()) {
        return false;
    }
    // Must contain at least one letter
    if !s.chars().any(|c| c.is_alphabetic()) {
        return false;
    }
    // Skip anything that looks like a path, URL, or file reference
    if s.contains('/') || s.contains('.') || s.contains(':') || s.contains('[')
        || s.contains(']') || s.contains('(') || s.contains(')')
        || s.contains('{') || s.contains('}') || s.contains('=')
        || s.contains('"') || s.contains('\'') || s.contains('\\')
        || s.contains(',') || s.contains(';') || s.contains('<')
        || s.contains('>') || s.contains('|') || s.contains('!')
        || s.contains('?') || s.contains('#') || s.contains('@')
        || s.contains('$') || s.contains('%') || s.contains('+')
    {
        return false;
    }
    // Must be pronounceable-ish: at least 50% letters
    let letter_count = s.chars().filter(|c| c.is_alphabetic()).count();
    if letter_count * 2 < s.len() {
        return false;
    }

    // Interesting if: contains underscore/hyphen, has mixed case, or has digits mixed with letters
    let has_separator = s.contains('_') || s.contains('-');
    let has_upper = s.chars().any(|c| c.is_uppercase());
    let has_lower = s.chars().any(|c| c.is_lowercase());
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    let is_mixed_case = has_upper && has_lower;

    has_separator || is_mixed_case || has_digit
}

/// Convert a written technical term to how a human would say it aloud.
///
/// Examples:
///   `SHA-256`    → `shaw two fifty six`
///   `--doc`      → `dash dash doc`
///   `.asm`       → `dot asm`
///   `serde`      → `serde`
///   `gRPC`       → `g r p c`
///   `OAuth`      → `o auth`
///   `snake_case` → `snake case`
///   `CamelCase`  → `camel case`
fn to_spoken(term: &str) -> String {
    // Check overrides first
    let lower = term.to_lowercase();
    if let Some(&spoken) = PRONUNCIATION_OVERRIDES.iter().find_map(|(k, v)| {
        if k.eq_ignore_ascii_case(term) { Some(v) } else { None }
    }) {
        return spoken.to_string();
    }

    let mut result = String::new();
    let mut chars = term.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '-' => {
                if !result.is_empty() && !result.ends_with(' ') {
                    result.push(' ');
                }
                // Check if it's a flag-style double dash
                if chars.peek() == Some(&'-') {
                    result.push_str("dash dash");
                    chars.next();
                } else if result.is_empty() || result.ends_with("dash ") {
                    result.push_str("dash");
                } else {
                    // Hyphen in compound word: just use space
                }
                result.push(' ');
            }
            '_' => {
                if !result.is_empty() && !result.ends_with(' ') {
                    result.push(' ');
                }
            }
            '.' => {
                if !result.is_empty() && !result.ends_with(' ') {
                    result.push(' ');
                }
                result.push_str("dot ");
            }
            c if c.is_uppercase() => {
                // CamelCase split: insert space before uppercase if preceded by lowercase
                if !result.is_empty() && !result.ends_with(' ') {
                    let prev = result.chars().last().unwrap();
                    if prev.is_lowercase() {
                        result.push(' ');
                    }
                }
                result.push(c.to_lowercase().next().unwrap());
            }
            c if c.is_ascii_digit() => {
                // Collect full number
                let mut num = String::new();
                num.push(c);
                while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                    num.push(chars.next().unwrap());
                }
                if !result.is_empty() && !result.ends_with(' ') {
                    result.push(' ');
                }
                result.push_str(&number_to_words(&num));
                result.push(' ');
            }
            c => {
                result.push(c);
            }
        }
    }

    let spoken = result.split_whitespace().collect::<Vec<_>>().join(" ");
    if spoken.is_empty() { lower } else { spoken }
}

fn number_to_words(num: &str) -> String {
    // Simple number pronunciation for common cases
    match num {
        "0" => "zero".to_string(),
        "1" => "one".to_string(),
        "2" => "two".to_string(),
        "3" => "three".to_string(),
        "4" => "four".to_string(),
        "5" => "five".to_string(),
        "6" => "six".to_string(),
        "7" => "seven".to_string(),
        "8" => "eight".to_string(),
        "9" => "nine".to_string(),
        "10" => "ten".to_string(),
        "16" => "sixteen".to_string(),
        "32" => "thirty two".to_string(),
        "64" => "sixty four".to_string(),
        "128" => "one twenty eight".to_string(),
        "256" => "two fifty six".to_string(),
        "512" => "five twelve".to_string(),
        "1024" => "ten twenty four".to_string(),
        _ => {
            // Fall back to digit-by-digit for unknown numbers
            num.chars()
                .map(|c| match c {
                    '0' => "zero",
                    '1' => "one",
                    '2' => "two",
                    '3' => "three",
                    '4' => "four",
                    '5' => "five",
                    '6' => "six",
                    '7' => "seven",
                    '8' => "eight",
                    '9' => "nine",
                    _ => "",
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

/// Manual pronunciation overrides for terms where the rules don't work
const PRONUNCIATION_OVERRIDES: &[(&str, &str)] = &[
    ("SHA-256", "shaw two fifty six"),
    ("SHA-512", "shaw five twelve"),
    ("SHA-1", "shaw one"),
    ("gRPC", "g r p c"),
    ("OAuth", "o auth"),
    ("OAuth2", "o auth two"),
    ("JWT", "j w t"),
    ("GGUF", "g g u f"),
    ("GGML", "g g m l"),
    ("ONNX", "onyx"),
    ("MLX", "m l x"),
    ("LoRA", "laura"),
    ("QLoRA", "q laura"),
    ("SQLite", "s q lite"),
    ("PostgreSQL", "postgres q l"),
    ("WebSocket", "web socket"),
    ("HuggingFace", "hugging face"),
    ("ffmpeg", "f f mpeg"),
    ("serde", "ser dee"),
    ("tokio", "tokyo"),
    ("axum", "axum"),
    ("reqwest", "request"),
    ("wgpu", "w g p u"),
    ("ratatui", "rata t u i"),
    ("i386", "i three eighty six"),
    ("x86_64", "x eighty six sixty four"),
    ("arm64", "arm sixty four"),
    ("aarch64", "a arch sixty four"),
    ("f32", "f thirty two"),
    ("f64", "f sixty four"),
    ("i32", "i thirty two"),
    ("i64", "i sixty four"),
    ("u8", "u eight"),
    ("u16", "u sixteen"),
    ("u32", "u thirty two"),
    ("u64", "u sixty four"),
    ("Blake3", "blake three"),
];

const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "are", "was", "were",
    "been", "have", "has", "had", "not", "but", "can", "will", "all", "each",
    "which", "their", "there", "when", "would", "make", "like", "just", "over",
    "such", "take", "also", "into", "than", "them", "very", "some", "could",
    "they", "other", "then", "its", "about", "use", "how", "any", "these",
    "may", "should", "does", "more", "most", "only", "what", "where", "why",
    "here", "still", "both", "between", "own", "under", "never", "being",
];

/// Well-known technical terms that are commonly misrecognized by ASR
const SEED_VOCAB: &[&str] = &[
    // Rust ecosystem
    "serde", "tokio", "axum", "hyper", "candle", "rubato", "rustfft",
    "wgpu", "naga", "ratatui", "clap", "anyhow", "thiserror",
    "tracing", "rayon", "crossbeam", "mio", "reqwest", "ureq",
    // ML/AI terms
    "GGUF", "GGML", "safetensors", "ONNX", "MLX", "LoRA", "QLoRA",
    // Tools & services
    "HuggingFace", "GitHub", "Xcode", "Homebrew", "ffmpeg",
    // General tech
    "WebSocket", "gRPC", "protobuf", "SQLite", "PostgreSQL",
    "OAuth", "JWT", "SHA-256", "Blake3",
];
