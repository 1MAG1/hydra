// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The main window: menu bar (in-window on Windows/Linux), toolbar,
//! categories tree, download table, plus dropdown/context-menu overlays.

use crate::app::{App, El, Message};
use crate::theme;
use crate::ui::{categories, menu, table, toolbar};
use iced::widget::{column, container, mouse_area, row, rule};
use iced::Length;

pub fn view(app: &App) -> El<'_> {
    let mut root = column![].width(Length::Fill).height(Length::Fill);

    // macOS gets the native menu bar; other platforms draw the in-window bar.
    // HYDRA_GUI_INWIN_MENU=1 forces the in-window bar for testing it on macOS.
    if cfg!(not(target_os = "macos")) || std::env::var_os("HYDRA_GUI_INWIN_MENU").is_some() {
        root = root.push(menu::bar(app));
        root = root.push(rule::horizontal(1));
    }

    root = root.push(toolbar::view(app));
    root = root.push(rule::horizontal(1));

    let mut center = row![].spacing(4).padding(4);
    if app.cfg.settings.show_categories {
        center = center.push(categories::view(app));
    }
    center = center.push(table::view(app));
    root = root.push(center.width(Length::Fill).height(Length::Fill));

    let base: El<'_> = mouse_area(
        container(root)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::window),
    )
    .on_move(Message::CursorMoved)
    .into();

    // Overlays: open dropdown menu, or the row context menu.
    if let Some(kind) = app.open_menu {
        return iced::widget::stack![base, menu::bar_overlay(app, kind)]
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    }
    if let (Some(start), Some(at)) = (app.queue_menu, app.ctx_at) {
        let items: Vec<menu::Entry> = app
            .cfg
            .queues
            .iter()
            .map(|q| {
                let action = if start {
                    crate::app::MenuAction::StartQueue(q.name.clone())
                } else {
                    crate::app::MenuAction::StopQueue(q.name.clone())
                };
                menu::Entry::plain(
                    crate::i18n::tr(&q.name),
                    action,
                    if start { !q.running } else { q.running },
                )
            })
            .collect();
        return iced::widget::stack![base, menu::overlay(app, items, at)]
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    }
    if let Some(at) = app.ctx_at {
        let items = menu::context_entries(app);
        if !items.is_empty() {
            return iced::widget::stack![base, menu::overlay(app, items, at)]
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }
    }
    base
}
