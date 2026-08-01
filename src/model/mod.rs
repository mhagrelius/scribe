//! The parts of Mynah that need no display.

pub mod config;
pub mod numbers;
pub mod store;
pub mod vocabulary;

pub use config::{Config, Delivery, Mode};
pub use store::{LoadOutcome, SaveError, Store};
pub use vocabulary::{Rule, Vocabulary};

/// Everything that happens to a transcript between the speech model and the
/// keyboard, in the order it happens.
///
/// The order matters and is not arbitrary. Spelled-out numbers are rewritten
/// first, because Parakeet writes "twenty twenty six" and a user's vocabulary
/// rule is far more likely to be written against `2026` than against the words.
/// Vocabulary substitution runs second so it can correct the model's spelling
/// of names and jargon. Trimming is last so neither step has to worry about
/// the whitespace it leaves behind.
pub fn polish(raw: &str, vocabulary: &Vocabulary, spell_numbers: bool) -> String {
    let text = if spell_numbers {
        numbers::to_digits(raw)
    } else {
        raw.to_string()
    };
    vocabulary.apply(&text).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_rewriting_runs_before_vocabulary() {
        // The rule is written against the digits, which is the whole point of
        // ordering the two steps this way.
        let vocab = Vocabulary::from_rules(vec![Rule::new("2026", "FY26")]);
        assert_eq!(
            polish("due in twenty twenty six", &vocab, true),
            "due in FY26"
        );
    }

    #[test]
    fn leaving_numbers_alone_leaves_the_words() {
        let vocab = Vocabulary::default();
        assert_eq!(
            polish("  twenty twenty six  ", &vocab, false),
            "twenty twenty six"
        );
    }
}
