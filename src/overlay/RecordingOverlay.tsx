import { listen } from "@tauri-apps/api/event";
import React, {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import "./RecordingOverlay.css";
import { commands, events } from "@/bindings";
import type {
  CorrectionNoticeSnapshot,
  StreamPhase,
  StreamPhaseEvent,
  StreamTextEvent,
  StreamWorkKind,
} from "@/bindings";
import i18n, { syncLanguageFromSettings } from "@/i18n";
import type { ModelStateEvent } from "@/lib/types/events";
import { getLanguageDirection } from "@/lib/utils/rtl";

// "learned" is a queued Dictionary correction notice, not a dictation session.
type OverlayState =
  | "recording"
  | "streaming"
  | "transcribing"
  | "processing"
  | "learned";

// Number of reactive bars in the waveform (the simple, smoothed style shared by
// every overlay form). Mic levels arrive as 16 FFT buckets; we take the first N.
const WAVE_BARS = 9;

// Only call out a model load in the Live preview once it has run this long.
// Warm loads finish in well under this (~0.2s on Apple Silicon, ~1.5s on a
// Windows CPU backend), so the common case never flashes a loading notice while
// the user is speaking; cold loads can take 40s+.
const SLOW_MODEL_LOAD_MS = 2000;

const RecordingOverlay: React.FC = () => {
  const { t } = useTranslation();
  const [isVisible, setIsVisible] = useState(false);
  const [state, setState] = useState<OverlayState>("recording");
  // `Stream::play()` returning does not mean hardware callbacks are flowing.
  // Stay visually in an arming state until the backend processes the first
  // actual microphone sample chunk.
  const [captureReady, setCaptureReady] = useState(false);
  // Recording starts while the model loads in the background. Say so, otherwise
  // a cold start looks like an empty Live preview that ignores speech, then a
  // hung "Transcribing..." spinner. Once recording stops, any in-flight load is
  // the wait, so the working label reflects it immediately; the Live notice
  // waits for the load to be slow so fast loads don't flash it mid-speech.
  const [modelLoading, setModelLoading] = useState(false);
  const [modelLoadSlow, setModelLoadSlow] = useState(false);
  const modelLoadTimerRef = useRef<ReturnType<typeof setTimeout>>();
  // Latched per Live session: once the loading notice has opened the panel, keep
  // it open after the load so it doesn't collapse and reopen when text arrives.
  const [loadNoticeShown, setLoadNoticeShown] = useState(false);
  const [levels, setLevels] = useState<number[]>(Array(WAVE_BARS).fill(0));
  const [streamText, setStreamText] = useState<StreamTextEvent>({
    committed: "",
    tentative: "",
  });
  const [phase, setPhase] = useState<StreamPhase>("listening");
  const [workKind, setWorkKind] = useState<StreamWorkKind>("transcribing");
  const [elapsed, setElapsed] = useState(0);
  // Bumped on each new streaming session so the Live card remounts fresh (replays
  // the pop-in, and never animates in from the previous panel's open size).
  const [session, setSession] = useState(0);
  // Overlay placement (top vs bottom of the screen). The Live panel grows downward
  // from a top overlay (oldest line under the pill) and upward from a bottom one.
  const [position, setPosition] = useState<"top" | "bottom">("bottom");
  // True once live text overflows the cap. A top overlay fades its top edge only
  // while overflowing, so the resting first line stays crisp flush under the pill.
  const [overflowing, setOverflowing] = useState(false);
  const [correctionSnapshot, setCorrectionSnapshot] =
    useState<CorrectionNoticeSnapshot | null>(null);
  const [noticeError, setNoticeError] = useState<{
    token: number;
    message: string;
  } | null>(null);
  const [hoveringNotice, setHoveringNotice] = useState(false);
  const [focusWithinNotice, setFocusWithinNotice] = useState(false);
  const [actionPendingToken, setActionPendingToken] = useState<number | null>(
    null,
  );

  const smoothedLevelsRef = useRef<number[]>(Array(16).fill(0));
  const overlayStateRef = useRef<OverlayState>("recording");
  const correctionSnapshotRef = useRef<CorrectionNoticeSnapshot | null>(null);
  const latestNoticeRevisionRef = useRef(-1);
  const presentationSequenceRef = useRef(0);
  const acknowledgedTokenRef = useRef<number | null>(null);
  const pauseSentRef = useRef<{ token: number; paused: boolean } | null>(null);
  // Live-text scroll-back: the text region "sticks" to the newest line while the
  // user is at the bottom; if they scroll up to read history, auto-follow pauses
  // until they scroll back down.
  const capRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);
  const direction = getLanguageDirection(i18n.language);

  const applyCorrectionSnapshot = useCallback(
    (snapshot: CorrectionNoticeSnapshot) => {
      if (snapshot.revision <= latestNoticeRevisionRef.current) return false;

      const previous = correctionSnapshotRef.current;
      const wasNoticeVisible = Boolean(
        previous && !previous.recording && previous.notice,
      );
      const previousToken = previous?.notice?.token ?? null;
      const nextToken = snapshot.notice?.token ?? null;
      const noticeIsVisible = !snapshot.recording && snapshot.notice !== null;
      const presentationChanged =
        noticeIsVisible !== wasNoticeVisible ||
        (noticeIsVisible && nextToken !== previousToken);

      latestNoticeRevisionRef.current = snapshot.revision;
      correctionSnapshotRef.current = snapshot;
      setCorrectionSnapshot(snapshot);

      if (noticeIsVisible) {
        if (presentationChanged) presentationSequenceRef.current += 1;
        overlayStateRef.current = "learned";
        setState("learned");
        setIsVisible(true);
      } else {
        setHoveringNotice(false);
        setFocusWithinNotice(false);
        if (overlayStateRef.current === "learned") {
          presentationSequenceRef.current += 1;
          setIsVisible(false);
        }
      }

      setActionPendingToken((token) =>
        token !== null && token !== nextToken ? null : token,
      );
      return presentationChanged && noticeIsVisible;
    },
    [],
  );

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];

    const refreshPresentationPreferences = async (sequence: number) => {
      const [, settingsResult] = await Promise.allSettled([
        syncLanguageFromSettings(),
        commands.getAppSettings(),
      ]);
      if (
        disposed ||
        sequence !== presentationSequenceRef.current ||
        settingsResult.status !== "fulfilled" ||
        settingsResult.value.status !== "ok"
      ) {
        return;
      }
      setPosition(
        settingsResult.value.data.overlay_position === "top" ? "top" : "bottom",
      );
    };

    const setupEventListeners = async () => {
      const register = async (listener: Promise<() => void>) => {
        try {
          const unlisten = await listener;
          if (disposed) {
            unlisten();
            return false;
          }
          unlisteners.push(unlisten);
          return true;
        } catch {
          return false;
        }
      };

      // Register the notice event first. Only then replay current state, so an
      // event that races the fetch wins through the monotonic revision check.
      const correctionRegistered = await register(
        listen<CorrectionNoticeSnapshot>(
          "correction-notice-changed",
          (event) => {
            if (disposed) return;
            const shown = applyCorrectionSnapshot(event.payload);
            if (shown) {
              void refreshPresentationPreferences(
                presentationSequenceRef.current,
              );
            }
          },
        ),
      );
      if (disposed) return;

      await Promise.all([
        register(
          listen("show-overlay", (event) => {
            const overlayState = event.payload as OverlayState;
            const sequence = ++presentationSequenceRef.current;
            overlayStateRef.current = overlayState;
            setState(overlayState);
            setIsVisible(true);

            // Reset synchronously before settings I/O. A fast microphone can emit
            // recording-ready while the awaits below are in flight; resetting after
            // them would overwrite that event and leave the overlay stuck arming.
            if (overlayState === "recording" || overlayState === "streaming") {
              setCaptureReady(false);
              smoothedLevelsRef.current = Array(16).fill(0);
              setLevels(Array(WAVE_BARS).fill(0));
              setStreamText({ committed: "", tentative: "" });
            }
            if (overlayState === "streaming") {
              setPhase("listening");
              setWorkKind("transcribing");
              setElapsed(0);
              setLoadNoticeShown(false);
              setSession((s) => s + 1); // remount the card fresh for this session
            }
            void refreshPresentationPreferences(sequence);
          }),
        ),
        register(
          listen("hide-overlay", () => {
            const snapshot = correctionSnapshotRef.current;
            if (snapshot?.notice && !snapshot.recording) return;
            presentationSequenceRef.current += 1;
            setIsVisible(false);
            setCaptureReady(false);
          }),
        ),
        register(
          listen("recording-ready", () => {
            setElapsed(0);
            setCaptureReady(true);
          }),
        ),
        register(
          listen<number[]>("mic-level", (event) => {
            const newLevels = event.payload as number[];
            // Exponential smoothing across the 16 buckets, then take the first N
            // bars for the shared waveform.
            const smoothed = smoothedLevelsRef.current.map((prev, i) => {
              const target = newLevels[i] || 0;
              return prev * 0.7 + target * 0.3;
            });
            smoothedLevelsRef.current = smoothed;
            setLevels(smoothed.slice(0, WAVE_BARS));
          }),
        ),
        register(
          events.streamTextEvent.listen((event) => {
            setStreamText(event.payload);
          }),
        ),
        register(
          events.streamPhaseEvent.listen((event) => {
            const payload: StreamPhaseEvent = event.payload;
            setPhase(payload.phase);
            if (payload.kind) setWorkKind(payload.kind);
          }),
        ),
        // The backend ends every `loading_started` with exactly one of completed
        // or failed, so only those end a load. Other events (e.g. `unloaded` from
        // the idle watcher) aren't ordered against an in-flight load and must not
        // clear it.
        register(
          listen<ModelStateEvent>("model-state-changed", (event) => {
            const type = event.payload.event_type;
            if (type === "loading_started") {
              clearTimeout(modelLoadTimerRef.current);
              setModelLoading(true);
              setModelLoadSlow(false);
              modelLoadTimerRef.current = setTimeout(
                () => setModelLoadSlow(true),
                SLOW_MODEL_LOAD_MS,
              );
            } else if (
              type === "loading_completed" ||
              type === "loading_failed"
            ) {
              clearTimeout(modelLoadTimerRef.current);
              setModelLoading(false);
              setModelLoadSlow(false);
            }
          }),
        ),
      ]);

      if (disposed || !correctionRegistered) return;

      try {
        const result = await commands.getCorrectionNotice();
        if (!disposed && result.status === "ok") {
          const shown = applyCorrectionSnapshot(result.data);
          if (shown) {
            void refreshPresentationPreferences(
              presentationSequenceRef.current,
            );
          }
        }
      } catch {
        // A future notice event can still recover the overlay.
      }
    };

    void setupEventListeners();
    return () => {
      disposed = true;
      presentationSequenceRef.current += 1;
      unlisteners.splice(0).forEach((unlisten) => unlisten());
      clearTimeout(modelLoadTimerRef.current);
    };
  }, [applyCorrectionSnapshot]);

  const visibleNotice =
    correctionSnapshot && !correctionSnapshot.recording
      ? correctionSnapshot.notice
      : null;
  const noticeBusy = Boolean(
    visibleNotice &&
      (visibleNotice.busy || actionPendingToken === visibleNotice.token),
  );

  // A notice becomes eligible for expiry only after React has painted it. The
  // token ref also prevents StrictMode and later snapshot revisions from
  // acknowledging the same notice more than once in this webview lifetime.
  useEffect(() => {
    if (!isVisible || state !== "learned" || !visibleNotice) return;
    if (acknowledgedTokenRef.current === visibleNotice.token) return;

    const token = visibleNotice.token;
    const frame = requestAnimationFrame(() => {
      acknowledgedTokenRef.current = token;
      void commands
        .acknowledgeCorrectionNotice(token)
        .then((result) => {
          if (result.status === "ok") {
            applyCorrectionSnapshot(result.data);
          } else {
            setNoticeError({
              token,
              message: t("settings.dictionary.actionError"),
            });
          }
        })
        .catch(() => {
          setNoticeError({
            token,
            message: t("settings.dictionary.actionError"),
          });
        });
    });
    return () => cancelAnimationFrame(frame);
  }, [applyCorrectionSnapshot, isVisible, state, t, visibleNotice]);

  const shouldPauseNotice =
    hoveringNotice || focusWithinNotice || actionPendingToken !== null;

  useEffect(() => {
    if (!visibleNotice) return;

    const token = visibleNotice.token;
    const last = pauseSentRef.current;
    if (!shouldPauseNotice && (!last || last.token !== token)) {
      pauseSentRef.current = { token, paused: false };
      return;
    }
    if (last?.token === token && last.paused === shouldPauseNotice) return;

    pauseSentRef.current = { token, paused: shouldPauseNotice };
    void commands
      .pauseCorrectionNotice(token, shouldPauseNotice)
      .then((result) => {
        if (result.status === "ok") {
          applyCorrectionSnapshot(result.data);
        } else {
          setNoticeError({
            token,
            message: t("settings.dictionary.actionError"),
          });
        }
      })
      .catch(() => {
        setNoticeError({
          token,
          message: t("settings.dictionary.actionError"),
        });
      });
  }, [applyCorrectionSnapshot, shouldPauseNotice, t, visibleNotice]);

  const runNoticeAction = async (
    token: number,
    action: "accept" | "reject" | "dismiss",
  ) => {
    setNoticeError(null);
    setActionPendingToken(token);
    try {
      const result =
        action === "dismiss"
          ? await commands.dismissCorrectionNotice(token)
          : await commands.actOnCorrectionNotice(token, action);
      if (result.status === "ok") {
        applyCorrectionSnapshot(result.data);
      } else {
        setNoticeError({
          token,
          message: t("settings.dictionary.actionError"),
        });
      }
    } catch {
      setNoticeError({
        token,
        message: t("settings.dictionary.actionError"),
      });
    } finally {
      setActionPendingToken((pendingToken) =>
        pendingToken === token ? null : pendingToken,
      );
    }
  };

  // Elapsed capture timer starts only once microphone samples are flowing.
  useEffect(() => {
    if (state !== "streaming" || !isVisible || !captureReady) return;
    const id = setInterval(() => setElapsed((e) => e + 1), 1000);
    return () => clearInterval(id);
  }, [state, isVisible, captureReady]);

  // Stick to the bottom as text streams in — but only while pinned, so a user who
  // has scrolled up to read history isn't yanked back down by the next chunk.
  useLayoutEffect(() => {
    const el = capRef.current;
    if (!el) return;
    // Fade the top edge only once text actually overflows the cap.
    setOverflowing(el.scrollHeight > el.clientHeight + 1);
    if (pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, [streamText]);

  // `session` re-runs this when a new Live session starts mid-load (e.g. a
  // retry during a cold load), since the reset doesn't change the other deps.
  useEffect(() => {
    if (modelLoadSlow && state === "streaming") setLoadNoticeShown(true);
  }, [modelLoadSlow, state, session]);

  // Each fresh streaming session starts pinned to the bottom, fade cleared.
  useEffect(() => {
    pinnedRef.current = true;
    setOverflowing(false);
  }, [session]);

  if (!isVisible) return null;

  // Re-pin when the user is within ~a line of the bottom; unpin otherwise.
  const handleStreamScroll = () => {
    const el = capRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 16;
  };

  const fmtTime = (s: number) =>
    `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;

  const transcribingLabel = modelLoading
    ? t("overlay.loadingModel")
    : t("overlay.transcribing");

  // ---- Shared building blocks (one visual language for every overlay form) ----
  const waveform = (
    <div className={`swave ${captureReady ? "ready" : "arming"}`}>
      {levels.map((v, i) => (
        <i
          key={i}
          style={{
            height: `${Math.max(3, Math.min(18, 3 + Math.pow(v, 0.7) * 15))}px`,
          }}
        />
      ))}
    </div>
  );

  const cancelBtn = (
    <button
      className="sx"
      aria-label="cancel"
      onClick={() => commands.cancelOperation()}
    >
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <path
          d="M4 4 L12 12 M12 4 L4 12"
          stroke="currentColor"
          strokeWidth="1.6"
          strokeLinecap="round"
        />
      </svg>
    </button>
  );

  // dot (left) | waveform (center) | timer + cancel (right) — same structure for
  // pill & panel, so the Live morph is a pure width change.
  const listeningRow = (showTimer: boolean, showCancel: boolean) => (
    <div className="sbase">
      <div className="sbase-l">
        <span className={`sdot ${captureReady ? "ready" : "arming"}`} />
      </div>
      {waveform}
      <div className="sbase-r">
        {showTimer && <span className="stimer">{fmtTime(elapsed)}</span>}
        {showCancel && cancelBtn}
      </div>
    </div>
  );

  // spinner (left) | label (center) | cancel (right) — same 3-zone grid as the
  // listening row, so the label is centered.
  const workingRow = (label: string, showCancel: boolean) => (
    <div className="sbase">
      <div className="sbase-l">
        <span className="sspinner" />
      </div>
      <span className="swork-label">{label}</span>
      <div className="sbase-r">{showCancel && cancelBtn}</div>
    </div>
  );

  // ---- Live overlay: a pill that sculpts open into a panel ----
  if (state === "streaming") {
    const hasText =
      streamText.committed.length > 0 || streamText.tentative.length > 0;
    const working = phase === "working";
    // While listening, a slow model load opens the panel with a notice where the
    // text would be, so speech that isn't transcribing yet doesn't look ignored.
    const loadingNotice = modelLoadSlow && !working && !hasText;
    // Keep the panel open whenever there's text — even while finalizing — so the
    // transcript stays put under a working spinner instead of collapsing and
    // squishing the text mid-stream. Only fall back to the small working pill
    // when there was no text to preserve.
    const open = hasText || loadingNotice || (loadNoticeShown && !working);
    const collapsed = working && !hasText;

    return (
      <div dir={direction} className={`ov-stage ${position}`}>
        <div
          key={session}
          className={`scard ${open ? "open" : ""} ${collapsed ? "working" : ""} ${
            isVisible ? "" : "leaving"
          }`}
        >
          <div className="stext">
            <div className="stext-clip">
              <div
                className={`stext-cap ${overflowing ? "overflowing" : ""}`}
                ref={capRef}
                onScroll={handleStreamScroll}
              >
                <p>
                  {loadingNotice && (
                    <span className="sloading">
                      {t("overlay.loadingModel")}
                    </span>
                  )}
                  <span className="committed">
                    {streamText.committed ? streamText.committed + " " : ""}
                  </span>
                  <span className="tentative">{streamText.tentative}</span>
                  {/* Drop the blinking caret once finalizing — it's no longer
                      capturing, and a static spinner conveys the work. */}
                  {!working && !loadingNotice && <span className="scaret" />}
                </p>
              </div>
            </div>
          </div>
          {working
            ? workingRow(
                workKind === "polishing"
                  ? t("overlay.processing")
                  : transcribingLabel,
                true,
              )
            : listeningRow(open, true)}
        </div>
      </div>
    );
  }

  // ---- Reliable correction notice. The backend owns the queue, countdown,
  // and token validation; this window only presents the current snapshot.
  if (state === "learned") {
    if (!visibleNotice) return null;
    const { entry, pending_count: pendingCount, token } = visibleNotice;
    const automatic = entry.auto_learned;
    const label = t(
      automatic ? "overlay.autoLearned" : "overlay.correctionQuestion",
      { wrong: entry.wrong, right: entry.right },
    );
    const error = noticeError?.token === token ? noticeError.message : null;

    return (
      <div dir={direction} className={`ov-stage ${position} ov-fade show`}>
        <div
          className="correction-card"
          onMouseEnter={() => setHoveringNotice(true)}
          onMouseLeave={() => setHoveringNotice(false)}
          onFocus={() => setFocusWithinNotice(true)}
          onBlur={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget)) {
              setFocusWithinNotice(false);
            }
          }}
        >
          <div className="correction-copy" tabIndex={0}>
            <div className="correction-title-row">
              <span className="scheck" aria-hidden="true">
                <svg viewBox="0 0 16 16">
                  <path
                    d="M3.5 8.5 L6.5 11.5 L12.5 5"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="1.8"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  />
                </svg>
              </span>
              <span className="correction-label">{label}</span>
              {pendingCount > 0 ? (
                <span className="correction-remaining">
                  {t("overlay.correctionRemaining", { count: pendingCount })}
                </span>
              ) : null}
            </div>
            {error ? (
              <div className="correction-error" role="alert">
                {error}
              </div>
            ) : null}
          </div>
          <div className="correction-actions">
            {automatic ? (
              <button
                className="correction-button primary"
                disabled={noticeBusy}
                onClick={() => void runNoticeAction(token, "reject")}
              >
                {t("overlay.autoUndo")}
              </button>
            ) : (
              <>
                <button
                  className="correction-button primary"
                  disabled={noticeBusy}
                  onClick={() => void runNoticeAction(token, "accept")}
                >
                  {t("settings.dictionary.approve")}
                </button>
                <button
                  className="correction-button"
                  disabled={noticeBusy}
                  onClick={() => void runNoticeAction(token, "reject")}
                >
                  {t("settings.dictionary.ignore")}
                </button>
              </>
            )}
            {pendingCount > 0 ? (
              <button
                className="correction-button next"
                disabled={noticeBusy}
                onClick={() => void runNoticeAction(token, "dismiss")}
              >
                {t("overlay.correctionNext")}
              </button>
            ) : (
              <button
                className="correction-button dismiss"
                aria-label={t("secureInput.dismiss")}
                disabled={noticeBusy}
                onClick={() => void runNoticeAction(token, "dismiss")}
              >
                <span aria-hidden="true">×</span>
              </button>
            )}
          </div>
        </div>
      </div>
    );
  }

  // ---- Minimal overlay: exactly one row at a time — waveform (recording), or a
  // spinner + label (transcribing / processing). Never both. The pill animates its
  // width between them; the cancel button is in both rows so it stays put.
  const working = state === "transcribing" || state === "processing";
  const workLabel =
    state === "processing" ? t("overlay.processing") : transcribingLabel;

  return (
    <div
      dir={direction}
      className={`ov-stage ${position} ov-fade ${isVisible ? "show" : ""}`}
    >
      <div
        className={`scard compact ${working && isVisible ? "cworking" : ""}`}
      >
        {working ? workingRow(workLabel, true) : listeningRow(false, true)}
      </div>
    </div>
  );
};

export default RecordingOverlay;
