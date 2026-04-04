use std::collections::HashMap;

use bee_phonetic::{
    enumerate_transcript_spans_with, feature_tokens_for_ipa, query_index, verify_shortlist,
    RetrievalQuery, SeedDataset, TranscriptAlignmentToken, TranscriptSpan, VerifiedCandidate,
};
use beeml::g2p::CachedEspeakG2p;

#[derive(Debug, Clone)]
struct Config {
    term: String,
    max_span_words: usize,
    shortlist_limit: usize,
    verify_limit: usize,
    recordings_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            term: String::new(),
            max_span_words: 4,
            shortlist_limit: 10,
            verify_limit: 10,
            recordings_limit: 3,
        }
    }
}

#[derive(Debug)]
struct SpanCaseDebug {
    span: TranscriptSpan,
    shortlist: Vec<bee_phonetic::RetrievalCandidate>,
    verified: Vec<VerifiedCandidate>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args()?;
    if config.term.trim().is_empty() {
        return Err("--term is required".into());
    }

    let dataset = SeedDataset::load_canonical()?;
    dataset.validate()?;
    let index = dataset.phonetic_index();
    let mut g2p = CachedEspeakG2p::english_with_persist_path(Some(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/phonetic-retrieval-eval-g2p-cache.tsv"),
    ))?;

    let term_aliases = index
        .aliases
        .iter()
        .filter(|alias| alias.term.eq_ignore_ascii_case(&config.term))
        .collect::<Vec<_>>();

    println!("term={}", config.term);
    println!("target_aliases={}", term_aliases.len());
    for alias in &term_aliases {
        println!("--- alias");
        println!("text={}", alias.alias_text);
        println!("source={:?}", alias.alias_source);
        println!("ipa={}", alias.ipa_tokens.join(" "));
        println!("reduced={}", alias.reduced_ipa_tokens.join(" "));
        println!("features={}", alias.feature_tokens.join(" "));
        println!(
            "flags=acronym:{} digits:{} snake:{} camel:{} symbol:{}",
            alias.identifier_flags.acronym_like,
            alias.identifier_flags.has_digits,
            alias.identifier_flags.snake_like,
            alias.identifier_flags.camel_like,
            alias.identifier_flags.symbol_like
        );
    }

    let rows = dataset
        .recording_examples
        .iter()
        .filter(|row| row.term.eq_ignore_ascii_case(&config.term))
        .take(config.recordings_limit)
        .collect::<Vec<_>>();

    println!("recordings={}", rows.len());

    for (idx, row) in rows.into_iter().enumerate() {
        println!();
        println!("===== recording {} =====", idx + 1);
        println!("text={}", row.text);
        println!("transcript={}", row.transcript);

        let spans = enumerate_transcript_spans_with::<_, TranscriptAlignmentToken>(
            &row.transcript,
            config.max_span_words,
            None,
            |text| g2p.ipa_tokens(text).ok().flatten(),
        );

        let mut cases = spans
            .into_iter()
            .map(|span| {
                let shortlist = query_index(
                    &index,
                    &RetrievalQuery {
                        text: span.text.clone(),
                        ipa_tokens: span.ipa_tokens.clone(),
                        reduced_ipa_tokens: span.reduced_ipa_tokens.clone(),
                        feature_tokens: Vec::new(),
                        token_count: (span.token_end - span.token_start) as u8,
                    },
                    config.shortlist_limit,
                );
                let verified = verify_shortlist(&span, &shortlist, &index, config.verify_limit);
                SpanCaseDebug {
                    span,
                    shortlist,
                    verified,
                }
            })
            .collect::<Vec<_>>();

        cases.sort_by(|a, b| score_case(b, &config.term).total_cmp(&score_case(a, &config.term)));

        for case in cases.iter().take(12) {
            print_case(case, &config.term);
        }
    }

    Ok(())
}

fn score_case(case: &SpanCaseDebug, term: &str) -> f32 {
    let target_verified = case
        .verified
        .iter()
        .find(|candidate| candidate.term.eq_ignore_ascii_case(term))
        .map(|candidate| 1000.0 + candidate.phonetic_score * 100.0 + candidate.coarse_score * 10.0);
    if let Some(score) = target_verified {
        return score;
    }

    let target_shortlist = case
        .shortlist
        .iter()
        .find(|candidate| candidate.term.eq_ignore_ascii_case(term))
        .map(|candidate| 500.0 + candidate.coarse_score * 10.0);
    if let Some(score) = target_shortlist {
        return score;
    }

    case.verified
        .first()
        .map(|candidate| candidate.phonetic_score * 100.0 + candidate.coarse_score * 10.0)
        .or_else(|| {
            case.shortlist
                .first()
                .map(|candidate| candidate.coarse_score * 10.0)
        })
        .unwrap_or(0.0)
}

fn print_case(case: &SpanCaseDebug, term: &str) {
    let target_in_shortlist = case
        .shortlist
        .iter()
        .any(|candidate| candidate.term.eq_ignore_ascii_case(term));
    let target_in_verified = case
        .verified
        .iter()
        .any(|candidate| candidate.term.eq_ignore_ascii_case(term));

    println!("--- span");
    println!(
        "text={} tokens={}:{} target_shortlist={} target_verified={}",
        case.span.text,
        case.span.token_start,
        case.span.token_end,
        target_in_shortlist,
        target_in_verified
    );
    println!("ipa={}", case.span.ipa_tokens.join(" "));
    println!("reduced={}", case.span.reduced_ipa_tokens.join(" "));
    println!(
        "features={}",
        feature_tokens_for_ipa(&case.span.ipa_tokens).join(" ")
    );

    let verified_by_alias = case
        .verified
        .iter()
        .map(|candidate| (candidate.alias_id, candidate))
        .collect::<HashMap<_, _>>();

    for candidate in case.shortlist.iter().take(6) {
        println!(
            "  candidate term={} alias={}",
            candidate.term, candidate.alias_text
        );
        println!(
            "    source={:?} lane={:?} q={} q_total={} coarse={:.3} best_view={:.3} support={} token_bonus={:.2} phone_bonus={:.2} length_penalty={:.2}",
            candidate.alias_source,
            candidate.matched_view,
            candidate.qgram_overlap,
            candidate.total_qgram_overlap,
            candidate.coarse_score,
            candidate.best_view_score,
            candidate.cross_view_support,
            candidate.token_bonus,
            candidate.phone_bonus,
            candidate.extra_length_penalty
        );
        if let Some(verified) = verified_by_alias.get(&candidate.alias_id) {
            println!(
                "    verify token={:.3} feature={:.3} bonus={:.3} used_feature_bonus={} phonetic={:.3}",
                verified.token_score,
                verified.feature_score,
                verified.feature_bonus,
                verified.used_feature_bonus,
                verified.phonetic_score
            );
        } else {
            println!("    verify <not in verified shortlist>");
        }
    }
}

fn parse_args() -> Result<Config, Box<dyn std::error::Error>> {
    let mut config = Config::default();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--term" => {
                config.term = args.next().ok_or("missing value for --term")?;
            }
            "--max-span-words" => {
                config.max_span_words = args
                    .next()
                    .ok_or("missing value for --max-span-words")?
                    .parse()?;
            }
            "--shortlist-limit" => {
                config.shortlist_limit = args
                    .next()
                    .ok_or("missing value for --shortlist-limit")?
                    .parse()?;
            }
            "--verify-limit" => {
                config.verify_limit = args
                    .next()
                    .ok_or("missing value for --verify-limit")?
                    .parse()?;
            }
            "--recordings" => {
                config.recordings_limit = args
                    .next()
                    .ok_or("missing value for --recordings")?
                    .parse()?;
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    Ok(config)
}
