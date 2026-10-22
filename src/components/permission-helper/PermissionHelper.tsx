import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { Check, X } from "lucide-react";
import {
  commands,
  events,
  type PermissionHelperDragItem,
  type PermissionHelperPhase,
} from "@/bindings";
import appIcon from "@/../src-tauri/icons/128x128.png";

// While the Accessibility helper runs, the backend shrinks the main window
// next to System Settings and keeps it on top; this layer covers the usual
// content (which stays mounted, so onboarding keeps polling) with the tile.
// Dragging the icon starts a native file drag of Handy.app, which the
// Settings list accepts like a drop from Finder.
export const PermissionHelper: React.FC = () => {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<PermissionHelperPhase>("done");
  const [dragItem, setDragItem] = useState<PermissionHelperDragItem | null>(
    null,
  );

  useEffect(() => {
    const unlisten = events.permissionHelperEvent.listen((event) =>
      setPhase(event.payload.phase),
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (phase !== "waiting" || dragItem) return;
    commands.accessibilityHelperDragItem().then((result) => {
      if (result.status === "ok") {
        setDragItem(result.data);
      } else {
        console.error(
          "No drag item for the Accessibility helper:",
          result.error,
        );
      }
    });
  }, [phase, dragItem]);

  if (phase === "done") return null;

  const handleMouseDown = (event: React.MouseEvent) => {
    if (!dragItem || event.button !== 0) return;
    event.preventDefault();
    startDrag({
      item: [dragItem.app_path],
      icon: dragItem.icon_path,
      // Never offer "move": dropping must not relocate Handy.app.
      mode: "copy",
    }).catch((e) => console.error("Failed to start the drag:", e));
  };

  return (
    <div className="fixed inset-0 z-[60] flex items-center gap-3 bg-background px-4 text-text select-none">
      {phase === "granted" ? (
        <>
          <div className="flex h-12 w-12 shrink-0 items-center justify-center rounded-full bg-green-500/15 text-green-500">
            <Check className="h-7 w-7" aria-hidden="true" />
          </div>
          <div className="min-w-0">
            <p className="text-sm font-semibold">
              {t("permissionHelper.granted.title")}
            </p>
            <p className="text-xs text-mid-gray">
              {t("permissionHelper.granted.description")}
            </p>
          </div>
        </>
      ) : (
        <>
          <img
            src={appIcon}
            alt=""
            draggable={false}
            onMouseDown={handleMouseDown}
            className="h-12 w-12 shrink-0 cursor-grab transition-transform hover:scale-110 active:cursor-grabbing"
          />
          <div className="min-w-0 pe-6">
            <p className="text-sm font-semibold">
              {t("permissionHelper.title")}
            </p>
            <p className="text-xs leading-snug text-mid-gray">
              {t("permissionHelper.description")}
            </p>
          </div>
          <button
            type="button"
            onClick={() => commands.closeAccessibilityHelper()}
            aria-label={t("permissionHelper.close")}
            className="absolute end-2 top-2 cursor-pointer rounded-md p-1 text-mid-gray hover:bg-mid-gray/10 hover:text-text"
          >
            <X className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </>
      )}
    </div>
  );
};
