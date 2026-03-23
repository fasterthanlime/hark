use crate::cmudict::CmuDict;

/// Get ARPAbet phonemes for a word. Uses CMUdict first, falls back to espeak-ng IPA → ARPAbet.
pub fn word_to_phonemes(word: &str, dict: &CmuDict) -> Vec<String> {
    let upper = word
        .to_uppercase()
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_string();

    // Try CMUdict first
    if let Some(phonemes) = dict.get(&upper) {
        return phonemes.clone();
    }

    // Fall back to espeak-ng → IPA → ARPAbet
    let ipa = word_to_ipa(word);
    ipa_to_arpabet(&ipa)
}

/// Get IPA pronunciation for a word using espeak-ng.
pub fn word_to_ipa(word: &str) -> String {
    match espeak_ng::text_to_ipa("en", word) {
        Ok(ipa) => ipa.trim().to_string(),
        Err(_) => word.to_lowercase(),
    }
}

/// Convert IPA string to ARPAbet phoneme sequence (approximate).
///
/// This is lossy but good enough for phoneme distance matching.
fn ipa_to_arpabet(ipa: &str) -> Vec<String> {
    let mut result = Vec::new();
    let chars: Vec<char> = ipa.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let remaining = &ipa[ipa.char_indices().nth(i).map(|(b, _)| b).unwrap_or(ipa.len())..];

        // Try two-char matches first
        let matched = if remaining.len() >= 2 {
            match &remaining[..remaining.char_indices().nth(2).map(|(b, _)| b).unwrap_or(remaining.len())] {
                s if s.starts_with("aɪ") => { i += 2; Some("AY") }
                s if s.starts_with("aʊ") => { i += 2; Some("AW") }
                s if s.starts_with("eɪ") => { i += 2; Some("EY") }
                s if s.starts_with("oʊ") => { i += 2; Some("OW") }
                s if s.starts_with("ɔɪ") => { i += 2; Some("OY") }
                s if s.starts_with("tʃ") => { i += 2; Some("CH") }
                s if s.starts_with("dʒ") => { i += 2; Some("JH") }
                s if s.starts_with("ŋk") => { i += 2; Some("NG") } // often followed by K
                _ => None,
            }
        } else {
            None
        };

        if let Some(p) = matched {
            result.push(p.to_string());
            continue;
        }

        // Single-char matches
        let p = match chars[i] {
            'ɑ' | 'ɒ' => "AA",
            'æ' => "AE",
            'ʌ' | 'ə' => "AH",
            'ɔ' => "AO",
            'ɛ' | 'e' => "EH",
            'ɝ' | 'ɜ' => "ER",
            'ɪ' => "IH",
            'i' => "IY",
            'ʊ' => "UH",
            'u' => "UW",
            'b' => "B",
            'd' => "D",
            'f' => "F",
            'ɡ' | 'g' => "G",
            'h' => "HH",
            'k' => "K",
            'l' => "L",
            'm' => "M",
            'n' => "N",
            'ŋ' => "NG",
            'p' => "P",
            'ɹ' | 'r' => "R",
            's' => "S",
            'ʃ' => "SH",
            't' => "T",
            'θ' => "TH",
            'ð' => "DH",
            'v' => "V",
            'w' => "W",
            'j' => "Y",
            'z' => "Z",
            'ʒ' => "ZH",
            'a' => "AA",
            'o' => "OW",
            // Skip stress markers, syllable boundaries, etc.
            'ˈ' | 'ˌ' | '.' | ':' | 'ː' | ' ' => { i += 1; continue; }
            _ => { i += 1; continue; }
        };

        result.push(p.to_string());
        i += 1;
    }

    result
}
