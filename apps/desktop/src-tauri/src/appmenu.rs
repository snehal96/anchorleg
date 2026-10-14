//! The macOS menu bar: Settings under the app menu (⌘,), the standard Edit and View menus, and
//! the app's pages under Window. A pick shows the window and tells it where to go (`navigate`).

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter as _, Runtime};

/// Menu ids are `nav:<page>`; the window knows the pages.
const PAGES: [(&str, &str, &str); 4] = [
    ("nav:new", "New Task", "CmdOrCtrl+N"),
    ("nav:sessions", "Tasks", "CmdOrCtrl+0"),
    ("nav:accounts", "Accounts", "CmdOrCtrl+1"),
    ("nav:usage", "Usage", "CmdOrCtrl+2"),
];

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let sep = || PredefinedMenuItem::separator(app);
    let settings = MenuItem::with_id(app, "nav:settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
    let app_menu = Submenu::with_items(
        app,
        "anchorleg",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About anchorleg"), None)?,
            &sep()?,
            &settings,
            &sep()?,
            &PredefinedMenuItem::services(app, None)?,
            &sep()?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &sep()?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &sep()?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[&PredefinedMenuItem::fullscreen(app, None)?],
    )?;
    let pages = PAGES
        .iter()
        .map(|(id, text, key)| MenuItem::with_id(app, *id, *text, true, Some(*key)))
        .collect::<tauri::Result<Vec<_>>>()?;
    let window = Submenu::new(app, "Window", true)?;
    for (i, p) in pages.iter().enumerate() {
        window.append(p)?;
        if i == 0 {
            window.append(&sep()?)?;
        }
    }
    window.append(&sep()?)?;
    window.append(&PredefinedMenuItem::minimize(app, None)?)?;
    window.append(&PredefinedMenuItem::close_window(app, None)?)?;
    Menu::with_items(app, &[&app_menu, &edit, &view, &window])
}

/// A pick from the menu bar: bring the window up and send it to the page.
pub fn handle<R: Runtime>(app: &AppHandle<R>, id: &str) {
    if let Some(page) = id.strip_prefix("nav:") {
        crate::tray::show(app);
        let _ = app.emit("navigate", page);
    }
}
