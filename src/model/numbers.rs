//! Turning spoken numbers into digits.
//!
//! Parakeet writes numbers the way they are said — "twenty twenty six", "one
//! hundred and three" — and almost nobody dictating into a terminal or a
//! spreadsheet wants that. NVIDIA's model card is explicit that inverse text
//! normalisation is left to the caller, so this is the caller doing it.
//!
//! Three readings of a run of number words have to be told apart, and no
//! single accumulator gets all three right:
//!
//! - **Scaled** — "one hundred and three", "two thousand twenty six". Addition
//!   and multiplication against `hundred`/`thousand`/`million`.
//! - **Paired** — "nineteen eighty four", "twenty twenty six". Two two-digit
//!   chunks read as a year, which the scaled reading would total as 103 and 46.
//! - **Digit-by-digit** — "four zero four", "one two three". Three or more
//!   single digits read as a string rather than summed.
//!
//! A standalone word below ten is left alone, because "I need 1 thing" reads
//! worse than the sentence it replaced. Everything else converts.

/// Rewrite every run of spoken number words in `text` as digits.
pub fn to_digits(text: &str) -> String {
    let words = split_words(text);
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    let mut i = 0;

    while i < words.len() {
        let Some(run) = run_at(text, &words, i) else {
            i += 1;
            continue;
        };
        let (end, value) = run;
        let first = &words[i];
        let last = &words[end - 1];

        // A single word under ten stays a word.
        let single_small = end == i + 1 && value.parse::<u64>().map(|n| n < 10).unwrap_or(false);
        if single_small {
            i = end;
            continue;
        }

        out.push_str(&text[cursor..first.start]);
        out.push_str(&value);
        cursor = last.end;
        i = end;
    }
    out.push_str(&text[cursor..]);
    out
}

struct Word {
    start: usize,
    end: usize,
    lower: String,
}

/// Split into alphabetic words, treating a hyphen as a separator so
/// "twenty-three" is seen the same as "twenty three".
fn split_words(text: &str) -> Vec<Word> {
    let mut words = Vec::new();
    let mut start = None;
    for (idx, ch) in text.char_indices() {
        if ch.is_alphabetic() {
            start.get_or_insert(idx);
        } else if let Some(s) = start.take() {
            words.push(Word {
                start: s,
                end: idx,
                lower: text[s..idx].to_lowercase(),
            });
        }
    }
    if let Some(s) = start {
        words.push(Word {
            start: s,
            end: text.len(),
            lower: text[s..].to_lowercase(),
        });
    }
    words
}

/// Only whitespace and hyphens may sit between two words of the same number.
/// A comma or a full stop ends the run, so "twenty, six" stays two numbers.
fn joined(text_between: &str) -> bool {
    !text_between.is_empty() && text_between.chars().all(|c| c.is_whitespace() || c == '-')
}

enum Tok {
    Unit(u64), // zero..nine
    Teen(u64), // ten..nineteen
    Ten(u64),  // twenty..ninety
    Hundred,
    Scale(u64), // thousand, million, billion
    And,
    Point,
}

fn classify(word: &str) -> Option<Tok> {
    Some(match word {
        "zero" | "nought" => Tok::Unit(0),
        "one" => Tok::Unit(1),
        "two" => Tok::Unit(2),
        "three" => Tok::Unit(3),
        "four" => Tok::Unit(4),
        "five" => Tok::Unit(5),
        "six" => Tok::Unit(6),
        "seven" => Tok::Unit(7),
        "eight" => Tok::Unit(8),
        "nine" => Tok::Unit(9),
        "ten" => Tok::Teen(10),
        "eleven" => Tok::Teen(11),
        "twelve" => Tok::Teen(12),
        "thirteen" => Tok::Teen(13),
        "fourteen" => Tok::Teen(14),
        "fifteen" => Tok::Teen(15),
        "sixteen" => Tok::Teen(16),
        "seventeen" => Tok::Teen(17),
        "eighteen" => Tok::Teen(18),
        "nineteen" => Tok::Teen(19),
        "twenty" => Tok::Ten(20),
        "thirty" => Tok::Ten(30),
        "forty" => Tok::Ten(40),
        "fifty" => Tok::Ten(50),
        "sixty" => Tok::Ten(60),
        "seventy" => Tok::Ten(70),
        "eighty" => Tok::Ten(80),
        "ninety" => Tok::Ten(90),
        "hundred" => Tok::Hundred,
        "thousand" => Tok::Scale(1_000),
        "million" => Tok::Scale(1_000_000),
        "billion" => Tok::Scale(1_000_000_000),
        "and" => Tok::And,
        "point" => Tok::Point,
        _ => return None,
    })
}

/// Find the run of number words starting at `i`, and what it reads as.
/// Returns the index one past the run's last word.
fn run_at(text: &str, words: &[Word], i: usize) -> Option<(usize, String)> {
    // A run may not begin with a connector.
    match classify(&words[i].lower) {
        Some(Tok::And) | Some(Tok::Point) | None => return None,
        _ => {}
    }

    let mut end = i + 1;
    while end < words.len() {
        if classify(&words[end].lower).is_none() {
            break;
        }
        // The gap between the previous word and this one has to be plain.
        // Byte ranges come from the same string, so this slice is in bounds.
        let gap_start = words[end - 1].end;
        let gap_end = words[end].start;
        if gap_start > gap_end || !joined(&text[gap_start..gap_end]) {
            break;
        }
        end += 1;
    }

    // Drop trailing connectors: "twenty three and" is a two-word number.
    while end > i {
        match classify(&words[end - 1].lower) {
            Some(Tok::And) | Some(Tok::Point) => end -= 1,
            _ => break,
        }
    }
    if end == i {
        return None;
    }
    Some((end, read(&words[i..end])))
}

fn read(run: &[Word]) -> String {
    let toks: Vec<Tok> = run.iter().filter_map(|w| classify(&w.lower)).collect();

    // "three point one four" — everything after `point` is read digit by digit.
    if let Some(p) = toks.iter().position(|t| matches!(t, Tok::Point)) {
        let whole = read_value(&toks[..p]);
        let frac: String = toks[p + 1..]
            .iter()
            .filter_map(|t| match t {
                Tok::Unit(n) => Some(n.to_string()),
                _ => None,
            })
            .collect();
        return if frac.is_empty() {
            whole
        } else {
            format!("{whole}.{frac}")
        };
    }
    read_value(&toks)
}

fn read_value(toks: &[Tok]) -> String {
    let has_scale = toks
        .iter()
        .any(|t| matches!(t, Tok::Hundred | Tok::Scale(_)));

    if !has_scale {
        let chunks = chunk(toks);
        // "four zero four" — three or more bare digits are a digit string.
        let all_digits =
            chunks.len() >= 3 && toks.iter().all(|t| matches!(t, Tok::Unit(_) | Tok::And));
        if all_digits {
            return chunks.iter().map(|c| c.to_string()).collect();
        }
        // "nineteen eighty four" — two two-digit chunks are a year.
        if chunks.len() == 2 && chunks.iter().all(|&c| (10..=99).contains(&c)) {
            return (chunks[0] * 100 + chunks[1]).to_string();
        }
        return chunks.iter().sum::<u64>().to_string();
    }

    let (mut total, mut current) = (0u64, 0u64);
    for tok in toks {
        match tok {
            Tok::Unit(n) | Tok::Teen(n) | Tok::Ten(n) => current += n,
            Tok::Hundred => current = current.max(1) * 100,
            Tok::Scale(s) => {
                total += current.max(1) * s;
                current = 0;
            }
            Tok::And | Tok::Point => {}
        }
    }
    (total + current).to_string()
}

/// Break a scale-free run into the two-digit groups a speaker actually says.
/// A new group starts whenever the next word cannot extend the current one.
fn chunk(toks: &[Tok]) -> Vec<u64> {
    let mut chunks: Vec<u64> = Vec::new();
    let mut open_tens = false;
    for tok in toks {
        match tok {
            Tok::Ten(n) => {
                chunks.push(*n);
                open_tens = true;
            }
            Tok::Unit(n) => {
                if open_tens {
                    // "twenty" then "six" completes the group.
                    *chunks.last_mut().expect("open_tens implies a chunk") += n;
                    open_tens = false;
                } else {
                    chunks.push(*n);
                }
            }
            Tok::Teen(n) => {
                chunks.push(*n);
                open_tens = false;
            }
            _ => {}
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::to_digits;

    #[track_caller]
    fn same(input: &str, expected: &str) {
        assert_eq!(to_digits(input), expected, "input: {input}");
    }

    #[test]
    fn years_read_as_pairs_not_sums() {
        same("due in twenty twenty six", "due in 2026");
        same("born nineteen eighty four", "born 1984");
        same("the year twenty twenty", "the year 2020");
    }

    #[test]
    fn scaled_numbers_accumulate() {
        same("one hundred and three", "103");
        same("two thousand twenty six", "2026");
        same("three million", "3000000");
        same("sixteen hundred", "1600");
    }

    #[test]
    fn digit_strings_stay_digit_strings() {
        same("error four zero four", "error 404");
        same("extension one two three", "extension 123");
    }

    #[test]
    fn a_lone_small_number_stays_a_word() {
        same("I need one thing", "I need one thing");
        same("just two", "just two");
    }

    #[test]
    fn a_lone_large_number_converts() {
        same("about twenty people", "about 20 people");
        same("fifteen minutes", "15 minutes");
    }

    #[test]
    fn hyphens_join_a_number_but_commas_break_it() {
        same("twenty-three items", "23 items");
        same("twenty, six", "20, six");
    }

    #[test]
    fn decimals_read_the_fraction_digit_by_digit() {
        same("pi is three point one four", "pi is 3.14");
    }

    #[test]
    fn trailing_connectors_are_not_swallowed() {
        same("twenty three and then some", "23 and then some");
    }

    #[test]
    fn text_without_numbers_is_returned_unchanged() {
        same("nothing numeric here at all", "nothing numeric here at all");
        same("", "");
    }

    #[test]
    fn number_words_inside_other_words_are_not_matched() {
        same("someone tenderly won", "someone tenderly won");
    }
}
