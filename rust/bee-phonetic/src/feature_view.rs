use std::sync::OnceLock;

use rspanphon::featuretable::FeatureTable;

static FEATURE_TABLE: OnceLock<FeatureTable> = OnceLock::new();

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

    let table = FEATURE_TABLE.get_or_init(FeatureTable::new);
    let distance = feature_edit_distance(table, a, b)?;
    let max_len = a.len().max(b.len()) as f32;
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

fn feature_edit_distance(table: &FeatureTable, a: &[String], b: &[String]) -> Option<f32> {
    let mut prev = vec![0.0f32; b.len() + 1];
    let mut curr = vec![0.0f32; b.len() + 1];

    let any_known = a.iter().chain(b.iter()).any(|token| feature_vector(table, token).is_some());
    if !any_known {
        return None;
    }

    for (j, by) in b.iter().enumerate() {
        prev[j + 1] = prev[j] + insertion_deletion_cost(std::slice::from_ref(by), table);
    }

    for ax in a {
        curr[0] = prev[0] + insertion_deletion_cost(std::slice::from_ref(ax), table);
        for (j, by) in b.iter().enumerate() {
            let del = prev[j + 1] + insertion_deletion_cost(std::slice::from_ref(ax), table);
            let ins = curr[j] + insertion_deletion_cost(std::slice::from_ref(by), table);
            let sub = prev[j] + substitution_cost(table, ax, by);
            curr[j + 1] = del.min(ins.min(sub));
        }
        prev.copy_from_slice(&curr);
    }

    Some(prev[b.len()])
}

fn substitution_cost(table: &FeatureTable, a: &str, b: &str) -> f32 {
    if a == b {
        return 0.0;
    }

    let Some(a_vec) = feature_vector(table, a) else {
        return 1.0;
    };
    let Some(b_vec) = feature_vector(table, b) else {
        return 1.0;
    };

    let diff_sum = a_vec
        .iter()
        .zip(&b_vec)
        .map(|(lhs, rhs)| (lhs - rhs).abs() / 2.0)
        .sum::<f32>();
    (diff_sum / a_vec.len().max(1) as f32).clamp(0.0, 1.0)
}

fn insertion_deletion_cost(tokens: &[String], table: &FeatureTable) -> f32 {
    let token = match tokens.last() {
        Some(token) => token,
        None => return 0.0,
    };

    let Some(vec) = feature_vector(table, token) else {
        return 1.0;
    };

    let syllabic = feature_flag(table, &vec, "syl");
    let continuant = feature_flag(table, &vec, "cont");
    let sonorant = feature_flag(table, &vec, "son");
    let glottal = feature_flag(table, &vec, "cg") || feature_flag(table, &vec, "sg");

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

fn feature_vector(table: &FeatureTable, token: &str) -> Option<Vec<f32>> {
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
