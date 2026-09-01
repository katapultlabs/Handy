//! The Dictionary: deterministic wrong -> right transcription corrections.
//!
//! MVP of the design in `docs/DICTIONARY_DESIGN.md`. Two parts:
//!
//! - [`apply_dictionary`]: the Tier 1 exact matcher. One pass, longest match
//!   first, no cascading (a replacement's output is never re-matched), literal
//!   insertion (no `$` expansion), word boundaries only where the pattern edge
//!   is alphanumeric (so `C++`, `.NET`, `@handle` match), case handling per
//!   entry.
//! - [`learn_pairs`]: turns an (original, corrected) text pair into proposed
//!   entries. Word-level diff gated by size, edit distance, and Double
//!   Metaphone phonetic similarity — a misheard word *sounds* like its fix; a
//!   rewrite does not.
//!
//! MVP deviations from the design doc: entries live in settings (not SQLite),
//! there is no proposed/active state machine (the frontend confirms pairs
//! before they are stored), and there is no in-place capture yet.

use serde::{Deserialize, Serialize};
use specta::Type;

/// How the replacement text is cased when an entry matches.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum CaseMode {
    /// Copy the case pattern of the matched text onto `right`
    /// (matched `MAINE` -> `MAIN`, matched `Maine` -> `Main`).
    Smart,
    /// Insert `right` exactly as stored. If `right` is all lowercase and the
    /// match sits at a sentence start, the first letter is capitalized.
    Exact,
}

/// One correction: replace `wrong` with `right`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Type)]
pub struct DictionaryEntry {
    pub wrong: String,
    pub right: String,
    pub case_mode: CaseMode,
    /// Where the entry came from: "manual" or "history".
    pub source: String,
}

// ---------------------------------------------------------------------------
// Tier 1 exact matcher
// ---------------------------------------------------------------------------

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Case-insensitive haystack with a byte map back into the original text.
struct FoldedText {
    lower: String,
    /// For every byte of `lower`, the byte offset in the original text of the
    /// character it came from.
    map: Vec<usize>,
    orig_len: usize,
}

impl FoldedText {
    fn new(text: &str) -> Self {
        let mut lower = String::with_capacity(text.len());
        let mut map = Vec::with_capacity(text.len());
        for (oi, ch) in text.char_indices() {
            for lch in ch.to_lowercase() {
                let start = lower.len();
                lower.push(lch);
                for _ in start..lower.len() {
                    map.push(oi);
                }
            }
        }
        Self {
            lower,
            map,
            orig_len: text.len(),
        }
    }

    /// Original byte offset of the character a `lower` byte belongs to.
    fn orig_start(&self, lower_idx: usize) -> usize {
        self.map[lower_idx]
    }

    /// Original byte offset just past the character the last `lower` byte of
    /// a match belongs to.
    fn orig_end(&self, lower_end: usize, text: &str) -> usize {
        if lower_end >= self.map.len() {
            return self.orig_len;
        }
        let start = self.map[lower_end - 1];
        // Advance one char from `start` in the original text.
        text[start..]
            .chars()
            .next()
            .map(|c| start + c.len_utf8())
            .unwrap_or(self.orig_len)
    }
}

/// True when the match at `[start, end)` (original byte offsets) sits on word
/// boundaries where the pattern requires them.
fn boundaries_ok(text: &str, start: usize, end: usize, wrong: &str) -> bool {
    let first_needs = wrong.chars().next().map(is_word_char).unwrap_or(false);
    let last_needs = wrong.chars().last().map(is_word_char).unwrap_or(false);

    if first_needs {
        if let Some(prev) = text[..start].chars().next_back() {
            if is_word_char(prev) {
                return false;
            }
        }
    }
    if last_needs {
        if let Some(next) = text[end..].chars().next() {
            if is_word_char(next) {
                return false;
            }
        }
    }
    true
}

/// True when `start` (original byte offset) is at a sentence start: the
/// beginning of the text, or preceded (ignoring whitespace) by `.`, `!`, `?`,
/// `…`, or a line break.
fn at_sentence_start(text: &str, start: usize) -> bool {
    for c in text[..start].chars().rev() {
        if c == '\n' || c == '\r' {
            // A line break starts a new sentence.
            return true;
        }
        if c.is_whitespace() {
            continue;
        }
        return matches!(c, '.' | '!' | '?' | '…');
    }
    // Only whitespace (or nothing) before the match.
    true
}

/// Classify a run's case pattern for `learn_pairs` and smart casing.
#[derive(PartialEq, Debug, Clone, Copy)]
enum CasePattern {
    AllLower,
    FirstUpper,
    AllUpper,
    Mixed,
}

fn case_pattern(s: &str) -> CasePattern {
    let letters: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return CasePattern::AllLower;
    }
    let uppers = letters.iter().filter(|c| c.is_uppercase()).count();
    if uppers == 0 {
        CasePattern::AllLower
    } else if uppers == letters.len() {
        if letters.len() == 1 {
            CasePattern::FirstUpper
        } else {
            CasePattern::AllUpper
        }
    } else if uppers == 1 && letters[0].is_uppercase() {
        CasePattern::FirstUpper
    } else {
        CasePattern::Mixed
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Render the replacement for one match.
fn render_replacement(matched: &str, entry: &DictionaryEntry, sentence_start: bool) -> String {
    match entry.case_mode {
        CaseMode::Exact => {
            if sentence_start && entry.right.chars().next().is_some_and(|c| c.is_lowercase()) {
                capitalize_first(&entry.right)
            } else {
                entry.right.clone()
            }
        }
        CaseMode::Smart => match case_pattern(matched) {
            CasePattern::AllUpper => entry.right.to_uppercase(),
            CasePattern::FirstUpper => capitalize_first(&entry.right),
            _ => entry.right.clone(),
        },
    }
}

/// Apply the dictionary to `text` in one pass.
///
/// Longest `wrong` wins on overlap. Spans produced by a replacement are never
/// re-matched. Insertion is literal — no regex, no `$` expansion. Whitespace
/// outside the matched spans is untouched.
pub fn apply_dictionary(text: &str, entries: &[DictionaryEntry]) -> String {
    if text.is_empty() || entries.is_empty() {
        return text.to_string();
    }

    let folded = FoldedText::new(text);

    // Longest pattern first so "New Hampshire" beats "New".
    let mut order: Vec<&DictionaryEntry> = entries
        .iter()
        .filter(|e| !e.wrong.trim().is_empty())
        .collect();
    order.sort_by_key(|e| std::cmp::Reverse(e.wrong.chars().count()));

    // Claimed spans in original byte offsets, kept sorted and non-overlapping.
    let mut claims: Vec<(usize, usize, String)> = Vec::new();

    for entry in order {
        let needle = entry.wrong.to_lowercase();
        if needle.is_empty() {
            continue;
        }
        for (pos, _) in folded.lower.match_indices(&needle) {
            let start = folded.orig_start(pos);
            let end = folded.orig_end(pos + needle.len(), text);
            if !boundaries_ok(text, start, end, &entry.wrong) {
                continue;
            }
            if claims.iter().any(|(s, e, _)| start < *e && *s < end) {
                continue; // overlaps an earlier (longer or same-length) claim
            }
            let replacement =
                render_replacement(&text[start..end], entry, at_sentence_start(text, start));
            claims.push((start, end, replacement));
        }
    }

    if claims.is_empty() {
        return text.to_string();
    }
    claims.sort_by_key(|(s, _, _)| *s);

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end, replacement) in claims {
        out.push_str(&text[cursor..start]);
        out.push_str(&replacement);
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

// ---------------------------------------------------------------------------
// learn(): from an edit to proposed entries
// ---------------------------------------------------------------------------

/// Punctuation trimmed from the edges of a diff run before it becomes a pair.
/// Deliberately conservative: `+`, `#`, `@`, `.` inside identifiers survive.
const EDGE_PUNCT: &[char] = &[',', '.', '!', '?', ';', ':', '"', '\'', '(', ')', '“', '”'];

fn trim_edges(s: &str) -> &str {
    s.trim_matches(|c: char| EDGE_PUNCT.contains(&c))
}

/// Normalize a run for similarity tests: lowercase, alphanumeric only.
fn norm_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn phonetic_key(s: &str) -> Option<String> {
    // Double Metaphone is defined for ASCII letters. Skip the phonetic gate
    // for anything else (CJK, Cyrillic, digits-only) per the design doc.
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    use rphonetic::Encoder;
    let encoder = rphonetic::DoubleMetaphone::default();
    let key = encoder.encode(s);
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

/// A word run changed between original and corrected text.
struct ChangedRun {
    wrong: String,
    right: String,
}

/// Compute changed runs with a word-level diff.
fn changed_runs(original: &str, corrected: &str) -> Vec<ChangedRun> {
    let old: Vec<&str> = original.split_whitespace().collect();
    let new: Vec<&str> = corrected.split_whitespace().collect();
    let diff = similar::TextDiff::from_slices(&old, &new);

    let mut runs = Vec::new();
    for op in diff.ops() {
        if let similar::DiffOp::Replace {
            old_index,
            old_len,
            new_index,
            new_len,
        } = *op
        {
            runs.push(ChangedRun {
                wrong: old[old_index..old_index + old_len].join(" "),
                right: new[new_index..new_index + new_len].join(" "),
            });
        }
        // Pure insertions and deletions are edits, not corrections. Skip.
    }
    runs
}

/// Decide whether one changed run is a learnable correction, and how.
fn evaluate_run(run: &ChangedRun) -> Option<DictionaryEntry> {
    let wrong = trim_edges(&run.wrong).trim();
    let right = trim_edges(&run.right).trim();
    if wrong.is_empty() || right.is_empty() || wrong == right {
        return None;
    }
    let word_count = |s: &str| s.split_whitespace().count();
    if !(1..=3).contains(&word_count(wrong)) || !(1..=3).contains(&word_count(right)) {
        return None;
    }

    let case_only = wrong.to_lowercase() == right.to_lowercase();
    if case_only {
        return Some(DictionaryEntry {
            wrong: wrong.to_string(),
            right: right.to_string(),
            case_mode: CaseMode::Exact,
            source: "history".to_string(),
        });
    }

    let (nw, nr) = (norm_key(wrong), norm_key(right));
    if nw.is_empty() || nr.is_empty() {
        return None;
    }

    // Look similar: bounded character edit distance.
    let longer = nw.chars().count().max(nr.chars().count());
    let distance = strsim::levenshtein(&nw, &nr);
    if longer == 0 || (distance as f64) / (longer as f64) > 0.6 {
        return None;
    }

    // Sound similar: Double Metaphone keys equal or one apart is a strong
    // match. Badly misheard proper nouns can differ more (e.g. "Bededa" ->
    // "Pereira" folds to PTT vs PRR), so a weak match — the keys begin with
    // the same sound — is also accepted; the edit-distance gate above already
    // holds. Different first sounds ("meeting" -> "sync") stay rejected.
    // When the phonetic algorithm does not cover the text (non-ASCII), the
    // edit distance gate stands alone.
    if let (Some(kw), Some(kr)) = (phonetic_key(&nw), phonetic_key(&nr)) {
        let close_keys = strsim::levenshtein(&kw, &kr) <= 1;
        let same_first_sound = kw.chars().next() == kr.chars().next();
        if !close_keys && !same_first_sound {
            return None;
        }
    }

    // The case of the fix is part of the fix (Maine -> main, github -> GitHub)
    // unless both sides carry the same pattern (Klein -> Cline).
    let case_mode = if case_pattern(wrong) == case_pattern(right) {
        CaseMode::Smart
    } else {
        CaseMode::Exact
    };

    Some(DictionaryEntry {
        wrong: wrong.to_string(),
        right: right.to_string(),
        case_mode,
        source: "history".to_string(),
    })
}

/// Extract proposed dictionary entries from an edit.
///
/// `original` is the text Handy produced (what was pasted); `corrected` is the
/// text after the user's edit. Returns deduplicated proposals; the caller
/// (frontend) confirms them before they are stored.
pub fn learn_pairs(original: &str, corrected: &str) -> Vec<DictionaryEntry> {
    let mut out: Vec<DictionaryEntry> = Vec::new();
    for run in changed_runs(original, corrected) {
        if let Some(entry) = evaluate_run(&run) {
            let dup = out.iter().any(|e| {
                e.wrong.to_lowercase() == entry.wrong.to_lowercase() && e.right == entry.right
            });
            if !dup {
                out.push(entry);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(wrong: &str, right: &str, case_mode: CaseMode) -> DictionaryEntry {
        DictionaryEntry {
            wrong: wrong.to_string(),
            right: right.to_string(),
            case_mode,
            source: "manual".to_string(),
        }
    }

    // ------------------------- apply_dictionary ---------------------------

    #[test]
    fn exact_replaces_case_insensitively() {
        let e = [entry("Maine", "main", CaseMode::Exact)];
        assert_eq!(
            apply_dictionary("I pushed to maine yesterday", &e),
            "I pushed to main yesterday"
        );
        assert_eq!(
            apply_dictionary("I pushed to Maine yesterday", &e),
            "I pushed to main yesterday"
        );
    }

    #[test]
    fn exact_capitalizes_at_sentence_start() {
        let e = [entry("Maine", "main", CaseMode::Exact)];
        assert_eq!(
            apply_dictionary("Maine is the branch.", &e),
            "Main is the branch."
        );
        assert_eq!(
            apply_dictionary("Push it. Maine has the fix.", &e),
            "Push it. Main has the fix."
        );
        assert_eq!(
            apply_dictionary("first line\nMaine again", &e),
            "first line\nMain again"
        );
    }

    #[test]
    fn smart_copies_case_pattern() {
        let e = [entry("cline", "Cline", CaseMode::Smart)];
        assert_eq!(
            apply_dictionary("ask CLINE about it", &e),
            "ask CLINE about it"
        );
        let e = [entry("klein", "cline", CaseMode::Smart)];
        assert_eq!(
            apply_dictionary("ask Klein about it", &e),
            "ask Cline about it"
        );
        assert_eq!(
            apply_dictionary("ask KLEIN about it", &e),
            "ask CLINE about it"
        );
        assert_eq!(
            apply_dictionary("ask klein about it", &e),
            "ask cline about it"
        );
    }

    #[test]
    fn word_boundaries_hold_for_alphanumeric_edges() {
        let e = [entry("main", "Maine", CaseMode::Exact)];
        assert_eq!(
            apply_dictionary("the mainland remains", &e),
            "the mainland remains"
        );
        assert_eq!(apply_dictionary("domain names", &e), "domain names");
    }

    #[test]
    fn non_alphanumeric_edges_need_no_boundary() {
        let e = [entry("C++", "C++23", CaseMode::Exact)];
        assert_eq!(
            apply_dictionary("I write C++ daily", &e),
            "I write C++23 daily"
        );
        let e = [entry(".net", ".NET", CaseMode::Exact)];
        assert_eq!(apply_dictionary("the .net runtime", &e), "the .NET runtime");
    }

    #[test]
    fn longest_match_wins_and_no_cascade() {
        let e = [
            entry("new", "New", CaseMode::Exact),
            entry("new hampshire", "New Hampshire", CaseMode::Exact),
        ];
        assert_eq!(
            apply_dictionary("moved to new hampshire", &e),
            "moved to New Hampshire"
        );
        // A replacement's output is never re-matched. (The first "aa" sits at
        // the start of the text, which is a sentence start, so an all-lowercase
        // Exact replacement gets its first letter capitalized there.)
        let e = [entry("aa", "aaa", CaseMode::Exact)];
        assert_eq!(apply_dictionary("aa aa", &e), "Aaa aaa");
        assert_eq!(apply_dictionary("x aa aa", &e), "x aaa aaa");
    }

    #[test]
    fn literal_insertion_no_expansion() {
        let e = [entry("price", "$1 (raw $ and \\ kept)", CaseMode::Exact)];
        assert_eq!(
            apply_dictionary("the price is right", &e),
            "the $1 (raw $ and \\ kept) is right"
        );
    }

    #[test]
    fn whitespace_and_newlines_survive() {
        let e = [entry("maine", "main", CaseMode::Exact)];
        assert_eq!(
            apply_dictionary("para one maine\n\npara  two   maine", &e),
            "para one main\n\npara  two   main"
        );
    }

    #[test]
    fn punctuation_around_match_survives() {
        let e = [entry("maine", "main", CaseMode::Exact)];
        assert_eq!(
            apply_dictionary("push to maine, then rest", &e),
            "push to main, then rest"
        );
        assert_eq!(apply_dictionary("(maine)", &e), "(main)");
    }

    #[test]
    fn multiword_phrase_matches() {
        let e = [entry("char gebee", "ChargeBee", CaseMode::Exact)];
        assert_eq!(
            apply_dictionary("we use char gebee here", &e),
            "we use ChargeBee here"
        );
    }

    #[test]
    fn unicode_text_is_safe() {
        let e = [entry("café", "Café Prüm", CaseMode::Exact)];
        assert_eq!(
            apply_dictionary("meet at café tomorrow", &e),
            "meet at Café Prüm tomorrow"
        );
        // Match at the very end, non-ASCII around it.
        assert_eq!(apply_dictionary("übung im café", &e), "übung im Café Prüm");
    }

    #[test]
    fn empty_inputs_are_identity() {
        assert_eq!(
            apply_dictionary("", &[entry("a", "b", CaseMode::Exact)]),
            ""
        );
        assert_eq!(apply_dictionary("text", &[]), "text");
        let blank = [entry("   ", "x", CaseMode::Exact)];
        assert_eq!(apply_dictionary("text", &blank), "text");
    }

    // ------------------------------ learn ---------------------------------

    #[test]
    fn learns_misheard_word() {
        let pairs = learn_pairs("I pushed to Maine yesterday", "I pushed to main yesterday");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].wrong, "Maine");
        assert_eq!(pairs[0].right, "main");
        assert_eq!(pairs[0].case_mode, CaseMode::Exact); // case is part of the fix
    }

    #[test]
    fn learns_name_with_same_case_pattern_as_smart() {
        let pairs = learn_pairs("ask Klein about it", "ask Cline about it");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].wrong, "Klein");
        assert_eq!(pairs[0].right, "Cline");
        assert_eq!(pairs[0].case_mode, CaseMode::Smart);
    }

    #[test]
    fn learns_case_only_change_as_exact() {
        let pairs = learn_pairs("check the github repo", "check the GitHub repo");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].wrong, "github");
        assert_eq!(pairs[0].right, "GitHub");
        assert_eq!(pairs[0].case_mode, CaseMode::Exact);
    }

    #[test]
    fn learns_split_name() {
        let pairs = learn_pairs("we use char gebee here", "we use ChargeBee here");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].wrong, "char gebee");
        assert_eq!(pairs[0].right, "ChargeBee");
    }

    #[test]
    fn rejects_rewrite() {
        assert!(learn_pairs("the meeting went well", "our sync went well").is_empty());
        assert!(learn_pairs("that is good", "that is great").is_empty());
    }

    #[test]
    fn learns_badly_misheard_proper_noun() {
        // Real user case: same first sound (B/P fold together), high but
        // passing edit distance, metaphone keys two apart.
        let pairs = learn_pairs(
            "I'm gonna be in Bededa for the rest of the day",
            "I'm gonna be in Pereira for the rest of the day",
        );
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].wrong, "Bededa");
        assert_eq!(pairs[0].right, "Pereira");
    }

    #[test]
    fn rejects_pure_insertion_and_deletion() {
        assert!(learn_pairs("push the branch", "push the new branch").is_empty());
        assert!(learn_pairs("push the new branch", "push the branch").is_empty());
    }

    #[test]
    fn rejects_full_rewrite_of_paragraph() {
        let orig = "This is a long paragraph about one topic entirely.";
        let edit = "Something different altogether, restructured and rephrased fully.";
        assert!(learn_pairs(orig, edit).is_empty());
    }

    #[test]
    fn strips_edge_punctuation_from_pairs() {
        let pairs = learn_pairs("deployed to Maine.", "deployed to main.");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].wrong, "Maine");
        assert_eq!(pairs[0].right, "main");
    }

    #[test]
    fn homophone_passes_gates() {
        let pairs = learn_pairs("over their by the door", "over there by the door");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].wrong, "their");
        assert_eq!(pairs[0].right, "there");
    }

    #[test]
    fn dedupes_repeated_pairs() {
        let pairs = learn_pairs("Maine here and Maine there", "main here and main there");
        assert_eq!(pairs.len(), 1);
    }

    #[test]
    fn cjk_change_uses_distance_gate_only() {
        // One character changed in a CJK word: phonetic gate skipped,
        // distance gate accepts.
        let pairs = learn_pairs("我用 微软 系统", "我用 微轮 系统");
        assert_eq!(pairs.len(), 1);
    }

    #[test]
    fn empty_edit_learns_nothing() {
        assert!(learn_pairs("same text", "same text").is_empty());
        assert!(learn_pairs("", "").is_empty());
    }

    // --------------------------- performance -------------------------------

    #[test]
    fn matcher_stays_fast_at_scale() {
        // 1,000 entries x ~1,000 words. Budget from the design doc is 1 ms on
        // the oldest supported hardware; allow slack for debug builds and CI.
        let entries: Vec<DictionaryEntry> = (0..1000)
            .map(|i| {
                entry(
                    &format!("uniqueword{i}"),
                    &format!("Fixed{i}"),
                    CaseMode::Exact,
                )
            })
            .collect();
        let text = "the quick brown fox uniqueword42 jumps over the lazy dog ".repeat(100);
        let start = std::time::Instant::now();
        let out = apply_dictionary(&text, &entries);
        let elapsed = start.elapsed();
        assert!(out.contains("Fixed42"));
        assert!(
            elapsed < std::time::Duration::from_millis(250),
            "matcher took {elapsed:?}"
        );
    }
}
