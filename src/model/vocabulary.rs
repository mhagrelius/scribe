//! User-supplied vocabulary.
//!
//! Parakeet has no contextual-biasing API — you cannot hand it a word list and
//! have the acoustic decoder prefer those words, the way a transducer with a
//! hotwords file can. What it does instead is spell an unfamiliar name the way
//! it sounded, consistently. So the vocabulary here is applied to the finished
//! transcript rather than to the decoder: the user says what the model gets
//! wrong and what it should have been, and every transcript is rewritten.
//!
//! That is less clever than biasing and more predictable than it, which for
//! names the user types every day is the better trade.

use serde::{Deserialize, Serialize};

/// One rewrite: replace `heard` with `write`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct Rule {
    /// What the model tends to produce.
    pub heard: String,
    /// What the user meant.
    pub write: String,
    /// Whether `heard` has to match the transcript's capitalisation.
    #[serde(default)]
    pub match_case: bool,
}

impl Rule {
    pub fn new(heard: &str, write: &str) -> Self {
        Self {
            heard: heard.to_string(),
            write: write.to_string(),
            match_case: false,
        }
    }

    /// A rule with nothing to match can only ever loop, so it is not a rule.
    pub fn is_usable(&self) -> bool {
        !self.heard.trim().is_empty()
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct Vocabulary {
    rules: Vec<Rule>,
}

impl Vocabulary {
    pub fn from_rules(rules: Vec<Rule>) -> Self {
        Self { rules }
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn set_rules(&mut self, rules: Vec<Rule>) {
        self.rules = rules;
    }

    pub fn is_empty(&self) -> bool {
        self.rules.iter().all(|r| !r.is_usable())
    }

    /// Rewrite `text` by every usable rule, longest `heard` first.
    ///
    /// Longest-first matters: with rules for both "code" and "code review",
    /// applying the shorter one first would leave "…-review" behind and the
    /// longer rule would never match. Each rule sees the output of the one
    /// before it, so a user can chain them deliberately.
    pub fn apply(&self, text: &str) -> String {
        let mut usable: Vec<&Rule> = self.rules.iter().filter(|r| r.is_usable()).collect();
        usable.sort_by_key(|r| std::cmp::Reverse(r.heard.chars().count()));

        let mut out = text.to_string();
        for rule in usable {
            out = if rule.match_case {
                out.replace(&rule.heard, &rule.write)
            } else {
                replace_ignoring_case(&out, &rule.heard, &rule.write)
            };
        }
        out
    }
}

/// `str::replace` with the needle matched case-insensitively.
///
/// Written out rather than pulled from a regex crate: the needle is a literal,
/// and lowercasing both sides is the whole of the logic. Indices come from the
/// lowercased copies, so this steps by the lowercased byte length and cannot
/// land inside a character on either string.
fn replace_ignoring_case(haystack: &str, needle: &str, replacement: &str) -> String {
    let hay_lower = haystack.to_lowercase();
    let needle_lower = needle.to_lowercase();
    if needle_lower.is_empty() {
        return haystack.to_string();
    }

    // Lowercasing can change byte length (ẞ becomes ss), which would make an
    // index into the lowercased copy meaningless against the original. When
    // that happens, fall back to matching exactly rather than corrupting text.
    if hay_lower.len() != haystack.len() {
        return haystack.replace(needle, replacement);
    }

    let mut out = String::with_capacity(haystack.len());
    let mut cursor = 0;
    while let Some(hit) = hay_lower[cursor..].find(&needle_lower) {
        let start = cursor + hit;
        let end = start + needle_lower.len();
        if !haystack.is_char_boundary(start) || !haystack.is_char_boundary(end) {
            break;
        }
        out.push_str(&haystack[cursor..start]);
        out.push_str(replacement);
        cursor = end;
    }
    out.push_str(&haystack[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab(rules: &[(&str, &str)]) -> Vocabulary {
        Vocabulary::from_rules(rules.iter().map(|(h, w)| Rule::new(h, w)).collect())
    }

    #[test]
    fn rewrites_a_misheard_name() {
        let v = vocab(&[("mina", "Mynah")]);
        assert_eq!(v.apply("I opened mina again"), "I opened Mynah again");
    }

    #[test]
    fn matching_ignores_case_by_default() {
        let v = vocab(&[("kubernetes", "k8s")]);
        assert_eq!(v.apply("Kubernetes and KUBERNETES"), "k8s and k8s");
    }

    #[test]
    fn case_sensitive_rules_leave_other_casings_alone() {
        let v = Vocabulary::from_rules(vec![Rule {
            heard: "IT".into(),
            write: "I.T.".into(),
            match_case: true,
        }]);
        assert_eq!(v.apply("IT says it works"), "I.T. says it works");
    }

    #[test]
    fn longer_rules_win_over_the_prefixes_they_contain() {
        let v = vocab(&[("code", "Code"), ("code review", "CR")]);
        assert_eq!(v.apply("time for code review"), "time for CR");
    }

    #[test]
    fn a_blank_rule_is_ignored_rather_than_looping() {
        let v = vocab(&[("", "x"), ("  ", "y")]);
        assert_eq!(v.apply("untouched"), "untouched");
        assert!(v.is_empty());
    }

    #[test]
    fn replacement_containing_the_needle_does_not_recurse() {
        let v = vocab(&[("cat", "cat dog")]);
        assert_eq!(v.apply("one cat"), "one cat dog");
    }

    #[test]
    fn multibyte_text_survives_a_rule_that_does_not_match_it() {
        let v = vocab(&[("cafe", "café")]);
        assert_eq!(v.apply("a café and a cafe"), "a café and a café");
    }
}
