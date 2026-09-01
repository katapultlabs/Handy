import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useSettings } from "../../hooks/useSettings";
import { Input } from "../ui/Input";
import { Button } from "../ui/Button";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { SettingContainer } from "../ui/SettingContainer";
import type { DictionaryEntry } from "@/bindings";

interface SectionProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

// JSX literal strings are disallowed by the i18n lint rule; the arrow is
// punctuation, not translatable copy.
const ARROW = "→";

export const DictionaryToggle: React.FC<SectionProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const enabled = getSetting("dictionary_enabled") || false;

    return (
      <ToggleSwitch
        checked={enabled}
        onChange={(v) => updateSetting("dictionary_enabled", v)}
        isUpdating={isUpdating("dictionary_enabled")}
        label={t("settings.advanced.dictionary.toggleLabel")}
        description={t("settings.advanced.dictionary.toggleDescription")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  },
);

const sanitize = (s: string) =>
  s.replace(/[<>]/g, "").replace(/\s+/g, " ").trim();

export const DictionaryEntries: React.FC<SectionProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const [wrong, setWrong] = useState("");
    const [right, setRight] = useState("");
    const entries: DictionaryEntry[] = getSetting("dictionary_entries") || [];

    const wrongClean = sanitize(wrong);
    const rightClean = sanitize(right);
    const canAdd =
      wrongClean.length > 0 &&
      rightClean.length > 0 &&
      wrongClean.length <= 80 &&
      rightClean.length <= 80 &&
      !isUpdating("dictionary_entries");

    const handleAdd = () => {
      if (!canAdd) return;
      const dup = entries.some(
        (e) =>
          e.wrong.toLowerCase() === wrongClean.toLowerCase() &&
          e.right === rightClean,
      );
      if (dup) {
        toast.error(
          t("settings.advanced.dictionary.duplicate", { wrong: wrongClean }),
        );
        return;
      }
      const entry: DictionaryEntry = {
        wrong: wrongClean,
        right: rightClean,
        // Manual entries keep the user's exact casing — it is what they typed.
        case_mode: "exact",
        source: "manual",
      };
      updateSetting("dictionary_entries", [...entries, entry]);
      setWrong("");
      setRight("");
    };

    const handleRemove = (index: number) => {
      updateSetting(
        "dictionary_entries",
        entries.filter((_, i) => i !== index),
      );
    };

    const handleKeyPress = (e: React.KeyboardEvent) => {
      if (e.key === "Enter") {
        e.preventDefault();
        handleAdd();
      }
    };

    return (
      <>
        <SettingContainer
          title={t("settings.advanced.dictionary.title")}
          description={t("settings.advanced.dictionary.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <div className="flex items-center gap-2">
            <Input
              type="text"
              className="max-w-32"
              value={wrong}
              onChange={(e) => setWrong(e.target.value)}
              onKeyDown={handleKeyPress}
              placeholder={t("settings.advanced.dictionary.wrongPlaceholder")}
              variant="compact"
              disabled={isUpdating("dictionary_entries")}
            />
            <span className="text-text/50">{ARROW}</span>
            <Input
              type="text"
              className="max-w-32"
              value={right}
              onChange={(e) => setRight(e.target.value)}
              onKeyDown={handleKeyPress}
              placeholder={t("settings.advanced.dictionary.rightPlaceholder")}
              variant="compact"
              disabled={isUpdating("dictionary_entries")}
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
        </SettingContainer>
        {entries.length > 0 && (
          <div
            className={`px-4 p-2 ${grouped ? "" : "rounded-lg border border-mid-gray/20"} flex flex-wrap gap-1`}
          >
            {entries.map((entry, index) => (
              <Button
                key={`${entry.wrong}→${entry.right}`}
                onClick={() => handleRemove(index)}
                disabled={isUpdating("dictionary_entries")}
                variant="secondary"
                size="sm"
                className="inline-flex items-center gap-1 cursor-pointer"
                aria-label={t("settings.advanced.dictionary.remove", {
                  wrong: entry.wrong,
                  right: entry.right,
                })}
              >
                <span>
                  {entry.wrong} {ARROW} {entry.right}
                </span>
                <svg
                  className="w-3 h-3"
                  fill="none"
                  stroke="currentColor"
                  viewBox="0 0 24 24"
                >
                  <path
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    strokeWidth={2}
                    d="M6 18L18 6M6 6l12 12"
                  />
                </svg>
              </Button>
            ))}
          </div>
        )}
      </>
    );
  },
);
