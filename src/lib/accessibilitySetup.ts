import { requestAccessibilityPermission } from "tauri-plugin-macos-permissions-api";
import { commands } from "@/bindings";

/**
 * Opens System Settings → Accessibility with the drag-to-add tile next to
 * it. Falls back to the plain system prompt if the helper cannot start.
 */
export async function openAccessibilitySetup(): Promise<void> {
  const result = await commands.openAccessibilityHelper();
  if (result.status === "error") {
    console.error(
      "Accessibility helper failed, using the system prompt:",
      result.error,
    );
    await requestAccessibilityPermission();
  }
}
