import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { ChevronLeft, ChevronUp, X } from "lucide-react";
import {
  commands,
  events,
  type PermissionHelperDirection,
  type PermissionHelperDragItem,
  type PermissionHelperPhase,
} from "@/bindings";
import appIcon from "@/../src-tauri/icons/128x128.png";
import "./PermissionHelper.css";

// While the Accessibility helper runs, the backend turns the main window into
// a chrome-less tile under (or beside) the System Settings list and keeps it
// on top; this layer covers the usual content, which stays mounted so
// onboarding keeps polling. The tile shows the gesture rather than explaining
// it: a ghost of the icon keeps lifting off toward the list. Dragging the
// real icon starts a native file drag of Handy.app, which the list accepts
// like a drop from Finder.
export const PermissionHelper: React.FC = () => {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<PermissionHelperPhase>("done");
  const [direction, setDirection] = useState<PermissionHelperDirection>("up");
  const [dragging, setDragging] = useState(false);
  const [dragItem, setDragItem] = useState<PermissionHelperDragItem | null>(
    null,
  );

  useEffect(() => {
    const unlisten = events.permissionHelperEvent.listen((event) => {
      setPhase(event.payload.phase);
      setDirection(event.payload.direction);
    });
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
    setDragging(true);
    startDrag(
      {
        item: [dragItem.app_path],
        icon: dragItem.icon_path,
        // Never offer "move": dropping must not relocate Handy.app.
        mode: "copy",
      },
      () => setDragging(false),
    ).catch((e) => {
      setDragging(false);
      console.error("Failed to start the drag:", e);
    });
  };

  if (phase === "granted") {
    return (
      <div className="fixed inset-0 z-[60] flex items-center gap-4 bg-background px-6 text-text select-none">
        <div className="flex h-14 w-14 shrink-0 items-center justify-center rounded-full bg-green-500/15 text-green-500">
          <svg
            className="ph-check h-8 w-8"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth={3}
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
          >
            <path d="M5 12.5l4.5 4.5L19 7.5" />
          </svg>
        </div>
        <div className="min-w-0">
          <p className="text-base font-semibold">
            {t("permissionHelper.granted.title")}
          </p>
          <p className="text-xs text-mid-gray">
            {t("permissionHelper.granted.description")}
          </p>
        </div>
      </div>
    );
  }

  const Chevron = direction === "left" ? ChevronLeft : ChevronUp;

  return (
    <div
      className={`fixed inset-0 z-[60] flex items-center gap-3 bg-background px-5 text-text select-none ${
        dragging ? "ph-paused" : ""
      }`}
    >
      <div className="relative h-14 w-14 shrink-0">
        <img
          src={appIcon}
          alt=""
          aria-hidden="true"
          draggable={false}
          data-direction={direction}
          className="ph-ghost"
        />
        <img
          src={appIcon}
          alt=""
          draggable={false}
          onMouseDown={handleMouseDown}
          data-direction={direction}
          className="ph-icon relative h-14 w-14 cursor-grab drop-shadow-md active:cursor-grabbing"
        />
      </div>
      <div
        className={`flex shrink-0 text-logo-primary ${
          direction === "left" ? "flex-row-reverse" : "flex-col-reverse"
        }`}
        aria-hidden="true"
      >
        <Chevron className="ph-chevron -m-1 h-5 w-5" strokeWidth={3} />
        <Chevron className="ph-chevron -m-1 h-5 w-5" strokeWidth={3} />
        <Chevron className="ph-chevron -m-1 h-5 w-5" strokeWidth={3} />
      </div>
      <div className="min-w-0 pe-6">
        <p className="text-base font-semibold leading-tight">
          {t("permissionHelper.title")}
        </p>
        <p className="mt-0.5 text-xs text-mid-gray">
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
    </div>
  );
};
