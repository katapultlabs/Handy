# Dictionary Design

This document is written in Simplified Technical English (STE).
It describes the design of the Dictionary feature.
It contains the design and future work. The MVP is implemented.

Current test build (0.9.6+dict.10): entries live in SQLite. Clear matches to
explicitly taught vocabulary can activate automatically with a small context
guard and Undo. Uncertain pairs remain inactive suggestions. Repeated edits
never override a user decision. Grammar changes and rewrites are filtered.
Manual additions and explicit approvals apply globally. The compiled matcher
is rebuilt after mutations and shared by the paste path. The broader features
below (such as vocabulary migration, CSV transfer, and prompt integration)
remain future work unless described in `docs/TEST_BUILD.md`.

Read `docs/ARCHITECTURE.md` first. It shows where the current code is.

## 1. Purpose

Speech-to-text models make the same mistakes many times.
Examples: names, product names, technical words, and words from other languages.

The Dictionary gives Handy a memory of the user's corrections.
Handy applies the corrections to each new transcription.
The user does not have to make the same correction again.

## 2. Goals

- One central store of corrections. All sources write to it. One consumer reads from it.
- Three ways to add a correction:
  1. The user edits the pasted text in the target application. Handy captures the edit.
  2. The user edits a saved transcription in the History screen.
  3. The user adds or edits an entry in the Dictionary screen.
- The user can see, disable, and delete each entry.
- Corrections apply on the local machine. No network call is necessary.
- The current Custom Words feature continues to work.
- The feature is off by default. It is an experimental feature. See section 10.1.

## 3. Non-goals

- Handy does not retain full target-application text. Capture reads a bounded window around the pasted text (section 7.3.1), compares it, and drops it. Only correction pairs and up to four context words are kept locally. Where the platform only offers a full-value read, Handy truncates to the window at once and never stores or sends the rest.
- Handy does not send the target application content to a server.
- Handy does not change text that the user did not dictate.
- Per-application dictionaries are not part of the first version. The data model permits them later.

## 4. Concepts

### 4.1 Entry

An entry is one correction. It has two parts:

- `wrong`: the text the model produced. This part is optional.
- `right`: the text the user wants.

An entry with `wrong` and `right` is a **replacement**. Example: `Cortex` -> `Kortix`.
An entry with only `right` is a **vocabulary word**. Example: `Kortix`.
A vocabulary word is the same as a Custom Word today.

### 4.2 Source

Each entry records where it came from:

- `manual`: the user typed it in the Dictionary screen.
- `history`: Handy learned it from an edit in the History screen.
- `capture`: Handy learned it from an edit in the target application.

### 4.3 Producers and consumer

A producer makes entries. There are three producers. See section 7.
The consumer applies entries to a transcription. There is one consumer. See section 6.

All producers use one function: `learn(original, corrected, source)`.
This function finds the differences and makes entries.
The Dictionary screen writes entries directly. It does not need `learn()`.

## 5. Data model

The entries live in their own SQLite file, `dictionary.db`, next to
`history.db`. The migrations are in `dictionary_db.rs` `MIGRATIONS`.
`rusqlite_migration` applies them.

Do not add a migration to `managers/history.rs`. `history.db` must keep
upstream's schema version. Upstream Handy stops at start when the version is
higher than its own migration count (`DatabaseTooFarAhead`). A person who
installs upstream Handy over a fork build must get a working app without the
Dictionary.

Fork builds up to 0.9.8-katapult.1 kept the table in `history.db` as
migrations 5 to 7. At start, `dictionary_db::move_out_of_history` copies those
rows to `dictionary.db`, drops the table, and sets `history.db` back to
version 4. It runs once, before the History migrations.

```sql
CREATE TABLE dictionary (
  id            INTEGER PRIMARY KEY,
  wrong         TEXT,                          -- NULL for a vocabulary word
  right         TEXT NOT NULL,
  match_mode    TEXT NOT NULL DEFAULT 'word',  -- 'word' or 'phrase'
  case_mode     TEXT NOT NULL DEFAULT 'smart', -- 'smart' or 'exact'
  source        TEXT NOT NULL,                 -- 'manual', 'history', 'capture'
  state         TEXT NOT NULL DEFAULT 'active',-- 'proposed', 'active', 'rejected'
  enabled       INTEGER NOT NULL DEFAULT 1,
  auto_learned  INTEGER NOT NULL DEFAULT 0,    -- active by classifier, not explicit approval
  context_words TEXT NOT NULL DEFAULT '[]', -- JSON: up to four lowercase context words
  seen_count    INTEGER NOT NULL DEFAULT 1,    -- times a producer proposed this pair
  applied_count INTEGER NOT NULL DEFAULT 0,    -- times the consumer used it
  app_id        TEXT NOT NULL DEFAULT '',      -- '' = all applications (reserved)
  wrong_key     TEXT NOT NULL,                 -- normalized: lowercase wrong, '' for vocabulary
  right_key     TEXT NOT NULL,                 -- normalized: lowercase right
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);
CREATE UNIQUE INDEX dictionary_pair ON dictionary (wrong_key, right_key, app_id);
CREATE UNIQUE INDEX dictionary_active_wrong ON dictionary (wrong_key, app_id)
  WHERE state = 'active' AND wrong_key != '';
```

Do not use `NULL` in a unique key. SQLite treats each `NULL` as distinct, so the
`UPSERT` never conflicts and `seen_count` never increases. Use `''` sentinels
and the normalized `*_key` columns instead. `ON CONFLICT` targets `dictionary_pair`.

The `dictionary_active_wrong` index enforces one active replacement per `wrong`.
When a new pair for an existing active `wrong` reaches activation, the old entry
moves to `state = 'proposed'` and the new one becomes active, in one transaction.
Only explicit approval or a manual change can displace an active rule. An
automatic candidate with the same wrong text remains proposed.

Rules:

- `wrong` and `right` are stored as the user wrote them. Matching normalizes them. Storage does not.
- `match_mode = 'word'` matches on word boundaries. `'phrase'` matches a run of words.
- `case_mode = 'smart'` keeps the case pattern of the matched text. `'exact'` writes `right` as stored.
- A producer that proposes a pair that exists increases `seen_count`. It does not make a second row. Use one SQL `UPSERT` in one transaction.
- A new learned pair can activate only under the strict tests in section 8.3. For an existing pair, learning only increases `seen_count` and preserves state, enabled flag, and context guard.
- Explicit approval (Always replace) clears automatic metadata and makes the pair globally applicable. A new suggestion or automatic candidate does not demote an active replacement.
- A user confirmation sets `state = 'active'` directly, at any count.
- `state = 'rejected'` means the user dismissed the pair. The `UPSERT` still increases `seen_count`, but it never changes `rejected` to another state. Only the user can, in the Dictionary screen.
- Only `state = 'active'` and `enabled = 1` entries apply.

### 5.1 Migration of Custom Words

On first start after the update:

1. Read `settings.custom_words`.
2. Insert each word as a vocabulary entry with `source = 'manual'`.

After the migration, **the dictionary table is the single owner** of vocabulary.
`settings.custom_words` becomes a derived copy: the active, enabled vocabulary entries.

- Every dictionary mutation (add, edit, delete, disable, reject) rewrites `settings.custom_words` from the table, in the same operation.
- The legacy `update_custom_words` command writes through the dictionary, not to the setting directly.
- No other code writes `settings.custom_words`.

This rule exists because the Whisper `initial_prompt` path reads `settings.custom_words`
directly (`managers/transcription.rs`). Without one owner, a word disabled in the
Dictionary would still reach the model through the prompt.
Do not remove `settings.custom_words` in the first version.

## 6. Consumer: apply the Dictionary

### 6.1 Position in the pipeline

Apply the Dictionary in `post_process_transcription_text()` in `managers/transcription.rs`.
This is where `apply_custom_words()` runs today.
This is before the LLM post-process step. The LLM then sees corrected text.

Also give the LLM the vocabulary list.
Add a `${dictionary}` placeholder for post-process prompts.
This is the same idea as upstream PR #1711.

### 6.2 Two tiers

Tier 1: **Exact replacements.** Apply all enabled entries that have `wrong`.

1. Build one matcher from all active replacement entries. Cache it. Rebuild it when the table changes. Do not compile a regex for each entry on each transcription.
2. Add a word boundary at the start of `wrong` only if its first character is a letter, a digit, or `_`. Do the same at the end. Then `C++`, `.NET`, and `@handle` match. CJK characters count as letters, so a CJK `wrong` inside a CJK run does not match. For CJK entries use `match_mode = 'phrase'`, which adds no boundaries.
3. Try the longest `wrong` first. This prevents a short entry from breaking a long one.
4. Do not match inside a span that an earlier replacement produced. Make one pass. Do not cascade.
5. Ignore case when you match, if `case_mode = 'smart'`. Copy the case pattern of the matched text onto `right`. Use the same function as `apply_custom_words()` (`preserve_case_pattern`).
6. Insert `right` as literal text. `$` and `\` in `right` must not expand. In Rust, use `regex::NoExpand` or build the output by hand.
7. Keep the punctuation that touches the matched text.
8. If `right` is empty, remove the match and the one space next to it. Do not run a whitespace cleanup on the whole text. That removes line breaks.

Tier 2: **Fuzzy vocabulary.** The same algorithm as today's `apply_custom_words()`, on the vocabulary entries.
Run it after Tier 1. Do not run it on spans that Tier 1 changed.

The current function cannot do this as it is. It takes no span input. It also
rebuilds the text with `split_whitespace()` and `join(" ")`, which removes line
breaks and repeated spaces — an existing defect that this work fixes. Build one
span-aware pass: tokenize once with byte offsets, let Tier 1 mark its output
spans as protected, run the fuzzy pass over the unprotected spans, and copy the
original whitespace through unchanged.

Tier 1 is deterministic. Tier 2 is not. The user can turn Tier 2 off.

### 6.3 Performance of the consumer

A transcription has fewer than 1,000 words in most cases.
The dictionary has fewer than 1,000 entries in most cases.
A single-pass Aho-Corasick or regex-set matcher is fast enough.
Do not call the database for each transcription. Read the table once into memory. Refresh on change.
The implemented matcher uses Aho-Corasick, preserves longest-first overlap
priority, checks context on original text, and does not cascade replacements.
Transcripts above 128 KiB or more than 16,384 raw matches skip this pass to
bound work on pathological inputs.
The budgets are in section 16.

## 7. Producers

### 7.1 Manual (Dictionary screen)

A settings screen named "Dictionary" replaces the "Custom Words" section.

- A table with columns: Wrong, Right, Source, Enabled, Times used.
- Add an entry. Leave "Wrong" empty for a vocabulary word.
- Edit an entry in place.
- Delete an entry.
- Filter by source.
- Import and export as CSV. Two columns: `wrong,right`.

All strings go through i18n.

### 7.2 History edit

The History screen shows the saved transcriptions.
Add an "Edit" action to each entry.

1. The user opens the entry. The text is editable.
2. The user saves.
3. Handy stores the edited text in a new column `user_edited_text`.
4. Handy calls `learn(pasted_text, user_edited_text, "history")`.
5. Handy activates only new pairs that pass section 8.3, with Undo. It shows other accepted pairs as **proposed**. Dismiss sets `state = 'rejected'`. See section 9.

`pasted_text` is the text Handy wrote into the target application. Store it in a new column `pasted_text` at paste time.
Do not diff against `transcription_text`. That text is from before the Dictionary ran. A diff against it learns the same corrections again and inflates `seen_count`.

### 7.3 In-place capture (target application)

This producer watches the text after Handy pastes it.
It is the most valuable producer. It is also the most difficult.
Build it last. Build it for macOS first.

#### 7.3.1 How it works on macOS

macOS gives an Accessibility API (AX). Handy already has the Accessibility permission.
The `objc2-application-services` crate gives the bindings.

Just before the paste (the **snapshot**):

1. Get the focused element: `AXUIElementCreateSystemWide()` -> `kAXFocusedUIElementAttribute`.
2. Read `kAXSelectedTextRangeAttribute` (the caret and any selection the paste will replace).
3. Store an **anchor**: the element reference, the process id, the caret position, the selection length, the pasted text, and the time.
4. The snapshot runs on the AX thread with a 100 ms budget (section 16.3). The paste does not wait past that budget: on timeout, paste without an anchor and skip capture for this dictation. The snapshot never reads the field content.
5. Do not store field text on disk. Anchor data stays in memory only.

At check time (implemented in dict.11):

1. Read a bounded window around the anchor. Prefer `AXStringForRange`. If the application does not support it, read `AXValue` and immediately retain only the bounded window around the anchor.
2. Align the edited paste conservatively at the caret-derived offset. An unchanged suffix or a bounded prefix-distance search identifies its end. Ambiguous alignment is skipped.
3. Coalesce value changes for 400 ms. An aligned edited span waits another 200 ms before learning. A new event cancels that confirmation until the next settled read.
4. Retain the last safely aligned span in memory so clearing the field can finalize an already-observed edit. A clear before any safe read cannot be recovered. Reverted text clears the candidate.
5. Run the shared classifier and transaction on the capture thread. A new or already-known correction finishes the anchor. An edit with no learnable pairs leaves the anchor watching. Expiry, cancellation, or an inaccessible element drops it.

Check triggers:

- A per-element `AXObserver` receives `AXValueChanged` on the dedicated capture thread. Its callback only increments a counter; it never reads field text.
- Until a value notification proves the observer works, a 750 ms active-anchor fallback checks a bounded window. The first notification disables repeated fallback reads.
- The next dictation requests a final check after taking ownership of the overlay. The request is asynchronous and adds no AX wait to recording startup.
- An anchor expires after 180 seconds. With no anchor, the thread blocks with no polling. Active observers pump their run loop in slices of at most 50 ms so channel messages remain responsive.

#### 7.3.2 When capture cannot work

- The focused element is a password field (AX role or subrole
  `AXSecureTextField`). Skip capture. Do not use the global secure-input
  state from `secure_input.rs` for this: another process (loginwindow, a
  chat application) can hold secure input for hours and would block every
  capture.
- The element does not give `kAXValueAttribute`. Some applications do not. Skip capture.
- The paste method is `external_script`. Handy does not know where the text went. Skip capture.
- The pasted text is not found near the anchor. The user deleted it or moved it. Skip capture.

Capture must fail silently. It must never block the paste.

#### 7.3.3 Windows and Linux

Windows has UI Automation (`windows` crate, `UIAutomationClient`). The design is the same.
Linux has AT-SPI2. Support differs between toolkits and Wayland compositors.
Both are later work. The interface for a "text anchor provider" must let each platform plug in.

## 8. Learn: from an edit to entries

`learn(original, corrected, source)` runs the same steps for all producers.

1. Tokenize both strings into words. Keep punctuation as separate tokens.
2. Compute a word-level diff. The `similar` crate gives this.
3. Walk the diff. Each change is a pair of runs: removed words and inserted words.
4. Accept a change as a candidate when all of these are true:
   - The removed run and the inserted run each have 1 to 3 words. One word on each side is the normal case. Two or three words cover a name that the model split or joined, for example `char gebee` -> `ChargeBee`.
   - The runs are not identical. A case-only change (`github` -> `GitHub`) is a valid candidate: it skips the two similarity tests below, and step 6 stores it with `case_mode = 'exact'`. Runs that are identical including case are not candidates.
   - The runs are not only punctuation or only whitespace.
   - The inserted run is not empty and the removed run is not empty.
   - The runs **sound similar**. See 8.1. This is the main test. A misheard word sounds like the right word. A rewrite does not.
   - The runs **look similar**. The character edit distance is below a limit. Default: 60 percent of the longer run.
5. Make a `replacement` entry from each candidate: `wrong = removed`, `right = inserted`.
6. Set `case_mode` for the new entry:
   - If the two sides differ only in spelling, use `'smart'`.
   - If the case of `right` differs from the case of the matched `wrong` (example: `Maine` -> `main`, or `github` -> `GitHub`), use `'exact'`. The case is part of the correction. Smart case would undo it.
7. If the pair exists, increase `seen_count`.

Do not accept a change that only adds or removes words. That is an edit, not a correction.
Do not use an LLM in this step. The rules are enough for most cases and they are predictable.
An optional LLM step can come later for the rejected candidates.

### 8.1 Sound similarity

The purpose of the Dictionary is to fix words the model **misheard**.
A misheard word and the right word have similar pronunciation. Other edits do not.
This test separates a correction from a rewrite.

1. Normalize both runs: lowercase, remove punctuation, join words with no space. `char gebee` becomes `chargebee`.
2. Compute a phonetic key for each side. Use Double Metaphone. The `rphonetic` crate (a port of Apache commons-codec) gives it. Soundex from the `natural` crate, which `apply_custom_words()` uses today, is too coarse: it keeps only the first letter and three digits, so `Klein` and `Cline` do not match. Double Metaphone handles names from other languages better.
3. Accept when the keys are equal, or when the edit distance between the keys is 1.
4. If a side has characters that the phonetic algorithm does not support (for example CJK), skip this test. Use only the "look similar" test.

Examples:

| Removed       | Inserted    | Sounds similar | Result               |
| ------------- | ----------- | -------------- | -------------------- |
| `Cortex`      | `Kortix`    | yes            | accept               |
| `Klein`       | `Cline`     | yes            | accept               |
| `char gebee`  | `ChargeBee` | yes            | accept               |
| `the meeting` | `our sync`  | no             | reject (rewrite)     |
| `good`        | `great`     | no             | reject (style edit)  |
| `their`       | `there`     | yes            | accept, but see note |

Note: homophone fixes such as `their` -> `there` depend on context and are risky as global replacements. The MVP rejects common-word grammar edits. It requires explicit approval for uncertain pairs and uses a local context guard for automatic pairs. See sections 5 and 8.3.

### 8.2 Tests for `learn()`

Write unit tests for each row of the table above.
Add tests for: a paragraph the user rewrote (no entries), a single typo fix, a name split in two, a change in the middle of a long text, CJK text, an empty edit.

### 8.3 Conservative automatic learning (implemented)

A candidate can activate automatically only when all of these checks pass:

- The complete target is a Custom Word or the target of an enabled, explicitly
  approved/manual correction. Automatic rules never supply new trusted terms.
- Both normalized sides have at least four ASCII letters. Character distance
  is at most 25 percent and Double Metaphone keys match exactly.
- At least three quarters of the edit's tokens stay unchanged in order.
- Each pair side occurs exactly once in its respective text.
- One to four useful, unchanged context words occur within four tokens at the
  same relative position in both texts. Only alphabetic words of at least four
  characters qualify. Stop words and pair words are excluded.
- The pair is new, and no active rule already owns that wrong text.

The runtime matcher applies such a rule only if at least one saved context
word occurs within four tokens. This is a conservative lexical check, not a
semantic understanding of the sentence. It can miss valid corrections; it
cannot guarantee that every nearby homophone has the intended meaning.

Inputs are capped at 32 KiB, 2,048 tokens, and 128 characters per token. The
classifier trims equal prefix/suffix tokens before its bounded LCS check and
falls back to suggestions when the residual comparison exceeds its budget.
It returns at most 16 pairs (capture stores at most 3). Classification, writes,
and matcher compilation run off the paste path. No model call is made.

## 9. Trust and confirmation

- New automatic pairs show a brief "Learned" notice with Undo. They appear in
  Your corrections with an Automatic marker; no approval step is needed.
- Other accepted pairs start as **proposed** and show "Suggested". They apply
  only after the user chooses Always replace.
- Undo and Ignore both persist a rejection. They stop future application and
  suppress repeated suggestions. Undo does not rewrite text already pasted.
- Repeated edits do not establish intent. `seen_count` is informational; it
  never promotes a suggestion or changes a disabled/ignored rule.
- Editing an active rule's text makes it explicit and clears the automatic
  guard. Toggling its enabled checkbox preserves the guard.
- The Dictionary screen separates Suggested, Your corrections, and a collapsed
  Ignored group. Ignored pairs can be explicitly approved later.
- Dictionary migration 2 (history.db migration 6 in older builds) moves older
  automatically active learned entries to Suggested once. Dictionary migration 3
  (history.db migration 7) adds automatic metadata without changing existing
  choices.
  No stored pairs are deleted.

### 9.1 Correction notices (dict.11)

`correction_notices.rs` owns one FIFO for newly committed capture and History
pairs. Native show/hide, queue mutations, and recording ownership serialize on
the main thread; classification, SQLite, and matcher rebuilds stay off it.
Previously known pairs and grammar edits do not create repeated notices.

Each card shows one complete pair. Proposed entries offer **Always replace**
and **Ignore**. Automatic entries offer **Undo**. **Next** or close dismisses
only the card; it does not make a dictionary decision. Stored entries remain
available in Settings. This transient queue lasts for the current process.

The 8-second clock starts after the frontend acknowledges rendering that
notice token. Hover, focus, and saving pause it. Recording pauses the card and
resumes its remaining time with a new token. New batches append in order.
Token/deadline checks prevent old timers or clicks from affecting another
card. A failed save retains the card and shows a localized retry error.

The overlay subscribes before fetching the current snapshot and rejects older
revisions, recovering events missed while the webview initialized. Successful
History edits use the same queue instead of a second toast. Recording visuals
can be disabled while correction notices still appear. Disabling Dictionary
or Experimental clears transient notices. Debug logs contain token/count/timing
metadata only, never correction text. System notifications are not used.

## 10. Settings

### 10.1 Placement

The Dictionary is an **experimental feature**.
Handy has a switch `experimental_enabled` in Advanced settings.
When it is on, `AdvancedSettings.tsx` shows an "Experimental" group.

- The master switch `dictionary_enabled` goes in the Experimental group. Default: off.
- When `dictionary_enabled` is off, Handy behaves as it does today. Custom Words work as before. No capture. No learning.
- When `dictionary_enabled` is on, a "Dictionary" screen appears in the sidebar. The other switches below live on that screen.
- The Custom Words section stays where it is while the Dictionary is off. When the Dictionary is on, the Custom Words section shows a link to the Dictionary screen.

### 10.2 Fields

Add these fields to `AppSettings`:

| Field                            | Type | Default | Function                                                   |
| -------------------------------- | ---- | ------- | ---------------------------------------------------------- |
| `dictionary_enabled`             | bool | false   | Master switch. Experimental group.                         |
| `dictionary_fuzzy_enabled`       | bool | true    | Tier 2 on or off.                                          |
| `dictionary_learn_from_history`  | bool | true    | Producer 7.2 on or off.                                    |
| `dictionary_learn_from_capture`  | bool | false   | Producer 7.3 on or off. Off by default until it is proven. |
| `dictionary_capture_window_secs` | u32  | 180     | How long an anchor lives.                                  |

Each field needs a `change_*_setting` command. See `docs/ARCHITECTURE.md` section 6.1.

## 11. Commands and events

Commands (`commands/dictionary.rs`):

- `list_dictionary_entries(filter) -> Vec<DictionaryEntry>`
- `add_dictionary_entry(wrong, right, options) -> DictionaryEntry`
- `update_dictionary_entry(id, patch) -> DictionaryEntry`
- `delete_dictionary_entry(id)`
- `confirm_dictionary_entry(id)` — sets `state = 'active'`
- `reject_dictionary_entry(id)` — sets `state = 'rejected'`
- `import_dictionary_csv(path) -> ImportReport`
- `export_dictionary_csv(path)`
- `update_history_entry_text(id, text) -> LearnReport`

Events:

- `DictionaryUpdated`. The frontend refreshes the table.
- `DictionaryLearned { entries }`. The frontend shows the notification.

## 12. Privacy

- All data stays on the local machine, with one exception, below.
- Capture reads only the focused element. It keeps the text in memory for the anchor lifetime. It writes only the learned pairs and, for automatic rules, up to four context words.
- Capture is off by default.
- The exception: if the user puts `${dictionary}` in a post-process prompt, the dictionary words go to the configured LLM provider with each post-processed transcription. That provider can be remote. The placeholder documentation and the prompt editor must say this. A test must show the dictionary is sent only when the prompt contains the placeholder.

## 13. Build order

Each phase is one pull request. Each phase works on its own.

1. **Store and consumer.** Table, migration, `DictionaryManager`, Tier 1 matcher, Tier 2 reuse, settings, commands. Migrate Custom Words. Unit tests for the matcher.
2. **Dictionary screen.** Replace the Custom Words UI. Import and export.
3. **Learn and History edit.** `learn()`, `user_edited_text` column, History edit UI, confirmation flow, notification.
4. **In-place capture, macOS.** Anchor provider, check triggers, secure-input guard. Behind `dictionary_learn_from_capture`.
5. **Prompt placeholder.** `${dictionary}` in post-process prompts.
6. **Windows capture.** UI Automation provider.

## 14. Open questions

- Should a `capture` entry from one application apply in all applications? First version: yes. `app_id` is reserved for later.
- (Decided) Handy learns case-only changes, for example `github` -> `GitHub`, with `case_mode = 'exact'`. See section 8, step 4.
- How does the anchor behave when the paste method is `direct` typing? The text arrives one character at a time. The anchor must wait for the typing to end.
- Does the Whisper `initial_prompt` path need the replacement entries, or only the vocabulary entries? Proposal: only vocabulary. The prompt tells the model what words exist. It cannot tell it what to replace.

## 15. Lessons from prior art

We reviewed two upstream pull requests. We do not reuse their code. We keep the good ideas. We avoid the mistakes.

### 15.1 PR #1533, "Word Replacements" (open)

Keep:

- `regex::NoExpand` for the replacement text. A `$` in the target is literal. It has a test.
- The boundary rule: add `\b` only when the edge character is alphanumeric or `_`. `C++` and `.NET` then match.
- Twelve unit tests: multi-word source, empty target deletes, punctuation kept, blank source skipped.
- `#[serde(default)]` on the new settings field. Old stores load.

Avoid:

- It runs a whitespace cleanup (`\s{2,}` -> space, then `trim()`) on the whole transcript after any rule fires. This removes paragraph breaks and fights `append_trailing_space`.
- It matches case-insensitively but does not keep the case of the matched text. `apply_custom_words()` does keep it. The two features then behave differently.
- It compiles one regex per rule on each transcription. No cache.
- Rules cascade. Rule N sees the output of rule N-1. This is hard to reason about.
- The frontend removes rules with a case-sensitive compare but adds them with a case-insensitive compare. React keys collide.
- The settings model has no id, no enable flag, and no scope.

### 15.2 PR #1369, "Self-learning corrections via LLM" (closed)

Keep:

- A SQLite table with `rusqlite_migration`. The fork first used `history.db`
  and its `MIGRATIONS` array; it now uses `dictionary.db` (section 5).
- `app.try_state::<Arc<Manager>>()` in the pipeline. The pipeline degrades if the manager is absent.
- `HistoryUpdatePayload::Updated` emitted after a history edit.
- JSON-schema structured output when the provider supports it. Reasoning effort forced to `none` for the extraction call.
- The intent of its prompt: "extract only word-level corrections that fix recognition errors; ignore punctuation, capitalization, style, added or removed words."

Avoid:

- It passes `&str` to `Regex::replace_all`. `$1` in a learned target then expands. Output is corrupted.
- Its fallback diff pairs words by index. One inserted word shifts the tail and creates many junk rules. No cap, no distance check.
- It writes to the database before the "Confirm / Dismiss" panel. Dismiss does nothing. The panel is not real.
- The non-structured LLM path does not strip code fences. `serde_json::from_str` fails almost always. It then falls back to the junk diff, silently.
- It sends the full transcript and the edit to the LLM provider on each history edit. No toggle. No consent. The prompt has no delimiters, so dictated text can steer the extraction.
- It diffs the edit against the raw transcription, not the pasted text. Already-applied corrections are learned again.
- It applies corrections late, in `actions.rs`, after OpenCC and after fuzzy custom words. It marks the result as `post_processed_text` when no post-process ran. History then shows wrong data.
- `SELECT` then `INSERT`/`UPDATE` with no transaction. New connection per call. All rows loaded on each transcription.
- Unrelated regression: it removed the settings write in `update_recording_retention_period`.
- i18n: v3 plural suffix (`_plural`) in an i18next v4 project. One key used for two purposes.

### 15.3 What this design does differently

- One pass, longest-first, no cascade (section 6.2).
- Literal insertion, case kept, boundaries only on alphanumeric edges, no global whitespace cleanup (section 6.2).
- Proposed state is in the database. Dismiss is real (sections 5 and 9).
- Diff against `pasted_text` (section 7.2).
- Rules-based `learn()` with a word-level diff library, size and distance limits, no LLM (section 8).
- Apply early, in `managers/transcription.rs`, in the same place as custom words (section 6.1).

## 16. Performance

Performance is a design principle for this feature, not an afterthought.
The rule: **the Dictionary must not make dictation feel slower, ever.**

### 16.1 The hot path

The hot path is the time between the end of transcription and the paste.
The user waits during this time. Every millisecond counts.

Only one Dictionary step runs on the hot path: the consumer (section 6).

Budgets, measured on the oldest supported hardware, not on a fast machine:

| Step                                              | Budget                                               |
| ------------------------------------------------- | ---------------------------------------------------- |
| Tier 1 exact matcher, 1,000 words x 1,000 entries | < 1 ms                                               |
| Tier 2 fuzzy (already exists today)               | no regression against current `apply_custom_words()` |
| Total added to the pipeline                       | < 2 ms                                               |

Enforce the budgets with benchmarks (`cargo bench` or a timed unit test).
A pull request that breaks a budget does not merge.

### 16.2 Off the hot path

Everything else runs off the hot path, on a background task:

- `learn()` — the diff, the phonetic keys, the database write. The paste never waits for learning.
- Matcher rebuild — rebuild in the background after a table change. The pipeline uses the old matcher until the new one is ready. Swap with an `ArcSwap` or a lock held only for the pointer swap.
- Database writes — one UPSERT transaction, on the background task. `applied_count` updates are batched; they are statistics, not state the pipeline reads.
- The `DictionaryUpdated` event and all UI refreshes.

### 16.3 Capture and the AX API

AX calls can block. An unresponsive target application can hold a call for seconds.

- All AX calls run on a dedicated thread, never on the pipeline thread and never on the main thread.
- Every AX call has a timeout: `AXUIElementSetMessagingTimeout`, 100 ms. A timeout means "skip capture", nothing more.
- The anchor snapshot (section 7.3.1) runs just before the paste, on the AX thread, inside one 100 ms budget. On timeout the paste proceeds without an anchor. The snapshot reads only the caret range, never the field content, so it is one cheap AX call.
- Check triggers (section 7.3.1) coalesce. A focus change during a running check does not start a second check.

### 16.4 Memory

This project trims memory after each dictation (`memory.rs`, `FinishGuard`). The Dictionary follows the same discipline.

- An anchor holds at most 32 KB of field text. A larger field stores only the 32 KB window around the caret.
- At most 4 anchors live at one time. A new anchor beyond that drops the oldest.
- Anchors drop at the end of the capture window (default 180 s) and on application quit.
- The in-memory dictionary table is small (< 1,000 entries, ~100 KB). Hold it as one `Arc`, not one copy per thread.

### 16.5 Startup

- Keep startup work small. `DictionaryManager::new` applies the `dictionary.db` migrations and reads the entries once. The one-time move out of `history.db` runs inside `HistoryManager::new`.
- Build the first matcher lazily, on the first transcription, not at startup.

### 16.6 What to measure before merge

Each phase (section 13) ships with numbers in the pull request:

1. Pipeline time with the feature off (baseline) and on, same audio, 10 runs, report the median.
2. Matcher build time at 100 / 1,000 / 10,000 entries.
3. `learn()` time on a 1,000-word edit.
4. For capture: paste-to-anchor time, and check time against a responsive and an unresponsive application.

### 16.7 Local verification for dict.10 (2026-09-16)

Release-mode measurements on the development Apple Silicon Mac, 51 samples
per case, one test thread, 1,000-word input with contextual automatic matches:

| Dictionary rules | Build snapshot | Apply median | Apply p95 |
| ---------------- | -------------- | ------------ | --------- |
| 100              | 0.110 ms       | 0.068 ms     | 0.108 ms  |
| 1,000            | 0.799 ms       | 0.095 ms     | 0.102 ms  |
| 10,000           | 20.308 ms      | 0.077 ms     | 0.093 ms  |

The bounded classifier on a 1,000-word edit measured 0.259 ms median and
0.339 ms p95. Learning, SQLite writes, and mutation-triggered matcher rebuilds
run off the paste path. Only the immutable snapshot is shared with dictation.
The initial snapshot is still loaded during DictionaryManager initialization;
lazy startup from section 16.5 remains future work.

These are local microbenchmarks. They do not establish the budget on the
oldest supported hardware or measure the full audio-to-paste path, existing
Custom Words fuzzy matching, or target-application Accessibility latency.

Reproduce the focused tests and measurements from `src-tauri`:

```bash
CMAKE_POLICY_VERSION_MINIMUM=3.5 cargo test --release --lib dictionary -- --include-ignored --nocapture --test-threads=1
```

The Dictionary UI tests use the real React component with mocked Tauri IPC.
They cover approval, persistent rejection, editing without approval, manual
additions, automatic Undo, and settings-window layout. Live dictation and
capture in third-party applications still need hands-on testing of this build.

### 16.8 Notice and capture verification for dict.11

Pure Rust tests exercise FIFO bursts, delayed render acknowledgement, stale
timers and actions, recording interruption, hover/action pauses, feature-off
cleanup, and candidate timing across rapid edits, clear, revert, and fallback.
Browser tests render the real overlay with mocked Tauri IPC to exercise the
controls, replay, revisions, and fixed-size native window layout. Local validation
passed 370 regular Rust tests plus the binding export check, and all 13
Dictionary/overlay browser tests. Frontend build, lint, formatting, and 24 locale
translation checks passed. Clippy completed with existing warnings in model
and transcription code. The two opt-in performance tests from dict.10 were not
rerun because their implementation did not change. These checks
do not replace hands-on capture testing in each target application. The
matcher and classifier measured in section 16.7 are unchanged by this build.
