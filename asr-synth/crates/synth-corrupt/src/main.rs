use anyhow::Result;
use clap::Parser;

mod cmudict;
mod corrupt;
mod g2p;

#[derive(Parser)]
struct Args {
    /// Term to find confusions for (demo mode if omitted)
    term: Option<String>,

    /// Path to CMUdict file
    #[arg(long, default_value = "data/cmudict.txt")]
    dict: String,

    /// Max phoneme edit distance for single-word matches
    #[arg(long, default_value = "3")]
    max_dist: usize,

    /// Max results per category
    #[arg(long, default_value = "10")]
    max_results: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();

    eprintln!("Loading CMUdict from {}...", args.dict);
    let dict = cmudict::load(&args.dict)?;
    eprintln!("Loaded {} entries", dict.len());

    eprintln!("Building phoneme index...");
    let index = corrupt::PhonemeIndex::new(&dict);
    eprintln!("Index built: {} length buckets", index.bucket_count());

    if let Some(term) = &args.term {
        show_confusions(term, &dict, &index, &args);
    } else {
        let terms = [
            "serde", "tokio", "axum", "ratatui", "kajit", "reqwest",
            "facet", "clippy", "nextest", "backtraces", "minijinja",
            "pulldown-cmark", "bearcove", "fasterthanlime",
        ];
        for term in terms {
            show_confusions(term, &dict, &index, &args);
        }
    }

    Ok(())
}

fn show_confusions(
    term: &str,
    dict: &cmudict::CmuDict,
    index: &corrupt::PhonemeIndex,
    args: &Args,
) {
    let phonemes = g2p::word_to_phonemes(term, dict);
    let ipa = g2p::word_to_ipa(term);
    println!("\n{}", "=".repeat(60));
    println!("Term: {}  →  ARPAbet: {}  IPA: {}", term, phonemes.join(" "), ipa);

    let singles = index.find_single_word(&phonemes, args.max_dist, args.max_results);
    let doubles = index.find_two_word(&phonemes, 2, args.max_results);

    if !singles.is_empty() {
        println!("  Single-word:");
        for (word, dist) in &singles {
            println!("    {:<25} (dist={})", word, dist);
        }
    }
    if !doubles.is_empty() {
        println!("  Two-word:");
        for (phrase, dist) in &doubles {
            println!("    {:<30} (dist={})", phrase, dist);
        }
    }
    if singles.is_empty() && doubles.is_empty() {
        println!("  (no confusions found)");
    }
}
