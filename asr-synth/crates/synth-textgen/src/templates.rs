use crate::corpus::VocabEntry;
use rand::prelude::*;
use rand::rng;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize)]
pub struct GeneratedSentence {
    /// Written form: "I've been using serde for all my serialization needs."
    pub text: String,
    /// Spoken form: "I've been using sir day for all my serialization needs."
    pub spoken: String,
    /// Which vocab terms appear
    pub vocab_terms: Vec<String>,
}

/// Configuration for where to find real text sources
pub struct TextSources {
    /// Path to blog content directory (e.g. ~/bearcove/fasterthanli.me)
    pub blog_dir: Option<String>,
    /// Paths to JSONL chat history files
    pub history_files: Vec<String>,
}

impl Default for TextSources {
    fn default() -> Self {
        Self {
            blog_dir: Some(shellexpand::tilde("~/bearcove/fasterthanli.me").to_string()),
            history_files: vec![
                shellexpand::tilde("~/.claude/history.jsonl").to_string(),
                shellexpand::tilde("~/.codex/history.jsonl").to_string(),
            ],
        }
    }
}

/// Extract real sentences from blog posts and chat history that contain vocab terms.
/// Returns up to `count` sentences, prioritizing those with more/rarer vocab terms.
pub fn generate(
    vocab: &[VocabEntry],
    count: usize,
    overrides: Option<&HashMap<String, String>>,
    sources: Option<&TextSources>,
) -> Vec<GeneratedSentence> {
    let default_sources = TextSources::default();
    let sources = sources.unwrap_or(&default_sources);

    // Build a lookup: lowercase term → VocabEntry
    let term_lookup: HashMap<String, &VocabEntry> = vocab
        .iter()
        .map(|v| (v.term.to_lowercase(), v))
        .collect();

    // Collect all candidate sentences from real sources
    let mut candidates: Vec<(String, Vec<String>)> = Vec::new(); // (sentence, matched_terms)

    // Blog posts
    if let Some(ref blog_dir) = sources.blog_dir {
        extract_blog_sentences(blog_dir, &term_lookup, &mut candidates);
    }

    // Chat history
    for path in &sources.history_files {
        extract_history_sentences(path, &term_lookup, &mut candidates);
    }

    // Deduplicate by normalized text
    let mut seen = HashSet::new();
    candidates.retain(|(text, _)| {
        let key = text.to_lowercase();
        seen.insert(key)
    });

    // Sort: sentences with more vocab terms first, then shuffle within same count
    let mut rng = rng();
    candidates.shuffle(&mut rng);
    candidates.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    // Take up to count
    candidates.truncate(count);

    // Convert to GeneratedSentence with spoken forms
    candidates
        .into_iter()
        .map(|(text, terms)| {
            let spoken = build_spoken_form(&text, &terms, vocab, overrides);
            GeneratedSentence {
                text,
                spoken,
                vocab_terms: terms,
            }
        })
        .collect()
}

/// Build the spoken form of a sentence by replacing vocab terms with their spoken equivalents
fn build_spoken_form(
    text: &str,
    matched_terms: &[String],
    vocab: &[VocabEntry],
    overrides: Option<&HashMap<String, String>>,
) -> String {
    let mut result = text.to_string();

    // Sort by length descending to replace longer terms first (avoid partial matches)
    let mut terms_sorted: Vec<&str> = matched_terms.iter().map(|s| s.as_str()).collect();
    terms_sorted.sort_by(|a, b| b.len().cmp(&a.len()));

    for term in terms_sorted {
        // Find the spoken form
        let spoken = overrides
            .and_then(|o| o.get(term))
            .map(|s| s.as_str())
            .or_else(|| {
                vocab
                    .iter()
                    .find(|v| v.term.eq_ignore_ascii_case(term))
                    .map(|v| v.spoken.as_str())
            });

        if let Some(spoken) = spoken {
            if spoken.to_lowercase() != term.to_lowercase() {
                // Case-insensitive replace while preserving surrounding text
                let lower_result = result.to_lowercase();
                let lower_term = term.to_lowercase();
                if let Some(pos) = lower_result.find(&lower_term) {
                    result = format!(
                        "{}{}{}",
                        &result[..pos],
                        spoken,
                        &result[pos + term.len()..]
                    );
                }
            }
        }
    }

    result
}

/// Extract sentences from blog markdown files that contain vocab terms
fn extract_blog_sentences(
    blog_dir: &str,
    term_lookup: &HashMap<String, &VocabEntry>,
    out: &mut Vec<(String, Vec<String>)>,
) {
    let mut md_files = Vec::new();
    walk_md(Path::new(blog_dir), &mut md_files);

    for path in md_files {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };

        let mut in_code_block = false;
        let mut current_para = String::new();

        for event in pulldown_cmark::Parser::new(&content) {
            match event {
                pulldown_cmark::Event::Start(pulldown_cmark::Tag::CodeBlock(_)) => {
                    in_code_block = true;
                }
                pulldown_cmark::Event::End(pulldown_cmark::TagEnd::CodeBlock) => {
                    in_code_block = false;
                }
                pulldown_cmark::Event::Code(code) if !in_code_block => {
                    // Inline code: include the text content (not the backticks)
                    current_para.push_str(&code);
                }
                pulldown_cmark::Event::Text(text) if !in_code_block => {
                    current_para.push_str(&text);
                }
                pulldown_cmark::Event::SoftBreak | pulldown_cmark::Event::HardBreak
                    if !in_code_block =>
                {
                    current_para.push(' ');
                }
                pulldown_cmark::Event::End(pulldown_cmark::TagEnd::Paragraph) => {
                    flush_paragraph(&current_para, term_lookup, out);
                    current_para.clear();
                }
                _ => {}
            }
        }
    }
}

/// Extract sentences from JSONL chat history files
fn extract_history_sentences(
    path: &str,
    term_lookup: &HashMap<String, &VocabEntry>,
    out: &mut Vec<(String, Vec<String>)>,
) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };

    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let text = v["display"]
            .as_str()
            .or_else(|| v["text"].as_str())
            .unwrap_or("");
        if text.len() >= 20
            && text.len() <= 300
            && !text.contains("[Pasted")
            && !text.contains("[Image")
            && !text.starts_with('/')
        {
            flush_paragraph(text, term_lookup, out);
        }
    }
}

/// Check if a paragraph contains any vocab terms. If so, split into sentences
/// and add matching ones to the output.
fn flush_paragraph(
    para: &str,
    term_lookup: &HashMap<String, &VocabEntry>,
    out: &mut Vec<(String, Vec<String>)>,
) {
    // Normalize whitespace — collapse newlines/tabs to single spaces
    let clean: String = para.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.len() < 15 || clean.len() > 300 {
        return;
    }

    // Split paragraph into sentences (simple heuristic)
    let sentences = split_sentences(&clean);

    for sentence in sentences {
        let sentence = sentence.trim();
        if sentence.len() < 10 || sentence.len() > 120 {
            continue;
        }

        // Find vocab terms in this sentence
        let lower = sentence.to_lowercase();
        let mut matched = Vec::new();
        for (term_lower, entry) in term_lookup {
            // Word-boundary-ish match: check the term appears and isn't part of a larger word
            if let Some(pos) = lower.find(term_lower.as_str()) {
                let before_ok = pos == 0
                    || !sentence.as_bytes()[pos - 1].is_ascii_alphanumeric();
                let after_pos = pos + term_lower.len();
                let after_ok = after_pos >= sentence.len()
                    || !sentence.as_bytes()[after_pos].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    matched.push(entry.term.clone());
                }
            }
        }

        if !matched.is_empty() {
            matched.sort();
            matched.dedup();
            out.push((sentence.to_string(), matched));
        }
    }
}

/// Simple sentence splitter
fn split_sentences(text: &str) -> Vec<&str> {
    let mut sentences = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();

    for i in 0..bytes.len() {
        if (bytes[i] == b'.' || bytes[i] == b'!' || bytes[i] == b'?')
            && (i + 1 >= bytes.len() || bytes[i + 1] == b' ' || bytes[i + 1] == b'\n')
        {
            let end = i + 1;
            let s = &text[start..end];
            if !s.trim().is_empty() {
                sentences.push(s.trim());
            }
            start = end;
        }
    }
    // Remainder
    let rest = &text[start..];
    if !rest.trim().is_empty() && rest.trim().len() >= 10 {
        sentences.push(rest.trim());
    }

    // If we didn't split at all, use the whole thing
    if sentences.is_empty() && !text.trim().is_empty() {
        sentences.push(text.trim());
    }

    sentences
}

fn walk_md(dir: &Path, results: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_md(&path, results);
            } else if path.extension().is_some_and(|e| e == "md") {
                results.push(path);
            }
        }
    }
}
