//! macOS-specific window-manager glue.
//!
//! winit installs a default application menu whose "Quit" item binds Cmd+Q to
//! AppKit's `terminate:`, which kills the process before egui runs a frame —
//! bypassing our unsaved-changes guard entirely. Rather than drop the menu
//! (losing About/Hide/Services/Quit and the Cmd+Q label), we repoint that one
//! item's action to `performClose:`. That sends the standard close request to
//! the key window, which winit turns into `WindowEvent::CloseRequested`, so
//! both Cmd+Q and clicking Quit flow through the same close guard as the
//! window's red close button.

use objc2::sel;
use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;

/// Repoint the menu's Quit item from `terminate:` to `performClose:`. Returns
/// `true` once done (or if there is nothing to do), `false` if the menu isn't
/// built yet and the caller should try again on a later frame.
pub fn reroute_menu_quit_to_close() -> bool {
    // AppKit menu calls must happen on the main thread; egui's update runs
    // there, but guard anyway rather than assume.
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let app = NSApplication::sharedApplication(mtm);
    let terminate = sel!(terminate:);
    let perform_close = sel!(performClose:);

    // These AppKit accessors are all `unsafe` in objc2's generated bindings;
    // they are ordinary Cocoa calls with no extra invariants for our use.
    unsafe {
        // The menu is created in `applicationDidFinishLaunching`; before that
        // the main menu is absent, so signal "retry next frame".
        let Some(main_menu) = app.mainMenu() else {
            return false;
        };
        // The Quit item lives in the application menu (the first top-level
        // item's submenu). If the structure isn't what we expect, give up
        // quietly rather than retry forever.
        let Some(app_menu) = main_menu.itemAtIndex(0).and_then(|item| item.submenu())
        else {
            return true;
        };
        for i in 0..app_menu.numberOfItems() {
            let Some(item) = app_menu.itemAtIndex(i) else {
                continue;
            };
            if item.action() == Some(terminate) {
                // Clear the target so the action walks the responder chain to
                // the key window (which answers `performClose:`), not NSApp.
                item.setTarget(None);
                item.setAction(Some(perform_close));
            }
        }
    }
    true
}
