//! One-click Accessibility setup (macOS).
//!
//! Opens System Settings at Privacy & Security → Accessibility and turns
//! Handy's own window into a small tile next to the Settings window, kept
//! above it while Settings is the active app. The tile's Handy icon is a real
//! file drag of Handy.app (`tauri-plugin-drag`): dropping it on the list adds
//! Handy, or registers it again after its signature changed, which removing
//! and re-adding it with the list's buttons does not always do. The window
//! follows Settings, shows "All set" when the permission arrives, and then
//! returns to its usual size and place.

use serde::Serialize;
use specta::Type;
use tauri::AppHandle;

/// What the main window shows while the helper runs.
#[derive(Clone, Copy, Debug, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionHelperPhase {
    /// The tile: drag Handy into the list.
    Waiting,
    /// The tile says "All set" for a moment.
    Granted,
    /// The window is back to normal.
    Done,
}

/// Which way the Settings list is from the tile, for the drag animation.
#[derive(Clone, Copy, Debug, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionHelperDirection {
    /// The tile is under the list (or over the window's bottom edge).
    Up,
    /// The tile is to the right of the Settings window.
    Left,
}

/// Sent to every window: the main window switches between the tile and its
/// usual content, the permission banner checks again.
#[derive(Clone, Debug, Serialize, Type, tauri_specta::Event)]
pub struct PermissionHelperEvent {
    pub phase: PermissionHelperPhase,
    pub direction: PermissionHelperDirection,
}

/// The file the tile drags and the image shown under the cursor.
#[derive(Clone, Debug, Serialize, Type)]
pub struct PermissionHelperDragItem {
    pub app_path: String,
    pub icon_path: String,
}

/// Opens Accessibility settings and turns the main window into the tile.
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

/// Ends the helper and restores the main window (the tile's close button).
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
    use objc2_app_kit::{
        NSScreen, NSWindow, NSWindowButton, NSWindowStyleMask, NSWindowTitleVisibility,
    };
    use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tauri::{Manager, WebviewWindow};
    use tauri_plugin_opener::OpenerExt;
    use tauri_specta::Event;

    const SETTINGS_URL: &str =
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
    const SETTINGS_BUNDLE_ID: &str = "com.apple.systempreferences";

    /// The tile's size. It has no title bar: the content fills the window.
    const TILE_WIDTH: f64 = 400.0;
    const TILE_HEIGHT: f64 = 96.0;
    /// Space between the Settings window and the tile.
    const GAP: f64 = 8.0;
    /// The tile lines up with the list column, the right part of the
    /// Settings window, and stays clear of its rounded corner.
    const RIGHT_INSET: f64 = 24.0;
    const POLL: Duration = Duration::from_millis(150);
    /// Settings has this long to open its window before the helper gives up.
    const SETTINGS_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
    /// How long "All set" stays on screen.
    const GRANTED_LINGER: Duration = Duration::from_millis(1600);
    /// The main window's minimum size outside the helper (see `lib.rs`).
    const MAIN_MIN_SIZE: NSSize = NSSize {
        width: 680.0,
        height: 570.0,
    };

    /// One follower thread at a time.
    static FOLLOWING: AtomicBool = AtomicBool::new(false);
    /// Set by the close button; the follower thread restores and ends.
    static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);
    /// The main window's frame (AppKit coordinates) before it became the tile.
    static SAVED: Mutex<Option<NSRect>> = Mutex::new(None);

    pub fn open(app: &AppHandle) -> Result<(), String> {
        app.opener()
            .open_url(SETTINGS_URL, None::<&str>)
            .map_err(|e| format!("Failed to open Accessibility settings: {e}"))?;

        CLOSE_REQUESTED.store(false, Ordering::SeqCst);
        if FOLLOWING.swap(true, Ordering::SeqCst) {
            // Already the tile: the running follower picks up the window.
            return Ok(());
        }

        let handle = app.clone();
        std::thread::spawn(move || {
            let granted = follow_settings_window(&handle);
            leave_tile_mode(&handle);
            emit(
                &handle,
                PermissionHelperPhase::Done,
                PermissionHelperDirection::Up,
            );
            if granted {
                crate::show_main_window(&handle);
            }
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
    /// arrives, Settings closes, or the user closes the tile. Returns whether
    /// the permission arrived.
    fn follow_settings_window(app: &AppHandle) -> bool {
        let started = Instant::now();
        let mut tile = false;
        let mut last_frame: Option<Frame> = None;
        let mut direction = PermissionHelperDirection::Up;

        loop {
            if CLOSE_REQUESTED.load(Ordering::SeqCst) {
                return false;
            }

            if accessibility_granted() {
                if tile {
                    emit(app, PermissionHelperPhase::Granted, direction);
                    std::thread::sleep(GRANTED_LINGER);
                }
                return true;
            }

            match settings_window_frame() {
                Some(frame) => {
                    if last_frame != Some(frame) {
                        let (position, now_direction) = tile_position(app, frame);
                        if !tile {
                            enter_tile_mode(app, position);
                            tile = true;
                        } else {
                            move_tile(app, position);
                        }
                        if last_frame.is_none() || now_direction != direction {
                            emit(app, PermissionHelperPhase::Waiting, now_direction);
                        }
                        direction = now_direction;
                        last_frame = Some(frame);
                    }
                }
                // Settings was closed (or moved to another Space).
                None if tile => return false,
                None if started.elapsed() > SETTINGS_OPEN_TIMEOUT => {
                    log::warn!(
                        "System Settings window not found; not showing the Accessibility helper"
                    );
                    return false;
                }
                None => {}
            }

            std::thread::sleep(POLL);
        }
    }

    fn emit(app: &AppHandle, phase: PermissionHelperPhase, direction: PermissionHelperDirection) {
        if let Err(e) = (PermissionHelperEvent { phase, direction }).emit(app) {
            log::error!("Failed to emit permission helper event: {e}");
        }
    }

    fn accessibility_granted() -> bool {
        unsafe { accessibility_sys::AXIsProcessTrusted() }
    }

    fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
        app.get_webview_window("main")
    }

    /// Runs `f` on the main thread with the main window's `NSWindow`.
    fn with_ns_window(
        app: &AppHandle,
        f: impl FnOnce(&NSWindow, MainThreadMarker) + Send + 'static,
    ) {
        let Some(window) = main_window(app) else {
            return;
        };
        let _ = app.run_on_main_thread(move || {
            let (Some(mtm), Ok(ptr)) = (MainThreadMarker::new(), window.ns_window()) else {
                return;
            };
            // SAFETY: Tauri's main window is an NSWindow that outlives this
            // call, and we are on the main thread.
            let ns_window = unsafe { &*(ptr as *const NSWindow) };
            f(ns_window, mtm);
        });
    }

    /// A top-left logical position (Tauri's space) as an AppKit frame, whose
    /// origin is the bottom-left of the primary screen.
    fn appkit_frame(mtm: MainThreadMarker, (x, y): (f64, f64), width: f64, height: f64) -> NSRect {
        let primary_height = NSScreen::screens(mtm)
            .firstObject()
            .map(|screen| screen.frame().size.height)
            .unwrap_or(0.0);
        NSRect::new(
            NSPoint::new(x, primary_height - y - height),
            NSSize::new(width, height),
        )
    }

    /// Saves the main window's frame, then turns it into the tile: no title
    /// bar or traffic lights (rounded corners and shadow stay), not
    /// resizable, kept above Settings (a floating window stays visible while
    /// another app is active), and gliding to `position`.
    fn enter_tile_mode(app: &AppHandle, position: (f64, f64)) {
        if let Some(window) = main_window(app) {
            let _ = window.show();
            let _ = window.set_always_on_top(true);
        }
        with_ns_window(app, move |ns_window, mtm| {
            *SAVED.lock().unwrap_or_else(|e| e.into_inner()) = Some(ns_window.frame());
            set_chrome(ns_window, false);
            ns_window.setContentMinSize(NSSize::new(0.0, 0.0));
            ns_window.setFrame_display_animate(
                appkit_frame(mtm, position, TILE_WIDTH, TILE_HEIGHT),
                true,
                true,
            );
        });
    }

    fn move_tile(app: &AppHandle, position: (f64, f64)) {
        with_ns_window(app, move |ns_window, mtm| {
            ns_window.setFrame_display_animate(
                appkit_frame(mtm, position, TILE_WIDTH, TILE_HEIGHT),
                true,
                false,
            );
        });
    }

    /// Glides the main window back to its saved frame and gives it its title
    /// bar, size limits, and normal level again.
    fn leave_tile_mode(app: &AppHandle) {
        if let Some(window) = main_window(app) {
            let _ = window.set_always_on_top(false);
        }
        with_ns_window(app, |ns_window, _| {
            let saved = SAVED.lock().unwrap_or_else(|e| e.into_inner()).take();
            set_chrome(ns_window, true);
            ns_window.setContentMinSize(MAIN_MIN_SIZE);
            if let Some(frame) = saved {
                ns_window.setFrame_display_animate(frame, true, true);
            }
        });
    }

    /// The usual window chrome (title bar, traffic lights, resizing) on or off.
    fn set_chrome(ns_window: &NSWindow, on: bool) {
        let mut mask = ns_window.styleMask();
        if on {
            mask.remove(NSWindowStyleMask::FullSizeContentView);
            mask.insert(NSWindowStyleMask::Resizable);
        } else {
            mask.insert(NSWindowStyleMask::FullSizeContentView);
            mask.remove(NSWindowStyleMask::Resizable);
        }
        ns_window.setStyleMask(mask);
        ns_window.setTitlebarAppearsTransparent(!on);
        ns_window.setTitleVisibility(if on {
            NSWindowTitleVisibility::Visible
        } else {
            NSWindowTitleVisibility::Hidden
        });
        for kind in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(button) = ns_window.standardWindowButton(kind) {
                button.setHidden(!on);
            }
        }
    }

    /// The Settings window's frame in global points (top-left origin), the
    /// same space as Tauri's logical positions on macOS.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Frame {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    /// Under the list column of the Settings window; to its right when there
    /// is no room below; over its bottom edge when there is room for neither.
    /// Also says which way the list is from there.
    fn tile_position(app: &AppHandle, frame: Frame) -> ((f64, f64), PermissionHelperDirection) {
        let under_list_x = frame.x + frame.width - TILE_WIDTH - RIGHT_INSET;
        let below = (
            (under_list_x, frame.y + frame.height + GAP),
            PermissionHelperDirection::Up,
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
                PermissionHelperDirection::Left,
            );
        }
        (
            (
                under_list_x,
                frame.y + frame.height - TILE_HEIGHT - RIGHT_INSET,
            ),
            PermissionHelperDirection::Up,
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
