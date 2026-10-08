# Katapult fork of Handy

This document is written in Simplified Technical English (STE).
It explains how this fork relates to upstream Handy
(https://github.com/cjpais/Handy) and which parts exist only here.

## 1. Branches

- `main`: the product the team runs. It is upstream `main` plus every
  change the fork has accepted. Merge into it with a merge commit, not a
  squash, so single commits stay available for upstream pull requests.
- `feat/*`: work in progress. Local test builds come from these branches
  with the version `<upstream>+dict.N` (or another feature name). Merge a
  feature branch into `main` when it is done.
- `up/*`: one branch per upstream pull request. Branch from
  `upstream/main`, cherry-pick the commits from `main`, open the pull
  request. After each review round, merge the `up/*` branch back into
  `main`. That keeps `main` equal to the reviewed shape, so the final
  upstream merge applies cleanly.

## 2. Releases

A team release is a GitHub release on this repository with the version
`<upstream>-katapult.N`, for example `0.9.7-katapult.2`. The part before
the hyphen is the upstream version inside the build. `N` counts fork
releases on that upstream version.

The hyphen form is a semver pre-release. The updater compares it
correctly: `0.9.7-katapult.3` is newer than `0.9.7-katapult.2`, and
`0.9.8-katapult.1` is newer than both. Do not use `+` for a release. The
updater ignores everything after `+`. `v0.9.7+katapult.1` was the last
release with that form.

`v0.9.7+katapult.1` and older builds also read upstream's feed with
upstream's key. They offer every new upstream release as an update. Upstream
Handy cannot open a database that has the Dictionary migrations, so it stops
at start with `DatabaseTooFarAhead`. The data is not changed. To recover,
install the newest fork release by hand over the upstream app. Tell each
person who still runs one of these builds to do this.

From 0.9.8-katapult.2, the Dictionary has its own file, `dictionary.db`, and
`history.db` keeps upstream's schema. Upstream Handy then starts on fork data;
only the Dictionary is missing until a fork build runs again.

### 2.1 Cut a release

1. On `main`, set `version` in `src-tauri/tauri.conf.json` to the new
   `<upstream>-katapult.N`. Set `bundle.windows.wix.version` in
   `src-tauri/tauri.windows.conf.json` to `<upstream>.N`, for example
   `0.9.7.2`. The MSI installer accepts only numbers.
2. Commit and push `main`.
3. Run the "Katapult Release" workflow on `main`, from the Actions tab or
   with:

   ```bash
   gh api -X POST repos/katapultlabs/Handy/actions/workflows/katapult-release.yml/dispatches -f ref=main
   ```

4. The workflow creates the tag `v<version>` and a draft release, builds
   all platforms, and signs the updater files. It publishes the release
   only when `latest.json` is attached. A failed run leaves only a draft;
   GitHub creates the tag when the release is published. Delete the draft,
   fix the problem, and run again.

### 2.2 Automatic updates

Installed fork builds read
`https://github.com/katapultlabs/Handy/releases/latest/download/latest.json`
and accept an update only if it is signed with the fork's updater key.
The public key is `plugins.updater.pubkey` in `src-tauri/tauri.conf.json`.
The private key and its password are the Actions secrets
`TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. A
copy stays with the fork owner, outside the repository. If the key is
lost, installed builds cannot update, and everybody must install the
next build by hand.

The fork has no Apple or Microsoft code signing certificate:

- macOS builds are ad-hoc signed. First start needs right-click -> Open.
  After each update, macOS asks again for Accessibility and Microphone.
- Windows installers are unsigned. SmartScreen warns on first install.
- Linux: the `.AppImage`, `.deb`, and `.rpm` packages all update
  themselves.

### 2.3 Workflows disabled on the fork

Upstream's "Release", "Main Branch Build", "Build Test", and "PR Test
Build" need upstream's Apple and Azure secrets, so they fail here. They
are disabled in the fork's Actions settings, not in the files, so
upstream merges stay clean. "test", "code quality", "nix build check",
and "Playwright" stay on.

Build a release locally with the steps in `docs/TEST_BUILD.md`,
section 9. A local build has no updater signature.

## 3. Merging upstream

Run `git merge upstream/main` into `main` (never rebase; the team builds
from the branch). These conflicts repeat and have fixed answers:

- `src-tauri/tauri.conf.json` `version`: take the new upstream version
  and add `-katapult.1`. Set `wix.version` in
  `src-tauri/tauri.windows.conf.json` to `<upstream>.1`. `package.json`
  and `src-tauri/Cargo.toml` keep the plain upstream version.
- `src-tauri/tauri.conf.json` `plugins.updater` and `bundle.windows`:
  keep ours. The fork's updater feed and key must survive every merge,
  and the fork has no Windows `signCommand`.
- `.github/workflows/build.yml`: keep the `sign-updater` input and the
  two `TAURI_SIGNING_*` lines that read it.
- `src-tauri/src/managers/history.rs`: take upstream's `MIGRATIONS`
  exactly. The fork adds only the `dictionary_db::move_out_of_history` call.
  Never put a fork migration in this file; Dictionary migrations go in
  `src-tauri/src/dictionary_db.rs`.
- `src/i18n/locales/*/translation.json`: keep both sides, then run
  `bun run check:translations`. A new upstream locale needs every
  fork-only key added before the check passes.

After the merge, run the full check set: `cargo test`, `bun run build`,
`bun run lint`, `bun run format:check`, `bun run check:translations`.

## 4. Fork-only parts

These parts exist in this fork and not upstream. Review this list at
every upstream merge. Remove an item when upstream accepts it.

| Part                                                | Where                                                                            | Upstream status                                         |
| --------------------------------------------------- | -------------------------------------------------------------------------------- | ------------------------------------------------------- |
| Version form `-katapult.N` / `+dict.N`              | `src-tauri/tauri.conf.json`, `src-tauri/tauri.windows.conf.json` (`wix.version`) | Never sent                                              |
| Updater feed and public key                         | `src-tauri/tauri.conf.json`, `plugins.updater`                                   | Never sent                                              |
| No Windows `signCommand`                            | `src-tauri/tauri.conf.json`, `bundle.windows`                                    | Never sent                                              |
| Release workflow                                    | `.github/workflows/katapult-release.yml`                                         | Never sent                                              |
| `sign-updater` build input                          | `.github/workflows/build.yml`                                                    | Could go upstream as a small CI change                  |
| Portable installer fallback link                    | `src/components/update-checker/portableInstaller.ts`                             | Never sent                                              |
| Tester guide                                        | `docs/TEST_BUILD.md`                                                             | Never sent                                              |
| This document                                       | `docs/FORK.md`                                                                   | Never sent                                              |
| Upstream plan and Discussion drafts                 | `docs/UPSTREAM.md`                                                               | Never sent                                              |
| Pointer to this document                            | `CLAUDE.md`, last line                                                           | Never sent                                              |
| Dictionary: store, matcher, learning, settings page | `src-tauri/src/dictionary*.rs`, `src/components/settings/dictionary/`            | Not yet sent; needs a Discussion first (feature freeze) |
| Correction notices in the overlay                   | `src-tauri/src/correction_notices.rs`, `src/overlay/`                            | Part of the Dictionary pull request                     |
| Learn from History edits                            | `src/components/settings/history/HistorySettings.tsx`, `commands/dictionary.rs`  | Follow-up to the Dictionary pull request                |
| macOS in-place capture                              | `src-tauri/src/dictionary_capture.rs`                                            | Separate pull request after the Dictionary              |
| Paste last transcript shortcut                      | `src-tauri/src/actions.rs`, `settings.rs`                                        | Separate Discussion; unrelated to the Dictionary        |
| Architecture docs and agent guidance                | `docs/ARCHITECTURE.md`, `AGENTS.md`                                              | Sent as documentation pull requests when stable         |

## 5. Upstream pull request order

1. Pure bug fixes, each alone.
2. Dictionary core, after a Discussion that cites the earlier
   learn-from-edits pull requests.
3. Learn from History edits.
4. macOS in-place capture.
5. Paste last transcript, with its own Discussion.

Read `.github/PULL_REQUEST_TEMPLATE.md` before each pull request and
fill every section.

## 6. Claude Code sessions

Several sessions can work on this fork at the same time. The rules:

- Start every session from the main checkout,
  `~/Projects/oss_projects/Handy`, which stays on `main`. Keep that
  checkout clean; do not commit there directly.
- A background job gets its own worktree under `.claude/worktrees/`,
  branched from `main`. Name the branch `feat/<topic>`. Commit and push
  it before the job ends; a worktree can be deleted with its session.
- Run `git status` before a merge or a build. A worktree can hold
  uncommitted work from another session. Commit it as its own unit if it
  passes the checks; do not discard it.
- Only one session lands work on `main` at a time. From the worktree:
  `git fetch origin`, `git merge origin/main`, run the checks, then
  `git push origin HEAD:main`. The main checkout picks it up with
  `git pull`.
- Never rebase a shared branch. Merge `upstream/main` or `main` in.
- Each worktree compiles the Rust side from scratch once (about 15
  minutes). Later builds in the same worktree take about 5 minutes.
- `.claude/settings.local.json` in the main checkout lets sessions run
  `gh release`, `gh pr`, `gh api`, remote branch deletion, and pruning.
  Writing repository secrets stays with a person.
- Project memory for Claude is shared across sessions of this repository,
  so a session can rely on what an earlier session recorded.
