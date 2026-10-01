# Sending the fork's work upstream

This document is written in Simplified Technical English (STE).
It is the working plan for the upstream pull requests listed in
`docs/FORK.md`, section 5. Update the status table as steps complete.

Upstream (https://github.com/cjpais/Handy) is in a feature freeze. A
feature needs community support in a Discussion before a pull request.
One fix or feature per pull request. Every pull request must fill
`.github/PULL_REQUEST_TEMPLATE.md`, including the Human Written
Description and the AI Assistance section.

## 1. Status

| Step                                | Branch             | Status                           |
| ----------------------------------- | ------------------ | -------------------------------- |
| Discussion: Dictionary              | none               | Draft below, not posted          |
| PR: Dictionary core                 | `up/dictionary`    | Waits for the Discussion         |
| PR: Learn from History edits        | `up/history-learn` | Waits for the core               |
| PR: macOS in-place capture          | `up/capture`       | Waits for the core               |
| Discussion + PR: Paste last         | `up/paste-last`    | Not started                      |
| PR: Architecture docs and AGENTS.md | `up/docs`          | Can go now; stable since 2026-09 |

No standalone bug fix is pending. `audio_toolkit/text.rs` is identical to
upstream.

## 2. How to cut an `up/*` branch

1. `git fetch upstream`
2. `git checkout -b up/<topic> upstream/main`
3. `git cherry-pick <commits from main>`; squash to a few clear commits.
4. Remove fork-only parts from the result: the version suffix,
   `docs/TEST_BUILD.md`, `docs/FORK.md`, this file, the `CLAUDE.md`
   line.
5. Run the full check set and push to `origin up/<topic>`.
6. Open the pull request against `cjpais/Handy:main` with the template.
   Leave the Human Written Description for a person to write.
7. After each review round, merge `up/<topic>` into `main`.

## 3. Discussion draft: Dictionary

Post in https://github.com/cjpais/Handy/discussions (Ideas). A person
should read it once and adjust the voice before posting.

---

**Title:** Dictionary: learn corrections from the user's edits (follow-up to #1533 and #1369)

Handy already has Custom Words, a flat list with fuzzy matching. Several
people have asked for the next step: wrong → right pairs that Handy
learns from what the user actually corrects.

- #1533 (open) adds deterministic word replacements. The maintainer said
  he wants to merge this or a variant.
- #1369 (closed) learned corrections from History edits through an LLM.
  It was closed on process, not on the idea.
- #1711 (open) exposes custom words to post-processing prompts.
- #1333 reports fuzzy Custom Words being too aggressive.

We built a version of this on our fork and have used it daily for a
month. We would like to send it upstream in small pull requests, and are
asking here first because of the feature freeze.

What it does:

1. Entries are wrong → right pairs in a SQLite table next to History.
   They apply before post-processing with an exact, word-boundary
   matcher. No LLM, no network.
2. Handy learns pairs from two places: the History edit dialog, and (on
   macOS, opt-in) the text field the transcript was pasted into, read
   through Accessibility right after the paste.
3. A learned pair is not applied blindly. Only a clear match to vocabulary
   the user already taught (close spelling, same phonetic key, unchanged
   context words) becomes an automatic rule, and it applies only near the
   same context. Everything else waits in a Suggested list until the user
   chooses Always replace or Ignore. Grammar and style edits (there/their,
   contractions, punctuation) are filtered out and never learned.
4. Everything is behind the Experimental toggle. The hot path is one
   compiled Aho-Corasick pass; learning runs off the paste path.

Proposed split, one pull request each: the store, matcher and settings
page; learning from History edits; macOS capture; and the paste-last
shortcut as a separate idea.

Design document, with the privacy and performance budgets:
https://github.com/katapultlabs/Handy/blob/main/docs/DICTIONARY_DESIGN.md

Would this be welcome, and is the split right?

---
