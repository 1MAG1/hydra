// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

//! The download list: File Name | Q | Size | Status | Time left |
//! Transfer rate | Last Try Date | Description, with a flat listview-style
//! header, sortable columns and a horizontal scrollbar. Q is the
//! icon-only queue-membership strip.

use crate::app::{App, El, Message, SortKey};
use crate::model::DownloadItem;
use crate::{fmt, i18n::tr, icons, theme};
use iced::widget::{column, container, mouse_area, row, scrollable, svg, text};
use iced::Length;

const COLS: [(&str, f32, Option<SortKey>); 8] = [
    ("File Name", 300.0, Some(SortKey::Name)),
    // "Q" strip: icon-only, so the width stays at the resize minimum.
    ("Q", 40.0, Some(SortKey::Queue)),
    ("Size", 110.0, Some(SortKey::Size)),
    ("Status", 120.0, Some(SortKey::Status)),
    ("Time left", 130.0, Some(SortKey::TimeLeft)),
    ("Transfer rate", 150.0, Some(SortKey::Rate)),
    ("Last Try Date", 175.0, Some(SortKey::LastTry)),
    ("Description", 200.0, Some(SortKey::Description)),
];

/// Width of the draggable divider between header cells.
const GRIP: f32 = 6.0;

pub fn default_widths() -> Vec<f32> {
    COLS.iter().map(|c| c.1).collect()
}

fn widths(app: &App) -> Vec<f32> {
    if app.col_widths.len() == COLS.len() {
        app.col_widths.clone()
    } else {
        default_widths()
    }
}

fn total_width(w: &[f32]) -> f32 {
    w.iter().sum::<f32>() + GRIP * w.len() as f32
}

fn header(app: &App) -> El<'_> {
    let w = widths(app);
    let mut r = row![].spacing(0);
    for (i, (label, _, key)) in COLS.iter().enumerate() {
        let arrow = match key {
            Some(k) if app.sort.0 == *k => {
                if app.sort.1 {
                    " ▴"
                } else {
                    " ▾"
                }
            }
            _ => "",
        };
        let cell = container(
            text(format!("{}{arrow}", tr(label)))
                .size(theme::FONT_SIZE)
                .wrapping(iced::widget::text::Wrapping::None),
        )
        .padding([3, 6])
        .width(w[i])
        .height(24.0)
        .clip(true)
        .style(theme::header_cell(app.hover_col == Some(i)));
        // A container under a `mouse_area`, not a `button`: a button forces the
        // hand cursor, and `mouse_area::interaction` can only override a child
        // that asks for none.
        let cell = mouse_area(cell)
            .interaction(iced::mouse::Interaction::Idle)
            .on_enter(Message::HeaderEnter(i))
            .on_exit(Message::HeaderExit(i));
        r = match key {
            Some(k) => r.push(cell.on_press(Message::SortBy(*k))),
            None => r.push(cell),
        };
        // Drag grip on the column's right edge, drawing the same hairline the
        // body rows do so the separators run continuously like a listview.
        r = r.push(
            mouse_area(rule_grip())
                .interaction(iced::mouse::Interaction::ResizingHorizontally)
                .on_press(Message::ColResizeStart(i)),
        );
    }
    column![r, row_line(total_width(&w))].into()
}

fn cell<'a>(content: El<'a>, w: f32) -> El<'a> {
    container(content)
        .width(w)
        .height(24.0)
        .padding([2, 6])
        .clip(true)
        .into()
}

/// The divider strip between columns: full drag-handle width, drawing one
/// centred 1 px vertical line — a single hairline, no boxed gaps.
fn rule_grip<'a>() -> El<'a> {
    container(
        container(iced::widget::space::horizontal())
            .width(1.0)
            .height(Length::Fill)
            .style(|t: &iced::Theme| container::Style {
                background: Some(iced::Background::Color(theme::grid_line(t))),
                ..Default::default()
            }),
    )
    .width(GRIP)
    .height(24.0)
    .padding(iced::Padding {
        left: (GRIP - 1.0) / 2.0,
        ..iced::Padding::ZERO
    })
    .into()
}

/// 1 px horizontal rule under a row.
fn row_line<'a>(tw: f32) -> El<'a> {
    container(iced::widget::space::horizontal())
        .width(tw)
        .height(1.0)
        .style(|t: &iced::Theme| container::Style {
            background: Some(iced::Background::Color(theme::grid_line(t))),
            ..Default::default()
        })
        .into()
}

/// An empty ruled row so the list shows a full grid below the data. It is a
/// press target too: a drag begun here sweeps into the rows above/below it,
/// and a plain click on it clears the selection.
fn filler_row<'a>(w: &[f32], tw: f32) -> El<'a> {
    let mut r = row![].spacing(0);
    for width in w {
        r = r.push(cell(text("").size(theme::FONT_SIZE).into(), *width));
        r = r.push(rule_grip());
    }
    mouse_area(column![r, row_line(tw)])
        .interaction(iced::mouse::Interaction::Idle)
        .on_press(Message::EmptyPress)
        .on_enter(Message::EmptyEnter)
        .into()
}

/// File-type glyph for a row: the item's category (or, when unset, the
/// extension-based guess) picks the same icon the category tree uses.
fn file_icon(app: &App, d: &DownloadItem) -> iced::widget::svg::Handle {
    let cat = d
        .category
        .clone()
        .or_else(|| crate::model::categorize(&d.file_name, &app.cfg.categories));
    match cat.as_deref() {
        Some(c) => crate::ui::categories::cat_icon(c),
        None => icons::file_generic(),
    }
}

/// The Q cell: shows the yellow queues icon while the item belongs to a
/// queue and drops it once the download completes — no queue-name text.
fn queue_glyph<'a>(d: &DownloadItem) -> El<'a> {
    if d.queue.is_some() && !matches!(d.state, crate::model::DlState::Complete) {
        container(svg(icons::queues()).width(14.0).height(14.0))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::Alignment::Center)
            .align_y(iced::Alignment::Center)
            .into()
    } else {
        text("").size(theme::FONT_SIZE).into()
    }
}

fn data_row<'a>(app: &App, d: &'a DownloadItem) -> El<'a> {
    let w = widths(app);
    let selected = app.selected.contains(&d.id);
    let size_txt = d.size.map(fmt::size2).unwrap_or_default();
    let eta_txt = match d.state {
        crate::model::DlState::Receiving => d.eta_secs.map(fmt::eta).unwrap_or_default(),
        _ => String::new(),
    };
    let rate_txt = if d.state.is_active() && d.rate > 0.0 {
        fmt::rate_steady(d.rate, app.effective_limit(d))
    } else {
        String::new()
    };
    let grip = rule_grip;
    let content = row![
        cell(
            row![
                svg(file_icon(app, d)).width(15.0).height(15.0),
                text(&d.file_name)
                    .size(theme::FONT_SIZE)
                    .wrapping(iced::widget::text::Wrapping::None),
            ]
            .spacing(5)
            .align_y(iced::Alignment::Center)
            .into(),
            w[0],
        ),
        grip(),
        cell(queue_glyph(d), w[1]),
        grip(),
        cell(text(size_txt).size(theme::FONT_SIZE).into(), w[2]),
        grip(),
        cell(text(d.status_text()).size(theme::FONT_SIZE).into(), w[3]),
        grip(),
        cell(text(eta_txt).size(theme::FONT_SIZE).into(), w[4]),
        grip(),
        cell(text(rate_txt).size(theme::FONT_SIZE).into(), w[5]),
        grip(),
        cell(
            text(d.last_try.map(fmt::date).unwrap_or_default())
                .size(theme::FONT_SIZE)
                .into(),
            w[6]
        ),
        grip(),
        cell(text(&d.description).size(theme::FONT_SIZE).into(), w[7]),
        grip(),
    ]
    .spacing(0);

    let tw = total_width(&w);
    // The row is a plain container, not a button: `button` reports its press
    // only on mouse-*release* (so the drag never knows where it started) and
    // always paints the hand cursor. `mouse_area` presses on the way down,
    // which is what both drag-selection and the arrow cursor need.
    column![
        mouse_area(
            container(content)
                .width(tw)
                .style(theme::row_cell(selected, app.hover_row == Some(d.id))),
        )
        .interaction(iced::mouse::Interaction::Idle)
        .on_press(Message::RowClick(d.id))
        .on_right_press(Message::RowRightClick(d.id))
        .on_enter(Message::RowEnter(d.id))
        .on_exit(Message::RowExit(d.id)),
        row_line(tw),
    ]
    .into()
}

pub fn view(app: &App) -> El<'_> {
    let w = widths(app);
    let tw = total_width(&w);
    let mut rows = column![].width(tw);
    let mut n = 0usize;
    for d in app.visible() {
        rows = rows.push(data_row(app, d));
        n += 1;
    }
    // Ruled empty rows below the data. Enough to cover any
    // realistic window height; scrolling past them is fine.
    for _ in n..n.max(60) {
        rows = rows.push(filler_row(&w, tw));
    }
    let body = column![header(app), rows].width(tw);
    container(
        scrollable(body)
            .direction(scrollable::Direction::Both {
                vertical: scrollable::Scrollbar::default(),
                horizontal: scrollable::Scrollbar::default(),
            })
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::panel)
    .into()
}
