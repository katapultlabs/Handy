// Isolated UI fixture: the real Dictionary component, with Tauri IPC mocked.
// The fixture never opens the user's database or changes their settings.
import React from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { DictionaryRow } from "../../src/bindings";
import "../../src/App.css";

const rows: DictionaryRow[] = [
  [1, "sense again", "sense", "capture"],
  [2, "catapult", "Katapult", "history"],
  [3, "enabled", "end-to-end", "capture"],
].map(([id, wrong, right, source]) => ({
  id: Number(id),
  wrong: String(wrong),
  right: String(right),
  source: String(source),
  state: "proposed",
  enabled: true,
  case_mode: "exact",
  auto_learned: false,
  context_words: [],
  seen_count: 1,
  created_at: 1,
  updated_at: 1,
}));

if (new URLSearchParams(location.search).has("automatic")) {
  rows.push({
    ...rows[1],
    id: 4,
    wrong: "katapolt",
    source: "capture",
    state: "active",
    auto_learned: true,
    context_words: ["releases"],
  });
}

const calls: string[] = [];
Object.assign(window, { dictionaryCalls: calls });
mockIPC(
  (command, payload) => {
    calls.push(command);
    if (command === "get_app_settings") return { app_language: "en" };
    if (command === "list_dictionary_entries") return structuredClone(rows);
    if (
      command === "confirm_dictionary_entry" ||
      command === "reject_dictionary_entry"
    ) {
      const row = rows.find((row) => row.id === payload?.id)!;
      row.state =
        command === "confirm_dictionary_entry" ? "active" : "rejected";
      row.enabled = command === "confirm_dictionary_entry";
      if (command === "confirm_dictionary_entry") {
        row.auto_learned = false;
        row.context_words = [];
      }
      return structuredClone(row);
    }
    if (command === "update_dictionary_entry") {
      const row = rows.find((row) => row.id === payload?.id)!;
      row.wrong = String(payload?.wrong);
      row.right = String(payload?.right);
      row.enabled = Boolean(payload?.active);
      return structuredClone(row);
    }
    if (command === "add_dictionary_entry") {
      const row: DictionaryRow = {
        id: 10,
        wrong: String(payload?.wrong),
        right: String(payload?.right),
        source: "manual",
        state: "active",
        enabled: true,
        case_mode: "exact",
        auto_learned: false,
        context_words: [],
        seen_count: 1,
        created_at: 1,
        updated_at: 1,
      };
      rows.push(row);
      return structuredClone(row);
    }
  },
  { shouldMockEvents: true },
);

const { useSettingsStore } = await import("../../src/stores/settingsStore");
useSettingsStore.setState({ isLoading: false });
await import("../../src/i18n");
const { DictionarySettings } = await import(
  "../../src/components/settings/dictionary/DictionarySettings"
);
createRoot(document.getElementById("root")!).render(
  <main className="min-h-screen bg-background text-text p-8">
    <DictionarySettings />
  </main>,
);
