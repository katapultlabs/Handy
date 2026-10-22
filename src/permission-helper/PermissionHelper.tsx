import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { Check, X } from "lucide-react";
import {
  commands,
  events,
  type PermissionHelperDragItem,
  type PermissionHelperPhase,
  type PermissionHelperPlacement,
} from "@/bindings";
import appIcon from "@/../src-tauri/icons/128x128.png";

// The floating tile next to System Settings → Accessibility. Dragging the
// icon starts a native file drag of Handy.app, which the Accessibility list
// accepts like a drop from Finder. The backend positions, shows, and hides
// this window and reports when the permission arrives.
//
// The window is transparent: the card, its shadow, and the caret that
// points at the Settings list are drawn here. The window keeps an 8 px band
// on every side for the shadow, plus the caret on the side facing Settings.
const PermissionHelper: React.FC = () => {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<PermissionHelperPhase>("waiting");
  const [placement, setPlacement] =
    useState<PermissionHelperPlacement>("below");
  const [dragItem, setDragItem] = useState<PermissionHelperDragItem | null>(
    null,
  );

  useEffect(() => {
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
    const unlisten = events.permissionHelperEvent.listen((event) => {
      setPhase(event.payload.phase);
      setPlacement(event.payload.placement);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

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

  // Room for the caret on the side that faces the Settings window.
  const padding =
    placement === "below"
      ? "px-2 pb-2 pt-[14px]"
      : placement === "right"
        ? "py-2 pe-2 ps-[14px]"
        : "p-2";

  return (
    <div className={`h-full w-full ${padding}`}>
      <div className="relative flex h-full w-full items-center gap-3 rounded-xl border border-mid-gray/20 bg-background px-3 text-text shadow-[0_4px_16px_rgba(0,0,0,0.18)] select-none">
        {placement !== "inside" && (
          <span
            aria-hidden="true"
            className={`absolute h-3 w-3 rotate-45 border-mid-gray/20 bg-background ${
              placement === "below"
                ? "-top-[7px] left-10 border-l border-t"
                : "-left-[7px] top-1/2 -mt-1.5 border-b border-l"
            }`}
          />
        )}

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
            <div className="min-w-0 pe-5">
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
              className="absolute end-1.5 top-1.5 cursor-pointer rounded-md p-1 text-mid-gray hover:bg-mid-gray/10 hover:text-text"
            >
              <X className="h-3.5 w-3.5" aria-hidden="true" />
            </button>
          </>
        )}
      </div>
    </div>
  );
};

export default PermissionHelper;
