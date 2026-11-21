//! What the tests of the pages misty draws as views share: drawing a page
//! over a window, finding where a control of it was drawn, clicking there,
//! and clicking there.

use misty::testing::{TestWindow, access_tree};
use misty::{ButtonInput, Frame, Input, Point, PointerButton, Theme};

use winter_model::editor::{EditorOutcome, Page};

/// `page` drawn over the whole of `window`, told first how many text rows
/// and columns the window holds, as the window tells a page before each
/// frame.
pub(crate) fn drawn(page: &mut dyn Page, window: &TestWindow) -> Frame {
    let mut frame = window.frame();
    let bounds = frame.bounds();
    let cell = frame.cell_metrics();
    let rows = (bounds.height / cell.height) as usize;
    let cols = (bounds.width / cell.width) as usize;
    page.content(rows, cols, false);
    page.draw_view(&mut frame, bounds, &Theme::dark());
    frame
}

/// The text `frame` holds, in drawing order, one line a run.
pub(crate) fn text_of(frame: &Frame) -> String {
    misty::testing::drawn_text(frame).join("\n")
}

/// The middle of the control screen readers call `name` in `frame`.
pub(crate) fn middle_of(frame: &Frame, name: &str) -> Point {
    let tree = access_tree(frame, "page");
    let bounds = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some(name))
        .and_then(|(_, node)| node.bounds())
        .unwrap_or_else(|| panic!("{name:?} is drawn"));
    Point::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    )
}

/// A left click at `point` on `page`: what the press and then the release
/// came to.
pub(crate) fn click(
    page: &mut dyn Page,
    window: &mut TestWindow,
    point: Point,
) -> (Option<EditorOutcome>, Option<EditorOutcome>) {
    let press = Input::PointerButton(ButtonInput::press(PointerButton::Left, point));
    let release = Input::PointerButton(ButtonInput::release(PointerButton::Left, point));
    let pressed = page.view_input(&press, &mut window.cx());
    (pressed, page.view_input(&release, &mut window.cx()))
}
