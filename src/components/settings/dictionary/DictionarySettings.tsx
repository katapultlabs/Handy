import React, { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Search, Trash2 } from "lucide-react";
import { toast } from "sonner";
import type { DictionaryEntry } from "@/bindings";
import { useSettings } from "../../../hooks/useSettings";
import { Button } from "../../ui/Button";
import { Input } from "../../ui/Input";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { DictionaryCaptureToggle } from "../Dictionary";

// JSX literal strings are disallowed by the i18n lint rule; the arrow is
// punctuation, not translatable copy.
const ARROW = "→";

const sanitize = (s: string) =>
  s.replace(/[<>]/g, "").replace(/\s+/g, " ").trim();

const sameEntry = (a: DictionaryEntry, b: DictionaryEntry) =>
  a.wrong.toLowerCase() === b.wrong.toLowerCase() && a.right === b.right;

const SOURCE_KEYS: Record<string, string> = {
  manual: "settings.dictionary.source.manual",
  history: "settings.dictionary.source.history",
  capture: "settings.dictionary.source.capture",
};

/**
 * Top-level Dictionary section. Shown in the sidebar once the Dictionary is
 * turned on under Advanced -> Experimental. Holds everything a user does
 * with entries: see them, search them, add one, delete one, and control
 * in-place learning.
 */
export const DictionarySettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const entries: DictionaryEntry[] = getSetting("dictionary_entries") || [];
  const busy = isUpdating("dictionary_entries");

  const [query, setQuery] = useState("");
  const [wrong, setWrong] = useState("");
  const [right, setRight] = useState("");

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
    if (!q) return entries;
    return entries.filter(
      (e) =>
        e.wrong.toLowerCase().includes(q) || e.right.toLowerCase().includes(q),
    );
  }, [entries, query]);

  const handleAdd = () => {
    if (!canAdd) return;
    const entry: DictionaryEntry = {
      wrong: wrongClean,
      right: rightClean,
      // Manual entries keep the user's exact casing. It is what they typed.
      case_mode: "exact",
      source: "manual",
    };
    if (entries.some((e) => sameEntry(e, entry))) {
      toast.error(
        t("settings.advanced.dictionary.duplicate", { wrong: wrongClean }),
      );
      return;
    }
    updateSetting("dictionary_entries", [...entries, entry]);
    setWrong("");
    setRight("");
  };

  // Delete by identity, not index: the list on screen may be filtered.
  const handleRemove = (entry: DictionaryEntry) => {
    updateSetting(
      "dictionary_entries",
      entries.filter((e) => !sameEntry(e, entry)),
    );
  };

  const handleKeyPress = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      handleAdd();
    }
  };

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("settings.dictionary.learningGroup")}>
        <DictionaryCaptureToggle descriptionMode="tooltip" grouped={true} />
      </SettingsGroup>

      <SettingsGroup
        title={t("settings.dictionary.entriesGroup", {
          count: entries.length,
        })}
        description={t("settings.advanced.dictionary.description")}
      >
        <div className="px-4 py-3 flex flex-wrap items-center gap-2 border-b border-mid-gray/20">
          <Input
            type="text"
            className="max-w-40"
            value={wrong}
            onChange={(e) => setWrong(e.target.value)}
            onKeyDown={handleKeyPress}
            placeholder={t("settings.advanced.dictionary.wrongPlaceholder")}
            variant="compact"
            disabled={busy}
          />
          <span className="text-text/50">{ARROW}</span>
          <Input
            type="text"
            className="max-w-40"
            value={right}
            onChange={(e) => setRight(e.target.value)}
            onKeyDown={handleKeyPress}
            placeholder={t("settings.advanced.dictionary.rightPlaceholder")}
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
          <div className="ms-auto flex items-center gap-2 text-text/50">
            <Search width={14} height={14} />
            <Input
              type="text"
              className="max-w-40"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t("settings.dictionary.search")}
              variant="compact"
            />
          </div>
        </div>

        {entries.length === 0 ? (
          <div className="px-4 py-6 text-center text-sm text-text/60">
            {t("settings.dictionary.empty")}
          </div>
        ) : visible.length === 0 ? (
          <div className="px-4 py-6 text-center text-sm text-text/60">
            {t("settings.dictionary.noMatch")}
          </div>
        ) : (
          <div className="divide-y divide-mid-gray/20">
            {visible.map((entry) => (
              <div
                key={`${entry.wrong}→${entry.right}`}
                className="px-4 py-2 flex items-center gap-3 text-sm"
              >
                <span className="min-w-0 flex-1 truncate">
                  <span className="text-text/60">{entry.wrong}</span>
                  <span className="text-text/40 px-2">{ARROW}</span>
                  <span className="font-medium">{entry.right}</span>
                </span>
                <span className="shrink-0 text-xs text-text/50 rounded-md border border-mid-gray/20 px-1.5 py-0.5">
                  {t(SOURCE_KEYS[entry.source] ?? SOURCE_KEYS.manual)}
                </span>
                <button
                  onClick={() => handleRemove(entry)}
                  disabled={busy}
                  className="shrink-0 p-1.5 rounded-md text-text/50 hover:text-logo-primary transition-colors cursor-pointer disabled:cursor-not-allowed disabled:text-text/20"
                  title={t("settings.advanced.dictionary.remove", {
                    wrong: entry.wrong,
                    right: entry.right,
                  })}
                  aria-label={t("settings.advanced.dictionary.remove", {
                    wrong: entry.wrong,
                    right: entry.right,
                  })}
                >
                  <Trash2 width={16} height={16} />
                </button>
              </div>
            ))}
          </div>
        )}
      </SettingsGroup>
    </div>
  );
};
