// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The left "Categories" tree: All Downloads with its category children,
//! Unfinished, Finished, Grabber projects, Queues.

use crate::app::{App, El, Message, TreeSel};
use crate::{i18n::tr, icons, theme};
use iced::widget::{button, column, container, row, scrollable, svg, text};
use iced::Length;

pub fn cat_icon(name: &str) -> svg::Handle {
    match name {
        "Compressed" => icons::folder_compressed(),
        "Documents" => icons::folder_documents(),
        "Music" => icons::folder_music(),
        "Programs" => icons::folder_programs(),
        "Video" => icons::folder_video(),
        _ => icons::file_generic(),
    }
}

fn node<'a>(
    app: &App,
    indent: u16,
    expander: Option<(u8, bool)>,
    icon: svg::Handle,
    label: String,
    sel: TreeSel,
) -> El<'a> {
    let selected = app.tree_sel == sel;
    let mut r = row![]
        .spacing(4)
        .align_y(iced::Alignment::Center)
        .padding(iced::Padding {
            left: (indent * 18) as f32 + 4.0,
            ..iced::Padding::ZERO
        });
    r = match expander {
        Some((idx, open)) => r.push(
            button(svg(icons::expander(open)).width(12.0).height(12.0))
                .padding(1)
                .style(theme::btn_toolbar)
                .on_press(Message::TreeToggle(idx)),
        ),
        None => r.push(iced::widget::space::horizontal().width(14.0)),
    };
    r = r
        .push(svg(icon).width(17.0).height(17.0))
        .push(text(label).size(theme::FONT_SIZE));
    button(r)
        .padding([2, 2])
        .width(Length::Fill)
        .style(theme::btn_row(selected))
        .on_press(Message::TreeSelect(sel))
        .into()
}

pub fn view(app: &App) -> El<'_> {
    let mut col = column![].spacing(1).width(Length::Fill);

    // Header strip: "Categories" + close.
    col = col.push(
        container(
            row![
                text(tr("Categories")).size(theme::FONT_SIZE),
                iced::widget::space::horizontal(),
                button(text("×").size(theme::FONT_SIZE))
                    .padding([0, 6])
                    .style(theme::btn_toolbar)
                    .on_press(Message::Menu(crate::app::MenuAction::HideCategories)),
            ]
            .align_y(iced::Alignment::Center),
        )
        .padding([2, 6])
        .width(Length::Fill),
    );

    col = col.push(node(
        app,
        0,
        Some((0, app.tree_open[0])),
        icons::folder_all(),
        tr("All Downloads"),
        TreeSel::All,
    ));
    if app.tree_open[0] {
        for c in app.cfg.categories.iter().filter(|c| c.name != "General") {
            col = col.push(node(
                app,
                2,
                None,
                cat_icon(&c.name),
                tr(&c.name),
                TreeSel::Cat(c.name.clone()),
            ));
        }
    }
    col = col.push(node(
        app,
        0,
        Some((1, app.tree_open[1])),
        icons::folder_unfinished(),
        tr("Unfinished"),
        TreeSel::Unfinished,
    ));
    if app.tree_open[1] {
        for c in app.cfg.categories.iter().filter(|c| c.name != "General") {
            col = col.push(node(
                app,
                2,
                None,
                cat_icon(&c.name),
                tr(&c.name),
                TreeSel::UnfCat(c.name.clone()),
            ));
        }
    }
    col = col.push(node(
        app,
        0,
        Some((2, app.tree_open[2])),
        icons::folder_finished(),
        tr("Finished"),
        TreeSel::Finished,
    ));
    if app.tree_open[2] {
        for c in app.cfg.categories.iter().filter(|c| c.name != "General") {
            col = col.push(node(
                app,
                2,
                None,
                cat_icon(&c.name),
                tr(&c.name),
                TreeSel::FinCat(c.name.clone()),
            ));
        }
    }
    col = col.push(node(
        app,
        0,
        Some((3, app.tree_open[3])),
        icons::queues(),
        tr("Queues"),
        TreeSel::Queues,
    ));
    if app.tree_open[3] {
        for q in &app.cfg.queues {
            col = col.push(node(
                app,
                2,
                None,
                icons::queues(),
                tr(&q.name),
                TreeSel::Queue(q.name.clone()),
            ));
        }
    }

    container(scrollable(col).height(Length::Fill))
        .width(230.0)
        .height(Length::Fill)
        .padding(2)
        .style(theme::panel)
        .into()
}
