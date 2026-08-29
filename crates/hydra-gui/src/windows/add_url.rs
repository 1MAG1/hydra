// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! "Enter new address to download" — Add URL dialog.

use crate::app::{App, El, Message, WinKind};
use crate::windows::{dlg_btn, dlg_btn_primary};
use crate::{i18n::tr, icons, theme};
use iced::widget::{checkbox, column, container, row, svg, text, text_input};
use iced::Length;

pub fn view(app: &App) -> El<'_> {
    let st = &app.add_url;
    let title = row![
        svg(icons::add_url(true)).width(20.0).height(20.0),
        text(tr("Enter new address to download")).size(theme::FONT_SIZE + 1.0),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center);

    let address = row![
        text(tr("Address"))
            .size(theme::FONT_SIZE)
            .wrapping(iced::widget::text::Wrapping::None)
            .width(70.0),
        crate::windows::ext_hint(
            text_input("http://", &st.address)
                .on_input(Message::AddrChanged)
                .on_submit(Message::AddUrlOk)
                .size(theme::FONT_SIZE)
                .style(theme::input)
                .width(Length::Fill),
            &st.address,
        ),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center);

    let auth = checkbox(st.use_auth)
        .label(tr("Use authorization"))
        .on_toggle(Message::AddrAuthToggled)
        .size(15.0)
        .text_size(theme::FONT_SIZE)
        .style(theme::check);

    let mut creds = row![].spacing(12).align_y(iced::Alignment::Center);
    if st.use_auth {
        creds = creds
            .push(
                text(tr("Login"))
                    .size(theme::FONT_SIZE)
                    .wrapping(iced::widget::text::Wrapping::None)
                    .width(70.0),
            )
            .push(
                text_input("", &st.login)
                    .on_input(Message::AddrLogin)
                    .size(theme::FONT_SIZE)
                    .style(theme::input)
                    .width(220.0),
            )
            .push(text(tr("Password")).size(theme::FONT_SIZE))
            .push(
                text_input("", &st.password)
                    .on_input(Message::AddrPass)
                    .secure(true)
                    .size(theme::FONT_SIZE)
                    .style(theme::input)
                    .width(220.0),
            );
    }

    let error: El<'_> = match &st.error {
        Some(e) => text(e.clone())
            .size(theme::FONT_SIZE)
            .color(iced::Color::from_rgb8(0xC0, 0x2B, 0x2B))
            .into(),
        None if !st.address.trim().is_empty()
            && crate::app::site_blocked(st.address.trim(), &app.cfg.settings.dont_start_sites) =>
        {
            text(tr(
                "This site is on the auto-start block list; it will be added, not started.",
            ))
            .size(theme::FONT_SIZE)
            .color(iced::Color::from_rgb8(0xC0, 0x2B, 0x2B))
            .into()
        }
        None => iced::widget::space::horizontal().height(0.0).into(),
    };

    let buttons = column![
        dlg_btn_primary(tr("OK"), Some(Message::AddUrlOk)),
        dlg_btn(
            tr("Cancel"),
            app.win_of(WinKind::AddUrl).map(Message::CloseThis),
        ),
    ]
    .spacing(8);

    let left = column![address, auth, creds, error]
        .spacing(10)
        .width(Length::Fill);

    container(
        column![title, row![left, buttons].spacing(16).padding([8, 0]),]
            .spacing(4)
            .padding(12),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::window)
    .into()
}
