import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface DoubleTapLockProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

// Push-to-talk only: the other activation modes already lock on a tap.
export const DoubleTapLock: React.FC<DoubleTapLockProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    if (getSetting("shortcut_activation") !== "push_to_talk") {
      return null;
    }

    return (
      <ToggleSwitch
        checked={getSetting("double_tap_lock") ?? true}
        onChange={(enabled) => updateSetting("double_tap_lock", enabled)}
        isUpdating={isUpdating("double_tap_lock")}
        label={t("settings.general.doubleTapLock.label")}
        description={t("settings.general.doubleTapLock.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        tooltipPosition="bottom"
      />
    );
  },
);
