//! The menu bar item: quota at a glance, what's waiting, open or quit. The window keeps it up to
//! date (`update_tray`) as it polls; closing the window only hides it, so sessions, notices and
//! the menu bar item go on until Quit.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager as _, Runtime};

const ID: &str = "main";

pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let mut tray = TrayIconBuilder::with_id(ID)
        .tooltip("anchorleg")
        .menu(&menu(app, &[], 0)?)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "open" => show(app),
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

pub fn show<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn menu<R: Runtime>(
    app: &AppHandle<R>,
    lines: &[String],
    waiting: usize,
) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    for (i, l) in lines.iter().enumerate() {
        menu.append(&MenuItem::with_id(
            app,
            format!("line-{i}"),
            l,
            false,
            None::<&str>,
        )?)?;
    }
    if waiting > 0 {
        let text = format!("{waiting} waiting for you");
        menu.append(&MenuItem::with_id(app, "open", text, true, None::<&str>)?)?;
    }
    if !lines.is_empty() || waiting > 0 {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append(&MenuItem::with_id(
        app,
        "open",
        "Open anchorleg",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        "quit",
        "Quit anchorleg",
        true,
        Some("CmdOrCtrl+Q"),
    )?)?;
    Ok(menu)
}

/// `title`: a few characters beside the icon (e.g. the fullest window, `!` when waiting).
#[tauri::command]
pub fn update_tray(
    app: AppHandle,
    title: Option<String>,
    lines: Vec<String>,
    waiting: usize,
) -> Result<(), String> {
    let tray = app.tray_by_id(ID).ok_or("no menu bar item")?;
    tray.set_menu(Some(
        menu(&app, &lines, waiting).map_err(|e| e.to_string())?,
    ))
    .map_err(|e| e.to_string())?;
    tray.set_title(title.as_deref()).map_err(|e| e.to_string())
}
