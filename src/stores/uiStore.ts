import { create } from "zustand";

/**
 * Small cross-component UI signals that do not belong in settings.
 *
 * `correctLatestPending`: set when the tray asks to correct the latest
 * transcription. HistorySettings consumes it and opens the newest entry in
 * edit mode.
 */
interface UiState {
  correctLatestPending: boolean;
  requestCorrectLatest: () => void;
  clearCorrectLatest: () => void;
}

export const useUiStore = create<UiState>((set) => ({
  correctLatestPending: false,
  requestCorrectLatest: () => set({ correctLatestPending: true }),
  clearCorrectLatest: () => set({ correctLatestPending: false }),
}));
