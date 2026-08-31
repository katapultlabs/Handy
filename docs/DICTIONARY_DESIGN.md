# Dictionary Design

This document is written in Simplified Technical English (STE).
It describes the design of the Dictionary feature.
It is a plan. The code does not exist yet.

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

- Handy does not read the full content of the target application.
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

The entries live in the SQLite database that History uses.
Use a new migration in `managers/history.rs` `MIGRATIONS`.
`rusqlite_migration` applies it.

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
  seen_count    INTEGER NOT NULL DEFAULT 1,    -- times a producer proposed this pair
  applied_count INTEGER NOT NULL DEFAULT 0,    -- times the consumer used it
  app_id        TEXT,                          -- NULL = all applications (reserved)
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL,
  UNIQUE (wrong, right, app_id)
);
```

Rules:

- `wrong` and `right` are stored as the user wrote them. Matching normalizes them. Storage does not.
- `match_mode = 'word'` matches on word boundaries. `'phrase'` matches a run of words.
- `case_mode = 'smart'` keeps the case pattern of the matched text. `'exact'` writes `right` as stored.
- A producer that proposes a pair that exists increases `seen_count`. It does not make a second row. Use one SQL `UPSERT` in one transaction.
- `state = 'rejected'` means the user dismissed the pair. Producers do not propose it again. The consumer does not apply it.
- Only `state = 'active'` and `enabled = 1` entries apply.

### 5.1 Migration of Custom Words

On first start after the update:

1. Read `settings.custom_words`.
2. Insert each word as a vocabulary entry with `source = 'manual'`.
3. Keep `settings.custom_words` in sync with the vocabulary entries. Old code paths continue to work.

Do not remove `settings.custom_words` in the first version.
The Whisper `initial_prompt` path reads it. See `managers/transcription.rs`.

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

Tier 2: **Fuzzy vocabulary.** Run the current `apply_custom_words()` on the vocabulary entries.
Run it after Tier 1. Do not run it on spans that Tier 1 changed.

Tier 1 is deterministic. Tier 2 is not. The user can turn Tier 2 off.

### 6.3 Performance of the consumer

A transcription has fewer than 1,000 words in most cases.
The dictionary has fewer than 1,000 entries in most cases.
A single-pass Aho-Corasick or regex-set matcher is fast enough.
Do not call the database for each transcription. Read the table once into memory. Refresh on change.
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
5. Handy shows the entries it learned as **proposed**. The user confirms or dismisses each one. Dismiss sets `state = 'rejected'`. Nothing becomes active without this step or the `seen_count` rule in section 9.

`pasted_text` is the text Handy wrote into the target application. Store it in a new column `pasted_text` at paste time.
Do not diff against `transcription_text`. That text is from before the Dictionary ran. A diff against it learns the same corrections again and inflates `seen_count`.

### 7.3 In-place capture (target application)

This producer watches the text after Handy pastes it.
It is the most valuable producer. It is also the most difficult.
Build it last. Build it for macOS first.

#### 7.3.1 How it works on macOS

macOS gives an Accessibility API (AX). Handy already has the Accessibility permission.
The `objc2-application-services` crate gives the bindings.

At paste time:

1. Get the focused element: `AXUIElementCreateSystemWide()` -> `kAXFocusedUIElementAttribute`.
2. Read `kAXValueAttribute` (the full text) and `kAXSelectedTextRangeAttribute` (the caret).
3. Store an **anchor**: the element reference, the process id, the caret position before the paste, the pasted text, and the time.
4. Do not store the full field text on disk. Keep it in memory only.

At check time:

1. Read `kAXValueAttribute` from the anchored element again.
2. Find the pasted text near the anchor position. Use a fuzzy locator, because the user can type before the anchor.
3. Compare the text at that location with the pasted text.
4. If the text changed, call `learn(pasted, current, "capture")`.
5. Drop the anchor.

Check triggers (any one of them):

- The user presses the transcribe shortcut again. Check before the new recording starts.
- The focused application changes. Watch `NSWorkspace.didActivateApplicationNotification`.
- A timer expires. Default: 20 seconds after the last check. Stop after 3 minutes.

#### 7.3.2 When capture cannot work

- Secure input is active. `secure_input.rs` detects this. Skip capture.
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
   - The runs are not the same after case folding.
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

Note: homophone fixes such as `their` -> `there` depend on context. They pass this test but they are risky as global replacements. Keep them as `proposed` until `seen_count >= 3`. Setting `dictionary_auto_apply_threshold` controls this. A future version can mark an entry as "context-dependent" and give it to the LLM prompt only.

### 8.2 Tests for `learn()`

Write unit tests for each row of the table above.
Add tests for: a paragraph the user rewrote (no entries), a single typo fix, a name split in two, a change in the middle of a long text, CJK text, an empty edit.

## 9. Trust and confirmation

Auto-learned entries can be wrong. The user must stay in control.

- `history` and `capture` entries start as **proposed**. They apply after `seen_count >= 2`, or after the user confirms them once.
- Setting: `dictionary_auto_apply_threshold`. Default: 2. Value 1 means "apply at once".
- When Handy learns an entry, show a small notification: "Learned: Cortex -> Kortix. Undo".
- The Dictionary screen shows proposed entries in a separate group.

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

| Field                             | Type | Default | Function                                                   |
| --------------------------------- | ---- | ------- | ---------------------------------------------------------- |
| `dictionary_enabled`              | bool | false   | Master switch. Experimental group.                         |
| `dictionary_fuzzy_enabled`        | bool | true    | Tier 2 on or off.                                          |
| `dictionary_learn_from_history`   | bool | true    | Producer 7.2 on or off.                                    |
| `dictionary_learn_from_capture`   | bool | false   | Producer 7.3 on or off. Off by default until it is proven. |
| `dictionary_auto_apply_threshold` | u32  | 2       | See section 9.                                             |
| `dictionary_capture_window_secs`  | u32  | 180     | How long an anchor lives.                                  |

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

- All data stays on the local machine.
- Capture reads only the focused element. It keeps the text in memory for the anchor lifetime. It writes only the learned pairs.
- Capture is off by default.
- The LLM post-process step receives the vocabulary list only if the prompt uses `${dictionary}`.

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
- Should Handy learn case-only changes, for example `github` -> `GitHub`? Proposal: yes, with `case_mode = 'exact'`.
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

- The SQLite table in `history.db` through the existing `MIGRATIONS` array.
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
- The anchor read at paste time (section 7.3.1) happens **after** the paste is sent, not before. The paste never waits for the anchor.
- Check triggers (section 7.3.1) coalesce. A focus change during a running check does not start a second check.

### 16.4 Memory

This project trims memory after each dictation (`memory.rs`, `FinishGuard`). The Dictionary follows the same discipline.

- An anchor holds at most 32 KB of field text. A larger field stores only the 32 KB window around the caret.
- At most 4 anchors live at one time. A new anchor beyond that drops the oldest.
- Anchors drop at the end of the capture window (default 180 s), on secure-input, and on application quit.
- The in-memory dictionary table is small (< 1,000 entries, ~100 KB). Hold it as one `Arc`, not one copy per thread.

### 16.5 Startup

- Do not open or migrate the dictionary table on the startup path before the tray appears. `HistoryManager` already owns the database; the Dictionary migration runs with the existing migration pass.
- Build the first matcher lazily, on the first transcription, not at startup.

### 16.6 What to measure before merge

Each phase (section 13) ships with numbers in the pull request:

1. Pipeline time with the feature off (baseline) and on, same audio, 10 runs, report the median.
2. Matcher build time at 100 / 1,000 / 10,000 entries.
3. `learn()` time on a 1,000-word edit.
4. For capture: paste-to-anchor time, and check time against a responsive and an unresponsive application.
