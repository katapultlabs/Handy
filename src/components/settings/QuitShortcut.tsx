import React from "react";
import { useTranslation } from "react-i18next";
import { type } from "@tauri-apps/plugin-os";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";
import type { QuitShortcutAction } from "@/bindings";

interface QuitShortcutProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

// macOS only, and only with the tray icon on: without it, ⌘Q always quits.
export const QuitShortcut: React.FC<QuitShortcutProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    if (type() !== "macos" || !(getSetting("show_tray_icon") ?? true)) {
      return null;
    }

    const options = [
      {
        value: "ask",
        label: t("settings.advanced.quitShortcut.options.ask"),
      },
      {
        value: "menu_bar",
        label: t("settings.advanced.quitShortcut.options.menuBar"),
      },
      {
        value: "quit",
        label: t("settings.advanced.quitShortcut.options.quit"),
      },
    ];

    return (
      <SettingContainer
        title={t("settings.advanced.quitShortcut.title")}
        description={t("settings.advanced.quitShortcut.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <Dropdown
          options={options}
          selectedValue={getSetting("quit_shortcut_action") ?? "ask"}
          onSelect={(value) =>
            updateSetting("quit_shortcut_action", value as QuitShortcutAction)
          }
          disabled={isUpdating("quit_shortcut_action")}
        />
      </SettingContainer>
    );
  },
);
