# Dictionary Test Build

This document is written in Simplified Technical English (STE).
It explains how to install and test the Handy Dictionary build.

## 1. What this build is

This is Handy 0.9.7-katapult.2, the Katapult team release on upstream
Handy 0.9.7. It adds the Dictionary:

- Handy learns clear matches to vocabulary you have taught it automatically,
  with **Undo**. These rules apply only near matching context words.
- Less certain corrections stay in Suggested until you choose **Always replace**.
  Common grammar changes and rewrites are filtered out.
- Three ways to teach it:
  1. Correct the pasted text where it landed (WhatsApp, Notes, Telegram,
     most macOS applications). Handy sees the edit and learns or suggests a pair.
  2. Menu bar icon -> "Correct Last Transcript...", or the pencil on a
     History entry. Edit the text and click Learn from correction. Review uncertain pairs directly in the correction overlay or later in Dictionary.
  3. Settings -> Dictionary. Add word pairs by hand.

Builds exist for macOS (Apple Silicon and Intel), Windows, and Linux.
They are not signed with an Apple or Microsoft certificate. The Dictionary
learns from edits in other applications on macOS only.

## 2. Install

Download the file for your system from
https://github.com/katapultlabs/Handy/releases/latest. You do this one
time. Later releases arrive inside Handy (section 3).

- macOS: the `.dmg` (`aarch64` for Apple Silicon, `x64` for Intel).
  1. Quit Handy if it runs.
  2. Open the `.dmg` and drag `Handy.app` to `/Applications`. Replace the
     old one.
  3. macOS blocks unsigned applications on first start. Right-click
     `Handy.app` -> Open -> Open. Or run
     `xattr -cr /Applications/Handy.app`, then open it.
  4. Grant Accessibility and Microphone again when macOS asks
     (System Settings -> Privacy & Security).
- Windows: the `-setup.exe`. SmartScreen warns about an unknown
  publisher. Click "More info" -> "Run anyway".
- Linux: the `.AppImage`, `.deb`, or `.rpm`. All three update
  themselves.

Your settings, models, and history stay. The build uses the same data as
the upstream release.

## 3. Updates

Handy checks for updates when it starts. It gets them from the Katapult
releases, not from upstream. Keep Settings -> Advanced -> "Check for
updates" on.

When an update is ready, Handy shows it in the footer. Click it to
download and restart. On macOS, grant Accessibility and Microphone again
after each update. macOS sees each build as a new application until the
builds carry an Apple certificate.

Builds `0.9.7+katapult.1` and older do not update to Katapult releases.
Install this build by hand one time.

## 4. Turn the Dictionary on

1. Open Handy Settings -> Advanced.
2. Turn on "Enable experimental features".
3. In the Experimental group, turn on "Dictionary". A Dictionary section
   appears in the sidebar.
4. In the Dictionary section, turn on "Learn from edits (macOS)" for
   in-place learning.

## 5. Test it

1. Teach Handy the correct vocabulary first. For this test, add a manual
   correction "katapolt" -> "Katapult" in Dictionary. Custom Words and approved
   suggestions also supply trusted vocabulary.
2. Dictate a sentence with a close mistake, such as "Catapult manages releases".
   Correct "Catapult" to "Katapult" in the target application.
3. Pause briefly after the edit. Handy checks settled edits without waiting for
   another dictation. Applications with Accessibility notifications normally
   produce a candidate about 600 ms after the last edit, plus application and
   storage response time. Unsupported applications use a bounded fallback.
4. A strong match to known vocabulary with unchanged useful nearby words shows
   "Learned: Catapult -> Katapult", with Undo. It is active immediately, only
   near the saved context. "A catapult launches rocks" stays unchanged when its
   nearby words do not match that context. A separate approved rule or the
   existing Custom Words fuzzy correction can still change that word.
5. An unfamiliar or less certain pair asks "Always replace wrong with right?"
   directly in the overlay. **Always replace** approves a global replacement.
   **Ignore** remembers that you do not want it.
6. Dictate again. Automatic rules apply in matching context; approved and
   manual corrections apply everywhere they match.

Each correction gets its own card and 8 seconds of visible reading time.
Hovering, focusing a control, or saving a choice pauses the clock. Recording
interrupts the card and then returns it with its remaining time. Multiple
corrections appear in order. **Next** or closing a card only skips the notice;
it does not approve or ignore the stored pair. You can review it in Dictionary
later. History edits use the same cards. Recording visuals can be off and
correction cards still appear.

Try a History edit with two corrections, approve the first, and ignore the
second. Then trigger another correction, start a recording while its card is
visible, and confirm it returns afterward. A card should not disappear while
the pointer is over it. Turning Dictionary off clears waiting cards.

A rapid edit followed by Send can still be missed if the application clears
its field before Handy has observed a settled edit. A safely observed edit can
survive a later clear. This does not provide access to text an application
never exposed through Accessibility.

The automatic test uses close spelling, matching phonetic codes, trusted
vocabulary, and unchanged context. Word counts alone do not decide: one word
can correctly become two. Short terms, unsupported scripts, distant sound
matches, and edits with ambiguous context remain suggestions. This first pass
favors fewer automatic rules over false replacements.

If in-place learning does not trigger (some applications do not expose
their text), use the menu bar: "Correct Last Transcript...".

Handy suggests small fixes that sound like the wrong word: one to three
words on each side, for example "Bededa" -> "Pereira". If you rewrite a
sentence, add or delete words, or fix punctuation, Handy shows "No new
suggestions from this edit". This is by design.

Handy filters common style and grammar edits, because an approved entry
applies to every later transcription. Examples of filtered edits:

- Common words on both sides: "there" -> "their", "the" -> "The".
- Contractions: "we are" -> "we're".
- Punctuation or spacing between the same words: "so there" ->
  "so, there", "hand created" -> "hand-created".

## 6. Control what it learned

Settings -> Dictionary separates Suggested, Your corrections, and Ignored.
Search across all groups. Each pair shows its source: Manual, History, or
Captured. Automatic rows also show Automatic. A suggestion remains inactive
no matter how often it is seen.

- **Always replace** approves a pair. If another correction for the same
  wrong text is active, it moves to Suggested. Merely discovering a new pair
  never replaces an approved correction.
- **Undo** stops an automatic rule and remembers the rejection. It does not
  change text already pasted. The correction remains in Ignored.
- **Ignore** remembers your choice. That pair stays off and is not suggested
  again. Expand Ignored to approve it later if you change your mind.
- The pencil edits a pair in place. Editing a suggestion does not approve it.
- The checkbox turns an approved pair off without deleting it.
- A pair you add manually applies immediately.

When upgrading from a build older than dict.9, its automatically learned
corrections move to Suggested once for review. Decisions made in dict.9 stay
as they were; existing suggestions are not automatically promoted. Manually
added entries stay as they were.
No entries are deleted. Entries imported from the old settings list follow the
same rule. "seen 3x" means Handy found that pair three times; it does not mean
that the pair was approved.

## 7. Paste the last transcript again

Handy restores your clipboard after each paste, so the transcript is gone
once it lands. Press Ctrl+Cmd+V to paste the most recent transcript again
at the cursor. Use it when the text went to the wrong field, or when you
want the same text in a second place.

Change the chord in Settings -> General -> "Paste Last Transcript".

## 8. Go back to the release version

Download Handy from https://handy.computer and install it over this build.
Your settings and history stay.

## 9. Build it yourself (macOS, Apple Silicon)

The team release is the tag `v0.9.7-katapult.2` on `main` of
https://github.com/katapultlabs/Handy. Work in progress is on `feat/*`
branches. A build takes about 5 minutes after the first compile, and
about 15 minutes the first time.

1. Install the tools once:

   ```bash
   xcode-select --install
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   curl -fsSL https://bun.sh/install | bash
   brew install cmake
   ```

2. Get the code and the VAD model:

   ```bash
   git clone https://github.com/katapultlabs/Handy.git
   cd Handy
   git checkout v0.9.7-katapult.2
   bun install
   mkdir -p src-tauri/resources/models
   curl -o src-tauri/resources/models/silero_vad_v4.onnx https://blob.handy.computer/silero_vad_v4.onnx
   ```

3. Build the application bundle:

   ```bash
   CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri build --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}'
   ```

   This builds the application without an auto-update artifact or updater key.

4. The application is at `src-tauri/target/release/bundle/macos/Handy.app`.
   Install it with the steps in section 2.

To run in development mode instead, with hot reload and debug logs, quit
the installed Handy first and run:

```bash
CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri dev
```

## 10. Local processing

Learning runs off the paste path. It uses local string comparisons and the
existing phonetic library; it makes no model or network calls. The matcher is
compiled after dictionary changes and reused for dictation. Capture retains
only the correction pair and up to four context words in the local database;
it does not save the surrounding field text.

## 11. Report problems

Tell us:

- What you dictated and what you expected.
- The application you pasted into.
- A screenshot of Settings -> Dictionary if a
  wrong entry was learned.

Debug logs: press Cmd+Shift+D in the settings window, open the log viewer,
and filter for "capture:". The log holds no dictated text.
