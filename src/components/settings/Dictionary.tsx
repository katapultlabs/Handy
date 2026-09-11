import React from "react";
import { useTranslation } from "react-i18next";
import { useSettings } from "../../hooks/useSettings";
import { ToggleSwitch } from "../ui/ToggleSwitch";

interface SectionProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

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

export const DictionaryCaptureToggle: React.FC<SectionProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const enabled = getSetting("dictionary_capture_enabled") || false;

    return (
      <ToggleSwitch
        checked={enabled}
        onChange={(v) => updateSetting("dictionary_capture_enabled", v)}
        isUpdating={isUpdating("dictionary_capture_enabled")}
        label={t("settings.advanced.dictionary.captureLabel")}
        description={t("settings.advanced.dictionary.captureDescription")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  },
);
