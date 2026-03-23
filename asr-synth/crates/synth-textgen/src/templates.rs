use crate::corpus::VocabEntry;
use rand::prelude::*;
use rand::rng;

#[derive(Debug, serde::Serialize)]
pub struct GeneratedSentence {
    /// Written form (correction target): "I'm using serde in my project."
    pub text: String,
    /// Spoken form (what to feed TTS): "I'm using ser dee in my project."
    pub spoken: String,
    /// Which vocab terms appear
    pub vocab_terms: Vec<String>,
}

const TEMPLATES_1: &[&str] = &[
    "I'm using {0} in my project.",
    "Have you tried {0} before?",
    "The {0} library is really good.",
    "We should switch to {0} for this.",
    "I need to install {0} first.",
    "Let me check the {0} documentation.",
    "There's a bug in {0} that I need to work around.",
    "Can you add {0} to the dependencies?",
    "I just upgraded {0} to the latest version.",
    "The {0} crate provides what we need.",
    "Make sure {0} is in your Cargo.toml.",
    "I'm getting an error from {0}.",
    "The performance of {0} is impressive.",
    "We're migrating away from {0}.",
    "I wrote a wrapper around {0}.",
];

const TEMPLATES_2: &[&str] = &[
    "I'm using {0} and {1} together.",
    "We replaced {0} with {1} last week.",
    "The combination of {0} and {1} works well.",
    "You need both {0} and {1} for this.",
    "I'm having trouble getting {0} to work with {1}.",
    "After switching from {0} to {1}, things improved.",
    "The {0} integration with {1} is seamless.",
    "Can you check if {0} is compatible with {1}?",
    "We use {0} for the frontend and {1} for the backend.",
    "I configured {0} to output {1} format.",
];

const TEMPLATES_3: &[&str] = &[
    "The {0} module uses {1} under the hood with {2} for acceleration.",
    "I set up {0}, {1}, and {2} in the pipeline.",
    "We need {0} for parsing, {1} for processing, and {2} for output.",
    "The project depends on {0}, {1}, and {2}.",
    "After configuring {0} with {1}, I also added {2}.",
];

pub fn generate(vocab: &[VocabEntry], count: usize) -> Vec<GeneratedSentence> {
    let mut rng = rng();
    let mut sentences = Vec::with_capacity(count);

    for _ in 0..count {
        // Randomly pick 1, 2, or 3 terms with weighted probability
        let n_terms: usize = match rng.random_range(0..10) {
            0..=4 => 1,  // 50% single term
            5..=8 => 2,  // 40% two terms
            _ => 3,      // 10% three terms
        };

        let templates: &[&str] = match n_terms {
            1 => TEMPLATES_1,
            2 => TEMPLATES_2,
            _ => TEMPLATES_3,
        };

        let template = templates[rng.random_range(0..templates.len())];
        let terms: Vec<&VocabEntry> = vocab
            .choose_multiple(&mut rng, n_terms)
            .collect();

        let mut text = template.to_string();
        let mut spoken = template.to_string();
        for (i, term) in terms.iter().enumerate() {
            text = text.replace(&format!("{{{i}}}"), &term.term);
            spoken = spoken.replace(&format!("{{{i}}}"), &term.spoken);
        }

        let vocab_terms: Vec<String> = terms.iter().map(|t| t.term.clone()).collect();
        sentences.push(GeneratedSentence { text, spoken, vocab_terms });
    }

    sentences
}
