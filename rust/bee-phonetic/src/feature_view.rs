use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use rspanphon::featuretable::FeatureTable;

static FEATURE_TABLE: OnceLock<FeatureTable> = OnceLock::new();
static FEATURE_VECTOR_CACHE: OnceLock<Mutex<HashMap<String, Option<Vec<f32>>>>> = OnceLock::new();

pub fn feature_tokens_for_ipa(ipa_tokens: &[String]) -> Vec<String> {
    let table = FEATURE_TABLE.get_or_init(FeatureTable::new);
    ipa_tokens
        .iter()
        .map(|token| match table.ft.get(token) {
            Some(features) => encode_feature_vector(features),
            None => format!("unk:{token}"),
        })
        .collect()
}

pub fn feature_similarity(a: &[String], b: &[String]) -> Option<f32> {
    if a.is_empty() || b.is_empty() {
        return None;
    }

    let a_vecs = feature_vectors_for_ipa(a);
    let b_vecs = feature_vectors_for_ipa(b);
    feature_similarity_from_vectors(&a_vecs, &b_vecs, a.len().max(b.len()))
}

pub fn feature_vectors_for_ipa(ipa_tokens: &[String]) -> Vec<Vec<f32>> {
    ipa_tokens
        .iter()
        .filter_map(|token| feature_vector_for_token(token))
        .collect()
}

pub fn feature_similarity_from_vectors(
    a: &[Vec<f32>],
    b: &[Vec<f32>],
    max_token_len: usize,
) -> Option<f32> {
    let distance = feature_edit_distance(a, b)?;
    let max_len = max_token_len.max(1) as f32;
    Some((1.0 - (distance / max_len)).clamp(0.0, 1.0))
}

fn encode_feature_vector(features: &[i8]) -> String {
    let mut out = String::with_capacity(features.len());
    for feature in features {
        out.push(match feature {
            -1 => '-',
            0 => '0',
            1 => '+',
            _ => '?',
        });
    }
    out
}

fn feature_edit_distance(a: &[Vec<f32>], b: &[Vec<f32>]) -> Option<f32> {
    let mut prev = vec![0.0f32; b.len() + 1];
    let mut curr = vec![0.0f32; b.len() + 1];
    if a.is_empty() || b.is_empty() {
        return None;
    }

    for (j, by) in b.iter().enumerate() {
        prev[j + 1] = prev[j] + insertion_deletion_cost(by);
    }

    for ax in a {
        curr[0] = prev[0] + insertion_deletion_cost(ax);
        for (j, by) in b.iter().enumerate() {
            let del = prev[j + 1] + insertion_deletion_cost(ax);
            let ins = curr[j] + insertion_deletion_cost(by);
            let sub = prev[j] + substitution_cost(ax, by);
            curr[j + 1] = del.min(ins.min(sub));
        }
        prev.copy_from_slice(&curr);
    }

    Some(prev[b.len()])
}

fn substitution_cost(a: &[f32], b: &[f32]) -> f32 {
    if a == b {
        return 0.0;
    }

    let diff_sum = a
        .iter()
        .zip(b)
        .map(|(lhs, rhs)| (lhs - rhs).abs() / 2.0)
        .sum::<f32>();
    (diff_sum / a.len().max(1) as f32).clamp(0.0, 1.0)
}

fn insertion_deletion_cost(vec: &[f32]) -> f32 {
    let table = FEATURE_TABLE.get_or_init(FeatureTable::new);
    let syllabic = feature_flag(table, vec, "syl");
    let continuant = feature_flag(table, vec, "cont");
    let sonorant = feature_flag(table, vec, "son");
    let glottal = feature_flag(table, vec, "cg") || feature_flag(table, vec, "sg");

    if glottal {
        0.55
    } else if syllabic || sonorant {
        0.72
    } else if continuant {
        0.82
    } else {
        0.9
    }
}

fn feature_flag(table: &FeatureTable, vec: &[f32], name: &str) -> bool {
    let Some(idx) = table.fnames.iter().position(|feature| feature == name) else {
        return false;
    };
    vec.get(idx).copied().unwrap_or(0.0) > 0.0
}

pub fn feature_vector_for_token(token: &str) -> Option<Vec<f32>> {
    let cache = FEATURE_VECTOR_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let guard = cache.lock().expect("feature vector cache poisoned");
        if let Some(cached) = guard.get(token) {
            return cached.clone();
        }
    }

    let table = FEATURE_TABLE.get_or_init(FeatureTable::new);
    let computed = compute_feature_vector(table, token);
    cache
        .lock()
        .expect("feature vector cache poisoned")
        .insert(token.to_string(), computed.clone());
    computed
}

fn compute_feature_vector(table: &FeatureTable, token: &str) -> Option<Vec<f32>> {
    if let Some(vec) = table.ft.get(token) {
        return Some(vec.iter().map(|value| *value as f32).collect());
    }

    let simplified = strip_modifiers(token);
    if simplified != token {
        if let Some(vec) = table.ft.get(&simplified) {
            return Some(vec.iter().map(|value| *value as f32).collect());
        }
    }

    let phonemes = table.phonemes(token);
    if !phonemes.is_empty() {
        return average_feature_vectors(table, &phonemes);
    }

    let simplified_phonemes = table.phonemes(&simplified);
    if !simplified_phonemes.is_empty() {
        return average_feature_vectors(table, &simplified_phonemes);
    }

    None
}

fn average_feature_vectors(table: &FeatureTable, phonemes: &[String]) -> Option<Vec<f32>> {
    let vectors = phonemes
        .iter()
        .filter_map(|phoneme| table.ft.get(phoneme))
        .collect::<Vec<_>>();
    let first = vectors.first()?;
    let mut out = vec![0.0f32; first.len()];
    for vec in &vectors {
        for (idx, value) in vec.iter().enumerate() {
            out[idx] += *value as f32;
        }
    }
    let denom = vectors.len() as f32;
    for value in &mut out {
        *value /= denom;
    }
    Some(out)
}

fn strip_modifiers(token: &str) -> String {
    token
        .chars()
        .filter(|ch| !matches!(ch, 'ː' | '˞' | 'ʰ' | 'ʲ' | 'ʷ' | '̃' | '̩' | '̯'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_feature_tokens_for_known_ipa_segments() {
        let tokens = feature_tokens_for_ipa(&["m".to_string(), "i".to_string(), "ə".to_string()]);
        assert_eq!(tokens.len(), 3);
        assert!(
            tokens.iter().all(|token| !token.starts_with("unk:")),
            "{tokens:#?}"
        );
    }

    #[test]
    fn feature_similarity_prefers_closer_segments() {
        let exact = feature_similarity(
            &["m".to_string(), "i".to_string()],
            &["m".to_string(), "i".to_string()],
        )
        .unwrap();
        let close = feature_similarity(
            &["m".to_string(), "i".to_string()],
            &["m".to_string(), "ɪ".to_string()],
        )
        .unwrap();
        let far = feature_similarity(
            &["m".to_string(), "i".to_string()],
            &["k".to_string(), "u".to_string()],
        )
        .unwrap();
        assert!(exact > close);
        assert!(close > far);
    }

    #[test]
    fn feature_similarity_handles_modifier_and_diphthongish_tokens() {
        let token = feature_similarity(
            &["ɜː".to_string(), "k".to_string(), "ə".to_string()],
            &["eə".to_string(), "k".to_string(), "əʊ".to_string()],
        )
        .unwrap();
        assert!(token > 0.45, "{token}");
    }
}
