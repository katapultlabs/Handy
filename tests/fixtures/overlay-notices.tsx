// Isolated UI fixture: the real overlay, with Tauri IPC and events mocked.
// It never opens the user's Dictionary database or native overlay window.
import React from "react";
import { createRoot } from "react-dom/client";
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import type {
  CorrectionNotice,
  CorrectionNoticeSnapshot,
  DictionaryRow,
} from "../../src/bindings";

type FixtureCall = {
  command: string;
  payload: Record<string, unknown> | undefined;
};

const makeRow = (
  id: number,
  wrong: string,
  right: string,
  autoLearned = false,
): DictionaryRow => ({
  id,
  wrong,
  right,
  case_mode: "exact",
  source: "capture",
  state: autoLearned ? "active" : "proposed",
  enabled: autoLearned,
  auto_learned: autoLearned,
  context_words: autoLearned ? ["release"] : [],
  seen_count: 1,
  created_at: 1,
  updated_at: 1,
});

const searchParams = new URLSearchParams(location.search);
const longPair = searchParams.has("long");
const rows = [
  makeRow(
    1,
    longPair ? "catapult catapult catapult ".repeat(12).trim() : "catapult",
    longPair ? "Katapult Labs Platform ".repeat(12).trim() : "Katapult",
  ),
  makeRow(2, "sense again", "Sensei Gen"),
  makeRow(3, "codexx", "Codex", true),
];

let revision = 1;
let queueIndex = 0;
let tokenEpoch = 0;
let currentSnapshot: CorrectionNoticeSnapshot = makeSnapshot();
let failAction = searchParams.get("fail");
const calls: FixtureCall[] = [];

function currentNotice(): CorrectionNotice | null {
  const entry = rows[queueIndex];
  if (!entry) return null;
  return {
    token: tokenEpoch * 1_000 + 100 + queueIndex,
    entry,
    remaining_ms: 8_000,
    pending_count: rows.length - queueIndex - 1,
    busy: false,
  };
}

function makeSnapshot(recording = false): CorrectionNoticeSnapshot {
  return {
    revision,
    recording,
    notice: currentNotice(),
  };
}

function cloneSnapshot(snapshot = currentSnapshot): CorrectionNoticeSnapshot {
  return structuredClone(snapshot);
}

function updateSnapshot(
  transform?: (snapshot: CorrectionNoticeSnapshot) => void,
): CorrectionNoticeSnapshot {
  revision += 1;
  currentSnapshot = makeSnapshot(currentSnapshot.recording);
  transform?.(currentSnapshot);
  return cloneSnapshot();
}

function advanceQueue(): CorrectionNoticeSnapshot {
  queueIndex += 1;
  return updateSnapshot();
}

async function publish(snapshot: CorrectionNoticeSnapshot) {
  currentSnapshot = cloneSnapshot(snapshot);
  revision = Math.max(revision, snapshot.revision);
  await emit("correction-notice-changed", cloneSnapshot());
}

async function setRecording(recording: boolean) {
  revision += 1;
  if (!recording) tokenEpoch += 1;
  const snapshot: CorrectionNoticeSnapshot = recording
    ? { revision, recording: true, notice: null }
    : makeSnapshot(false);
  await publish(snapshot);
}

async function publishFreshThenStale() {
  const stale = cloneSnapshot();
  const fresh = cloneSnapshot();
  fresh.revision = 100;
  if (fresh.notice) {
    fresh.notice.entry = makeRow(9, "fresh phrase", "Fresh Phrase");
  }
  await publish(fresh);
  stale.revision = 99;
  await emit("correction-notice-changed", stale);
}

Object.assign(window, {
  overlayNoticeFixture: {
    calls,
    publish,
    publishFreshThenStale,
    setRecording,
    snapshot: () => cloneSnapshot(),
    failNext(action: string) {
      failAction = action;
    },
  },
});

mockIPC(
  async (command, payload) => {
    calls.push({ command, payload });
    if (command === "get_app_settings") {
      return { app_language: "en", overlay_position: "bottom" };
    }
    if (command === "get_correction_notice") {
      const replay = cloneSnapshot();
      return replay;
    }
    if (command === "acknowledge_correction_notice") {
      return updateSnapshot();
    }
    if (command === "pause_correction_notice") {
      return updateSnapshot();
    }
    if (command === "dismiss_correction_notice") {
      return advanceQueue();
    }
    if (command === "act_on_correction_notice") {
      const action = String(payload?.action);
      if (failAction === action) {
        failAction = null;
        throw "fixture action failure";
      }
      return advanceQueue();
    }
  },
  { shouldMockEvents: true },
);

await import("../../src/i18n");
const { default: RecordingOverlay } = await import(
  "../../src/overlay/RecordingOverlay"
);
createRoot(document.getElementById("root")!).render(<RecordingOverlay />);
