// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! System-tray icon (docs/logo.png) with a control menu: the app
//! keeps downloading from the tray after the main window closes.
//!
//! macOS + Windows. Linux needs a GTK event loop for tray icons and is
//! deferred until that integration can actually be tested.

#![cfg(any(target_os = "macos", target_os = "windows"))]

use crate::app::MenuAction;
use crate::i18n::tr;
use std::cell::RefCell;
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

thread_local! {
    static CURRENT: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
}

use crate::icons::logo_rgba as decode_rgba;

/// Monochrome silhouette of the logo, menu-bar style: opaque pixels become
/// the mark, the white "H" strokes become transparent cutouts so the glyph
/// stays readable at 16 px. `white` selects the variant for dark taskbars.
fn mono_icon(white: bool) -> Option<tray_icon::Icon> {
    let (rgba, w, h) = decode_rgba()?;
    let v = if white { 255 } else { 0 };
    let mut out = vec![0u8; rgba.len()];
    for (src, dst) in rgba.chunks(4).zip(out.chunks_mut(4)) {
        let whiteish = src[0] > 220 && src[1] > 220 && src[2] > 220;
        dst[0] = v;
        dst[1] = v;
        dst[2] = v;
        dst[3] = if whiteish { 0 } else { src[3] };
    }
    tray_icon::Icon::from_rgba(out, w, h).ok()
}

fn item(label: &str, action: MenuAction) -> MenuItem {
    MenuItem::with_id(action.id(), tr(label), true, None)
}

/// Create the tray icon. Must run on the main thread after the event loop is
/// up (first `WindowOpened`); later calls are no-ops.
pub fn install(queues: &[String], power_save: bool) {
    let installed = CURRENT.with(|c| c.borrow().is_some());
    if installed {
        return;
    }
    crate::menubus::ensure_menu_handler();

    let menu = build_menu(queues, power_save);
    install_with_menu(menu);
}

/// Translated menu for the current locale.
fn build_menu(queues: &[String], power_save: bool) -> Menu {
    let menu = Menu::new();
    let _ = menu.append(&MenuItem::with_id(
        "show_main",
        tr("Show Hydra"),
        true,
        None,
    ));
    let _ = menu.append(&PredefinedMenuItem::separator());

    let downloads = Submenu::new(tr("Downloads"), true);
    let _ = downloads.append_items(&[
        &item("Add new download", MenuAction::AddNewDownload),
        &item("Pause All", MenuAction::PauseAll),
        &item("Stop All", MenuAction::StopAll),
        &PredefinedMenuItem::separator(),
        &item("Scheduler", MenuAction::Scheduler),
    ]);
    let _ = menu.append(&downloads);

    let start_q = Submenu::new(tr("Start queue"), true);
    let stop_q = Submenu::new(tr("Stop queue"), true);
    for q in queues {
        let _ = start_q.append(&item(q, MenuAction::StartQueue(q.clone())));
        let _ = stop_q.append(&item(q, MenuAction::StopQueue(q.clone())));
    }
    let _ = menu.append(&start_q);
    let _ = menu.append(&stop_q);

    let _ = menu.append(&PredefinedMenuItem::separator());
    // Checkable: the tick reflects the setting; the menu is rebuilt on
    // toggle so it stays truthful.
    let _ = menu.append(&CheckMenuItem::with_id(
        MenuAction::PowerSaveToggle.id(),
        tr("Power save mode"),
        true,
        power_save,
        None,
    ));
    let _ = menu.append(&item("Options", MenuAction::Options));
    let _ = menu.append(&item("About Hydra", MenuAction::About));
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&item("Exit", MenuAction::Exit));
    menu
}

/// Rebuild the menu (language switch, power-save toggle, queue changes).
pub fn reinstall(queues: &[String], power_save: bool) {
    CURRENT.with(|c| {
        if let Some(tray) = c.borrow().as_ref() {
            tray.set_menu(Some(Box::new(build_menu(queues, power_save))));
        }
    });
}

fn install_with_menu(menu: Menu) {
    // Left-click on the icon (Windows convention) brings the main window
    // back; on macOS a click opens the menu instead, which has Show Hydra.
    let tx = crate::menubus::sender();
    TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: tray_icon::MouseButton::Left,
            button_state: tray_icon::MouseButtonState::Up,
            ..
        } = ev
        {
            let _ = tx.send("show_main".into());
        }
    }));

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Hydra");
    // macOS: a TEMPLATE image — black + alpha that AppKit recolors itself
    // for the light/dark menu bar (and inverts while highlighted). Windows
    // has no template concept, so pick white/black from the system theme
    // once at startup.
    #[cfg(target_os = "macos")]
    {
        if let Some(icon) = mono_icon(false) {
            builder = builder.with_icon(icon).with_icon_as_template(true);
        }
    }
    #[cfg(target_os = "windows")]
    {
        let dark_taskbar = matches!(dark_light::detect(), Ok(dark_light::Mode::Dark));
        if let Some(icon) = mono_icon(dark_taskbar) {
            builder = builder.with_icon(icon);
        }
    }
    match builder.build() {
        Ok(tray) => CURRENT.with(|c| *c.borrow_mut() = Some(tray)),
        Err(e) => crate::log::warn(&format!("tray icon unavailable: {e}")),
    }
}
