# Dictionary Test Build

This document is written in Simplified Technical English (STE).
It explains how to install and test the Handy Dictionary build.

## 1. What this build is

This is Handy 0.10.0-dict.1. It is a test build. It adds the Dictionary:

- Handy learns your corrections. It then fixes the same mistake in every
  later transcription.
- Three ways to teach it:
  1. Correct the pasted text where it landed (WhatsApp, Notes, Telegram,
     most macOS applications). Handy sees the edit and learns it.
  2. Menu bar icon -> "Correct Last Transcript...". Edit the text and click
     the learned pair.
  3. Settings -> Advanced -> Experimental -> Dictionary. Add word pairs by
     hand.

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
3. In the Experimental group, turn on "Dictionary".
4. Turn on "Learn from edits (macOS)" for in-place learning.

## 4. Test it

1. Dictate a sentence with a word Handy gets wrong. A name works well.
2. Fix the word in the application where the text landed.
3. Switch applications, or start the next dictation.
4. Handy shows a toast: "Added to Dictionary: wrong -> right".
5. Dictate the sentence again. The word comes out right.

If in-place learning does not trigger (some applications do not expose
their text), use the menu bar: "Correct Last Transcript...".

## 5. Control what it learned

Settings -> Advanced -> Experimental -> Dictionary shows every entry.
Click an entry to delete it.

## 6. Go back to the release version

Download Handy from https://handy.computer and install it over this build.
Your settings and history stay.

## 7. Report problems

Tell us:

- What you dictated and what you expected.
- The application you pasted into.
- A screenshot of Settings -> Advanced -> Experimental -> Dictionary if a
  wrong entry was learned.

Debug logs: press Cmd+Shift+D in the settings window, open the log viewer,
and filter for "capture:". The log holds no dictated text.
