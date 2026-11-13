//! ⌘Q on macOS: quit Handy, or keep it running in the menu bar.
//!
//! The default app menu's Quit item calls `[NSApp terminate:]`, and tao gives
//! the app no way to cancel that. So the app menu here is Tauri's default
//! menu with one change: its Quit item is a normal item with the same ⌘Q
//! accelerator, and Handy decides what it does. Quit from the Dock, Quit from
//! the tray, logout, and shutdown still quit.

use serde::Serialize;
use specta::Type;
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;
use tauri_specta::Event;

use crate::settings::{self, QuitShortcutAction};

/// Menu id of the app menu's "Quit Handy" item.
pub const QUIT_MENU_ID: &str = "app_menu_quit";
/// File → Transcribe File… in the menu bar.
pub const TRANSCRIBE_FILE_MENU_ID: &str = "app_menu_transcribe_file";
/// Asks the main window to open the file picker in History.
pub const TRANSCRIBE_FILE_EVENT: &str = "menu-transcribe-file";

/// Asks the main window to show the quit prompt.
#[derive(Clone, Debug, Serialize, Type, tauri_specta::Event)]
pub struct QuitShortcutPromptEvent;

/// Tauri's default menu, with the app menu's Quit item replaced by one that
/// Handy handles.
#[cfg(target_os = "macos")]
pub fn build_app_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind};

    let menu = Menu::default(app)?;
    add_transcribe_file_item(app, &menu)?;
    let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() else {
        log::warn!("App menu not found; ⌘Q keeps the default quit");
        return Ok(menu);
    };

    let items = app_menu.items()?;
    let last_is_quit = match items.last() {
        Some(MenuItemKind::Predefined(item)) => item.text()?.starts_with("Quit"),
        _ => false,
    };
    if !last_is_quit {
        log::warn!("Quit item not found in the app menu; ⌘Q keeps the default quit");
        return Ok(menu);
    }

    app_menu.remove_at(items.len() - 1)?;
    let quit = MenuItem::with_id(
        app,
        QUIT_MENU_ID,
        format!("Quit {}", app.package_info().name),
        true,
        Some("CmdOrCtrl+Q"),
    )?;
    app_menu.append(&quit)?;
    Ok(menu)
}

#[cfg(target_os = "macos")]
/// Puts "Transcribe File…" (⌘O) first in the File menu, where a Mac app
/// opens files. Without a File menu the menu bar stays as it is.
fn add_transcribe_file_item(
    app: &AppHandle,
    menu: &tauri::menu::Menu<tauri::Wry>,
) -> tauri::Result<()> {
    use tauri::menu::{MenuItem, MenuItemKind, PredefinedMenuItem};

    for item in menu.items()? {
        if let MenuItemKind::Submenu(submenu) = item {
            if submenu.text()? == "File" {
                let transcribe = MenuItem::with_id(
                    app,
                    TRANSCRIBE_FILE_MENU_ID,
                    "Transcribe File…",
                    true,
                    Some("CmdOrCtrl+O"),
                )?;
                submenu.insert(&transcribe, 0)?;
                submenu.insert(&PredefinedMenuItem::separator(app)?, 1)?;
                return Ok(());
            }
        }
    }
    log::warn!("File menu not found; no Transcribe File item in the menu bar");
    Ok(())
}

/// File → Transcribe File…: bring Handy forward and open the picker in History.
#[cfg(target_os = "macos")]
pub fn handle_transcribe_file(app: &AppHandle) {
    use tauri::Emitter;
    crate::show_main_window(app);
    if let Err(e) = app.emit(TRANSCRIBE_FILE_EVENT, ()) {
        log::error!("Failed to open the file picker from the menu: {}", e);
    }
}

/// Runs when the user presses ⌘Q or picks "Quit Handy" in the app menu.
#[cfg(target_os = "macos")]
pub fn handle_quit_shortcut(app: &AppHandle) {
    // Without a tray icon, a hidden Handy is only reachable from the Dock,
    // which a user who pressed ⌘Q does not expect. Quit, as usual.
    if !crate::tray_available(app) {
        app.exit(0);
        return;
    }

    match settings::get_settings(app).quit_shortcut_action {
        QuitShortcutAction::Ask => {
            crate::show_main_window(app);
            if let Err(e) = QuitShortcutPromptEvent.emit(app) {
                log::error!("Failed to ask about ⌘Q, quitting: {}", e);
                app.exit(0);
            }
        }
        QuitShortcutAction::MenuBar => crate::hide_main_window(app),
        QuitShortcutAction::Quit => app.exit(0),
    }
}

/// Carries out the answer to the quit prompt. With `remember`, the answer
/// becomes the setting and the prompt does not show again.
#[tauri::command]
#[specta::specta]
pub fn resolve_quit_shortcut(
    app: AppHandle,
    action: QuitShortcutAction,
    remember: bool,
) -> Result<(), String> {
    if remember && action != QuitShortcutAction::Ask {
        save_action(&app, action)?;
    }

    match action {
        QuitShortcutAction::Ask => {}
        QuitShortcutAction::MenuBar => crate::hide_main_window(&app),
        QuitShortcutAction::Quit => app.exit(0),
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_quit_shortcut_action_setting(
    app: AppHandle,
    action: QuitShortcutAction,
) -> Result<(), String> {
    save_action(&app, action)
}

/// Writes the setting and saves the store at once, so the choice survives a
/// quit that follows straight after.
fn save_action(app: &AppHandle, action: QuitShortcutAction) -> Result<(), String> {
    let mut app_settings = settings::get_settings(app);
    app_settings.quit_shortcut_action = action;
    settings::write_settings(app, app_settings);

    app.store(crate::portable::store_path(settings::SETTINGS_STORE_PATH))
        .map_err(|e| e.to_string())?
        .save()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_by_default() {
        assert_eq!(
            settings::get_default_settings().quit_shortcut_action,
            QuitShortcutAction::Ask
        );
    }

    #[test]
    fn stored_names_stay_stable() {
        // The settings store and the frontend use these strings.
        for (action, name) in [
            (QuitShortcutAction::Ask, "\"ask\""),
            (QuitShortcutAction::MenuBar, "\"menu_bar\""),
            (QuitShortcutAction::Quit, "\"quit\""),
        ] {
            assert_eq!(serde_json::to_string(&action).unwrap(), name);
            assert_eq!(
                serde_json::from_str::<QuitShortcutAction>(name).unwrap(),
                action
            );
        }
    }
}
