// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The update dialog: "a new version is available", its release notes, and
//! the Update Now / Cancel pair. While an update runs the button row is
//! replaced by a progress bar; the notes stay on screen throughout.

use crate::app::{App, El, Message, UpdatePhase};
use crate::windows::{dlg_btn, dlg_btn_primary};
use crate::{fmt, i18n::tr, theme};
use iced::widget::{button, column, container, image, progress_bar, row, scrollable, text};
use iced::Length;

pub fn view(app: &App) -> El<'_> {
    let Some(info) = &app.updater.info else {
        // The window can outlive a cancelled offer by one frame.
        return container(text(""))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::window)
            .into();
    };

    let header = row![
        image(image::Handle::from_bytes(
            include_bytes!("../../../../docs/logo.png").as_slice()
        ))
        .width(48.0)
        .height(48.0),
        column![
            text(tr("A new version of Hydra is available")).size(theme::FONT_SIZE + 3.0),
            text(format!(
                "{} {}   —   {} {}",
                tr("New version:"),
                info.version,
                tr("You have:"),
                env!("CARGO_PKG_VERSION")
            ))
            .size(theme::FONT_SIZE)
            .color(theme::dim_text(&iced::Theme::Light)),
        ]
        .spacing(4),
    ]
    .spacing(12)
    .align_y(iced::Alignment::Center);

    // Release notes, rendered from the API's markdown.
    let notes = container(scrollable(notes_body(&info.notes)).width(Length::Fill))
        .padding(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::panel);

    let status: El<'_> = match &app.updater.phase {
        UpdatePhase::Idle => text(format!(
            "{} {} ({})",
            tr("Download:"),
            info.asset_name,
            fmt::size2(info.size)
        ))
        .size(theme::FONT_SIZE - 1.0)
        .color(theme::dim_text(&iced::Theme::Light))
        .into(),
        UpdatePhase::Downloading { got, total } => {
            let frac = match total {
                Some(t) if *t > 0 => *got as f32 / *t as f32,
                _ => 0.0,
            };
            let label = match total {
                Some(t) if *t > 0 => format!(
                    "{} {} / {} ({})",
                    tr("Downloading update..."),
                    fmt::size2(*got),
                    fmt::size2(*t),
                    fmt::pct(*got, *t)
                ),
                _ => format!("{} {}", tr("Downloading update..."), fmt::size2(*got)),
            };
            column![
                progress_bar(0.0..=1.0, frac.clamp(0.0, 1.0))
                    .girth(14.0)
                    .style(theme::progress),
                text(label).size(theme::FONT_SIZE - 1.0),
            ]
            .spacing(6)
            .into()
        }
        UpdatePhase::Verifying => text(tr("Verifying the downloaded file..."))
            .size(theme::FONT_SIZE)
            .into(),
        UpdatePhase::Preparing => text(tr("Preparing the update..."))
            .size(theme::FONT_SIZE)
            .into(),
        UpdatePhase::Restarting => text(tr("Hydra will now restart to finish the update."))
            .size(theme::FONT_SIZE)
            .into(),
        UpdatePhase::Failed(e) => text(format!("{} {e}", tr("Update failed:")))
            .size(theme::FONT_SIZE)
            .color(iced::Color::from_rgb8(0xC4, 0x2B, 0x1C))
            .into(),
    };

    // The release page link is always harmless; the action buttons depend on
    // the phase.
    let page_btn: El<'_> = button(
        text(tr("View release page"))
            .size(theme::FONT_SIZE - 1.0)
            .color(iced::Color::from_rgb8(0x1F, 0x3F, 0xC4)),
    )
    .padding([4, 2])
    .style(|_t, _s| button::Style::default())
    .on_press(Message::UpdateOpenPage)
    .into();

    let buttons: El<'_> = match &app.updater.phase {
        UpdatePhase::Idle => row![
            page_btn,
            iced::widget::space::horizontal(),
            dlg_btn_primary(tr("Update Now"), Some(Message::UpdateNow)),
            dlg_btn(tr("Cancel"), Some(Message::UpdateCancel)),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into(),
        UpdatePhase::Downloading { .. } => row![
            iced::widget::space::horizontal(),
            dlg_btn(tr("Cancel"), Some(Message::UpdateCancel)),
        ]
        .spacing(8)
        .into(),
        UpdatePhase::Verifying | UpdatePhase::Preparing | UpdatePhase::Restarting => row![
            iced::widget::space::horizontal(),
            dlg_btn(tr("Cancel"), None),
        ]
        .spacing(8)
        .into(),
        UpdatePhase::Failed(_) => row![
            page_btn,
            iced::widget::space::horizontal(),
            dlg_btn_primary(tr("Try Again"), Some(Message::UpdateNow)),
            dlg_btn(tr("Close"), Some(Message::UpdateCancel)),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into(),
    };

    container(
        column![header, notes, status, buttons]
            .spacing(12)
            .padding(16),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::window)
    .into()
}

/// Render GitHub-flavoured markdown release notes as simple styled lines:
/// headings become emphasised text, list markers become bullets, and inline
/// `**bold**`/`` `code` ``/link syntax is stripped down to its text. Good
/// enough to read a changelog; anything fancier belongs in the browser via
/// "View release page".
fn notes_body(notes: &str) -> El<'_> {
    let mut col = column![].spacing(5);
    let mut prev_blank = false;
    for raw in notes.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            // Collapse runs of blank lines into one paragraph gap.
            if !prev_blank {
                col = col.push(text("").size(4.0));
            }
            prev_blank = true;
            continue;
        }
        prev_blank = false;
        let trimmed = line.trim_start();
        let el: El<'_> = if let Some(h) = trimmed
            .strip_prefix("### ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("# "))
        {
            text(clean_inline(h)).size(theme::FONT_SIZE + 2.0).into()
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            row![
                text("•").size(theme::FONT_SIZE),
                text(clean_inline(item)).size(theme::FONT_SIZE),
            ]
            .spacing(6)
            .into()
        } else {
            text(clean_inline(trimmed)).size(theme::FONT_SIZE).into()
        };
        col = col.push(el);
    }
    col.width(Length::Fill).into()
}

/// Strip the inline markdown that would otherwise render as literal noise:
/// `**`, backticks, and `[text](url)` down to `text`.
fn clean_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '`' => {}
            '[' => {
                // `[text](url)` -> `text`; a bare `[` stays as-is.
                let rest: String = chars.clone().collect();
                if let Some(close) = rest.find(']') {
                    if rest[close..].starts_with("](") {
                        if let Some(end) = rest[close..].find(')') {
                            out.push_str(&rest[..close]);
                            for _ in 0..close + end + 1 {
                                chars.next();
                            }
                            continue;
                        }
                    }
                }
                out.push('[');
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::clean_inline;

    #[test]
    fn inline_markdown_strips_to_text() {
        assert_eq!(clean_inline("**Bold** fix"), "Bold fix");
        assert_eq!(clean_inline("use `hydra update`"), "use hydra update");
        assert_eq!(
            clean_inline("by [someone](https://github.com/x) in #12"),
            "by someone in #12"
        );
        assert_eq!(clean_inline("a bare [ bracket"), "a bare [ bracket");
    }
}
