//! Conservative confidence classification for learned Dictionary pairs.
//!
//! [`crate::dictionary::learn_pairs`] remains the authority for deciding
//! whether an edit is a plausible correction. This module adds the stronger,
//! bounded checks needed before a caller may apply a learned pair
//! automatically. A non-empty [`LearningCandidate::context_words`] is the
//! explicit signal that all automatic-use checks passed. Suggested pairs keep
//! an empty context list.

use std::collections::HashSet;

use crate::dictionary::{learn_pairs, DictionaryEntry};

const MAX_INPUT_BYTES: usize = 32 * 1024;
const MAX_WHITESPACE_WORDS: usize = 2_048;
const MAX_ALNUM_TOKENS: usize = 2_048;
const MAX_TOKEN_CHARS: usize = 128;
const MAX_CANDIDATES: usize = 16;
const CONTEXT_RADIUS: usize = 4;
const MAX_CONTEXT_WORDS: usize = 4;
const MAX_RESIDUAL_LCS_CELLS: usize = 65_536;

/// A pair accepted by the base learner, plus its automatic-use confidence.
///
/// `context_words` contains one to four unchanged, useful words only when the
/// pair is eligible for contextual automatic use. An empty vector means the
/// pair must remain a suggestion.
#[derive(Debug, Clone, PartialEq)]
pub struct LearningCandidate {
    pub entry: DictionaryEntry,
    pub context_words: Vec<String>,
}

/// Classify plausible learned pairs without I/O or retained source text.
///
/// `trusted_terms` is authoritative vocabulary supplied by the caller. Each
/// value is the exact, trimmed, lowercase form of a complete target term (not
/// an individual token). It must contain only manual/custom vocabulary or an
/// explicitly approved active rule; automatically learned terms must never be
/// fed back into this set.
///
/// Inputs that exceed the byte, word, token, or token-length limits are
/// skipped before the word diff runs. At most [`MAX_CANDIDATES`] are returned.
pub fn classify_pairs(
    original: &str,
    corrected: &str,
    trusted_terms: &HashSet<String>,
) -> Vec<LearningCandidate> {
    let Some(original_tokens) = bounded_tokens(original) else {
        return Vec::new();
    };
    let Some(corrected_tokens) = bounded_tokens(corrected) else {
        return Vec::new();
    };

    let pairs: Vec<(DictionaryEntry, bool)> = learn_pairs(original, corrected)
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|entry| {
            let strong = strong_pair(&entry, trusted_terms);
            (entry, strong)
        })
        .collect();

    // Weak or unknown pairs are suggestions regardless of the surrounding
    // edit, so do not spend the residual LCS budget on them.
    let broad_rewrite = pairs.iter().any(|(_, strong)| *strong)
        && is_broad_rewrite(&original_tokens, &corrected_tokens);

    pairs
        .into_iter()
        .map(|(entry, strong)| {
            let context_words = if strong && !broad_rewrite {
                unchanged_context(&entry, &original_tokens, &corrected_tokens)
            } else {
                Vec::new()
            };
            LearningCandidate {
                entry,
                context_words,
            }
        })
        .collect()
}

/// Tokenize only after enforcing all limits that protect the base learner.
fn bounded_tokens(text: &str) -> Option<Vec<String>> {
    if text.len() > MAX_INPUT_BYTES {
        return None;
    }

    let mut whitespace_words = 0;
    for word in text.split_whitespace() {
        whitespace_words += 1;
        if whitespace_words > MAX_WHITESPACE_WORDS || word.chars().count() > MAX_TOKEN_CHARS {
            return None;
        }
    }

    let mut tokens = Vec::new();
    for token in text.split(|c: char| !c.is_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        if token.chars().count() > MAX_TOKEN_CHARS || tokens.len() == MAX_ALNUM_TOKENS {
            return None;
        }
        tokens.push(token.to_lowercase());
    }
    Some(tokens)
}

fn normalized_pair_side(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn phonetic_key(text: &str) -> Option<String> {
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    use rphonetic::Encoder;
    let key = rphonetic::DoubleMetaphone::default().encode(text);
    (!key.is_empty()).then_some(key)
}

/// Automatic use needs caller-owned vocabulary plus two independent signals:
/// close spelling and the same phonetic code. The normalized forms are
/// limited to ASCII letters because Double Metaphone cannot provide equivalent
/// evidence for unsupported scripts.
fn strong_pair(entry: &DictionaryEntry, trusted_terms: &HashSet<String>) -> bool {
    let trusted_target = entry.right.trim().to_lowercase();
    if !trusted_terms.contains(&trusted_target) {
        return false;
    }

    let wrong = normalized_pair_side(&entry.wrong);
    let right = normalized_pair_side(&entry.right);
    if wrong.len() < 4
        || right.len() < 4
        || !wrong.chars().all(|c| c.is_ascii_alphabetic())
        || !right.chars().all(|c| c.is_ascii_alphabetic())
    {
        return false;
    }

    let longer = wrong.len().max(right.len());
    let distance = strsim::levenshtein(&wrong, &right);
    if distance.saturating_mul(4) > longer {
        return false;
    }

    matches!(
        (phonetic_key(&wrong), phonetic_key(&right)),
        (Some(wrong_key), Some(right_key)) if wrong_key == right_key
    )
}

/// A large rewrite can leave a coincidental learnable word pair. Require at
/// least three quarters of the bounded token sequence to remain in order
/// before exposing any automatic-use context. Identical edges are accounted
/// for directly. If the remaining exact LCS would exceed its cell budget, the
/// edit conservatively stays a suggestion.
fn is_broad_rewrite(original: &[String], corrected: &[String]) -> bool {
    let longer = original.len().max(corrected.len());
    if longer == 0 {
        return false;
    }

    // ceil(3 * longer / 4): the exact number of unchanged tokens needed.
    let required_unchanged = (3 * longer).div_ceil(4);
    let shared_limit = original.len().min(corrected.len());

    let mut prefix = 0;
    while prefix < shared_limit && original[prefix] == corrected[prefix] {
        prefix += 1;
    }

    let mut suffix = 0;
    while prefix + suffix < shared_limit
        && original[original.len() - 1 - suffix] == corrected[corrected.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let fixed_unchanged = prefix + suffix;
    if fixed_unchanged >= required_unchanged {
        return false;
    }

    let original_middle = &original[prefix..original.len() - suffix];
    let corrected_middle = &corrected[prefix..corrected.len() - suffix];
    let most_possible = fixed_unchanged + original_middle.len().min(corrected_middle.len());
    if most_possible < required_unchanged {
        return true;
    }

    let Some(cells) = original_middle.len().checked_mul(corrected_middle.len()) else {
        return true;
    };
    if cells > MAX_RESIDUAL_LCS_CELLS {
        return true;
    }

    fixed_unchanged + lcs_len(original_middle, corrected_middle) < required_unchanged
}

fn lcs_len(left: &[String], right: &[String]) -> usize {
    if right.len() > left.len() {
        return lcs_len(right, left);
    }

    let mut row = vec![0; right.len() + 1];
    for left_token in left {
        let mut diagonal = 0;
        for (index, right_token) in right.iter().enumerate() {
            let previous_row = row[index + 1];
            if left_token == right_token {
                row[index + 1] = diagonal + 1;
            } else {
                row[index + 1] = row[index + 1].max(row[index]);
            }
            diagonal = previous_row;
        }
    }
    row[right.len()]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum ContextSide {
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct NearbyWord {
    word: String,
    side: ContextSide,
    distance: usize,
}

fn unique_occurrence(haystack: &[String], needle: &[String]) -> Option<(usize, usize)> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }

    let mut found = None;
    for start in 0..=haystack.len() - needle.len() {
        if haystack[start..start + needle.len()] == *needle {
            if found.is_some() {
                return None;
            }
            found = Some((start, start + needle.len()));
        }
    }
    found
}

fn is_common_word(word: &str) -> bool {
    use std::sync::OnceLock;
    static COMMON: OnceLock<HashSet<&'static str>> = OnceLock::new();
    let words = COMMON.get_or_init(|| stop_words::get("en").iter().copied().collect());
    words.contains(word)
}

fn nearby_useful_words(
    tokens: &[String],
    start: usize,
    end: usize,
    pair_words: &HashSet<String>,
) -> Vec<NearbyWord> {
    let mut nearby = Vec::new();
    for (index, word) in tokens
        .iter()
        .enumerate()
        .take(start)
        .skip(start.saturating_sub(CONTEXT_RADIUS))
    {
        if word.chars().count() >= 4
            && word.chars().all(char::is_alphabetic)
            && !pair_words.contains(word)
            && !is_common_word(word)
        {
            nearby.push(NearbyWord {
                word: word.clone(),
                side: ContextSide::Before,
                distance: start - index,
            });
        }
    }
    for (offset, word) in tokens[end..tokens.len().min(end + CONTEXT_RADIUS)]
        .iter()
        .enumerate()
    {
        if word.chars().count() >= 4
            && word.chars().all(char::is_alphabetic)
            && !pair_words.contains(word)
            && !is_common_word(word)
        {
            nearby.push(NearbyWord {
                word: word.clone(),
                side: ContextSide::After,
                distance: offset + 1,
            });
        }
    }
    nearby.sort_by_key(|word| (word.distance, word.side));
    nearby
}

fn unchanged_context(
    entry: &DictionaryEntry,
    original: &[String],
    corrected: &[String],
) -> Vec<String> {
    let Some(wrong_tokens) = bounded_tokens(&entry.wrong) else {
        return Vec::new();
    };
    let Some(right_tokens) = bounded_tokens(&entry.right) else {
        return Vec::new();
    };
    let Some((wrong_start, wrong_end)) = unique_occurrence(original, &wrong_tokens) else {
        return Vec::new();
    };
    let Some((right_start, right_end)) = unique_occurrence(corrected, &right_tokens) else {
        return Vec::new();
    };

    let pair_words: HashSet<String> = wrong_tokens.iter().chain(&right_tokens).cloned().collect();
    let old_context = nearby_useful_words(original, wrong_start, wrong_end, &pair_words);
    let new_context: HashSet<NearbyWord> =
        nearby_useful_words(corrected, right_start, right_end, &pair_words)
            .into_iter()
            .collect();

    let mut selected = Vec::new();
    for nearby in old_context {
        if new_context.contains(&nearby) && !selected.contains(&nearby.word) {
            selected.push(nearby.word);
            if selected.len() == MAX_CONTEXT_WORDS {
                break;
            }
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted(terms: &[&str]) -> HashSet<String> {
        terms.iter().map(|term| (*term).to_string()).collect()
    }

    #[test]
    fn trusted_close_phonetic_pair_with_unchanged_context_is_automatic() {
        let candidates = classify_pairs(
            "Magnitude deploys Catapult earthquake alerts daily",
            "Magnitude deploys Katapult earthquake alerts daily",
            &trusted(&["katapult"]),
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].entry.wrong, "Catapult");
        assert_eq!(candidates[0].entry.right, "Katapult");
        assert!(!candidates[0].context_words.is_empty());
        assert!(candidates[0].context_words.len() <= MAX_CONTEXT_WORDS);
        assert!(candidates[0]
            .context_words
            .iter()
            .all(|word| word != "catapult" && word != "katapult"));
    }

    #[test]
    fn split_or_joined_known_name_can_be_automatic() {
        for (original, corrected, target) in [
            (
                "Our billing uses char gebee invoices every single day",
                "Our billing uses ChargeBee invoices every single day",
                "chargebee",
            ),
            (
                "The developers use Githubdesktop for repository management every day",
                "The developers use GitHub Desktop for repository management every day",
                "github desktop",
            ),
        ] {
            let pairs = classify_pairs(original, corrected, &trusted(&[target]));
            assert_eq!(pairs.len(), 1, "{original}");
            assert!(!pairs[0].context_words.is_empty(), "{original}");
        }
    }

    #[test]
    fn unknown_target_stays_a_suggestion() {
        let candidates = classify_pairs(
            "Magnitude deploys Catapult earthquake alerts daily",
            "Magnitude deploys Katapult earthquake alerts daily",
            &HashSet::new(),
        );

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].context_words.is_empty());
    }

    #[test]
    fn case_only_change_needs_a_trusted_target() {
        let original = "Developers browse github repositories daily";
        let corrected = "Developers browse GitHub repositories daily";
        let suggested = classify_pairs(original, corrected, &HashSet::new());
        let automatic = classify_pairs(original, corrected, &trusted(&["github"]));

        assert_eq!(suggested.len(), 1);
        assert!(suggested[0].context_words.is_empty());
        assert!(!automatic[0].context_words.is_empty());
    }

    #[test]
    fn short_normalized_sides_never_activate_automatically() {
        let candidates = classify_pairs(
            "Developers document api clients daily",
            "Developers document API clients daily",
            &trusted(&["api"]),
        );

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].context_words.is_empty());
    }

    #[test]
    fn weak_proper_noun_match_stays_a_suggestion_even_when_trusted() {
        let candidates = classify_pairs(
            "Magnitude routes Bededa earthquake alerts daily",
            "Magnitude routes Pereira earthquake alerts daily",
            &trusted(&["pereira"]),
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].entry.right, "Pereira");
        assert!(candidates[0].context_words.is_empty());
    }

    #[test]
    fn base_learner_still_rejects_deletions_and_contextual_rewrites() {
        assert!(classify_pairs("sense again.", "sense.", &HashSet::new()).is_empty());
        assert!(classify_pairs("enabled", "end-to-end", &HashSet::new()).is_empty());
    }

    #[test]
    fn unchanged_word_followed_by_question_and_bullet_is_not_a_correction() {
        for corrected in [
            "automatically? ● Y",
            "automatically? • Y",
            "automatically?\n• Y",
        ] {
            let candidates = classify_pairs("automatically", corrected, &HashSet::new());
            assert!(
                candidates.is_empty(),
                "formatting became a correction: {candidates:?}"
            );
        }
    }

    #[test]
    fn homophones_and_common_grammar_remain_ignored() {
        assert!(classify_pairs(
            "over their by the door",
            "over there by the door",
            &trusted(&["there"]),
        )
        .is_empty());
        assert!(
            classify_pairs("more then that", "more than that", &trusted(&["than"]),).is_empty()
        );
    }

    #[test]
    fn numbers_and_short_tokens_cannot_unlock_automatic_rules() {
        let terms = trusted(&["katapult"]);
        let weak = classify_pairs("We have Catapult 2026 x", "We have Katapult 2026 x", &terms);
        assert_eq!(weak.len(), 1);
        assert!(weak[0].context_words.is_empty());

        let mixed = classify_pairs(
            "We shipped Catapult 2026 x today",
            "We shipped Katapult 2026 x today",
            &terms,
        );
        assert_eq!(mixed.len(), 1);
        assert!(!mixed[0].context_words.is_empty());
        assert!(mixed[0]
            .context_words
            .iter()
            .all(|word| word != "2026" && word != "x"));
        let candidate = mixed.into_iter().next().unwrap();
        let matcher = crate::dictionary_matcher::DictionaryMatcher::new(vec![
            crate::dictionary_matcher::DictionaryRule {
                entry: candidate.entry,
                context_words: candidate.context_words,
            },
        ])
        .unwrap();
        assert_eq!(
            matcher.apply("a catapult 2026 x records"),
            "a catapult 2026 x records"
        );
        assert_eq!(
            matcher.apply("We shipped catapult software"),
            "We shipped Katapult software"
        );
    }

    #[test]
    fn missing_context_keeps_a_strong_pair_suggested() {
        let candidates = classify_pairs("Catapult", "Katapult", &trusted(&["katapult"]));

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].context_words.is_empty());
    }

    #[test]
    fn unsupported_script_stays_a_suggestion() {
        let candidates = classify_pairs(
            "Teams discuss Málaga earthquakes daily",
            "Teams discuss Malaga earthquakes daily",
            &trusted(&["malaga"]),
        );

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].context_words.is_empty());
    }

    #[test]
    fn repeated_pair_occurrence_is_ambiguous() {
        let candidates = classify_pairs(
            "Catapult alpha Catapult beta",
            "Katapult alpha Catapult beta",
            &trusted(&["katapult"]),
        );

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].context_words.is_empty());
    }

    #[test]
    fn broad_or_multiple_edits_cannot_lend_misleading_context() {
        let candidates = classify_pairs(
            "alpha Catapult omega old material one two three",
            "alpha Katapult omega new content four five six",
            &trusted(&["katapult"]),
        );

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].context_words.is_empty());
    }

    #[test]
    fn broad_rewrite_trims_large_identical_edges_before_lcs() {
        let prefix: Vec<String> = (0..600).map(|index| format!("prefix{index}")).collect();
        let suffix: Vec<String> = (0..600).map(|index| format!("suffix{index}")).collect();
        let mut original = prefix.clone();
        original.push("Catapult".to_string());
        original.extend(suffix.clone());
        let mut corrected = prefix;
        corrected.push("Katapult".to_string());
        corrected.extend(suffix);

        assert!(!is_broad_rewrite(&original, &corrected));
    }

    #[test]
    fn broad_rewrite_uses_exact_three_quarter_threshold() {
        let original = ["alpha", "bravo", "charlie", "delta"].map(String::from);
        let one_change = ["alpha", "other", "charlie", "delta"].map(String::from);
        let two_changes = ["alpha", "other", "different", "delta"].map(String::from);

        assert!(!is_broad_rewrite(&original, &one_change));
        assert!(is_broad_rewrite(&original, &two_changes));
    }

    #[test]
    fn broad_rewrite_falls_back_conservatively_above_lcs_cell_budget() {
        let original: Vec<String> = (0..300).map(|index| format!("original{index}")).collect();
        let corrected: Vec<String> = (0..300).map(|index| format!("corrected{index}")).collect();

        assert!(original.len() * corrected.len() > MAX_RESIDUAL_LCS_CELLS);
        assert!(is_broad_rewrite(&original, &corrected));
    }

    #[test]
    fn oversized_or_pathological_inputs_are_skipped_before_diffing() {
        let oversized = "a".repeat(MAX_INPUT_BYTES + 1);
        assert!(classify_pairs(&oversized, "b", &HashSet::new()).is_empty());

        let long_token = "a".repeat(MAX_TOKEN_CHARS + 1);
        assert!(classify_pairs(&long_token, "b", &HashSet::new()).is_empty());

        let too_many_words = vec!["word"; MAX_WHITESPACE_WORDS + 1].join(" ");
        assert!(classify_pairs(&too_many_words, "word", &HashSet::new()).is_empty());
    }

    #[test]
    fn candidate_count_is_capped() {
        let original = (0..40)
            .map(|index| format!("Catapult{index} anchor{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let corrected = (0..40)
            .map(|index| format!("Katapult{index} anchor{index}"))
            .collect::<Vec<_>>()
            .join(" ");

        let candidates = classify_pairs(&original, &corrected, &HashSet::new());
        assert_eq!(candidates.len(), MAX_CANDIDATES);
    }

    #[test]
    #[ignore = "microbenchmark: run with cargo test --release -- --ignored --nocapture"]
    fn benchmark_learning_on_1000_word_edit() {
        use std::hint::black_box;
        use std::time::Instant;

        let mut original_words: Vec<String> =
            (0..1_000).map(|index| format!("context{index}")).collect();
        original_words[499] = "releases".to_string();
        original_words[500] = "Catapult".to_string();
        original_words[501] = "deployments".to_string();
        let mut corrected_words = original_words.clone();
        corrected_words[500] = "Katapult".to_string();
        let original = original_words.join(" ");
        let corrected = corrected_words.join(" ");
        let trusted = trusted(&["katapult"]);
        let mut samples = Vec::new();

        for _ in 0..51 {
            let start = Instant::now();
            let candidates = classify_pairs(
                black_box(&original),
                black_box(&corrected),
                black_box(&trusted),
            );
            let candidates = black_box(candidates);
            samples.push(start.elapsed());
            assert_eq!(candidates.len(), 1);
            assert!(!candidates[0].context_words.is_empty());
        }

        samples.sort_unstable();
        let median = samples[samples.len() / 2];
        let p95_index = (samples.len() * 95).div_ceil(100) - 1;
        let p95 = samples[p95_index];
        eprintln!("classify_pairs 1000-word edit: median={median:?}, p95={p95:?}");
        assert!(
            median < std::time::Duration::from_millis(5),
            "median classifier time {median:?} exceeded 5 ms"
        );
    }
}
