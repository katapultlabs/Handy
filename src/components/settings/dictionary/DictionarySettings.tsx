import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { Check, Pencil, Search, Trash2, X } from "lucide-react";
import { toast } from "sonner";
import { commands, type DictionaryRow } from "@/bindings";
import { Button } from "../../ui/Button";
import { Input } from "../../ui/Input";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { DictionaryCaptureToggle } from "../Dictionary";

// JSX literal strings are disallowed by the i18n lint rule; the arrow is
// punctuation, not translatable copy.
const ARROW = "→";

const sanitize = (s: string) =>
  s.replace(/[<>]/g, "").replace(/\s+/g, " ").trim();

const SOURCE_KEYS: Record<string, string> = {
  manual: "settings.dictionary.source.manual",
  history: "settings.dictionary.source.history",
  capture: "settings.dictionary.source.capture",
};

const isActive = (row: DictionaryRow) => row.state === "active" && row.enabled;

/**
 * Top-level Dictionary section. Shown in the sidebar once the Dictionary is
 * turned on under Advanced -> Experimental. Entries live in SQLite; every
 * change goes through a command, and the backend's `dictionary-entries-changed`
 * event tells this page to reload.
 */
export const DictionarySettings: React.FC = () => {
  const { t } = useTranslation();
  const [rows, setRows] = useState<DictionaryRow[]>([]);
  const [busy, setBusy] = useState(false);
  const [query, setQuery] = useState("");
  const [wrong, setWrong] = useState("");
  const [right, setRight] = useState("");
  // Inline edit: the row being edited and its draft texts.
  const [editingId, setEditingId] = useState<number | null>(null);
  const [draftWrong, setDraftWrong] = useState("");
  const [draftRight, setDraftRight] = useState("");

  const reload = useCallback(async () => {
    const result = await commands.listDictionaryEntries();
    if (result.status === "ok") {
      setRows(result.data);
    } else {
      toast.error(t("settings.dictionary.actionError"));
    }
  }, [t]);

  useEffect(() => {
    reload();
    const unlisten = listen("dictionary-entries-changed", () => {
      reload();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [reload]);

  const wrongClean = sanitize(wrong);
  const rightClean = sanitize(right);
  const canAdd =
    wrongClean.length > 0 &&
    rightClean.length > 0 &&
    wrongClean.length <= 80 &&
    rightClean.length <= 80 &&
    !busy;

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return rows;
    return rows.filter(
      (e) =>
        e.wrong.toLowerCase().includes(q) || e.right.toLowerCase().includes(q),
    );
  }, [rows, query]);

  const suggested = visible.filter((row) => row.state === "proposed");
  const approved = visible.filter((row) => row.state === "active");
  const ignored = visible.filter((row) => row.state === "rejected");

  const run = async (
    action: () => Promise<
      { status: "ok" } | { status: "error"; error: string }
    >,
    onError?: (error: string) => void,
  ) => {
    setBusy(true);
    try {
      const result = await action();
      if (result.status === "error") {
        if (onError) onError(result.error);
        else toast.error(t("settings.dictionary.actionError"));
        return false;
      }
      await reload();
      return true;
    } catch (error) {
      console.error("Dictionary command failed:", error);
      toast.error(t("settings.dictionary.actionError"));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const handleAdd = async () => {
    if (!canAdd) return;
    const ok = await run(
      () => commands.addDictionaryEntry(wrongClean, rightClean),
      (error) => {
        if (error === "duplicate") {
          toast.error(
            t("settings.advanced.dictionary.duplicate", { wrong: wrongClean }),
          );
        } else {
          toast.error(t("settings.dictionary.actionError"));
        }
      },
    );
    if (ok) {
      setWrong("");
      setRight("");
    }
  };

  const handleApprove = (row: DictionaryRow) =>
    run(() => commands.confirmDictionaryEntry(row.id));

  const handleIgnore = (row: DictionaryRow) =>
    run(() => commands.rejectDictionaryEntry(row.id));

  const handleRemove = (row: DictionaryRow) =>
    run(() => commands.deleteDictionaryEntry(row.id));

  const handleToggleActive = (row: DictionaryRow, active: boolean) =>
    run(() =>
      commands.updateDictionaryEntry(
        row.id,
        row.wrong,
        row.right,
        row.case_mode,
        active,
      ),
    );

  const startEdit = (row: DictionaryRow) => {
    setEditingId(row.id);
    setDraftWrong(row.wrong);
    setDraftRight(row.right);
  };

  const cancelEdit = () => setEditingId(null);

  const saveEdit = async (row: DictionaryRow) => {
    const w = sanitize(draftWrong);
    const r = sanitize(draftRight);
    if (!w || !r) return;
    const ok = await run(
      () =>
        commands.updateDictionaryEntry(
          row.id,
          w,
          r,
          row.case_mode,
          isActive(row),
        ),
      (error) => {
        if (error === "duplicate") {
          toast.error(
            t("settings.advanced.dictionary.duplicate", { wrong: w }),
          );
        } else {
          toast.error(t("settings.dictionary.actionError"));
        }
      },
    );
    if (ok) setEditingId(null);
  };

  const onAddKey = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      handleAdd();
    }
  };

  const onEditKey = (row: DictionaryRow) => (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      saveEdit(row);
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancelEdit();
    }
  };

  const iconButton =
    "shrink-0 p-1.5 rounded-md text-text/50 hover:text-logo-primary transition-colors cursor-pointer disabled:cursor-not-allowed disabled:text-text/20";

  const renderRow = (row: DictionaryRow) => {
    const approved = row.state === "active";
    const editing = editingId === row.id;
    return (
      <div
        key={row.id}
        className="px-4 py-3 flex flex-wrap items-center gap-3 text-sm"
      >
        {approved && (
          <input
            type="checkbox"
            className="shrink-0 cursor-pointer accent-logo-primary"
            checked={isActive(row)}
            disabled={busy}
            onChange={(e) => handleToggleActive(row, e.target.checked)}
            title={t("settings.dictionary.active")}
            aria-label={t("settings.dictionary.active")}
          />
        )}
        {editing ? (
          <div className="min-w-0 flex-1 flex items-center gap-2">
            <Input
              type="text"
              value={draftWrong}
              onChange={(e) => setDraftWrong(e.target.value)}
              onKeyDown={onEditKey(row)}
              aria-label={t("settings.advanced.dictionary.wrongPlaceholder")}
              variant="compact"
              disabled={busy}
              autoFocus
            />
            <span className="text-text/40">{ARROW}</span>
            <Input
              type="text"
              value={draftRight}
              onChange={(e) => setDraftRight(e.target.value)}
              onKeyDown={onEditKey(row)}
              aria-label={t("settings.advanced.dictionary.rightPlaceholder")}
              variant="compact"
              disabled={busy}
            />
          </div>
        ) : (
          <span
            className={`min-w-0 flex-1 break-words ${approved && !row.enabled ? "opacity-50" : ""}`}
          >
            <span className="text-text/60">{row.wrong}</span>
            <span className="text-text/40 px-2">{ARROW}</span>
            <span className="font-medium">{row.right}</span>
          </span>
        )}
        <span className="shrink-0 text-xs text-text/50 rounded-md border border-mid-gray/20 px-1.5 py-0.5">
          {t(SOURCE_KEYS[row.source] ?? SOURCE_KEYS.manual)}
        </span>
        {row.auto_learned && row.state === "active" && (
          <span
            className="shrink-0 text-xs text-text/50"
            title={t("settings.dictionary.automaticHint")}
          >
            {t("settings.dictionary.automatic")}
          </span>
        )}
        {row.seen_count > 1 && (
          <span className="shrink-0 text-xs text-text/40">
            {t("settings.dictionary.seen", { count: row.seen_count })}
          </span>
        )}
        <div className="flex items-center gap-1 ms-auto">
          {editing ? (
            <>
              <button
                onClick={() => saveEdit(row)}
                disabled={
                  busy || !sanitize(draftWrong) || !sanitize(draftRight)
                }
                className={iconButton}
                title={t("settings.dictionary.save")}
                aria-label={t("settings.dictionary.save")}
              >
                <Check width={16} height={16} />
              </button>
              <button
                onClick={cancelEdit}
                disabled={busy}
                className={iconButton}
                title={t("settings.dictionary.cancel")}
                aria-label={t("settings.dictionary.cancel")}
              >
                <X width={16} height={16} />
              </button>
            </>
          ) : (
            <>
              {row.state !== "rejected" && (
                <button
                  onClick={() => startEdit(row)}
                  disabled={busy}
                  className={iconButton}
                  title={t("settings.dictionary.edit")}
                  aria-label={t("settings.dictionary.edit")}
                >
                  <Pencil width={16} height={16} />
                </button>
              )}
              {!approved && (
                <Button
                  onClick={() => handleApprove(row)}
                  disabled={busy}
                  variant="primary-soft"
                  size="sm"
                >
                  {t("settings.dictionary.approve")}
                </Button>
              )}
              {row.state !== "rejected" &&
                (approved && row.source === "manual" ? (
                  <button
                    onClick={() => handleRemove(row)}
                    disabled={busy}
                    className={iconButton}
                    title={t("settings.advanced.dictionary.remove", row)}
                    aria-label={t("settings.advanced.dictionary.remove", row)}
                  >
                    <Trash2 width={16} height={16} />
                  </button>
                ) : (
                  <Button
                    onClick={() => handleIgnore(row)}
                    disabled={busy}
                    variant="ghost"
                    size="sm"
                  >
                    {t(
                      row.auto_learned && approved
                        ? "settings.dictionary.autoUndo"
                        : "settings.dictionary.ignore",
                    )}
                  </Button>
                ))}
            </>
          )}
        </div>
      </div>
    );
  };

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("settings.dictionary.learningGroup")}>
        <DictionaryCaptureToggle descriptionMode="tooltip" grouped={true} />
      </SettingsGroup>

      <div className="flex items-center gap-2 px-1 text-text/50">
        <Search width={16} height={16} aria-hidden="true" />
        <Input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("settings.dictionary.search")}
          aria-label={t("settings.dictionary.search")}
          variant="compact"
        />
      </div>

      <SettingsGroup
        title={t("settings.dictionary.suggestedGroup", {
          count: suggested.length,
        })}
        description={t("settings.dictionary.suggestedDescription")}
      >
        {suggested.length ? (
          suggested.map(renderRow)
        ) : (
          <p className="px-4 py-5 text-sm text-text/60">
            {t(
              query
                ? "settings.dictionary.noMatch"
                : "settings.dictionary.noSuggestions",
            )}
          </p>
        )}
      </SettingsGroup>

      <SettingsGroup
        title={t("settings.dictionary.entriesGroup", {
          count: approved.length,
        })}
        description={t("settings.advanced.dictionary.description")}
      >
        {approved.length ? (
          approved.map(renderRow)
        ) : (
          <p className="px-4 py-5 text-sm text-text/60">
            {t(
              query
                ? "settings.dictionary.noMatch"
                : "settings.dictionary.empty",
            )}
          </p>
        )}
      </SettingsGroup>

      <SettingsGroup title={t("settings.dictionary.addTitle")}>
        <div className="px-4 py-3 flex flex-wrap items-center gap-2">
          <Input
            type="text"
            className="min-w-24 flex-1"
            value={wrong}
            onChange={(e) => setWrong(e.target.value)}
            onKeyDown={onAddKey}
            placeholder={t("settings.advanced.dictionary.wrongPlaceholder")}
            aria-label={t("settings.advanced.dictionary.wrongPlaceholder")}
            variant="compact"
            disabled={busy}
          />
          <span className="text-text/50">{ARROW}</span>
          <Input
            type="text"
            className="min-w-24 flex-1"
            value={right}
            onChange={(e) => setRight(e.target.value)}
            onKeyDown={onAddKey}
            placeholder={t("settings.advanced.dictionary.rightPlaceholder")}
            aria-label={t("settings.advanced.dictionary.rightPlaceholder")}
            variant="compact"
            disabled={busy}
          />
          <Button
            onClick={handleAdd}
            disabled={!canAdd}
            variant="primary"
            size="md"
          >
            {t("settings.advanced.dictionary.add")}
          </Button>
        </div>
      </SettingsGroup>

      {ignored.length > 0 && (
        <details className="text-sm">
          <summary className="cursor-pointer px-4 py-2 text-text/60">
            {t("settings.dictionary.ignoredGroup", { count: ignored.length })}
          </summary>
          <p className="px-4 py-2 text-xs text-text/60">
            {t("settings.dictionary.ignoredDescription")}
          </p>
          <div className="border border-mid-gray/20 rounded-lg divide-y divide-mid-gray/20">
            {ignored.map(renderRow)}
          </div>
        </details>
      )}
    </div>
  );
};
