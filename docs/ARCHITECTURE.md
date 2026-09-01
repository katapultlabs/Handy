# Handy Architecture

This document is written in Simplified Technical English (STE).
Read this document before you change the code.
It tells you where the code is and how the data moves.

Line counts are approximate. They show you which files are large.

## 1. What Handy is

Handy is a desktop application for speech-to-text.
It runs on macOS, Windows, and Linux.
It uses Tauri 2. The backend is Rust. The frontend is React and TypeScript.

Handy does all transcription on the local machine.
Handy does not send audio to a remote server.
Handy can send the transcribed text to a remote LLM for post-processing.
This step is optional.

## 2. Data flow

One dictation goes through these steps:

1. The user presses the global shortcut.
2. The backend starts the microphone and records audio.
3. The user releases the shortcut, or presses it again.
4. The backend stops the recording.
5. The VAD removes silence from the audio.
6. The ASR engine converts the audio to text.
7. The text filters apply custom words and remove filler words.
8. If post-processing is on, the backend sends the text to an LLM. The LLM returns new text.
9. The backend writes the text into the active application. It uses the clipboard or a typing tool.
10. The backend saves the text and the audio in the history database.

```
Shortcut -> Audio (cpal) -> VAD (Silero) -> ASR engine -> Text filters
        -> [LLM post-process] -> Paste -> History
```

The overlay window shows the current step to the user.
The tray icon also shows the current step.

## 3. Backend map (`src-tauri/src/`)

Total: approximately 28,000 lines of Rust in 60 files.

### 3.1 Entry and setup

| File               | Lines | Function                                                                                                      |
| ------------------ | ----- | ------------------------------------------------------------------------------------------------------------- |
| `main.rs`          | 18    | Parses the CLI arguments. Calls `lib.rs`.                                                                     |
| `lib.rs`           | 1,045 | Builds the Tauri application. Creates the managers. Registers all commands and events. Applies the CLI flags. |
| `cli.rs`           | 63    | Defines the CLI flags with `clap`.                                                                            |
| `signal_handle.rs` | 58    | Defines `send_transcription_input()`. Unix signals and CLI flags use this function.                           |
| `portable.rs`      | 182   | Portable mode. If a file named `portable` is next to the executable, all data goes in a `Data/` folder.       |
| `autostart.rs`     | 155   | Starts Handy at login.                                                                                        |
| `memory.rs`        | 55    | Tunes the glibc allocator on Linux.                                                                           |
| `utils.rs`         | 199   | Platform helpers. Overlay show and hide helpers.                                                              |

### 3.2 Pipeline control

| File                           | Lines | Function                                                                                                                                                        |
| ------------------------------ | ----- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `transcription_coordinator.rs` | 1,617 | Receives shortcut presses. Keeps the state machine for one dictation. Makes sure press and release stay in pairs.                                               |
| `actions.rs`                   | 1,039 | Defines the `ShortcutAction` trait. `TranscribeAction` runs the full pipeline. `post_process_transcription()` calls the LLM. `CancelAction` stops the pipeline. |
| `shortcut/mod.rs`              | 1,405 | Binds global shortcuts. Defines the `change_*_setting` commands.                                                                                                |
| `shortcut/handler.rs`          | 76    | Maps a shortcut event to a `ShortcutAction`.                                                                                                                    |
| `shortcut/handy_keys.rs`       | 574   | Keyboard backend that uses `rdev`.                                                                                                                              |
| `shortcut/tauri_impl.rs`       | 205   | Keyboard backend that uses the Tauri global-shortcut plugin.                                                                                                    |

### 3.3 Managers

Each manager is one Tauri state object. `lib.rs` creates them at startup.

| File                             | Lines | Function                                                                                                                                                                                |
| -------------------------------- | ----- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `managers/audio.rs`              | 1,101 | Opens the microphone. Records audio. Lists audio devices.                                                                                                                               |
| `managers/transcription.rs`      | 2,509 | Loads and unloads the ASR model. Runs batch transcription with `transcribe()`. Runs streaming transcription with `start_stream()` and `finalize_stream()`. Selects the GPU accelerator. |
| `managers/model.rs`              | 2,988 | Defines `EngineType`. Lists, downloads, and deletes models. Finds model files on disk.                                                                                                  |
| `managers/model/download.rs`     | 384   | Downloads model files. Has a test suite in `download/tests.rs`.                                                                                                                         |
| `managers/model_capabilities.rs` | 270   | Reads what a GGUF model can do, for example which languages.                                                                                                                            |
| `managers/gguf_meta.rs`          | 494   | Reads the GGUF file header. Has no external dependencies.                                                                                                                               |
| `managers/history.rs`            | 737   | Saves and loads transcription history in SQLite.                                                                                                                                        |

### 3.4 ASR engines

`EngineType` in `managers/model.rs` lists the engines:

- `TranscribeCpp`. This engine uses the `transcribe-cpp` crate. It runs all GGML and GGUF models. Examples: Whisper, Parakeet, Voxtral, Qwen3-ASR, Nemotron. The engine detects the architecture from the file.
- `Parakeet`, `Moonshine`, `MoonshineStreaming`, `SenseVoice`, `GigaAM`, `Canary`, `Cohere`. These engines use the `transcribe-rs` crate and ONNX Runtime.

`catalog/mod.rs` (286 lines) holds the model catalog.
The catalog is a JSON file. The build script `scripts/gen_catalog.py` makes it.
The application does not download the catalog at run time.

### 3.5 Audio toolkit (`audio_toolkit/`)

| File                  | Lines | Function                                                                                           |
| --------------------- | ----- | -------------------------------------------------------------------------------------------------- |
| `audio/recorder.rs`   | 1,027 | Records audio from `cpal`.                                                                         |
| `audio/resampler.rs`  | 351   | Resamples audio to 16 kHz with `rubato`.                                                           |
| `audio/device.rs`     | 52    | Lists audio devices.                                                                               |
| `audio/visualizer.rs` | 162   | Computes audio levels for the overlay.                                                             |
| `vad/silero.rs`       | 62    | Silero VAD backend.                                                                                |
| `vad/earshot.rs`      | 116   | Earshot VAD backend.                                                                               |
| `vad/smoothed.rs`     | 237   | Smooths the VAD output.                                                                            |
| `text.rs`             | 828   | Text filters. `apply_custom_words()`, `remove_filler_words()`, `normalize_transcription_output()`. |
| `lang_id.rs`          | 170   | Finds the language of a text. Used only when the model does not report a language.                 |
| `bin/cli.rs`          | 371   | Standalone CLI binary for tests.                                                                   |

### 3.6 Post-processing (LLM)

| File                    | Lines | Function                                                                                                                |
| ----------------------- | ----- | ----------------------------------------------------------------------------------------------------------------------- |
| `llm_client.rs`         | 734   | HTTP client for chat-completion APIs. `send_chat_completion()`, `send_chat_completion_with_schema()`, `fetch_models()`. |
| `apple_intelligence.rs` | 84    | Apple Intelligence provider. macOS on Apple Silicon only.                                                               |

The providers are in `settings.rs`, in `PostProcessProvider`.
Default providers: OpenAI, Anthropic, Groq, OpenRouter, Z.AI, and a custom OpenAI-compatible endpoint.
API keys are in a `SecretMap`. The settings store keeps them.

### 3.7 Output

| File                  | Lines | Function                                                                                                                     |
| --------------------- | ----- | ---------------------------------------------------------------------------------------------------------------------------- |
| `clipboard.rs`        | 1,015 | `paste()` is the entry point. It selects one of `paste_via_clipboard()`, `paste_direct()`, or `paste_via_external_script()`. |
| `paste_tx/mod.rs`     | 283   | "Reliable paste". Waits for a receipt before it restores the clipboard.                                                      |
| `paste_tx/macos.rs`   | 336   | macOS reliable paste.                                                                                                        |
| `paste_tx/windows.rs` | 624   | Windows reliable paste.                                                                                                      |
| `input.rs`            | 274   | Sends keystrokes with `enigo`.                                                                                               |
| `secure_input.rs`     | 671   | Detects macOS Secure Event Input. Gives a fallback when a password field blocks the keyboard.                                |

### 3.8 User interface (native side)

| File                   | Lines | Function                                                                         |
| ---------------------- | ----- | -------------------------------------------------------------------------------- |
| `overlay.rs`           | 834   | Creates the overlay window. Platform-specific. On Linux it uses GTK layer shell. |
| `tray.rs`              | 744   | Creates the tray icon and menu. `set_tray_state()` updates the icon.             |
| `tray_i18n.rs`         | 82    | Translates the tray menu.                                                        |
| `audio_feedback.rs`    | 142   | Plays start and stop sounds with `rodio`.                                        |
| `helpers/clamshell.rs` | 86    | Detects a closed laptop lid. Switches the microphone.                            |

### 3.9 Settings and commands

| File                        | Lines | Function                                                                                                                                  |
| --------------------------- | ----- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `settings.rs`               | 1,716 | Defines `AppSettings`. Defines `get_settings()` and `write_settings()`. Each field has a default. A missing field does not stop the load. |
| `commands/mod.rs`           | 196   | General commands. Example: `get_app_settings`, `cancel_operation`, `open_log_dir`.                                                        |
| `commands/models.rs`        | 206   | Model commands. Example: `download_model`, `set_active_model`.                                                                            |
| `commands/audio.rs`         | 380   | Audio device commands.                                                                                                                    |
| `commands/history.rs`       | 154   | History commands.                                                                                                                         |
| `commands/transcription.rs` | 40    | Model load and unload commands.                                                                                                           |

## 4. Frontend map (`src/`)

Total: approximately 15,000 lines of TypeScript.

| Path                                  | Function                                                                                  |
| ------------------------------------- | ----------------------------------------------------------------------------------------- |
| `main.tsx`                            | Mounts the React application.                                                             |
| `App.tsx`                             | Main window. Shows onboarding on the first run.                                           |
| `bindings.ts`                         | Generated file. Do not edit it by hand. Holds `commands`, `events`, and all shared types. |
| `stores/settingsStore.ts`             | Zustand store for settings. Maps each setting key to one backend command.                 |
| `stores/modelStore.ts`                | Zustand store for models.                                                                 |
| `hooks/useSettings.ts`                | Hook that wraps `settingsStore`. Components use `updateSetting(key, value)`.              |
| `components/settings/`                | One component for each setting. Example: `PasteMethod.tsx`.                               |
| `components/model-selector/`          | Model list, download, and selection.                                                      |
| `components/onboarding/`              | First-run screens.                                                                        |
| `components/overlay/` and `overlay/`  | Overlay window. It is a second Vite entry point.                                          |
| `components/update-checker/`          | Update notifications.                                                                     |
| `components/ui/`, `shared/`, `icons/` | Shared UI parts.                                                                          |
| `i18n/`                               | Translations. All user text must use `t('key')`.                                          |
| `lib/`, `utils/`                      | Types, constants, helpers.                                                                |

## 5. How the frontend and the backend talk

- The frontend calls the backend with Tauri commands.
- The backend calls the frontend with Tauri events.
- `tauri-specta` generates `src/bindings.ts` from the Rust types.
- The generation runs in debug builds only. Run `bun run tauri dev` one time to refresh the file.

`lib.rs` registers the commands in `collect_commands![...]`.
`lib.rs` registers the events in `collect_events![...]`.
Current events: `HistoryUpdatePayload`, `StreamTextEvent`, `StreamPhaseEvent`.

## 6. Procedures

### 6.1 Add a setting

1. Open `src-tauri/src/settings.rs`.
2. Add a field to `AppSettings`. Give it `#[serde(default = "default_my_field")]`.
3. Write the `default_my_field()` function.
4. Add the field to the `get_default_settings()` return value.
5. Open `src-tauri/src/shortcut/mod.rs`.
6. Add a `change_my_field_setting` command. Copy `change_paste_delay_ms_setting` as a pattern. Mark it with `#[tauri::command]` and `#[specta::specta]`.
7. Open `src-tauri/src/lib.rs`. Add the command to `collect_commands![...]`.
8. Run `bun run tauri dev` one time. This refreshes `src/bindings.ts`.
9. Open `src/stores/settingsStore.ts`. Map the key to the new command.
10. Add a component in `src/components/settings/`. Use `useSettings()`.
11. Add the text keys to `src/i18n/locales/en/translation.json`.

### 6.2 Add a command

1. Write the function in a file under `src-tauri/src/commands/`.
2. Mark it with `#[tauri::command]` and `#[specta::specta]`.
3. Make sure all argument and return types derive `specta::Type`.
4. Add the function to `collect_commands![...]` in `lib.rs`.
5. Run `bun run tauri dev` one time to refresh `src/bindings.ts`.
6. Call it from the frontend with `commands.myCommand()`.

### 6.3 Add an event

1. Define a struct that derives `Serialize`, `Clone`, `specta::Type`, and `tauri_specta::Event`.
2. Add it to `collect_events![...]` in `lib.rs`.
3. Emit it from Rust with `MyEvent { ... }.emit(&app)`.
4. Listen in the frontend with `events.myEvent.listen(...)`.

### 6.4 Add a post-processing provider

1. Open `src-tauri/src/settings.rs`.
2. Add the provider to the default `PostProcessProvider` list.
3. If the API is not OpenAI-compatible, change `llm_client.rs`.
4. Add the text keys for the provider name to `translation.json`.

### 6.5 Change the pipeline

1. Open `src-tauri/src/actions.rs`.
2. Find `impl ShortcutAction for TranscribeAction`.
3. `start()` begins the recording. `stop()` runs the rest of the pipeline.
4. Keep the `FinishGuard`. It tells the coordinator when the pipeline ends.
5. Check for cancellation with `complete_unless_cancelled()` around long steps.

## 7. Run-time control

| Flag                      | Function                                                               |
| ------------------------- | ---------------------------------------------------------------------- |
| `--toggle-transcription`  | Starts or stops a recording in the running instance.                   |
| `--toggle-post-process`   | Same, with post-processing on.                                         |
| `--cancel`                | Cancels the current operation in the running instance.                 |
| `--start-hidden`          | Starts without the main window.                                        |
| `--no-tray`               | Starts without the tray icon.                                          |
| `--debug`                 | Turns on trace logging.                                                |
| `--transcribe-file <WAV>` | Transcribes one 16 kHz mono WAV file and exits. Does not start the UI. |
| `--model <id>`            | Selects the model for `--transcribe-file`.                             |
| `--device <N>`            | Selects the compute device for `--transcribe-file`.                    |
| `--list-devices`          | Lists the compute devices and exits.                                   |
| `--list-models`           | Lists the models and exits.                                            |
| `--repeat <N>`            | Repeats `--transcribe-file` N times. Reports the fastest run.          |
| `--json`                  | Writes `--transcribe-file` results as JSON.                            |

Handy runs as one instance. A second instance sends its flags to the first instance and exits.
Exception: `--transcribe-file`, `--list-devices`, and `--list-models` run as a separate process.

## 8. Tests

- Rust unit tests are inside the source files. Run `cargo test` in `src-tauri/`.
- `managers/model/download/tests.rs` is the largest test file.
- The frontend has no unit tests. It has Playwright end-to-end tests in `tests/`. Run `bun run test:playwright`.
- CI workflows are in `.github/workflows/`. `test.yml` runs the Rust tests when `src-tauri/**` changes. `code-quality.yml` runs lint and format checks when `src/**` or the scripts change.

## 9. Where to look first

| If you work on...         | Open...                                                                     |
| ------------------------- | --------------------------------------------------------------------------- |
| Recording start or stop   | `transcription_coordinator.rs`, `actions.rs`                                |
| A new ASR model or engine | `managers/model.rs`, `managers/transcription.rs`, `catalog/`                |
| LLM post-processing       | `actions.rs` (`post_process_transcription`), `llm_client.rs`, `settings.rs` |
| Text cleanup              | `audio_toolkit/text.rs`                                                     |
| Paste problems            | `clipboard.rs`, `paste_tx/`, `input.rs`, `secure_input.rs`                  |
| A setting                 | `settings.rs`, `shortcut/mod.rs`, `stores/settingsStore.ts`                 |
| The overlay               | `overlay.rs`, `src/overlay/`                                                |
| The tray                  | `tray.rs`                                                                   |
| Keyboard shortcuts        | `shortcut/`                                                                 |
