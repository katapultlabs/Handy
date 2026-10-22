import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, events, type QuitShortcutAction } from "@/bindings";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { useSettings } from "../../hooks/useSettings";

// Asks what ⌘Q should do when the "When you press ⌘Q" setting is "Ask".
// The backend shows the window and sends the event; closing the dialog
// without a choice keeps Handy open.
export const QuitPrompt: React.FC = () => {
  const { t } = useTranslation();
  const { refreshSettings } = useSettings();
  const [open, setOpen] = useState(false);
  const [remember, setRemember] = useState(false);
  const keepButtonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const unlisten = events.quitShortcutPromptEvent.listen(() => {
      setRemember(false);
      setOpen(true);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const choose = async (action: QuitShortcutAction) => {
    setOpen(false);
    const result = await commands.resolveQuitShortcut(action, remember);
    if (result.status === "error") {
      console.error("Failed to resolve the quit shortcut:", result.error);
      return;
    }
    // Keep Settings → Advanced in step with a remembered choice.
    if (remember) {
      await refreshSettings();
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={setOpen}
      title={t("quitPrompt.title")}
      description={t("quitPrompt.description")}
      closeLabel={t("quitPrompt.cancel")}
      initialFocusRef={keepButtonRef}
      contentFades={false}
      className="max-w-md"
      footer={
        <>
          <Button variant="secondary" onClick={() => choose("quit")}>
            {t("quitPrompt.quit")}
          </Button>
          <Button ref={keepButtonRef} onClick={() => choose("menu_bar")}>
            {t("quitPrompt.keep")}
          </Button>
        </>
      }
    >
      <label className="flex cursor-pointer items-center gap-2 text-sm text-text">
        <input
          type="checkbox"
          className="shrink-0 cursor-pointer accent-logo-primary"
          checked={remember}
          onChange={(e) => setRemember(e.target.checked)}
        />
        {t("quitPrompt.remember")}
      </label>
      <p className="mt-2 text-xs text-mid-gray">
        {t("quitPrompt.settingsHint")}
      </p>
    </Dialog>
  );
};
