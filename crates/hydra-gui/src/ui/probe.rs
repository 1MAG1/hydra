//! Pointer position without a message per motion event.
//!
//! `mouse_area::on_move` publishes a message for every pointer motion, and in
//! iced *any* message rebuilds and relayouts the whole window — so merely
//! sweeping the mouse across the list repainted the table at the pointer's
//! event rate (120+ Hz on a trackpad) even though nothing on screen changed.
//!
//! Most of the app only needs the *last* position, and only at the moment a
//! menu opens or a drag starts. [`CursorProbe`] is a zero-sized widget that
//! records it into a shared cell during the event pass and publishes nothing,
//! so those call sites read a fresh position while the idle pointer costs no
//! redraws. `on_move` is then attached only while a rubber-band or a column
//! resize is actually in flight (see `windows::main_win`), where every motion
//! genuinely changes what is drawn.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use iced::advanced::widget::{tree, Widget};
use iced::advanced::{layout, mouse, renderer, Clipboard, Layout, Shell};
use iced::{Element, Event, Length, Point, Rectangle, Size};

/// The last pointer position seen anywhere in the window.
///
/// Two atomics rather than a `Cell<Point>` so the widget stays `Send`/`Sync`
/// like every other element in the tree; the value is written from the event
/// pass and read from `update`, never concurrently.
#[derive(Debug, Default)]
pub struct CursorCell {
    x: AtomicU32,
    y: AtomicU32,
}

impl CursorCell {
    pub fn set(&self, p: Point) {
        self.x.store(p.x.to_bits(), Ordering::Relaxed);
        self.y.store(p.y.to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self) -> Point {
        Point::new(
            f32::from_bits(self.x.load(Ordering::Relaxed)),
            f32::from_bits(self.y.load(Ordering::Relaxed)),
        )
    }
}

pub struct CursorProbe {
    cell: Arc<CursorCell>,
}

/// A 0×0 element that keeps `cell` up to date. Place it in the window's root
/// layer — inside a `scrollable` the cursor arrives translated by the scroll
/// offset, which is not what menu placement wants.
pub fn cursor_probe<'a, Message, Theme, Renderer>(
    cell: Arc<CursorCell>,
) -> Element<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer + 'a,
    Message: 'a,
    Theme: 'a,
{
    Element::new(CursorProbe { cell })
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for CursorProbe
where
    Renderer: iced::advanced::Renderer,
{
    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Shrink,
            height: Length::Shrink,
        }
    }

    fn layout(
        &mut self,
        _tree: &mut tree::Tree,
        _renderer: &Renderer,
        _limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(Size::ZERO)
    }

    fn draw(
        &self,
        _tree: &tree::Tree,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }

    fn update(
        &mut self,
        _tree: &mut tree::Tree,
        event: &Event,
        _layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        _shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        record(&self.cell, event, cursor);
    }
}

/// The probe's whole job, split out so it can be tested without a renderer.
fn record(cell: &CursorCell, event: &Event, cursor: mouse::Cursor) {
    if matches!(event, Event::Mouse(mouse::Event::CursorMoved { .. })) {
        if let Some(p) = cursor.position() {
            cell.set(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_lands_in_the_cell_and_nothing_else_does() {
        let cell = CursorCell::default();
        let at = Point::new(12.0, 34.0);

        record(
            &cell,
            &Event::Mouse(mouse::Event::CursorMoved { position: at }),
            mouse::Cursor::Available(at),
        );
        assert_eq!(cell.get(), at);

        // A press must not move the recorded position, and a motion event
        // with the pointer outside the window must not zero it.
        record(
            &cell,
            &Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            mouse::Cursor::Available(Point::new(99.0, 99.0)),
        );
        record(
            &cell,
            &Event::Mouse(mouse::Event::CursorMoved { position: at }),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(cell.get(), at);
    }
}
