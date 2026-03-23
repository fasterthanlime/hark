/// Phoneme substitution cost matrix.
///
/// Returns a cost between 0.0 (identical) and 1.0 (maximally different).
/// Costs are based on articulatory similarity + real ASR confusion patterns.
///
/// ARPAbet inventory (39 phonemes):
///   Stops:      P B T D K G
///   Fricatives: F V TH DH S Z SH ZH HH
///   Affricates: CH JH
///   Nasals:     M N NG
///   Liquids:    L R
///   Glides:     W Y
///   Vowels:     IY IH EH EY AE AA AO OW OY UH UW AH ER AW AY
pub fn substitution_cost(a: &str, b: &str) -> f32 {
    if a == b {
        return 0.0;
    }

    // Check explicit pair table first (symmetric)
    if let Some(cost) = lookup(a, b).or_else(|| lookup(b, a)) {
        return cost;
    }

    // Fallback: vowel↔consonant is always expensive
    if is_vowel(a) != is_vowel(b) {
        return 1.0;
    }

    // Unknown pair: assume expensive
    0.9
}

fn is_vowel(p: &str) -> bool {
    matches!(p, "IY" | "IH" | "EH" | "EY" | "AE" | "AA" | "AO" | "OW" | "OY" | "UH" | "UW" | "AH" | "ER" | "AW" | "AY")
}

/// Explicit similarity scores for phoneme pairs.
///
/// Principles:
/// - Voicing pairs (P/B, T/D, K/G, F/V, S/Z, SH/ZH, CH/JH, TH/DH): 0.15–0.20
///   These are the #1 source of ASR confusions
/// - Same place, different manner (T/S, D/Z, K/NG): 0.25–0.35
/// - Adjacent place, same manner (P/T, T/K, M/N, N/NG): 0.35–0.45
/// - Similar vowels (IY/IH, EH/AE, UH/UW, AO/OW): 0.20–0.35
/// - Distant vowels (IY/AA, AE/UW): 0.70–0.90
/// - Liquid/glide confusion (L/R, W/L): 0.30
/// - Vowel reduction to schwa (anything→AH): 0.25–0.40
fn lookup(a: &str, b: &str) -> Option<f32> {
    Some(match (a, b) {
        // ══════════════════════════════════════════════════════
        // VOICING PAIRS — very commonly confused by ASR
        // ══════════════════════════════════════════════════════
        ("P", "B") | ("B", "P") => 0.15,
        ("T", "D") | ("D", "T") => 0.15,
        ("K", "G") | ("G", "K") => 0.15,
        ("F", "V") | ("V", "F") => 0.15,
        ("S", "Z") | ("Z", "S") => 0.15,
        ("SH", "ZH") | ("ZH", "SH") => 0.15,
        ("CH", "JH") | ("JH", "CH") => 0.15,
        ("TH", "DH") | ("DH", "TH") => 0.15,

        // ══════════════════════════════════════════════════════
        // SAME PLACE, DIFFERENT MANNER — common confusions
        // ══════════════════════════════════════════════════════
        // Alveolar: T/D ↔ S/Z (stop↔fricative)
        ("T", "S") | ("S", "T") => 0.25,
        ("D", "Z") | ("Z", "D") => 0.25,
        ("T", "Z") | ("Z", "T") => 0.30,
        ("D", "S") | ("S", "D") => 0.30,
        // Alveolar: T/D ↔ N (stop↔nasal)
        ("T", "N") | ("N", "T") => 0.30,
        ("D", "N") | ("N", "D") => 0.30,
        // Postalveolar: SH/ZH ↔ CH/JH (fricative↔affricate)
        ("SH", "CH") | ("CH", "SH") => 0.25,
        ("ZH", "JH") | ("JH", "ZH") => 0.25,
        // Velar: K/G ↔ NG (stop↔nasal)
        ("K", "NG") | ("NG", "K") => 0.30,
        ("G", "NG") | ("NG", "G") => 0.30,
        // Bilabial: P/B ↔ M (stop↔nasal)
        ("P", "M") | ("M", "P") => 0.30,
        ("B", "M") | ("M", "B") => 0.25, // B→M especially common

        // ══════════════════════════════════════════════════════
        // ADJACENT PLACE, SAME MANNER — moderately confused
        // ══════════════════════════════════════════════════════
        // Stop pairs across place
        ("P", "T") | ("T", "P") => 0.40,
        ("B", "D") | ("D", "B") => 0.40,
        ("T", "K") | ("K", "T") => 0.40,
        ("D", "G") | ("G", "D") => 0.40,
        ("P", "K") | ("K", "P") => 0.50,
        ("B", "G") | ("G", "B") => 0.50,
        // Nasal pairs across place
        ("M", "N") | ("N", "M") => 0.35,
        ("N", "NG") | ("NG", "N") => 0.35,
        ("M", "NG") | ("NG", "M") => 0.50,
        // Fricative pairs across place
        ("F", "TH") | ("TH", "F") => 0.30, // very commonly confused!
        ("V", "DH") | ("DH", "V") => 0.30, // very commonly confused!
        ("F", "S") | ("S", "F") => 0.40,
        ("V", "Z") | ("Z", "V") => 0.40,
        ("S", "SH") | ("SH", "S") => 0.30,
        ("Z", "ZH") | ("ZH", "Z") => 0.30,
        ("TH", "S") | ("S", "TH") => 0.35,
        ("DH", "Z") | ("Z", "DH") => 0.35,

        // ══════════════════════════════════════════════════════
        // LIQUIDS & GLIDES — frequently confused
        // ══════════════════════════════════════════════════════
        ("L", "R") | ("R", "L") => 0.30,
        ("W", "L") | ("L", "W") => 0.40,
        ("R", "W") | ("W", "R") => 0.40,
        ("Y", "IY") | ("IY", "Y") => 0.20, // glide↔vowel
        ("W", "UW") | ("UW", "W") => 0.20, // glide↔vowel

        // ══════════════════════════════════════════════════════
        // HH (glottal) — often dropped or inserted by ASR
        // ══════════════════════════════════════════════════════
        ("HH", _) if !is_vowel(b) => 0.50,

        // ══════════════════════════════════════════════════════
        // VOWELS — organized by proximity in vowel space
        // ══════════════════════════════════════════════════════

        // Near-identical (tense/lax pairs)
        ("IY", "IH") | ("IH", "IY") => 0.15, // beat/bit
        ("UW", "UH") | ("UH", "UW") => 0.15, // boot/book
        ("EY", "EH") | ("EH", "EY") => 0.20, // bait/bet
        ("AO", "OW") | ("OW", "AO") => 0.20, // bought/boat
        ("AO", "OY") | ("OY", "AO") => 0.25,
        ("OW", "OY") | ("OY", "OW") => 0.25,

        // Schwa (AH) — the great neutralizer, everything reduces to it
        ("AH", "IH") | ("IH", "AH") => 0.25,
        ("AH", "EH") | ("EH", "AH") => 0.25,
        ("AH", "AE") | ("AE", "AH") => 0.30,
        ("AH", "AA") | ("AA", "AH") => 0.25,
        ("AH", "AO") | ("AO", "AH") => 0.30,
        ("AH", "UH") | ("UH", "AH") => 0.25,
        ("AH", "ER") | ("ER", "AH") => 0.20, // very close
        ("AH", "IY") | ("IY", "AH") => 0.35,
        ("AH", "UW") | ("UW", "AH") => 0.35,
        ("AH", "OW") | ("OW", "AH") => 0.30,
        ("AH", "EY") | ("EY", "AH") => 0.30,

        // ER is close to AH and UH
        ("ER", "IH") | ("IH", "ER") => 0.30,
        ("ER", "EH") | ("EH", "ER") => 0.30,
        ("ER", "UH") | ("UH", "ER") => 0.30,

        // Front vowel ladder: IY → IH → EH → EY → AE
        ("IY", "EH") | ("EH", "IY") => 0.35,
        ("IY", "EY") | ("EY", "IY") => 0.30,
        ("IH", "EH") | ("EH", "IH") => 0.25,
        ("IH", "EY") | ("EY", "IH") => 0.30,
        ("EH", "AE") | ("AE", "EH") => 0.25,
        ("EY", "AE") | ("AE", "EY") => 0.30,
        ("IY", "AE") | ("AE", "IY") => 0.50,
        ("IH", "AE") | ("AE", "IH") => 0.35,

        // Back vowel ladder: UW → UH → OW → AO → AA
        ("UW", "OW") | ("OW", "UW") => 0.30,
        ("UH", "OW") | ("OW", "UH") => 0.30,
        ("UH", "AO") | ("AO", "UH") => 0.35,
        ("UW", "AO") | ("AO", "UW") => 0.40,
        ("OW", "AA") | ("AA", "OW") => 0.40,
        ("AO", "AA") | ("AA", "AO") => 0.30,
        ("UW", "AA") | ("AA", "UW") => 0.60,

        // Front↔back (distant)
        ("IY", "UW") | ("UW", "IY") => 0.60,
        ("IY", "OW") | ("OW", "IY") => 0.60,
        ("IY", "AA") | ("AA", "IY") => 0.70,
        ("IH", "UH") | ("UH", "IH") => 0.50,
        ("EH", "OW") | ("OW", "EH") => 0.50,
        ("AE", "AA") | ("AA", "AE") => 0.35, // actually somewhat close
        ("AE", "AO") | ("AO", "AE") => 0.45,
        ("AE", "OW") | ("OW", "AE") => 0.55,
        ("AE", "UW") | ("UW", "AE") => 0.70,

        // Diphthongs vs monophthongs
        ("AY", "AE") | ("AE", "AY") => 0.30,
        ("AY", "AA") | ("AA", "AY") => 0.30,
        ("AY", "IY") | ("IY", "AY") => 0.40,
        ("AY", "EY") | ("EY", "AY") => 0.35,
        ("AY", "OY") | ("OY", "AY") => 0.40,
        ("AW", "AO") | ("AO", "AW") => 0.30,
        ("AW", "OW") | ("OW", "AW") => 0.35,
        ("AW", "AA") | ("AA", "AW") => 0.30,
        ("AW", "AE") | ("AE", "AW") => 0.35,
        ("AW", "AY") | ("AY", "AW") => 0.30,

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voicing_pairs_are_cheap() {
        assert!(substitution_cost("K", "G") <= 0.15);
        assert!(substitution_cost("T", "D") <= 0.15);
        assert!(substitution_cost("S", "Z") <= 0.15);
    }

    #[test]
    fn k_g_cheaper_than_k_p() {
        assert!(substitution_cost("K", "G") < substitution_cost("K", "P"));
    }

    #[test]
    fn k_p_cheaper_than_k_b() {
        // K→P: place change but same voicing
        // K→B: place change + voicing change
        assert!(substitution_cost("K", "P") < substitution_cost("K", "B"));
    }

    #[test]
    fn similar_vowels_are_cheap() {
        assert!(substitution_cost("IY", "IH") <= 0.20);
        assert!(substitution_cost("UW", "UH") <= 0.20);
    }

    #[test]
    fn distant_vowels_are_expensive() {
        assert!(substitution_cost("IY", "AA") >= 0.60);
        assert!(substitution_cost("IY", "UW") >= 0.50);
    }

    #[test]
    fn schwa_is_close_to_everything() {
        // AH (schwa) should be relatively close to all vowels
        assert!(substitution_cost("AH", "IH") <= 0.30);
        assert!(substitution_cost("AH", "EH") <= 0.30);
        assert!(substitution_cost("AH", "AA") <= 0.30);
        assert!(substitution_cost("AH", "UH") <= 0.30);
    }

    #[test]
    fn f_th_confusion() {
        // F and TH are notoriously confused
        assert!(substitution_cost("F", "TH") <= 0.35);
    }

    #[test]
    fn vowel_consonant_is_expensive() {
        assert!(substitution_cost("K", "AE") >= 0.90);
    }

    #[test]
    fn identity_is_zero() {
        assert_eq!(substitution_cost("K", "K"), 0.0);
        assert_eq!(substitution_cost("AE", "AE"), 0.0);
    }
}
