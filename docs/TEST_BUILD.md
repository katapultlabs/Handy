# Dictionary Test Build

This document is written in Simplified Technical English (STE).
It explains how to install and test the Handy Dictionary build.

## 1. What this build is

This is Handy 0.9.6+dict.5. It is a test build. It adds the Dictionary:

- Handy learns your corrections. It then fixes the same mistake in every
  later transcription.
- Three ways to teach it:
  1. Correct the pasted text where it landed (WhatsApp, Notes, Telegram,
     most macOS applications). Handy sees the edit and learns it.
  2. Menu bar icon -> "Correct Last Transcript...", or the pencil on a
     History entry. Edit the text and click Save. Handy learns the fix.
  3. Settings -> Dictionary. Add word pairs by hand.

The build is macOS (Apple Silicon) only. It is not signed with a developer
certificate.

## 2. Install

1. Quit Handy if it runs.
2. Copy `Handy.app` to `/Applications`. Replace the old one.
3. macOS blocks unsigned applications on first start. Do one of these:
   - Right-click `Handy.app` -> Open -> Open.
   - Or run: `xattr -cr /Applications/Handy.app`, then open it.
4. macOS treats this build as a new application. Grant the permissions
   again when it asks: Accessibility and Microphone
   (System Settings -> Privacy & Security).

Your settings, models, and history stay. The build uses the same data as
the release version.

## 3. Turn the Dictionary on

1. Open Handy Settings -> Advanced.
2. Turn on "Enable experimental features".
3. In the Experimental group, turn on "Dictionary". A Dictionary section
   appears in the sidebar.
4. In the Dictionary section, turn on "Learn from edits (macOS)" for
   in-place learning.
5. Back in Advanced, in the App group, turn off "Check for updates". An
   upstream release would otherwise offer to replace this test build.

## 4. Test it

1. Dictate a sentence with a word Handy gets wrong. A name works well.
2. Fix the word in the application where the text landed.
3. Switch applications, or start the next dictation.
4. Handy shows "Learned: wrong -> right" in the recording overlay for four
   seconds. Click Undo on it if the pair is wrong. If the overlay is set
   to None, there is no notice on screen. The settings window also shows
   a toast with Undo.
5. Dictate the sentence again. The word comes out right.

If in-place learning does not trigger (some applications do not expose
their text), use the menu bar: "Correct Last Transcript...".

Handy learns only small fixes that sound like the wrong word: one to three
words on each side, for example "Bededa" -> "Pereira". If you rewrite a
sentence, add or delete words, or fix punctuation, Handy shows "No
corrections learned from this edit". This is by design.

Handy also does not learn style and grammar edits, because an entry
applies to every later transcription:

- Common words on both sides: "there" -> "their", "the" -> "The".
- Contractions: "we are" -> "we're".
- Punctuation or spacing between the same words: "so there" ->
  "so, there", "hand created" -> "hand-created".

## 5. Control what it learned

Settings -> Dictionary shows every entry with where it came from
(Manual, History, Captured). Search the list, add a pair, or click the
trash icon to delete one.

## 6. Go back to the release version

Download Handy from https://handy.computer and install it over this build.
Your settings and history stay.

## 7. Build it yourself (macOS, Apple Silicon)

The source is on the `feat/dictionary-mvp` branch of
https://github.com/katapultlabs/Handy. A build takes about 5 minutes
after the first compile, and about 15 minutes the first time.

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
   git checkout feat/dictionary-mvp
   bun install
   mkdir -p src-tauri/resources/models
   curl -o src-tauri/resources/models/silero_vad_v4.onnx https://blob.handy.computer/silero_vad_v4.onnx
   ```

3. Build the application bundle:

   ```bash
   CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri build --bundles app
   ```

   The build ends with an error about a missing `TAURI_SIGNING_PRIVATE_KEY`.
   This is expected. It only affects the auto-update artifact, not the
   application.

4. The application is at `src-tauri/target/release/bundle/macos/Handy.app`.
   Install it with the steps in section 2.

To run in development mode instead, with hot reload and debug logs, quit
the installed Handy first and run:

```bash
CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri dev
```

## 8. Report problems

Tell us:

- What you dictated and what you expected.
- The application you pasted into.
- A screenshot of Settings -> Dictionary if a
  wrong entry was learned.

Debug logs: press Cmd+Shift+D in the settings window, open the log viewer,
and filter for "capture:". The log holds no dictated text.
