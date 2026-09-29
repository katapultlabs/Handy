# Katapult fork of Handy

This document is written in Simplified Technical English (STE).
It explains how this fork relates to upstream Handy
(https://github.com/cjpais/Handy) and which parts exist only here.

## 1. Branches

- `main`: the product the team runs. It is upstream `main` plus every
  change the fork has accepted. Merge into it with a merge commit, not a
  squash, so single commits stay available for upstream pull requests.
- `feat/*`: work in progress. Test builds come from these branches with
  the version `<upstream>+dict.N` (or another feature name). Merge a
  feature branch into `main` when it is done.
- `up/*`: one branch per upstream pull request. Branch from
  `upstream/main`, cherry-pick the commits from `main`, open the pull
  request. After each review round, merge the `up/*` branch back into
  `main`. That keeps `main` equal to the reviewed shape, so the final
  upstream merge applies cleanly.

## 2. Releases

A team release is an annotated tag on `main`, for example
`v0.9.7+katapult.1`. The number before the plus sign is the upstream
version inside the build. The number after `katapult.` counts fork
releases on that upstream version. `src-tauri/tauri.conf.json` carries
the same string, so the application reports it in About and in logs.

Build a release from the tag with the steps in `docs/TEST_BUILD.md`,
section 8.

## 3. Merging upstream

Run `git merge upstream/main` into `main` (never rebase; the team builds
from the branch). Two conflicts repeat and have fixed answers:

- `src-tauri/tauri.conf.json` `version`: take the new upstream version
  and add `+katapult.N`. `package.json` and `src-tauri/Cargo.toml` keep
  the plain upstream version.
- `src/i18n/locales/*/translation.json`: keep both sides, then run
  `bun run check:translations`. A new upstream locale needs every
  fork-only key added before the check passes.

After the merge, run the full check set: `cargo test`, `bun run build`,
`bun run lint`, `bun run format:check`, `bun run check:translations`.

## 4. Fork-only parts

These parts exist in this fork and not upstream. Review this list at
every upstream merge. Remove an item when upstream accepts it.

| Part                                                | Where                                                                           | Upstream status                                         |
| --------------------------------------------------- | ------------------------------------------------------------------------------- | ------------------------------------------------------- |
| Version suffix `+katapult.N` / `+dict.N`            | `src-tauri/tauri.conf.json`                                                     | Never sent                                              |
| Tester guide                                        | `docs/TEST_BUILD.md`                                                            | Never sent                                              |
| This document                                       | `docs/FORK.md`                                                                  | Never sent                                              |
| Dictionary: store, matcher, learning, settings page | `src-tauri/src/dictionary*.rs`, `src/components/settings/dictionary/`           | Not yet sent; needs a Discussion first (feature freeze) |
| Correction notices in the overlay                   | `src-tauri/src/correction_notices.rs`, `src/overlay/`                           | Part of the Dictionary pull request                     |
| Learn from History edits                            | `src/components/settings/history/HistorySettings.tsx`, `commands/dictionary.rs` | Follow-up to the Dictionary pull request                |
| macOS in-place capture                              | `src-tauri/src/dictionary_capture.rs`                                           | Separate pull request after the Dictionary              |
| Paste last transcript shortcut                      | `src-tauri/src/actions.rs`, `settings.rs`                                       | Separate Discussion; unrelated to the Dictionary        |
| Architecture docs and agent guidance                | `docs/ARCHITECTURE.md`, `AGENTS.md`                                             | Sent as documentation pull requests when stable         |

## 5. Upstream pull request order

1. Pure bug fixes, each alone.
2. Dictionary core, after a Discussion that cites the earlier
   learn-from-edits pull requests.
3. Learn from History edits.
4. macOS in-place capture.
5. Paste last transcript, with its own Discussion.

Read `.github/PULL_REQUEST_TEMPLATE.md` before each pull request and
fill every section.
