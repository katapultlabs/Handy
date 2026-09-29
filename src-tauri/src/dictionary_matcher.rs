//! Compiled dictionary snapshot. Construct after a mutation, reuse at paste time.
//!
//! Automatic rules require nearby context; explicit rules apply everywhere.
//! No database, phonetics, model calls, or pattern compilation run in `apply`.
use crate::dictionary::{
    at_sentence_start, boundaries_ok, render_replacement, DictionaryEntry, FoldedText,
};
use aho_corasick::AhoCorasick;
use std::collections::BTreeMap;

pub struct DictionaryRule {
    pub entry: DictionaryEntry,
    /// Lowercase alphanumeric words observed near an automatic correction.
    /// Empty for an explicit, globally applicable correction.
    pub context_words: Vec<String>,
}

#[derive(Default)]
pub struct DictionaryMatcher {
    matcher: Option<AhoCorasick>,
    rules: Vec<DictionaryRule>,
    lengths: Vec<usize>,
}

// Bound pathological overlapping pattern sets. Skip the whole dictionary pass
// rather than apply an incomplete set of corrections or stall a dictation.
const MAX_MATCHES: usize = 16_384;
const MAX_TRANSCRIPT_BYTES: usize = 128 * 1024;
const CONTEXT_RADIUS: usize = 4;

struct ContextToken {
    start: usize,
    end: usize,
    lower: String,
}

fn context_tokens(text: &str) -> Vec<ContextToken> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        if ch.is_alphanumeric() {
            start.get_or_insert(index);
        } else if let Some(begin) = start.take() {
            tokens.push(ContextToken {
                start: begin,
                end: index,
                lower: text[begin..index].to_lowercase(),
            });
        }
    }
    if let Some(begin) = start {
        tokens.push(ContextToken {
            start: begin,
            end: text.len(),
            lower: text[begin..].to_lowercase(),
        });
    }
    tokens
}

fn context_matches(tokens: &[ContextToken], start: usize, end: usize, words: &[String]) -> bool {
    let left = tokens.partition_point(|token| token.end <= start);
    let right = tokens.partition_point(|token| token.start < end);
    tokens[left.saturating_sub(CONTEXT_RADIUS)..left]
        .iter()
        .chain(tokens[right..].iter().take(CONTEXT_RADIUS))
        .any(|token| words.contains(&token.lower))
}

impl DictionaryMatcher {
    pub fn new(rules: Vec<DictionaryRule>) -> Result<Self, aho_corasick::BuildError> {
        let rules: Vec<_> = rules
            .into_iter()
            .filter(|rule| !rule.entry.wrong.trim().is_empty())
            .collect();
        if rules.is_empty() {
            return Ok(Self::default());
        }
        let patterns: Vec<_> = rules
            .iter()
            .map(|rule| rule.entry.wrong.to_lowercase())
            .collect();
        let lengths = rules
            .iter()
            .map(|rule| rule.entry.wrong.chars().count())
            .collect();
        let matcher = Some(AhoCorasick::new(patterns)?);
        Ok(Self {
            matcher,
            rules,
            lengths,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn apply(&self, text: &str) -> String {
        let Some(matcher) = &self.matcher else {
            return text.to_string();
        };
        if text.len() > MAX_TRANSCRIPT_BYTES {
            return text.to_string();
        }
        let folded = FoldedText::new(text);
        let mut candidates = Vec::new();
        let mut context = None;
        // Match the reference's non-overlapping scan for each individual
        // pattern, even when a raw match later fails its word boundaries.
        let mut pattern_ends = vec![0; self.rules.len()];
        for (match_count, found) in matcher.find_overlapping_iter(&folded.lower).enumerate() {
            if match_count >= MAX_MATCHES {
                return text.to_string();
            }
            let index = found.pattern().as_usize();
            if found.start() < pattern_ends[index] {
                continue;
            }
            pattern_ends[index] = found.end();
            let rule = &self.rules[index];
            let start = folded.orig_start(found.start());
            let end = folded.orig_end(found.end(), text);
            if !boundaries_ok(text, start, end, &rule.entry.wrong) {
                continue;
            }
            if !rule.context_words.is_empty() {
                let tokens = context.get_or_insert_with(|| context_tokens(text));
                if !context_matches(tokens, start, end, &rule.context_words) {
                    continue;
                }
            }
            candidates.push((index, start, end));
        }
        // Preserve the original longest-pattern-first semantics, including
        // stable entry order for equal lengths. Never cascade replacements.
        candidates.sort_unstable_by_key(|(index, start, _)| {
            (std::cmp::Reverse(self.lengths[*index]), *index, *start)
        });
        let mut claims: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
        for (index, start, end) in candidates {
            if claims
                .range(..end)
                .next_back()
                .is_some_and(|(_, (previous_end, _))| *previous_end > start)
            {
                continue;
            }
            claims.insert(start, (end, index));
        }
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0;
        for (start, (end, index)) in claims {
            out.push_str(&text[cursor..start]);
            out.push_str(&render_replacement(
                &text[start..end],
                &self.rules[index].entry,
                at_sentence_start(text, start),
            ));
            cursor = end;
        }
        out.push_str(&text[cursor..]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::{apply_dictionary, CaseMode};
    use std::time::{Duration, Instant};

    fn rule(wrong: &str, right: &str, context: &[&str]) -> DictionaryRule {
        DictionaryRule {
            entry: DictionaryEntry {
                wrong: wrong.into(),
                right: right.into(),
                source: "history".into(),
                case_mode: CaseMode::Exact,
            },
            context_words: context.iter().map(|word| (*word).into()).collect(),
        }
    }

    #[test]
    fn automatic_rule_applies_only_near_its_context() {
        let matcher =
            DictionaryMatcher::new(vec![rule("catapult", "Katapult", &["releases"])]).unwrap();
        assert_eq!(
            matcher.apply("catapult manages our releases."),
            "Katapult manages our releases."
        );
        assert_eq!(
            matcher.apply("a catapult launches rocks."),
            "a catapult launches rocks."
        );
        assert_eq!(
            matcher.apply("releases follow. One two three four five catapult"),
            "releases follow. One two three four five catapult"
        );
        assert_eq!(
            matcher.apply("we use CATAPULT for RELEASES"),
            "we use Katapult for RELEASES"
        );
    }

    #[test]
    fn explicit_rules_preserve_existing_matching_semantics() {
        let rules = vec![
            rule("aa", "aaa", &[]),
            rule("new hampshire", "New Hampshire", &[]),
            rule("new", "brand new", &[]),
            rule(".net", ".NET", &[]),
            rule("café", "Café Prüm", &[]),
            rule("price", "$1 \\ raw", &[]),
        ];
        let entries: Vec<_> = rules.iter().map(|r| r.entry.clone()).collect();
        let matcher = DictionaryMatcher::new(rules).unwrap();
        for text in [
            "aa aaa aa",
            "new hampshire and new homes",
            "use .net and café",
            "price\n\nprice   price",
            "domain mainland",
            "",
        ] {
            assert_eq!(
                matcher.apply(text),
                apply_dictionary(text, &entries),
                "{text}"
            );
        }
    }

    #[test]
    fn automatic_context_does_not_cascade_from_replacement_output() {
        let matcher = DictionaryMatcher::new(vec![
            rule("deploy", "releases", &[]),
            rule("catapult", "Katapult", &["releases"]),
        ])
        .unwrap();
        assert_eq!(matcher.apply("deploy catapult"), "Releases catapult");
    }

    #[test]
    fn compiled_matcher_handles_dense_repetition_and_longest_overlaps() {
        let rules = vec![
            rule("aa aa", "long", &[]),
            rule("aa", "short", &[]),
            rule("aaa", "wide", &[]),
        ];
        let entries: Vec<_> = rules.iter().map(|r| r.entry.clone()).collect();
        let matcher = DictionaryMatcher::new(rules).unwrap();
        let text = "aa aa aaa aa aa ".repeat(100);
        assert_eq!(matcher.apply(&text), apply_dictionary(&text, &entries));
    }

    #[test]
    fn pathological_input_skips_the_dictionary_without_partial_edits() {
        let matcher = DictionaryMatcher::new(vec![rule("a", "b", &[])]).unwrap();
        let oversized = "a ".repeat(MAX_TRANSCRIPT_BYTES / 2 + 1);
        assert_eq!(matcher.apply(&oversized), oversized);
        // Even raw matches rejected by boundaries count toward the budget.
        let too_many_matches = "a".repeat(MAX_MATCHES + 1);
        assert_eq!(matcher.apply(&too_many_matches), too_many_matches);
        let dense = "a ".repeat(MAX_MATCHES + 1);
        assert_eq!(matcher.apply(&dense), dense);
    }

    #[test]
    #[ignore = "optimized performance measurement; run with --release --ignored --nocapture"]
    fn dictionary_performance_budget() {
        let mut sample = String::new();
        let words = ["the", "release", "is", "ready", "uniqueword42"];
        for i in 0..1000 {
            if i > 0 {
                sample.push(' ');
            }
            sample.push_str(words[i % words.len()]);
        }
        for size in [100, 1000, 10_000] {
            let rules: Vec<_> = (0..size)
                .map(|i| {
                    rule(
                        &format!("uniqueword{i}"),
                        &format!("Fixed{i}"),
                        &["release"],
                    )
                })
                .collect();
            let build_start = Instant::now();
            let matcher = DictionaryMatcher::new(rules).unwrap();
            let build = build_start.elapsed();
            let mut samples = Vec::new();
            for _ in 0..51 {
                let start = Instant::now();
                let out = std::hint::black_box(matcher.apply(std::hint::black_box(&sample)));
                samples.push(start.elapsed());
                assert!(out.contains("Fixed42"));
            }
            samples.sort();
            let median = samples[25];
            println!("dictionary {size} rules, 1000 words: build={build:?}, apply median={median:?}, p95={:?}", samples[48]);
            if size == 1000 {
                assert!(
                    median < Duration::from_millis(1),
                    "matcher exceeds 1 ms budget: {median:?}"
                );
            }
        }
    }
}
