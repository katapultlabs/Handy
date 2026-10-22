//! One-click Accessibility setup (macOS).
//!
//! Opens System Settings at Privacy & Security → Accessibility and shows a
//! small floating tile next to its window. The tile's Handy icon is a real
//! file drag of Handy.app (`tauri-plugin-drag`): dropping it on the list adds
//! Handy, or registers it again after its signature changed, which removing
//! and re-adding it with the list's buttons does not always do. The tile
//! follows the Settings window, turns into "All set" when the permission
//! arrives, and then hides.

use serde::Serialize;
use specta::Type;
use tauri::AppHandle;

/// Label of the tile window.
pub const WINDOW_LABEL: &str = "permission_helper";

/// What the tile shows: waiting for the drop, or done.
#[derive(Clone, Copy, Debug, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionHelperPhase {
    Waiting,
    Granted,
}

/// Where the tile sits relative to the Settings window, so its caret can
/// point at the list.
#[derive(Clone, Copy, Debug, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionHelperPlacement {
    /// Under the window: the caret points up.
    Below,
    /// Beside the window: the caret points left.
    Right,
    /// Over the window's bottom edge: no room outside it, no caret.
    Inside,
}

/// Sent to every window: the tile switches its view, the permission banner
/// checks again.
#[derive(Clone, Debug, Serialize, Type, tauri_specta::Event)]
pub struct PermissionHelperEvent {
    pub phase: PermissionHelperPhase,
    pub placement: PermissionHelperPlacement,
}

/// The file the tile drags and the image shown under the cursor.
#[derive(Clone, Debug, Serialize, Type)]
pub struct PermissionHelperDragItem {
    pub app_path: String,
    pub icon_path: String,
}

/// Opens Accessibility settings and shows the tile next to them.
#[tauri::command]
#[specta::specta]
pub fn open_accessibility_helper(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return macos::open(&app);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("The Accessibility helper is macOS only".into())
    }
}

/// Hides the tile (its close button).
#[tauri::command]
#[specta::specta]
pub fn close_accessibility_helper(app: AppHandle) {
    #[cfg(target_os = "macos")]
    macos::close(&app);
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

#[tauri::command]
#[specta::specta]
pub fn accessibility_helper_drag_item(app: AppHandle) -> Result<PermissionHelperDragItem, String> {
    #[cfg(target_os = "macos")]
    return macos::drag_item(&app);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("The Accessibility helper is macOS only".into())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    use tauri::{LogicalPosition, Manager, WebviewUrl};
    use tauri_nspanel::{
        tauri_panel, CollectionBehavior, ManagerExt, PanelBuilder, PanelLevel, StyleMask,
    };
    use tauri_plugin_opener::OpenerExt;
    use tauri_specta::Event;

    const SETTINGS_URL: &str =
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
    const SETTINGS_BUNDLE_ID: &str = "com.apple.systempreferences";

    const TILE_WIDTH: f64 = 380.0;
    /// Includes the transparent band around the card for its shadow and the
    /// caret on the side facing Settings (see PermissionHelper.tsx).
    const TILE_HEIGHT: f64 = 100.0;
    /// Space between the Settings window and the tile.
    const GAP: f64 = 4.0;
    /// The tile lines up with the list column, which is the right part of
    /// the Settings window; this keeps it off the window's rounded corner.
    const RIGHT_INSET: f64 = 24.0;
    const POLL: Duration = Duration::from_millis(150);
    /// Settings has this long to open its window before the tile gives up.
    const SETTINGS_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
    /// How long "All set" stays on screen.
    const GRANTED_LINGER: Duration = Duration::from_millis(1600);

    /// One follower thread at a time.
    static FOLLOWING: AtomicBool = AtomicBool::new(false);
    /// Set by the close button; the follower thread hides the tile and ends.
    static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);

    tauri_panel! {
        panel!(PermissionHelperPanel {
            config: {
                can_become_key_window: false,
                is_floating_panel: true
            }
        })
    }

    pub fn open(app: &AppHandle) -> Result<(), String> {
        app.opener()
            .open_url(SETTINGS_URL, None::<&str>)
            .map_err(|e| format!("Failed to open Accessibility settings: {e}"))?;

        CLOSE_REQUESTED.store(false, Ordering::SeqCst);
        if FOLLOWING.swap(true, Ordering::SeqCst) {
            // Already showing: the running follower picks up the window.
            return Ok(());
        }

        let handle = app.clone();
        std::thread::spawn(move || {
            follow_settings_window(&handle);
            hide_tile(&handle);
            FOLLOWING.store(false, Ordering::SeqCst);
        });
        Ok(())
    }

    pub fn close(_app: &AppHandle) {
        CLOSE_REQUESTED.store(true, Ordering::SeqCst);
    }

    pub fn drag_item(app: &AppHandle) -> Result<PermissionHelperDragItem, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // .../Handy.app/Contents/MacOS/handy → .../Handy.app. A dev build
        // is a bare binary, which the Accessibility list also accepts.
        let app_path = exe
            .ancestors()
            .find(|p| p.extension().is_some_and(|ext| ext == "app"))
            .map(PathBuf::from)
            .unwrap_or(exe);

        let icon_path = app
            .path()
            .app_cache_dir()
            .map_err(|e| e.to_string())?
            .join("permission-helper-icon.png");
        if !icon_path.exists() {
            if let Some(dir) = icon_path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&icon_path, include_bytes!("../icons/128x128.png"))
                .map_err(|e| e.to_string())?;
        }

        Ok(PermissionHelperDragItem {
            app_path: app_path.to_string_lossy().into_owned(),
            icon_path: icon_path.to_string_lossy().into_owned(),
        })
    }

    /// Keeps the tile next to the Settings window until the permission
    /// arrives, Settings closes, or the user closes the tile.
    fn follow_settings_window(app: &AppHandle) {
        let started = Instant::now();
        let mut shown = false;
        let mut last_position: Option<(f64, f64)> = None;
        let mut placement = PermissionHelperPlacement::Below;

        loop {
            if CLOSE_REQUESTED.load(Ordering::SeqCst) {
                return;
            }

            if accessibility_granted() {
                emit(app, PermissionHelperPhase::Granted, placement);
                if shown {
                    std::thread::sleep(GRANTED_LINGER);
                }
                return;
            }

            match settings_window_frame() {
                Some(frame) => {
                    let (position, now_placement) = tile_position(app, frame);
                    if !shown {
                        show_tile(app, position);
                        shown = true;
                    } else if last_position != Some(position) {
                        move_tile(app, position);
                    }
                    if last_position.is_none() || now_placement != placement {
                        emit(app, PermissionHelperPhase::Waiting, now_placement);
                    }
                    placement = now_placement;
                    last_position = Some(position);
                }
                // Settings was closed (or moved to another Space).
                None if shown => return,
                None if started.elapsed() > SETTINGS_OPEN_TIMEOUT => {
                    log::warn!(
                        "System Settings window not found; not showing the Accessibility helper"
                    );
                    return;
                }
                None => {}
            }

            std::thread::sleep(POLL);
        }
    }

    fn emit(app: &AppHandle, phase: PermissionHelperPhase, placement: PermissionHelperPlacement) {
        if let Err(e) = (PermissionHelperEvent { phase, placement }).emit(app) {
            log::error!("Failed to emit permission helper event: {e}");
        }
    }

    fn accessibility_granted() -> bool {
        unsafe { accessibility_sys::AXIsProcessTrusted() }
    }

    /// The Settings window's frame in global points (top-left origin), the
    /// same space as Tauri's logical positions on macOS.
    #[derive(Clone, Copy, Debug)]
    struct Frame {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    /// Under the list column of the Settings window; to its right when there
    /// is no room below; over its bottom edge when there is room for neither.
    fn tile_position(app: &AppHandle, frame: Frame) -> ((f64, f64), PermissionHelperPlacement) {
        let under_list_x = frame.x + frame.width - TILE_WIDTH - RIGHT_INSET;
        let below = (
            (under_list_x, frame.y + frame.height + GAP),
            PermissionHelperPlacement::Below,
        );
        let Some(screen) = screen_containing(app, frame) else {
            return below;
        };

        if below.0 .1 + TILE_HEIGHT <= screen.y + screen.height {
            return below;
        }
        let right_x = frame.x + frame.width + GAP;
        if right_x + TILE_WIDTH <= screen.x + screen.width {
            return (
                (right_x, frame.y + frame.height / 2.0 - TILE_HEIGHT / 2.0),
                PermissionHelperPlacement::Right,
            );
        }
        (
            (
                under_list_x,
                frame.y + frame.height - TILE_HEIGHT - 2.0 * RIGHT_INSET,
            ),
            PermissionHelperPlacement::Inside,
        )
    }

    /// The monitor that holds the centre of `frame`, in logical points.
    fn screen_containing(app: &AppHandle, frame: Frame) -> Option<Frame> {
        let (cx, cy) = (frame.x + frame.width / 2.0, frame.y + frame.height / 2.0);
        app.available_monitors().ok()?.into_iter().find_map(|m| {
            let scale = m.scale_factor();
            let screen = Frame {
                x: m.position().x as f64 / scale,
                y: m.position().y as f64 / scale,
                width: m.size().width as f64 / scale,
                height: m.size().height as f64 / scale,
            };
            (cx >= screen.x
                && cx < screen.x + screen.width
                && cy >= screen.y
                && cy < screen.y + screen.height)
                .then_some(screen)
        })
    }

    fn show_tile(app: &AppHandle, (x, y): (f64, f64)) {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Ok(panel) = handle.get_webview_panel(WINDOW_LABEL) {
                if let Some(window) = handle.get_webview_window(WINDOW_LABEL) {
                    let _ = window.set_position(LogicalPosition { x, y });
                }
                panel.show();
                return;
            }
            let built = PanelBuilder::<_, PermissionHelperPanel>::new(&handle, WINDOW_LABEL)
                .url(WebviewUrl::App("src/permission-helper/index.html".into()))
                .title("Handy")
                .position(tauri::Position::Logical(LogicalPosition { x, y }))
                .size(tauri::Size::Logical(tauri::LogicalSize {
                    width: TILE_WIDTH,
                    height: TILE_HEIGHT,
                }))
                // Above System Settings, which is the active app while the
                // tile is up. Panels hide when their app is inactive by
                // default; this one must not, or it only shows once the
                // user switches back to Handy.
                .level(PanelLevel::Status)
                .hides_on_deactivate(false)
                .has_shadow(false)
                .transparent(true)
                .no_activate(true)
                .corner_radius(0.0)
                .style_mask(StyleMask::empty().borderless().nonactivating_panel())
                .with_window(|w| w.decorations(false).transparent(true).focusable(false))
                .collection_behavior(CollectionBehavior::new().can_join_all_spaces())
                .build();
            match built {
                Ok(panel) => panel.show(),
                Err(e) => log::error!("Failed to create the Accessibility helper: {e}"),
            }
        });
    }

    fn move_tile(app: &AppHandle, (x, y): (f64, f64)) {
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            let _ = window.set_position(LogicalPosition { x, y });
        }
    }

    fn hide_tile(app: &AppHandle) {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Ok(panel) = handle.get_webview_panel(WINDOW_LABEL) {
                panel.hide();
            }
        });
    }

    // --- Finding the Settings window -------------------------------------

    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;

    #[repr(C)]
    #[derive(Default)]
    struct CGRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect)
            -> bool;
    }

    const ON_SCREEN_ONLY: u32 = 1 << 0;
    const EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

    /// The frontmost normal window of System Settings. Window bounds and
    /// owner PIDs need no Screen Recording permission (only titles do).
    fn settings_window_frame() -> Option<Frame> {
        let pid = settings_pid()?;
        let list =
            unsafe { CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP_ELEMENTS, 0) };
        if list.is_null() {
            return None;
        }
        let windows: CFArray<CFDictionary<CFString, CFType>> =
            unsafe { CFArray::wrap_under_create_rule(list) };

        let number = |info: &CFDictionary<CFString, CFType>, key: &'static str| {
            info.find(CFString::from_static_string(key))
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
        };

        windows.iter().find_map(|info| {
            if number(&info, "kCGWindowOwnerPID") != Some(pid as i64)
                || number(&info, "kCGWindowLayer") != Some(0)
            {
                return None;
            }
            let bounds = info.find(CFString::from_static_string("kCGWindowBounds"))?;
            let mut rect = CGRect::default();
            let ok = unsafe {
                CGRectMakeWithDictionaryRepresentation(
                    bounds.as_CFTypeRef() as CFDictionaryRef,
                    &mut rect,
                )
            };
            // Skip sheets and tiny helper windows.
            (ok && rect.width > 300.0 && rect.height > 200.0).then_some(Frame {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            })
        })
    }

    fn settings_pid() -> Option<i32> {
        use objc2_app_kit::NSRunningApplication;
        use objc2_foundation::NSString;

        let id = NSString::from_str(SETTINGS_BUNDLE_ID);
        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&id);
        apps.iter()
            .map(|a| a.processIdentifier())
            .find(|pid| *pid > 0)
    }
}
