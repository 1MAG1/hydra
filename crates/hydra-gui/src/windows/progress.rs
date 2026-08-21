// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The download-progress window: Download status / Speed Limiter / Options on
//! completion tabs, overall progress bar, the "start positions and download
//! progress by connections" strip, and the per-connection table.

use crate::app::{App, El, Message, ProgTab};
use crate::model::{DlState, DownloadItem};
use crate::windows::{dlg_btn, dlg_btn_primary};
use crate::{fmt, i18n::tr, theme};
use iced::widget::{
    button, checkbox, column, container, progress_bar, row, scrollable, text, text_input,
};
use iced::Length;

fn tab_btn<'a>(label: String, selected: bool, msg: Message) -> El<'a> {
    button(text(label).size(theme::FONT_SIZE))
        .padding([4, 12])
        .style(theme::btn_tab(selected))
        .on_press(msg)
        .into()
}

fn kv<'a>(k: String, v: String) -> El<'a> {
    row![
        text(k).size(theme::FONT_SIZE).width(130.0),
        text(v).size(theme::FONT_SIZE),
    ]
    .spacing(8)
    .into()
}

fn status_tab<'a>(app: &'a App, d: &'a DownloadItem) -> El<'a> {
    let pct = d
        .size
        .map(|s| fmt::pct(d.downloaded, s))
        .unwrap_or_default();
    column![
        crate::windows::ext_hint(
            text(d.url.clone())
                .size(theme::FONT_SIZE - 1.0)
                .wrapping(iced::widget::text::Wrapping::None),
            &d.url,
        ),
        row![
            text(tr("Status")).size(theme::FONT_SIZE).width(130.0),
            text(if d.status_line.is_empty() {
                d.status_text()
            } else {
                d.status_line.clone()
            })
            .size(theme::FONT_SIZE)
            .color(iced::Color::from_rgb8(0x1F, 0x3F, 0xC4)),
        ]
        .spacing(8),
        kv(
            tr("File size"),
            d.size.map(fmt::size3).unwrap_or_else(|| "?".into())
        ),
        kv(
            tr("Downloaded"),
            format!(
                "{}{}",
                fmt::size3(d.downloaded),
                if pct.is_empty() {
                    String::new()
                } else {
                    format!("  ( {pct} )")
                }
            ),
        ),
        kv(
            tr("Transfer rate"),
            fmt::rate_capped(d.rate, app.effective_limit(d))
        ),
        kv(
            tr("Time left"),
            d.eta_secs.map(fmt::eta).unwrap_or_default()
        ),
        kv(
            tr("Resume capability"),
            match d.resume {
                Some(true) => tr("Yes"),
                Some(false) => tr("No"),
                None => "?".into(),
            },
        ),
    ]
    .spacing(7)
    .into()
}

fn speed_tab<'a>(app: &'a App, d: &'a DownloadItem) -> El<'a> {
    let p = app.prog.get(&d.id).cloned().unwrap_or_default();
    column![
        kv(
            tr("Transfer rate"),
            fmt::rate_capped(d.rate, app.effective_limit(d))
        ),
        checkbox(p.limit_on)
            .label(tr("Use Speed Limiter"))
            .on_toggle(move |b| Message::ProgLimitOn(d.id, b))
            .size(15.0)
            .text_size(theme::FONT_SIZE)
            .style(theme::check),
        text(tr("Maximum download speed:"))
            .size(theme::FONT_SIZE)
            .color(theme::dim_text(&iced::Theme::Light)),
        row![
            text_input("10", &p.limit_kb)
                .on_input(move |s| Message::ProgLimitKb(d.id, s))
                .size(theme::FONT_SIZE)
                .style(theme::input)
                .width(120.0),
            text(tr("KBytes/sec")).size(theme::FONT_SIZE),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
        checkbox(p.remember_limit)
            .label(tr(
                "Remember Speed Limiter settings for this file on download stop/resume"
            ))
            .on_toggle(move |b| Message::ProgLimitRemember(d.id, b))
            .size(15.0)
            .text_size(theme::FONT_SIZE)
            .style(theme::check),
        if app.cfg.settings.show_hide_buttons {
            crate::windows::dlg_btn(tr("Hide tab"), Some(Message::ProgHideTab(d.id, 1)))
        } else {
            iced::widget::space::horizontal().height(0.0).into()
        },
    ]
    .spacing(10)
    .into()
}

fn completion_tab<'a>(app: &'a App, d: &'a DownloadItem) -> El<'a> {
    column![
        row![
            text(tr("Save To:")).size(theme::FONT_SIZE).width(70.0),
            text(d.full_path().to_string_lossy().into_owned()).size(theme::FONT_SIZE),
        ]
        .spacing(8),
        checkbox(app.cfg.settings.show_complete_dialog)
            .label(tr("Show download complete dialog"))
            .size(15.0)
            .text_size(theme::FONT_SIZE)
            .style(theme::check),
        text(tr(
            "These settings are unavailable when \"Show download complete dialog\" is turned on"
        ))
        .size(theme::FONT_SIZE)
        .color(theme::dim_text(&iced::Theme::Light)),
        if app.cfg.settings.show_hide_buttons {
            crate::windows::dlg_btn(tr("Hide tab"), Some(Message::ProgHideTab(d.id, 2)))
        } else {
            iced::widget::space::horizontal().height(0.0).into()
        },
    ]
    .spacing(10)
    .into()
}

/// The blue chunk strip: 120 buckets over the object, filled where held.
fn chunk_strip<'a>(d: &DownloadItem) -> El<'a> {
    const BUCKETS: usize = 120;
    let mut filled = [false; BUCKETS];
    if let Some(size) = d.size.filter(|s| *s > 0) {
        for &(lo, hi) in &d.held {
            let a = (lo as f64 / size as f64 * BUCKETS as f64) as usize;
            let b = ((hi as f64 / size as f64 * BUCKETS as f64).ceil() as usize).min(BUCKETS);
            for f in filled.iter_mut().take(b).skip(a) {
                *f = true;
            }
        }
    }
    let mut r = row![].spacing(0).width(Length::Fill).height(10.0);
    for on in filled {
        r = r.push(
            container(iced::widget::space::horizontal())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(if on {
                    theme::chunk_on
                } else {
                    theme::chunk_off
                }),
        );
    }
    container(r)
        .width(Length::Fill)
        .padding(1)
        .style(theme::panel)
        .into()
}

fn conn_table<'a>(d: &'a DownloadItem) -> El<'a> {
    // Header stays put; only the rows scroll.
    let header = row![
        container(text(tr("N.")).size(theme::FONT_SIZE))
            .width(50.0)
            .padding([2, 6]),
        container(text(tr("Downloaded")).size(theme::FONT_SIZE))
            .width(150.0)
            .padding([2, 6]),
        container(text(tr("Info")).size(theme::FONT_SIZE))
            .width(Length::Fill)
            .padding([2, 6]),
    ]
    .spacing(0);
    // Exactly 8 rows are visible (blank band rows pad shorter transfers so
    // the box looks identical for 4 vs 8 connections); more connections
    // scroll inside the same viewport instead of growing the box.
    const ROW_H: f32 = 20.0;
    const VISIBLE: usize = 8;
    let mut rows = column![].width(Length::Fill);
    let n_rows = d.conns.len().max(VISIBLE);
    let blank = crate::model::ConnRow::default();
    for i in 0..n_rows {
        let c = d.conns.get(i).unwrap_or(&blank);
        rows = rows.push(
            container(
                row![
                    container(text(format!("{}", i + 1)).size(theme::FONT_SIZE))
                        .width(50.0)
                        .padding([1, 6]),
                    container(
                        text(if c.downloaded > 0 {
                            fmt::size3(c.downloaded)
                        } else {
                            String::new()
                        })
                        .size(theme::FONT_SIZE)
                    )
                    .width(150.0)
                    .padding([1, 6]),
                    container(text(c.info.clone()).size(theme::FONT_SIZE))
                        .width(Length::Fill)
                        .padding([1, 6]),
                ]
                .spacing(0)
                .height(ROW_H),
            )
            .style(|t: &iced::Theme| container::Style {
                background: Some(iced::Background::Color(if theme::is_dark(t) {
                    iced::Color::from_rgb8(0x33, 0x2E, 0x38)
                } else {
                    iced::Color::from_rgb8(0xC9, 0xBF, 0xC9)
                })),
                text_color: Some(theme::text_color(t)),
                ..Default::default()
            }),
        );
    }
    // The viewport is exactly VISIBLE rows tall — the dialog ends right after
    // the table instead of stretching an empty band below the connections.
    container(column![
        header,
        scrollable(rows)
            .height(ROW_H * VISIBLE as f32)
            .width(Length::Fill),
    ])
    .width(Length::Fill)
    .style(theme::panel)
    .into()
}

pub fn view(app: &App, id: crate::model::DlId) -> El<'_> {
    let Some(d) = app.item(id) else {
        return container(text(""))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::window)
            .into();
    };
    let p = app.prog.get(&id).cloned().unwrap_or_default();
    let s = &app.cfg.settings;

    let mut tabs = row![tab_btn(
        tr("Download status"),
        p.tab == ProgTab::Status,
        Message::ProgTabSet(id, ProgTab::Status),
    )]
    .spacing(1);
    if s.show_speed_tab {
        tabs = tabs.push(tab_btn(
            tr("Speed Limiter"),
            p.tab == ProgTab::Speed,
            Message::ProgTabSet(id, ProgTab::Speed),
        ));
    }
    if s.show_completion_tab {
        tabs = tabs.push(tab_btn(
            tr("Options on completion"),
            p.tab == ProgTab::Completion,
            Message::ProgTabSet(id, ProgTab::Completion),
        ));
    }

    let tab_body: El<'_> = match p.tab {
        ProgTab::Status => status_tab(app, d),
        ProgTab::Speed => speed_tab(app, d),
        ProgTab::Completion => completion_tab(app, d),
    };

    let active = d.state.is_active();
    let pause_label = if active { tr("Pause") } else { tr("Start") };

    // The tab body is pinned to one height for every tab, so the progress
    // bar, buttons and connections panel below never jump when switching.
    let mut col = column![
        tabs,
        container(scrollable(tab_body))
            .width(Length::Fill)
            .height(210.0)
            .padding(12)
            .style(theme::panel),
        progress_bar(0.0..=1.0, d.disp_progress.clamp(0.0, 1.0))
            .girth(18.0)
            .style(theme::progress),
        row![
            dlg_btn(
                if p.details {
                    format!("<< {}", tr("Hide details"))
                } else {
                    format!("{} >>", tr("Show details"))
                },
                Some(Message::ProgToggleDetails(id)),
            ),
            iced::widget::space::horizontal(),
            dlg_btn_primary(pause_label, Some(Message::ProgPauseResume(id))),
            dlg_btn(tr("Cancel"), Some(Message::ProgCancel(id))),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(10)
    .padding(12)
    .width(Length::Fill);

    if p.details {
        col = col.push(crate::windows::centered(
            tr("Start positions and download progress by connections"),
            theme::FONT_SIZE,
        ));
        col = col.push(chunk_strip(d));
        col = col.push(conn_table(d));
    } else {
        col = col.push(iced::widget::space::vertical());
    }
    let col = col.height(Length::Fill);

    container(col)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::window)
        .into()
}

/// Window title: `4% meilisearch-linux-aarch64` while receiving.
pub fn title(app: &App, id: crate::model::DlId) -> String {
    match app.item(id) {
        Some(d) => {
            let pct = d
                .size
                .filter(|s| *s > 0 && d.state != DlState::Complete)
                .map(|s| format!("{:.0}% ", d.downloaded as f64 * 100.0 / s as f64))
                .unwrap_or_default();
            format!("{pct}{}", d.file_name)
        }
        None => "Hydra".into(),
    }
}
