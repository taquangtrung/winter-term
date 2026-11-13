//! [`EditorView`]: one view of a Cherry [`Editor`] as a misty widget. The
//! app owns the editor (its documents, views, and
//! undo histories) and an `EditorView` per view it shows; the widget
//! feeds the view keys, keeps it sized to the area it is drawn in, and
//! paints it through the cell grid, Cherry's styles mapped to theme colors
//! and its cursor drawn as a block or a bar.
//!
//! Keys come from [`KeyInput`](misty::keys::KeyInput). What the editor asks
//! of its host (save a document, close a view, show a message) comes back
//! from the lent [`Widget::handle`](misty::Widget::handle) as [`EditorResponse::Requests`], for the
//! app to act on in order. A half-typed key sequence (the `g` of `g g`)
//! times out on the deadline [`EditorView::about_to_wait`] returns. While
//! a search (`/`, `?`) or command line (`:`) prompt is open, the view gives
//! its bottom row to the prompt, whose cursor takes over.
//!
//! Input method events go to the lent [`Widget::handle`](misty::Widget::handle): text being
//! composed is drawn underlined over the cells from the cursor on, and
//! committed text is typed as keys while the view takes text (Insert mode
//! or a prompt). [`EditorView::ime_area`] says where the cursor is then,
//! for [`ImeFocus::follow`](misty::ImeFocus::follow).
//!
//! The mouse goes to the lent [`Widget::handle`](misty::Widget::handle): a press moves the
//! cursor to the character under the pointer, a drag selects, a double
//! click selects a word and a triple click a line, and the wheel scrolls,
//! all through Cherry's own mouse handling, in cells counted from the
//! view's top left corner.
//!
//! While Cherry's Insert-mode completion menu is open (`C-x C-n` and the
//! like), the view draws it as a popup under the text being completed,
//! the match typed now on the selection color.
//!
//! Text a syntax highlighter marks (with the `syntax` feature, through
//! `cherry-syntax`) is drawn in the theme's
//! [`SyntaxColors`](misty::syntax::SyntaxColors) for its highlight
//! name. Decorations the app sets on a document are drawn as underlines
//! by kind (an undercurl in the danger color for an error, in the warning
//! color for a warning, in the accent color for information, a dotted
//! muted line for a hint), and signs in the sign column in the same
//! colors.
//!
//! When the view scrolls by a few lines its text glides to the new place
//! rather than jumping: the lines scrolled out slide away as the new ones
//! slide in. A scroll that cannot be shown that way (wrapped lines,
//! relative line numbers, a jump of a screen or more) jumps. The glide
//! needs [`EditorView::about_to_wait`] called each loop, and
//! [`EditorView::set_reduced_motion`] turns it off.

mod completion;

use std::mem;
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

use cherry::{
    ClickCounter, Clipboard, ClipboardSelection, DecorationKind, Document, Editor, Handled,
    HostRequest, Layers, Mode, MouseEvent, MouseEventKind, Position, ScrollOffset, Surface, View,
    ViewId, render_prompt, render_view,
};
use misty::accesskit::{Node, NodeId, Role, TextPosition, TextSelection};
use misty::cell::{self, Attributes, CellBuffer, CellSource, CellWidth, Color, UnderlineStyle};
use misty::color::Rgb;
use misty::geometry::{Point, Rect};
use misty::key::{Key, KeyCode, Mods};
use misty::scroll::SmoothScroll;
use misty::text;
use misty::theme::Theme;
use misty::{
    CellMetrics, ClipboardKind, EventCx, FocusId, Font, Frame, ImeInput, Input, Outcome,
    PointerButton, Preedit, PreeditChange, SystemClipboard, WheelDelta, Widget,
};

use self::completion::draw_completion;
use super::clock;
use super::keys::{mods_to_cherry, to_cherry};

// ========================================================================
// Constants
// ========================================================================

/// The longest gap between two presses on one cell of a double or triple
/// click.
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(400);

/// The rows Cherry scrolls for one notch of the wheel, which a touchpad's
/// pixel scrolling is counted in.
const ROWS_PER_NOTCH: f32 = 3.0;

/// What a blank cell at a level of indentation shows: a thin guide line.
const INDENT_GUIDE: &str = "\u{2502}";

/// The palette colors phrases lit by hand take, in turn: the six named
/// colors from red to cyan.
const PHRASE_COLORS: [usize; 6] = [1, 2, 3, 4, 5, 6];

// ========================================================================
// Data Structures
// ========================================================================

/// One view of an [`Editor`] the app owns, drawn as a widget.
#[derive(Debug)]
pub struct EditorView {
    /// The view's cells as the grid draws them.
    cells: CellBuffer,
    /// Turns quick presses on one cell into double and triple clicks.
    clicks: ClickCounter,
    /// The cursor's cell in the last frame, in physical pixels, or `None`
    /// when it was out of sight.
    cursor_area: Option<Rect>,
    /// The font the text is drawn in, or `None` for the window's.
    font: Option<Font>,
    /// How far from its place the text was drawn in the last frame, in
    /// rows.
    drawn_offset: f32,
    /// How the host's highlight ids are drawn, by id, once it gave them;
    /// `None` draws the ids of `cherry-syntax` in the theme's syntax colors.
    highlights: Option<Vec<HighlightStyle>>,
    /// Where the view's rows were drawn in the last frame, and the size of
    /// a cell, to find the cell under the pointer; `None` before the first
    /// frame.
    layout: Option<ViewLayout>,
    /// The modifier keys held with the last pointer input, for
    /// Shift-click.
    modifiers: Mods,
    /// The glide of the text toward its place after a scroll, in rows
    /// still to go: positive while the text slides up into place.
    motion: SmoothScroll,
    /// Scratch cells the next frame is painted into.
    next: CellBuffer,
    /// Where the pointer is, in physical pixels, or `None` outside the
    /// window.
    pointer: Option<Point>,
    /// Text an input method is composing at the cursor.
    preedit: Option<Preedit>,
    /// Whether the primary button went down over the view and is still
    /// held, so pointer moves are drags.
    pressed: bool,
    /// The cells from before the scroll being glided, drawn where the new
    /// cells have not slid in yet.
    previous: CellBuffer,
    /// The prompt row's cells as the grid draws them.
    prompt_cells: CellBuffer,
    /// What the editor last painted of the prompt.
    prompt_surface: Surface,
    /// Where the view was scrolled in the last frame.
    scroll: Option<ScrollOffset>,
    /// How many rows `previous` is ahead of `cells`: the lines the view
    /// scrolled by.
    shift: isize,
    /// What the editor last painted of the view.
    surface: Surface,
    /// The view shown.
    view: ViewId,
    /// The touchpad scroll not yet worth a wheel notch, in rows: positive
    /// toward the end of the document.
    wheel_rest: f32,
}

/// How text of one [`HighlightId`](cherry::HighlightId) of the host's is
/// drawn: a syntax scope, a semantic token, an inlay hint, a diff's added
/// line. Its foreground colors highlighted text, an inlay, a virtual line,
/// and a sign of [`DecorationKind::Background`]; its background is the wash
/// of a [`DecorationKind::Background`] decoration, and lies under
/// highlighted text that has no background of its own. A wash whose style
/// sets a foreground colors the text it lies under too, over the text's
/// own highlight: a color swatch's text, dark on a light color.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HighlightStyle {
    /// Bold, italic, and the other attributes the text takes.
    pub attributes: Attributes,
    /// The background, or `None` for the one the cell has anyway.
    pub background: Option<Rgb>,
    /// The text color, or `None` for the one the cell has anyway.
    pub foreground: Option<Rgb>,
}

/// An [`EditorView`] lent the [`Editor`] it shows a view of, for one call:
/// the [`Widget`] [`EditorView::lend`] makes, to hand inputs and draw.
#[derive(Debug)]
pub struct LentEditor<'a> {
    /// The editor lent.
    editor: &'a mut Editor,
    /// The view.
    view: &'a mut EditorView,
}

/// Misty's [`SystemClipboard`] as the clipboard a Cherry [`Editor`] copies
/// to and pastes from, handed to `Editor::set_clipboard`: registers `+`
/// and `*` reach the system clipboard and the primary selection.
#[derive(Debug)]
pub struct EditorClipboard(SystemClipboard);

/// What the pointer did, from a window event, for the view to turn into
/// Cherry's mouse events.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PointerInput {
    /// The pointer moved to this point, in physical pixels.
    Moved(Point),
    /// The primary button went down.
    Pressed,
    /// The primary button went up.
    Released,
    /// The wheel or touchpad scrolled by this much.
    Wheel(WheelDelta),
}

/// Where a view's rows were drawn, for finding the cell under a point.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewLayout {
    /// The rows of the view, without an open prompt's row, in physical
    /// pixels.
    area: Rect,
    /// The size of a cell.
    metrics: CellMetrics,
}

/// What became of a key fed to the lent [`Widget::handle`](misty::Widget::handle).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorResponse {
    /// The editor used the key and asked nothing of the app.
    Handled,
    /// The key ran nothing; the app may pass it on.
    Ignored,
    /// The editor used the key and asks these of the app, oldest first:
    /// save a document, close a view, show a message.
    Requests(Vec<HostRequest>),
}

// ========================================================================
// EditorView Implementations
// ========================================================================

impl EditorView {
    /// A widget showing `view` of the app's editor.
    pub fn new(view: ViewId) -> Self {
        Self {
            cells: CellBuffer::new(0, 0),
            clicks: ClickCounter::new(MULTI_CLICK_INTERVAL),
            cursor_area: None,
            drawn_offset: 0.0,
            font: None,
            highlights: None,
            layout: None,
            modifiers: Mods::NONE,
            motion: SmoothScroll::default(),
            next: CellBuffer::new(0, 0),
            pointer: None,
            preedit: None,
            pressed: false,
            previous: CellBuffer::new(0, 0),
            prompt_cells: CellBuffer::new(0, 0),
            prompt_surface: Surface::new(0, 0),
            scroll: None,
            shift: 0,
            surface: Surface::new(0, 0),
            view,
            wheel_rest: 0.0,
        }
    }

    /// The view shown.
    pub fn view(&self) -> ViewId {
        self.view
    }

    /// Draws the text in `font`, its cells that font's in size, rather than
    /// in the window's (`None`): what a host whose text panes have a font of
    /// their own (a terminal's) hands the view.
    pub fn set_font(&mut self, font: Option<Font>) {
        self.font = font;
    }

    /// The size of a cell of the text in `frame`.
    fn metrics(&self, frame: &Frame) -> CellMetrics {
        match &self.font {
            Some(font) => frame.cell_metrics_of(font),
            None => frame.cell_metrics(),
        }
    }

    /// Draws `cells` at `origin` of `frame` in the text's font.
    fn draw_cells(&self, frame: &mut Frame, cells: &CellBuffer, origin: Point, theme: &Theme) {
        match &self.font {
            Some(font) => frame.draw_cells_in(cells, origin, font, theme),
            None => frame.draw_cells(cells, origin, theme),
        }
    }

    /// Draws the host's highlight ids as `styles` gives them, indexed by
    /// [`HighlightId::index`](cherry::HighlightId::index), in place of the
    /// theme's syntax colors: what a host whose own highlighter numbers its
    /// ids (syntect's scopes) hands the view, again whenever its theme
    /// changes. An id past the end draws plain.
    pub fn set_highlight_styles(&mut self, styles: Vec<HighlightStyle>) {
        self.highlights = Some(styles);
    }

    /// The cursor's cell in the last frame, in physical pixels: in the
    /// prompt while one is open; `None` when it was out of sight or before
    /// the first frame. Where a host hangs a popup of its own from the
    /// cursor, such as a hover card.
    pub fn cursor_rect(&self) -> Option<Rect> {
        self.cursor_area
    }

    /// The cell char `offset` of the document shown is in, in physical
    /// pixels, as the view was last drawn; `None` when it is out of sight
    /// or before the first frame. Where a host hangs a popup of its own
    /// from the text, such as a diagnostic's card or a code action's list.
    pub fn offset_rect(&self, editor: &Editor, offset: usize) -> Option<Rect> {
        let layout = self.layout?;
        let (column, row) = editor.screen_cell(self.view, offset)?;
        Some(layout.cell_rect(column, row))
    }

    /// The view lent the `editor` it shows a view of, as a [`Widget`] to
    /// hand inputs and draw.
    pub fn lend<'a>(&'a mut self, editor: &'a mut Editor) -> LentEditor<'a> {
        LentEditor { editor, view: self }
    }

    /// Feeds `key` to the view in `editor`, asks for a redraw
    /// when the editor used it, and hands back what the editor asked of
    /// the app.
    pub(crate) fn handle_key(
        &mut self,
        editor: &mut Editor,
        cx: &mut EventCx<'_>,
        key: Key,
    ) -> EditorResponse {
        let handled = editor.handle_key(self.view, to_cherry(key), clock::now());
        respond(editor, cx, handled)
    }

    /// Reacts to a mouse `input` through the view in
    /// `editor`: a press over the view moves the cursor (Shift extends the
    /// selection), a drag selects, quick presses on one cell select a word
    /// then a line, and the wheel over the view scrolls it. Asks for a
    /// redraw when the editor used the event. Offer it every event, so it
    /// sees the pointer move and the modifiers change.
    fn handle_mouse(
        &mut self,
        editor: &mut Editor,
        cx: &mut EventCx<'_>,
        input: &Input,
    ) -> EditorResponse {
        let mut response = EditorResponse::Ignored;
        for mouse in self.mouse_events(input, Instant::now()) {
            let handled = editor.handle_mouse(self.view, mouse);
            response = match (response, respond(editor, cx, handled)) {
                (EditorResponse::Requests(mut earlier), EditorResponse::Requests(later)) => {
                    earlier.extend(later);
                    EditorResponse::Requests(earlier)
                }
                (
                    EditorResponse::Requests(earlier),
                    EditorResponse::Handled | EditorResponse::Ignored,
                ) => EditorResponse::Requests(earlier),
                (EditorResponse::Handled | EditorResponse::Ignored, later) => later,
            };
        }
        response
    }

    /// Reacts to an input method event for the view in `editor`: text
    /// being composed is kept to draw at the cursor, and committed text is
    /// typed into the view key by key while it takes text (Insert mode or
    /// a prompt); in other modes a commit is dropped, since its characters
    /// would run as commands. Asks for a redraw when anything changed.
    fn handle_ime(
        &mut self,
        editor: &mut Editor,
        cx: &mut EventCx<'_>,
        ime: &ImeInput,
    ) -> EditorResponse {
        let text = match Preedit::apply(&mut self.preedit, ime) {
            PreeditChange::Changed => {
                // Cherry lays the text into the line where keys type it; a
                // prompt's is drawn over its row.
                if lays_preedit(editor, self.view) {
                    let text = self.preedit.as_ref().map_or("", |preedit| preedit.text());
                    editor.set_preedit(self.view, text);
                }
                cx.request_redraw();
                return EditorResponse::Handled;
            }
            PreeditChange::Commit(text) => text,
            PreeditChange::Unchanged => return EditorResponse::Ignored,
        };
        if !takes_text(editor, self.view) {
            return EditorResponse::Ignored;
        }
        let mut requests = Vec::new();
        let mut response = EditorResponse::Ignored;
        for key in text.chars().filter_map(typed_key) {
            match self.handle_key(editor, cx, key) {
                EditorResponse::Handled => response = EditorResponse::Handled,
                EditorResponse::Ignored => {}
                EditorResponse::Requests(asked) => requests.extend(asked),
            }
        }
        match requests.is_empty() {
            true => response,
            false => EditorResponse::Requests(requests),
        }
    }

    /// The cursor's cell in the last frame, in physical pixels, while the
    /// view in `editor` takes text (Insert mode or a prompt); `None` in
    /// other modes, when the input method should be off, or when the
    /// cursor was out of sight. What the app hands
    /// [`ImeFocus::follow`](misty::ImeFocus::follow) while the view has the
    /// focus.
    pub fn ime_area(&self, editor: &Editor) -> Option<Rect> {
        self.cursor_area.filter(|_| takes_text(editor, self.view))
    }

    /// Settles every half-typed key sequence of `editor` whose timeout has
    /// passed, asking for a redraw of `window` when one did, and returns
    /// when the next one times out. Call it from
    /// [`Application::about_to_wait`](misty::Application::about_to_wait) and
    /// return the deadline from there; what a settled sequence asked of the
    /// app is taken with [`Editor::take_requests`].
    pub fn about_to_wait(&self, editor: &mut Editor, cx: &mut EventCx<'_>) -> Option<Instant> {
        let now = Instant::now();
        if editor.tick(clock::stamp(now)) {
            cx.request_redraw();
        }
        // The last frame drawn may have been a step short of the end.
        let glide = (self.drawn_offset != 0.0).then(|| self.motion.next_frame(now).unwrap_or(now));
        if let Some(next) = glide {
            cx.request_redraw_at(next);
        }
        earliest(editor.deadline().map(clock::instant), glide)
    }

    /// Makes the text jump to a new scroll position (`true`) rather than
    /// glide, for users who asked their system for reduced motion.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.motion.set_reduced_motion(reduced_motion);
    }

    /// Draws the view in `area`, resizing it to the whole cells that fit:
    /// text in the theme's foreground, the cursor line on the hover color,
    /// the selection on the selection color, search matches on the warning
    /// color, line numbers muted, and the cursor as the editor shapes it.
    /// An open prompt takes the bottom row and the cursor.
    fn draw_view(&mut self, editor: &mut Editor, frame: &mut Frame, area: Rect, theme: &Theme) {
        let metrics = self.metrics(frame);
        let cols = u16::try_from(metrics.cols_in(area.width)).unwrap_or(u16::MAX);
        let rows = u16::try_from(metrics.rows_in(area.height)).unwrap_or(u16::MAX);
        let prompt_open = editor
            .view(self.view)
            .is_some_and(|view| view.prompt().is_some());
        let view_rows = match prompt_open {
            true => rows.saturating_sub(1),
            false => rows,
        };
        let resized = editor
            .view(self.view)
            .is_some_and(|view| (view.cols(), view.rows()) != (cols, view_rows));
        if resized {
            editor.resize_view(self.view, cols, view_rows);
        }
        self.layout = Some(ViewLayout {
            area: Rect::new(
                area.x,
                area.y,
                f32::from(cols) * metrics.width,
                f32::from(view_rows) * metrics.height,
            ),
            metrics,
        });
        let cursor = render_view(editor, self.view, &mut self.surface);
        let cursor = cursor.filter(|_| !prompt_open);
        let highlights = match &self.highlights {
            Some(styles) => styles.clone(),
            None => highlight_colors(theme),
        };
        fill_cells(&self.surface, cursor, theme, &highlights, &mut self.next);
        let scroll = editor.view(self.view).map(|view| view.scroll());
        let now = Instant::now();
        if let (Some(old), Some(new)) = (self.scroll, scroll)
            && old != new
        {
            self.follow_scroll(old, new, now);
        }
        self.scroll = scroll;
        mem::swap(&mut self.cells, &mut self.next);
        let prompt_top = area.y + f32::from(view_rows) * metrics.height;
        if prompt_open {
            let cursor = render_prompt(editor, self.view, &mut self.prompt_surface);
            fill_cells(
                &self.prompt_surface,
                cursor,
                theme,
                &highlights,
                &mut self.prompt_cells,
            );
        }
        // The cursor is in the prompt while one is open, in the view
        // otherwise; a prompt's preedit is drawn over it, the view's is in
        // its text.
        let (cursor_cells, cursor_origin) = match prompt_open {
            true => (&mut self.prompt_cells, Point::new(area.x, prompt_top)),
            false => (&mut self.cells, Point::new(area.x, area.y)),
        };
        self.cursor_area = cursor_cells.cursor().map(|cursor| {
            Rect::new(
                cursor_origin.x + cursor.col as f32 * metrics.width,
                cursor_origin.y + cursor.row as f32 * metrics.height,
                metrics.width,
                metrics.height,
            )
        });
        if let Some(preedit) = self.preedit.as_ref().filter(|_| prompt_open) {
            overlay_preedit(cursor_cells, preedit);
        }
        let offset = self.motion.position(now);
        self.drawn_offset = offset;
        match offset == 0.0 {
            true => self.draw_cells(frame, &self.cells, Point::new(area.x, area.y), theme),
            false => {
                let height = f32::from(view_rows) * metrics.height;
                self.draw_glide(
                    frame,
                    Rect::new(area.x, area.y, area.width, height),
                    offset,
                    theme,
                );
            }
        }
        if prompt_open {
            self.draw_cells(
                frame,
                &self.prompt_cells,
                Point::new(area.x, prompt_top),
                theme,
            );
            return;
        }
        let completion = editor.view(self.view).and_then(|view| view.completion());
        if let Some(completion) = completion
            && let Some(anchor) = self.offset_rect(editor, completion.anchor())
        {
            draw_completion(frame, completion, anchor, theme);
        }
    }

    /// The Cherry mouse events `input`, at `now`, makes over the view as
    /// last drawn: none for an input that is not the pointer's or falls
    /// outside the view, several for a wheel turned several notches.
    fn mouse_events(&mut self, input: &Input, now: Instant) -> Vec<MouseEvent> {
        let pointer = match input {
            Input::PointerButton(button) if button.button == PointerButton::Left => {
                self.modifiers = button.mods;
                self.pointer = Some(button.position);
                match button.pressed {
                    true => PointerInput::Pressed,
                    false => PointerInput::Released,
                }
            }
            Input::PointerLeft => {
                self.pointer = None;
                return Vec::new();
            }
            Input::PointerMoved(position) => PointerInput::Moved(*position),
            Input::Wheel(wheel) => {
                self.modifiers = wheel.mods;
                self.pointer = Some(wheel.position);
                PointerInput::Wheel(wheel.delta)
            }
            Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::Ime(_)
            | Input::Key(_)
            | Input::KeyReleased(_)
            | Input::PointerButton(_) => return Vec::new(),
        };
        self.pointer_events(pointer, now)
    }

    /// The Cherry mouse events `input`, at `now`, makes over the view as
    /// last drawn: a move is a drag while the button is held from a press
    /// over the view, a press over the view may count as a double or
    /// triple click, and a wheel turn over the view is a scroll per notch.
    fn pointer_events(&mut self, input: PointerInput, now: Instant) -> Vec<MouseEvent> {
        let Some(layout) = self.layout else {
            return Vec::new();
        };
        let mods = mods_to_cherry(self.modifiers);
        let at = |point: Option<Point>, kind: MouseEventKind, clamp: bool| {
            point
                .and_then(|point| layout.cell_at(point, clamp))
                .map(|(column, row)| MouseEvent {
                    column,
                    kind,
                    mods,
                    row,
                })
        };
        let event = match input {
            PointerInput::Moved(point) => {
                self.pointer = Some(point);
                match self.pressed {
                    true => at(self.pointer, MouseEventKind::Drag, true),
                    false => None,
                }
            }
            PointerInput::Pressed => {
                let press = at(self.pointer, MouseEventKind::Press, false);
                self.pressed = press.is_some();
                press.map(|press| self.clicks.count(press, clock::stamp(now)))
            }
            PointerInput::Released if self.pressed => {
                self.pressed = false;
                at(self.pointer, MouseEventKind::Release, true)
            }
            PointerInput::Released => None,
            PointerInput::Wheel(delta) => {
                let Some(notch) = at(self.pointer, MouseEventKind::ScrollDown, false) else {
                    return Vec::new();
                };
                let notches = self.wheel_notches(delta, layout.metrics.height);
                let kind = match notches < 0 {
                    true => MouseEventKind::ScrollUp,
                    false => MouseEventKind::ScrollDown,
                };
                return vec![MouseEvent { kind, ..notch }; notches.unsigned_abs()];
            }
        };
        event.into_iter().collect()
    }

    /// The whole notches the wheel's `delta` turns, negative toward the
    /// start of the document; a touchpad's pixels count a notch per
    /// [`ROWS_PER_NOTCH`] rows of `row_height`, the rest kept for later.
    fn wheel_notches(&mut self, delta: WheelDelta, row_height: f32) -> isize {
        match delta {
            WheelDelta::Lines(lines) => -lines.y.round() as isize,
            WheelDelta::Pixels(pixels) => {
                self.wheel_rest -= pixels.y / row_height.max(1.0);
                let notches = (self.wheel_rest / ROWS_PER_NOTCH).trunc();
                self.wheel_rest -= notches * ROWS_PER_NOTCH;
                notches as isize
            }
        }
    }

    /// Describes the view as last drawn to screen readers as `id`: a
    /// multiline text area holding the lines in sight as text runs (line
    /// `index` is the `index`th child of `id`), with the cursor as its
    /// text selection, clamped to the lines in sight: collapsed at the
    /// cursor, or in a visual mode running from where the selection started
    /// through the cursor (whole lines in Visual Line mode; a block as the
    /// characters between its corners, since a text selection has no
    /// rectangles). Nothing before the first frame.
    pub fn access(&self, editor: &Editor, frame: &mut Frame, id: FocusId) {
        let (Some(layout), Some(view)) = (self.layout, editor.view(self.view)) else {
            return;
        };
        let Some(document) = editor.document(view.document()) else {
            return;
        };
        let first = view.scroll().line;
        let last = (first + usize::from(view.rows()))
            .min(document.len_lines())
            .saturating_sub(1);
        let mut area = Node::new(Role::MultilineTextInput);
        let (anchor, focus) = selection_ends(view, document);
        let lines = first..=last;
        if let (Some(anchor), Some(focus)) = (
            text_position(document, anchor, lines.clone(), id),
            text_position(document, focus, lines, id),
        ) {
            area.set_text_selection(TextSelection { anchor, focus });
        }
        frame.enter_access(id, layout.area, area);
        for (row, line) in (first..=last).enumerate() {
            let mut text = document.line_text(line).unwrap_or_default();
            if line + 1 < document.len_lines() {
                text.push('\n');
            }
            let lengths = text.chars().map(|c| c.len_utf8() as u8).collect::<Vec<_>>();
            let mut run = Node::new(Role::TextRun);
            run.set_value(text);
            run.set_character_lengths(lengths);
            let height = layout.metrics.height;
            let top = layout.area.y + row as f32 * height;
            let rect = Rect::new(layout.area.x, top, layout.area.width, height);
            frame.access(id.child(line as u64), rect, run);
        }
        frame.leave_access();
    }

    /// Starts gliding from the last frame's cells to the new ones in
    /// `next` after the view scrolled from `old` to `new`, when the two
    /// line up: the same rows of text, moved by the lines scrolled, less
    /// than a screen away counting a glide already under way. Otherwise
    /// the text jumps.
    fn follow_scroll(&mut self, old: ScrollOffset, new: ScrollOffset, now: Instant) {
        let shift = new.line as isize - old.line as isize;
        let rows = self.next.rows();
        let start = self.motion.position(now) + shift as f32;
        let lines_up = old.row == 0
            && new.row == 0
            && old.column == new.column
            && shift != 0
            && start.abs() < rows as f32
            && rows_line_up(&self.cells, &self.next, shift);
        if !lines_up {
            self.motion.jump_to(0.0);
            return;
        }
        mem::swap(&mut self.previous, &mut self.cells);
        self.previous.set_cursor(None);
        self.shift = shift;
        self.motion.jump_to(start);
        self.motion.scroll_to(0.0, now);
    }

    /// Draws the glide in `area` with the new cells `offset` rows from
    /// their place, and the cells from before the scroll filling the rows
    /// the new ones have left bare.
    fn draw_glide(&self, frame: &mut Frame, area: Rect, offset: f32, theme: &Theme) {
        let height = self.metrics(frame).height;
        let top = |rows: f32| (area.y + rows * height).round();
        let bare = match offset > 0.0 {
            true => Rect::new(area.x, area.y, area.width, top(offset) - area.y),
            false => {
                let edge = top(self.cells.rows() as f32 + offset);
                Rect::new(area.x, edge, area.width, area.y + area.height - edge)
            }
        };
        frame.clipped(bare, |frame| {
            let origin = Point::new(area.x, top(offset - self.shift as f32));
            self.draw_cells(frame, &self.previous, origin, theme);
        });
        frame.clipped(area, |frame| {
            self.draw_cells(frame, &self.cells, Point::new(area.x, top(offset)), theme);
        });
    }
}

impl Widget for LentEditor<'_> {
    type Output = ();
    type Response = EditorResponse;

    /// Reacts to `input` through the view in the lent editor: keys are fed
    /// to the editor (hand them only while the view has the keyboard),
    /// input method text is composed at the cursor and typed while the
    /// view takes text, and the pointer moves the cursor, selects (Shift
    /// extends, quick presses select a word then a line), and scrolls.
    /// Asks for a redraw when the editor used the input, and hands back
    /// what the editor asked of the app.
    fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> EditorResponse {
        let (view, editor) = (&mut *self.view, &mut *self.editor);
        match input {
            Input::Ime(ime) => view.handle_ime(editor, cx, ime),
            Input::Key(key) => view.handle_key(editor, cx, *key),
            // An editor acts on presses alone.
            Input::KeyReleased(_) => EditorResponse::Ignored,
            Input::AppearanceChanged(_)
            | Input::CloseRequested
            | Input::FileDropped(_)
            | Input::FileHovered(_)
            | Input::FileHoverCancelled
            | Input::FocusGained
            | Input::FocusLost
            | Input::PointerButton(_)
            | Input::PointerLeft
            | Input::PointerMoved(_)
            | Input::Wheel(_) => view.handle_mouse(editor, cx, input),
        }
    }

    /// Draws the view in `area`, resizing it to the whole cells that fit:
    /// text in the theme's foreground, the cursor line on the hover color,
    /// the selection on the selection color, search matches on the warning
    /// color, line numbers muted, and the cursor as the editor shapes it.
    /// An open prompt takes the bottom row and the cursor.
    fn draw(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.view.draw_view(self.editor, frame, area, theme);
    }
}

impl Outcome for EditorResponse {
    fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }
}

// ========================================================================
// EditorClipboard Implementations
// ========================================================================

impl EditorClipboard {
    /// `clipboard` as the one an editor copies to and pastes from.
    pub fn new(clipboard: SystemClipboard) -> Self {
        Self(clipboard)
    }
}

impl Clipboard for EditorClipboard {
    fn read(&mut self, selection: ClipboardSelection) -> Option<String> {
        self.0.read_text(clipboard_kind(selection))
    }

    fn write(&mut self, selection: ClipboardSelection, text: &str) {
        // The text is kept in the process either way; Cherry has no way to
        // hear that other apps will not see it.
        let _ = self.0.write_text(clipboard_kind(selection), text);
    }
}

// ========================================================================
// ViewLayout Implementations
// ========================================================================

impl ViewLayout {
    /// The rect of the cell at `column` and `row` of the view.
    fn cell_rect(&self, column: u16, row: u16) -> Rect {
        let metrics = self.metrics;
        Rect::new(
            self.area.x + f32::from(column) * metrics.width,
            self.area.y + f32::from(row) * metrics.height,
            metrics.width,
            metrics.height,
        )
    }

    /// The cell column and row under `point`, from the view's top left
    /// corner: `None` outside the view, unless `clamp` asks for the
    /// nearest cell on the view's edge, as a drag past the edge wants.
    fn cell_at(&self, point: Point, clamp: bool) -> Option<(u16, u16)> {
        let (width, height) = (self.metrics.width, self.metrics.height);
        if width <= 0.0 || height <= 0.0 || self.area.width <= 0.0 || self.area.height <= 0.0 {
            return None;
        }
        if !clamp && !self.area.contains(point) {
            return None;
        }
        let cols = (self.area.width / width).round().max(1.0);
        let rows = (self.area.height / height).round().max(1.0);
        let column = ((point.x - self.area.x) / width)
            .floor()
            .clamp(0.0, cols - 1.0);
        let row = ((point.y - self.area.y) / height)
            .floor()
            .clamp(0.0, rows - 1.0);
        Some((column as u16, row as u16))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The misty clipboard a Cherry clipboard `selection` is.
fn clipboard_kind(selection: ClipboardSelection) -> ClipboardKind {
    match selection {
        ClipboardSelection::Clipboard => ClipboardKind::Clipboard,
        ClipboardSelection::Primary => ClipboardKind::Primary,
    }
}

/// The AccessKit id of the node misty describes as `id`.
fn node_id(id: FocusId) -> NodeId {
    NodeId(id.get())
}

/// The earlier of two optional deadlines: whichever is set when only one
/// is, and `None` when neither is.
fn earliest(first: Option<Instant>, second: Option<Instant>) -> Option<Instant> {
    match (first, second) {
        (Some(first), Some(second)) => Some(first.min(second)),
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    }
}

/// What became of an event the editor took, `handled`, with what it asks
/// of the app taken from `editor`; asks for a redraw of `window` when the
/// editor used the event or asked anything.
fn respond(editor: &mut Editor, cx: &mut EventCx<'_>, handled: Handled) -> EditorResponse {
    let requests = editor.take_requests();
    match (handled, requests.is_empty()) {
        (Handled::Ignored, true) => EditorResponse::Ignored,
        (Handled::Consumed, true) => {
            cx.request_redraw();
            EditorResponse::Handled
        }
        (Handled::Consumed | Handled::Ignored, false) => {
            cx.request_redraw();
            EditorResponse::Requests(requests)
        }
    }
}

/// How each highlight name is drawn under `theme`, by highlight id: the
/// names of `cherry-syntax`, whose ids index them, in the theme's syntax
/// colors.
#[cfg(feature = "syntax")]
fn highlight_colors(theme: &Theme) -> Vec<HighlightStyle> {
    cherry::syntax::HIGHLIGHT_NAMES
        .iter()
        .map(|name| HighlightStyle {
            foreground: theme.syntax.color(name),
            ..HighlightStyle::default()
        })
        .collect()
}

/// No highlight styles: without the `syntax` feature no highlight id has a
/// name to color it by.
#[cfg(not(feature = "syntax"))]
fn highlight_colors(_theme: &Theme) -> Vec<HighlightStyle> {
    Vec::new()
}

/// Whether `new` shows the rows of `old` moved up by `shift` rows (down
/// for a negative shift) wherever both have them, comparing text only,
/// since the cursor line and selection move with the cursor.
fn rows_line_up(old: &CellBuffer, new: &CellBuffer, shift: isize) -> bool {
    if (old.cols(), old.rows()) != (new.cols(), new.rows()) {
        return false;
    }
    let rows = new.rows() as isize;
    (0..rows)
        .filter_map(|row| {
            let from = row + shift;
            (0..rows)
                .contains(&from)
                .then_some((row as usize, from as usize))
        })
        .all(|(row, from)| {
            (0..new.cols()).all(|col| match (new.cell(row, col), old.cell(from, col)) {
                (Some(new), Some(old)) => new.ch == old.ch && new.tail == old.tail,
                (None, None) => true,
                (Some(_), None) | (None, Some(_)) => false,
            })
        })
}

/// Writes `surface` and `cursor` into `cells`, sized to match, Cherry's
/// styles mapped to `theme`'s colors and highlighted text drawn as
/// `highlights` says, indexed by highlight id.
fn fill_cells(
    surface: &Surface,
    cursor: Option<cherry::Cursor>,
    theme: &Theme,
    highlights: &[HighlightStyle],
    cells: &mut CellBuffer,
) {
    let (cols, rows) = (surface.cols(), surface.rows());
    cells.resize(usize::from(cols), usize::from(rows));
    for row in 0..rows {
        let mut after_wide = false;
        for col in 0..cols {
            let Some(painted) = surface.cell(col, row) else {
                continue;
            };
            let guide = painted.layers.contains(Layers::INDENT_GUIDE) && painted.symbol == " ";
            let symbol = if guide {
                INDENT_GUIDE
            } else {
                painted.symbol.as_str()
            };
            let mut chars = symbol.chars();
            let style = cell_style(painted, theme, highlights);
            let cell = match chars.next() {
                None if after_wide => cell::Cell {
                    width: CellWidth::Continuation,
                    ..cell::Cell::new(' ', style)
                },
                None => cell::Cell::new(' ', style),
                Some(first) => {
                    let rest = chars.as_str();
                    cell::Cell {
                        tail: (!rest.is_empty()).then(|| rest.into()),
                        width: match text::width(&painted.symbol) > 1 {
                            true => CellWidth::Wide,
                            false => CellWidth::Single,
                        },
                        ..cell::Cell::new(first, style)
                    }
                }
            };
            after_wide = cell.width == CellWidth::Wide;
            cells.set(usize::from(row), usize::from(col), cell);
        }
    }
    cells.set_cursor(cursor.map(|cursor| {
        let shape = match cursor.shape {
            cherry::CursorShape::Bar => cell::CursorShape::Bar,
            cherry::CursorShape::Block => cell::CursorShape::Block,
            cherry::CursorShape::Underline => cell::CursorShape::Underline,
            // A shape added after this was written: the block shows the
            // cursor's place whatever it means.
            _ => cell::CursorShape::Block,
        };
        cell::Cursor::new(usize::from(cursor.row), usize::from(cursor.column), shape)
    }));
}

/// Draws `preedit` over `cells` from the cursor on, underlined, cut at the
/// row's end, with a bar cursor at the input method's caret (none when it
/// hides it). Covered cells are hidden until the text is committed.
fn overlay_preedit(cells: &mut CellBuffer, preedit: &Preedit) {
    let Some(cursor) = cells.cursor() else {
        return;
    };
    let style = cell::Style {
        underline: UnderlineStyle::Single,
        ..cell::Style::default()
    };
    let mut col = cursor.col;
    for c in preedit.text().chars() {
        let width = text::char_width(c);
        // Zero-width marks have no cell of their own.
        if width == 0 {
            continue;
        }
        if col + width > cells.cols() {
            break;
        }
        let wide = width > 1;
        let width = match wide {
            true => CellWidth::Wide,
            false => CellWidth::Single,
        };
        cells.set(
            cursor.row,
            col,
            cell::Cell {
                width,
                ..cell::Cell::new(c, style)
            },
        );
        if wide {
            let continuation = cell::Cell {
                width: CellWidth::Continuation,
                ..cell::Cell::new(' ', style)
            };
            cells.set(cursor.row, col + 1, continuation);
            col += 1;
        }
        col += 1;
    }
    let caret = preedit
        .caret_cells()
        .map(|cells_before| (cursor.col + cells_before).min(cells.cols().saturating_sub(1)));
    cells.set_cursor(caret.map(|col| cell::Cursor::new(cursor.row, col, cell::CursorShape::Bar)));
}

/// Where char `offset` of `document` is among the text runs of the
/// `lines` in sight, which are the children of `id` numbered by line: an
/// offset above them at the start of the first, one below them at the end
/// of the last; `None` when no line is in sight.
fn text_position(
    document: &Document,
    offset: usize,
    lines: RangeInclusive<usize>,
    id: FocusId,
) -> Option<TextPosition> {
    let position = document.char_to_position(offset)?;
    let (line, character_index) = if position.line < *lines.start() {
        (*lines.start(), 0)
    } else if position.line > *lines.end() {
        let end = document.line_text(*lines.end())?.chars().count();
        (*lines.end(), end)
    } else {
        (position.line, position.column)
    };
    lines.contains(&line).then(|| TextPosition {
        character_index,
        node: node_id(id.child(line as u64)),
    })
}

/// The char offsets between which `view`'s selection lies in `document`,
/// as a text selection's anchor and focus (positions between characters):
/// both at the cursor outside a visual mode; in one, from the anchor
/// through the cursor's character, or the whole lines of both in Visual
/// Line mode, the focus before the anchor when the cursor is.
fn selection_ends(view: &View, document: &Document) -> (usize, usize) {
    let cursor = view.cursor();
    let Some(anchor) = view.visual_anchor() else {
        return (cursor, cursor);
    };
    if view.mode() == Mode::VisualLine {
        let line_of = |offset| document.char_to_position(offset).map_or(0, |at| at.line);
        let (anchor_line, cursor_line) = (line_of(anchor), line_of(cursor));
        let start = |line| {
            document
                .position_to_char(Position { column: 0, line })
                .unwrap_or_else(|| document.len_chars())
        };
        let (top, bottom) = (anchor_line.min(cursor_line), anchor_line.max(cursor_line));
        let (top, bottom) = (start(top), start(bottom + 1));
        return if anchor_line <= cursor_line {
            (top, bottom)
        } else {
            (bottom, top)
        };
    }
    if anchor <= cursor {
        (anchor, document.next_grapheme_boundary(cursor))
    } else {
        (document.next_grapheme_boundary(anchor), cursor)
    }
}

/// Whether `view` of `editor` takes typed text: in Insert or Replace
/// mode, or with a prompt open.
fn takes_text(editor: &Editor, view: ViewId) -> bool {
    editor.view(view).is_some_and(|view| match view.mode() {
        Mode::CommandLine | Mode::Insert | Mode::Replace => true,
        Mode::Normal
        | Mode::OperatorPending
        | Mode::Visual
        | Mode::VisualBlock
        | Mode::VisualLine => false,
        // A mode added after this was written: typed text could run as
        // commands, so the input method stays off.
        _ => false,
    })
}

/// Whether Cherry lays a preedit into the text of `view` of `editor`: in
/// Insert or Replace mode with no prompt open, where keys type text.
fn lays_preedit(editor: &Editor, view: ViewId) -> bool {
    editor.view(view).is_some_and(|view| {
        view.prompt().is_none() && matches!(view.mode(), Mode::Insert | Mode::Replace)
    })
}

/// The key typing `c` presses: Enter for a line break, Tab for a tab, the
/// character otherwise; `None` for other control characters.
fn typed_key(c: char) -> Option<Key> {
    let code = match c {
        '\n' | '\r' => KeyCode::Enter,
        '\t' => KeyCode::Tab,
        c if c.is_control() => return None,
        c => KeyCode::Char(c),
    };
    Some(Key::new(code, Mods::NONE))
}

/// The cell style `painted` is drawn in under `theme`: its Cherry style,
/// its highlighted text as `highlights` draws its highlight id (the text
/// color unless the style sets one, an inlay's always; the background
/// unless the style sets one), a wash (its text too, where its id sets a
/// text color) or a sign of a host's own kind in its id's colors, and the
/// underline of its decoration.
fn cell_style(painted: &cherry::Cell, theme: &Theme, highlights: &[HighlightStyle]) -> cell::Style {
    let host = |id: cherry::HighlightId| highlights.get(usize::from(id.index())).copied();
    let mut style = style_of(painted.style, theme);
    match painted.style {
        cherry::Style::Background(id) => {
            if let Some(drawn) = host(id) {
                if let Some(background) = drawn.background {
                    style.background = Color::Rgb(background);
                }
                if let Some(foreground) = drawn.foreground {
                    style.foreground = Color::Rgb(foreground);
                }
            }
        }
        cherry::Style::Sign(DecorationKind::Background(id)) => {
            if let Some(foreground) = host(id).and_then(|drawn| drawn.foreground) {
                style.foreground = Color::Rgb(foreground);
            }
        }
        _ => {}
    }
    if let Some(drawn) = painted.highlight.and_then(host) {
        let inlay = painted.style == cherry::Style::Inlay;
        if let Some(color) = drawn
            .foreground
            .filter(|_| inlay || style.foreground == Color::Default)
        {
            style.foreground = Color::Rgb(color);
        }
        if let Some(color) = drawn
            .background
            .filter(|_| style.background == Color::Default)
        {
            style.background = Color::Rgb(color);
        }
        style.attributes = style.attributes | drawn.attributes;
    }
    if let Some(kind) = painted.underline {
        (style.underline, style.underline_color) = underline_of(kind, theme);
    }
    let layers = painted.layers;
    let plain_background = style.background == Color::Default;
    if layers.contains(Layers::INDENT_GUIDE) && style.foreground == Color::Default {
        style.foreground = Color::Rgb(theme.border);
    }
    if layers.contains(Layers::OCCURRENCE) && plain_background {
        style.background = Color::Rgb(theme.border);
    } else if layers.contains(Layers::PINNED) && plain_background {
        style.background = Color::Rgb(theme.surface);
    }
    style
}

/// The color decorations and signs of `kind` are drawn in under `theme`:
/// errors in the danger color, warnings in the warning color, information
/// and highlights in the accent color, hints and plain underlines muted.
fn decoration_color(kind: DecorationKind, theme: &Theme) -> Rgb {
    match kind {
        DecorationKind::Error => theme.danger,
        DecorationKind::Highlight | DecorationKind::Info => theme.accent,
        DecorationKind::Hint | DecorationKind::Underline => theme.muted,
        DecorationKind::Warning => theme.warning,
        // A kind added after this was written: drawn as quietly as a hint.
        _ => theme.muted,
    }
}

/// How text under a decoration of `kind` is underlined under `theme`: an
/// undercurl for a diagnostic, a dotted line for a hint, a plain line in
/// the text's color for an underline.
fn underline_of(kind: DecorationKind, theme: &Theme) -> (UnderlineStyle, Color) {
    let color = Color::Rgb(decoration_color(kind, theme));
    match kind {
        DecorationKind::Error | DecorationKind::Info | DecorationKind::Warning => {
            (UnderlineStyle::Curly, color)
        }
        // Cherry paints a highlight as a style, not an underline.
        DecorationKind::Highlight => (UnderlineStyle::None, Color::Default),
        DecorationKind::Hint => (UnderlineStyle::Dotted, color),
        DecorationKind::Underline => (UnderlineStyle::Single, Color::Default),
        _ => (UnderlineStyle::Single, Color::Default),
    }
}

/// The cell style Cherry's `style` is drawn in under `theme`.
fn style_of(style: cherry::Style, theme: &Theme) -> cell::Style {
    match style {
        // A wash of the host's own takes its color from the host's styles;
        // without one, the raised color.
        cherry::Style::Background(_) => cell::Style {
            background: Color::Rgb(theme.raised),
            ..cell::Style::default()
        },
        cherry::Style::CursorLine => cell::Style {
            background: Color::Rgb(theme.cursor_line),
            ..cell::Style::default()
        },
        // Text a decoration highlights (the other uses of a name) stands
        // out less than a selection: on the border color.
        cherry::Style::Highlight => cell::Style {
            background: Color::Rgb(theme.border),
            ..cell::Style::default()
        },
        cherry::Style::CursorLineNumber => cell::Style::default(),
        // An extra cursor as the main one is drawn: a block in the cursor
        // color over the text.
        cherry::Style::ExtraCursor => cell::Style {
            background: Color::Rgb(theme.palette.cursor),
            foreground: Color::Rgb(theme.palette.background),
            ..cell::Style::default()
        },
        // Text that is not the document's own reads apart from it: muted
        // and slanted, unless its highlight colors it.
        cherry::Style::Inlay => cell::Style {
            attributes: Attributes::ITALIC,
            foreground: Color::Rgb(theme.muted),
            ..cell::Style::default()
        },
        cherry::Style::LineNumber => cell::Style {
            foreground: Color::Rgb(theme.muted),
            ..cell::Style::default()
        },
        cherry::Style::MatchingBracket => cell::Style {
            attributes: Attributes::BOLD,
            background: Color::Rgb(theme.border),
            ..cell::Style::default()
        },
        cherry::Style::Preedit => cell::Style {
            underline: UnderlineStyle::Single,
            ..cell::Style::default()
        },
        // Each phrase on a palette color of its own, cycling, its text dark.
        cherry::Style::Phrase(number) => cell::Style {
            background: Color::Rgb(
                theme.palette.colors[PHRASE_COLORS[usize::from(number) % PHRASE_COLORS.len()]],
            ),
            foreground: Color::Rgb(theme.palette.background),
            ..cell::Style::default()
        },
        // Unlike a selection, a match stands out wherever it is: dark text
        // on the warning color.
        cherry::Style::SearchMatch => cell::Style {
            background: Color::Rgb(theme.warning),
            foreground: Color::Rgb(theme.palette.background),
            ..cell::Style::default()
        },
        cherry::Style::Selection => cell::Style {
            background: Color::Rgb(theme.selection),
            ..cell::Style::default()
        },
        cherry::Style::Sign(kind) => cell::Style {
            foreground: Color::Rgb(decoration_color(kind, theme)),
            ..cell::Style::default()
        },
        cherry::Style::Text => cell::Style::default(),
        // A style added after this was written: drawn as plain text.
        _ => cell::Style::default(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use cherry::{Decoration, EditorOptions, Sign};
    use misty::Composition;
    use misty::testing::{TestWindow, access_node, access_tree};

    use super::*;

    /// The preedit an input method composing `text`, its cursor at the
    /// byte range `cursor`, makes.
    fn preedit(text: &str, cursor: Option<(usize, usize)>) -> Preedit {
        let mut preedit = None;
        let composition = Composition {
            cursor,
            text: text.to_owned(),
        };
        Preedit::apply(&mut preedit, &ImeInput::Preedit(composition));
        preedit.expect("a preedit")
    }

    /// The cells of a view 10 by 3 onto `text`, painted by Cherry, with
    /// `keys` typed first.
    fn painted(text: &str, keys: &[&str], theme: &Theme) -> CellBuffer {
        let mut editor = Editor::new(EditorOptions::default());
        let document = editor.open_document(text);
        let view = editor.new_view(document).expect("a view");
        editor.resize_view(view, 10, 3);
        for spec in keys {
            let key = cherry::keys::parse_key(spec).expect("a valid key");
            editor.handle_key(view, key, clock::now());
        }
        let mut surface = Surface::new(0, 0);
        let cursor = render_view(&editor, view, &mut surface);
        let mut cells = CellBuffer::new(0, 0);
        fill_cells(
            &surface,
            cursor,
            theme,
            &highlight_colors(theme),
            &mut cells,
        );
        cells
    }

    #[test]
    fn test_guides_draw_as_lines_and_phrases_take_colors_of_their_own() {
        let theme = Theme::dark();
        let options = EditorOptions {
            indent_guides: true,
            ..EditorOptions::default()
        };
        let mut editor = Editor::new(options);
        let document = editor.open_document("ab ab\n    cd");
        let view = editor.new_view(document).expect("a view");
        for spec in ["M-s", "h", "."] {
            let key = cherry::keys::parse_key(spec).expect("a valid key");
            editor.handle_key(view, key, clock::now());
        }
        let cells = cells_of(&mut editor, view, &theme);
        let at = |row: usize, col: usize| cells.cell(row, col).expect("in the grid").clone();
        assert_eq!(at(1, 0).ch, '\u{2502}', "a guide where the indent starts");
        assert_eq!(at(1, 1).ch, ' ', "no guide between levels");
        let phrase = Color::Rgb(theme.palette.colors[PHRASE_COLORS[0]]);
        assert_eq!(at(0, 3).style.background, phrase, "the second ab lit too");
    }

    /// The cells of `editor`'s `view`, 10 by 3, as `theme` draws them.
    fn cells_of(editor: &mut Editor, view: ViewId, theme: &Theme) -> CellBuffer {
        editor.resize_view(view, 10, 3);
        let mut surface = Surface::new(0, 0);
        let cursor = render_view(editor, view, &mut surface);
        let mut cells = CellBuffer::new(0, 0);
        fill_cells(
            &surface,
            cursor,
            theme,
            &highlight_colors(theme),
            &mut cells,
        );
        cells
    }

    /// An editor widget whose view was drawn 10 cells by 3 of 10 by 20
    /// pixels at the window's top left corner, onto `text`.
    fn drawn_view(text: &str) -> (Editor, EditorView) {
        let mut editor = Editor::new(EditorOptions::default());
        let document = editor.open_document(text);
        let id = editor.new_view(document).expect("a view");
        editor.resize_view(id, 10, 3);
        let mut view = EditorView::new(id);
        view.layout = Some(ViewLayout {
            area: Rect::new(0.0, 0.0, 100.0, 60.0),
            metrics: CellMetrics {
                baseline: 15.0,
                height: 20.0,
                width: 10.0,
            },
        });
        (editor, view)
    }

    /// Cells showing `rows`, one line of text each, 4 columns wide.
    fn lines(rows: &[&str]) -> CellBuffer {
        let mut cells = CellBuffer::new(4, rows.len());
        for (row, text) in rows.iter().enumerate() {
            for (col, ch) in text.chars().enumerate() {
                cells.set(row, col, cell::Cell::new(ch, cell::Style::default()));
            }
        }
        cells
    }

    #[test]
    fn test_rows_line_up_only_when_moved_by_the_lines_scrolled() {
        let top = lines(&["one", "two", "six"]);
        let down = lines(&["two", "six", "ten"]);
        assert!(rows_line_up(&top, &down, 1), "scrolled a line down");
        assert!(rows_line_up(&down, &top, -1), "and back up");
        assert!(!rows_line_up(&top, &down, 2));
        assert!(!rows_line_up(&top, &lines(&["two", "six"]), 1), "resized");
    }

    #[test]
    fn test_a_short_scroll_glides_and_a_long_one_jumps() {
        let theme = Theme::dark();
        let window = TestWindow::new(400, 400);
        let metrics = window.frame().cell_metrics();
        let text: String = (0..200).map(|line| format!("line {line}\n")).collect();
        let mut editor = Editor::new(EditorOptions::default());
        let document = editor.open_document(&text);
        let id = editor.new_view(document).expect("a view");
        let mut view = EditorView::new(id);
        // Ten rows, so twelve lines down is a short scroll.
        let area = Rect::new(0.0, 0.0, 20.0 * metrics.width, 10.0 * metrics.height);
        let draw = |editor: &mut Editor, view: &mut EditorView| {
            let mut frame = window.frame();
            view.lend(editor).draw(&mut frame, area, &theme);
        };
        draw(&mut editor, &mut view);
        for _ in 0..12 {
            let key = cherry::keys::parse_key("j").expect("a key");
            editor.handle_key(id, key, clock::now());
        }
        draw(&mut editor, &mut view);
        assert!(
            view.motion.is_animating(Instant::now()),
            "a short scroll glides"
        );
        assert!(view.shift > 0);
        let key = cherry::keys::parse_key("G").expect("a key");
        editor.handle_key(id, key, clock::now());
        draw(&mut editor, &mut view);
        assert!(
            !view.motion.is_animating(Instant::now()),
            "a long one jumps"
        );
    }

    #[test]
    fn test_a_wide_character_becomes_a_wide_cell_and_its_continuation() {
        let cells = painted("a漢b", &[], &Theme::dark());
        assert_eq!((cells.cols(), cells.rows()), (10, 3));
        let cell = |col: usize| cells.cell(0, col).expect("a cell");
        assert_eq!((cell(1).ch, cell(1).width), ('漢', CellWidth::Wide));
        assert_eq!(cell(2).width, CellWidth::Continuation);
        assert_eq!((cell(3).ch, cell(3).width), ('b', CellWidth::Single));
    }

    #[test]
    fn test_the_cursor_keeps_its_place_and_shape_from_the_editor() {
        let theme = Theme::dark();
        let normal = painted("abc", &["l"], &theme).cursor().expect("a cursor");
        assert_eq!((normal.row, normal.col), (0, 1));
        assert_eq!(normal.shape, cell::CursorShape::Block);
        let insert = painted("abc", &["l", "i"], &theme)
            .cursor()
            .expect("a cursor");
        assert_eq!(insert.shape, cell::CursorShape::Bar);
    }

    #[test]
    fn test_a_preedit_covers_cells_from_the_cursor_underlined_with_the_caret_inside() {
        let mut cells = painted("abcdef", &["i"], &Theme::dark());
        let preedit = preedit("x漢y", Some((1, 1)));
        overlay_preedit(&mut cells, &preedit);
        let cell = |col: usize| cells.cell(0, col).expect("a cell");
        assert_eq!(
            (cell(0).ch, cell(0).style.underline),
            ('x', UnderlineStyle::Single)
        );
        assert_eq!((cell(1).ch, cell(1).width), ('漢', CellWidth::Wide));
        assert_eq!(cell(2).width, CellWidth::Continuation);
        assert_eq!(cell(3).ch, 'y');
        assert_eq!(cell(4).ch, 'e', "the cells past the preedit stay");
        let cursor = cells.cursor().expect("a cursor");
        assert_eq!((cursor.col, cursor.shape), (1, cell::CursorShape::Bar));
    }

    #[test]
    fn test_a_preedit_stops_at_the_row_end_and_a_hidden_caret_hides_the_cursor() {
        let mut cells = painted("abcdefgh", &["$", "a"], &Theme::dark());
        let preedit = preedit("漢漢漢", None);
        overlay_preedit(&mut cells, &preedit);
        // The cursor sits at column 8 of 10: one wide character fits.
        let cell = |col: usize| cells.cell(0, col).expect("a cell");
        assert_eq!((cell(8).ch, cell(8).width), ('漢', CellWidth::Wide));
        assert_eq!(cells.cursor(), None);
    }

    #[test]
    fn test_a_commit_types_into_insert_mode_and_is_dropped_in_normal_mode() {
        let mut editor = Editor::new(EditorOptions::default());
        let document = editor.open_document("");
        let view = editor.new_view(document).expect("a view");
        assert!(!takes_text(&editor, view));
        let i = cherry::keys::parse_key("i").expect("a valid key");
        editor.handle_key(view, i, clock::now());
        assert!(takes_text(&editor, view));
        for key in "你好\n".chars().filter_map(typed_key) {
            editor.handle_key(view, to_cherry(key), clock::now());
        }
        let text = editor
            .document(document)
            .expect("a document")
            .text_with_line_endings();
        assert_eq!(text, "你好\n");
        assert_eq!(typed_key('\u{7}'), None);
    }

    #[test]
    fn test_an_open_prompt_takes_the_bottom_row_and_the_cursor() {
        let theme = Theme::dark();
        let mut editor = Editor::new(EditorOptions::default());
        let document = editor.open_document("ab\ncd\nef\ngh");
        let view = editor.new_view(document).expect("a view");
        let key = cherry::keys::parse_key("/").expect("a valid key");
        editor.handle_key(view, key, clock::now());
        let mut editor_view = EditorView::new(view);
        let mut rows = 0;
        let Some(snapshot) = TestWindow::new(200, 120).render(|frame| {
            let metrics = frame.cell_metrics();
            let area = Rect::new(0.0, 0.0, 10.0 * metrics.width, 3.0 * metrics.height);
            editor_view.lend(&mut editor).draw(frame, area, &theme);
            rows = editor.view(view).map_or(0, |view| view.rows());
        }) else {
            return;
        };
        assert_eq!(rows, 2, "the view gave up a row");
        let metrics = snapshot.metrics();
        let prompt_row = 2.5 * metrics.height;
        snapshot.assert_pixel(
            metrics.width,
            prompt_row,
            theme.palette.cursor,
            "the prompt's bar",
        );
        snapshot.assert_pixel(1.0, 1.0, theme.cursor_line, "no block cursor in the view");
    }

    #[test]
    fn test_presses_drags_and_releases_become_cells_clamped_while_dragging() {
        let (_, mut view) = drawn_view("");
        let now = Instant::now();
        let mut feed = |input| -> Vec<(MouseEventKind, u16, u16)> {
            view.pointer_events(input, now)
                .into_iter()
                .map(|event| (event.kind, event.column, event.row))
                .collect()
        };
        assert!(feed(PointerInput::Moved(Point::new(15.0, 25.0))).is_empty());
        assert_eq!(feed(PointerInput::Pressed), [(MouseEventKind::Press, 1, 1)]);
        assert_eq!(
            feed(PointerInput::Moved(Point::new(35.0, 500.0))),
            [(MouseEventKind::Drag, 3, 2)],
            "a drag below the view stays on its last row"
        );
        assert_eq!(
            feed(PointerInput::Released),
            [(MouseEventKind::Release, 3, 2)]
        );
        assert!(feed(PointerInput::Moved(Point::new(45.0, 5.0))).is_empty());
        assert!(feed(PointerInput::Moved(Point::new(150.0, 5.0))).is_empty());
        assert!(feed(PointerInput::Pressed).is_empty(), "outside the view");
        assert!(feed(PointerInput::Released).is_empty(), "nothing pressed");
    }

    #[test]
    fn test_quick_presses_on_one_cell_count_as_a_double_click() {
        let (_, mut view) = drawn_view("");
        let now = Instant::now();
        view.pointer_events(PointerInput::Moved(Point::new(15.0, 5.0)), now);
        let kinds: Vec<MouseEventKind> = [
            PointerInput::Pressed,
            PointerInput::Released,
            PointerInput::Pressed,
        ]
        .into_iter()
        .flat_map(|input| view.pointer_events(input, now))
        .map(|event| event.kind)
        .collect();
        assert_eq!(
            kinds,
            [
                MouseEventKind::Press,
                MouseEventKind::Release,
                MouseEventKind::DoubleClick
            ]
        );
    }

    #[test]
    fn test_wheel_notches_scroll_and_touchpad_pixels_add_up_to_notches() {
        let (_, mut view) = drawn_view("");
        let now = Instant::now();
        view.pointer_events(PointerInput::Moved(Point::new(15.0, 5.0)), now);
        let mut kinds = |delta| -> Vec<MouseEventKind> {
            view.pointer_events(PointerInput::Wheel(delta), now)
                .into_iter()
                .map(|event| event.kind)
                .collect()
        };
        assert_eq!(
            kinds(WheelDelta::Lines(Point::new(0.0, -2.0))),
            [MouseEventKind::ScrollDown, MouseEventKind::ScrollDown]
        );
        // A notch is three rows of 20 pixels; 30 pixels is half of one.
        let pixels = WheelDelta::Pixels(Point::new(0.0, 30.0));
        assert!(kinds(pixels).is_empty());
        assert_eq!(kinds(pixels), [MouseEventKind::ScrollUp]);
    }

    #[test]
    fn test_a_press_moves_the_editor_cursor_to_the_character_under_the_pointer() {
        let (mut editor, mut view) = drawn_view("abc\ndef\n");
        let now = Instant::now();
        view.pointer_events(PointerInput::Moved(Point::new(15.0, 25.0)), now);
        for event in view.pointer_events(PointerInput::Pressed, now) {
            editor.handle_mouse(view.view(), event);
        }
        let cursor = editor.view(view.view()).map(|view| view.cursor());
        assert_eq!(cursor, Some(5), "line 2, column 2");
    }

    #[test]
    fn test_decorations_draw_undercurls_and_signs_in_their_kind_color() {
        let theme = Theme::dark();
        let options = EditorOptions {
            sign_column: true,
            ..EditorOptions::default()
        };
        let mut editor = Editor::new(options);
        let document = editor.open_document("abc def\n");
        let view = editor.new_view(document).expect("a view");
        let decorations = vec![
            Decoration::new(0..3, DecorationKind::Error),
            Decoration::new(4..7, DecorationKind::Hint),
        ];
        editor.set_decorations(document, "test", decorations);
        editor.set_signs(
            document,
            "test",
            vec![Sign::new(0, "W", DecorationKind::Warning)],
        );
        let cells = cells_of(&mut editor, view, &theme);
        let row: Vec<&cell::Cell> = (0..cells.cols())
            .filter_map(|col| cells.cell(0, col))
            .collect();
        let sign = row.iter().find(|cell| cell.ch == 'W').expect("the sign");
        assert_eq!(sign.style.foreground, Color::Rgb(theme.warning));
        let text = row
            .iter()
            .position(|cell| cell.ch == 'a')
            .expect("the text");
        let underline = |col: usize| (row[col].style.underline, row[col].style.underline_color);
        assert_eq!(
            underline(text),
            (UnderlineStyle::Curly, Color::Rgb(theme.danger))
        );
        assert_eq!(underline(text + 3).0, UnderlineStyle::None, "the space");
        assert_eq!(
            underline(text + 4),
            (UnderlineStyle::Dotted, Color::Rgb(theme.muted))
        );
    }

    #[test]
    fn test_an_error_undercurl_is_drawn_in_the_danger_color_under_its_text() {
        let theme = Theme::dark();
        let mut editor = Editor::new(EditorOptions::default());
        let document = editor.open_document("abc\n");
        let view = editor.new_view(document).expect("a view");
        let decorations = vec![Decoration::new(0..3, DecorationKind::Error)];
        editor.set_decorations(document, "test", decorations);
        let key = cherry::keys::parse_key("j").expect("a valid key");
        editor.handle_key(view, key, clock::now());
        let mut editor_view = EditorView::new(view);
        let Some(snapshot) = TestWindow::new(200, 120).render(|frame| {
            let metrics = frame.cell_metrics();
            let area = Rect::new(0.0, 0.0, 10.0 * metrics.width, 3.0 * metrics.height);
            editor_view.lend(&mut editor).draw(frame, area, &theme);
        }) else {
            return;
        };
        let metrics = snapshot.metrics();
        let band = |col: f32| {
            Rect::new(
                (col + 1.0) * metrics.width,
                metrics.height * 0.6,
                metrics.width * 0.2,
                metrics.height * 0.4,
            )
        };
        assert!(
            snapshot.contains_color(band(1.0), theme.danger),
            "an undercurl under the decorated text"
        );
        assert!(
            !snapshot.contains_color(band(4.0), theme.danger),
            "none past it"
        );
    }

    #[test]
    fn test_an_open_completion_menu_is_drawn_under_the_completed_text() {
        let theme = Theme::dark();
        let mut editor = Editor::new(EditorOptions::default());
        let document = editor.open_document("alpha\nalpine\n");
        let view = editor.new_view(document).expect("a view");
        for spec in ["G", "o", "a", "l", "C-x", "C-n"] {
            let key = cherry::keys::parse_key(spec).expect("a valid key");
            editor.handle_key(view, key, clock::now());
        }
        let items = editor
            .view(view)
            .and_then(|view| view.completion())
            .map(|completion| completion.items().len());
        assert_eq!(items, Some(2), "alpha and alpine");
        let mut editor_view = EditorView::new(view);
        let Some(snapshot) = TestWindow::new(300, 200).render(|frame| {
            let metrics = frame.cell_metrics();
            let area = Rect::new(0.0, 0.0, 20.0 * metrics.width, 6.0 * metrics.height);
            editor_view.lend(&mut editor).draw(frame, area, &theme);
        }) else {
            return;
        };
        let metrics = snapshot.metrics();
        // The new line is the third; the menu's first row is the fourth.
        let first_row = Rect::new(
            0.0,
            3.0 * metrics.height,
            12.0 * metrics.width,
            metrics.height,
        );
        assert!(
            snapshot.contains_color(first_row, theme.selection),
            "the typed match is lit"
        );
        assert!(
            snapshot.contains_color(first_row, theme.border),
            "inside the popup's border"
        );
    }

    #[test]
    fn test_the_view_describes_its_lines_as_text_runs_with_the_cursor() {
        let id = FocusId::new("editor");
        let mut editor = Editor::new(EditorOptions::default());
        let document = editor.open_document("ab\ncd\n");
        let view = editor.new_view(document).expect("a view");
        let mut editor_view = EditorView::new(view);
        let mut frame = TestWindow::new(400, 300).frame();
        for spec in ["j", "l"] {
            let key = cherry::keys::parse_key(spec).expect("a valid key");
            editor.handle_key(view, key, clock::now());
        }
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        editor_view
            .lend(&mut editor)
            .draw(&mut frame, area, &Theme::dark());
        editor_view.access(&editor, &mut frame, id);
        let update = access_tree(&frame, "t");
        let node = |id| access_node(&update, id).expect("described");
        assert_eq!(node(id).role(), Role::MultilineTextInput);
        let second = node(id.child(1));
        assert_eq!(
            (second.role(), second.value()),
            (Role::TextRun, Some("cd\n"))
        );
        assert_eq!(second.character_lengths(), [1, 1, 1]);
        let caret = TextPosition {
            character_index: 1,
            node: node_id(id.child(1)),
        };
        assert_eq!(
            node(id).text_selection().copied(),
            Some(TextSelection {
                anchor: caret,
                focus: caret
            }),
            "the cursor, on line 2 column 2"
        );
    }

    #[test]
    fn test_a_visual_selection_is_described_from_its_anchor_through_the_cursor() {
        let id = FocusId::new("editor");
        let mut editor = Editor::new(EditorOptions::default());
        let document = editor.open_document("ab\ncd\nef\n");
        let view = editor.new_view(document).expect("a view");
        let mut editor_view = EditorView::new(view);
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        let mut selection_after = |keys: &[&str]| {
            for spec in keys {
                let key = cherry::keys::parse_key(spec).expect("a valid key");
                editor.handle_key(view, key, clock::now());
            }
            let mut frame = TestWindow::new(400, 300).frame();
            editor_view
                .lend(&mut editor)
                .draw(&mut frame, area, &Theme::dark());
            editor_view.access(&editor, &mut frame, id);
            let update = access_tree(&frame, "t");
            let selection = access_node(&update, id)
                .expect("described")
                .text_selection();
            selection.map(|selection| {
                let at = |position: TextPosition| {
                    let line = (0..3).find(|line| node_id(id.child(*line)) == position.node);
                    (line.expect("a line in sight"), position.character_index)
                };
                (at(selection.anchor), at(selection.focus))
            })
        };
        assert_eq!(
            selection_after(&["l", "v", "j"]),
            Some(((0, 1), (1, 2))),
            "both ends included: b through d"
        );
        assert_eq!(
            selection_after(&["o"]),
            Some(((1, 2), (0, 1))),
            "o swaps the ends, so the focus is before the anchor"
        );
        assert_eq!(
            selection_after(&["V"]),
            Some(((2, 0), (0, 0))),
            "whole lines, from the end of line 2 back to the start of line 1"
        );
        assert_eq!(
            selection_after(&["<esc>"]),
            Some(((0, 1), (0, 1))),
            "collapsed at the cursor again in Normal mode"
        );
    }

    /// A painted cell showing `x` in `style`, highlighted as `highlight`.
    fn painted_cell(style: cherry::Style, highlight: Option<u16>) -> cherry::Cell {
        cherry::Cell {
            highlight: highlight.map(cherry::HighlightId::new),
            layers: Layers::default(),
            style,
            symbol: "x".to_owned(),
            underline: None,
        }
    }

    /// Host styles: id 0 plain, id 1 a bold red text color, id 2 a green
    /// wash with a blue text color.
    fn host_styles() -> Vec<HighlightStyle> {
        vec![
            HighlightStyle::default(),
            HighlightStyle {
                attributes: Attributes::BOLD,
                background: None,
                foreground: Some(Rgb::new(200, 0, 0)),
            },
            HighlightStyle {
                attributes: Attributes::NONE,
                background: Some(Rgb::new(0, 90, 0)),
                foreground: Some(Rgb::new(0, 0, 200)),
            },
        ]
    }

    #[test]
    fn test_host_styles_color_highlighted_text_but_a_selection_keeps_its_background() {
        let theme = Theme::dark();
        let styles = host_styles();
        let text = cell_style(&painted_cell(cherry::Style::Text, Some(1)), &theme, &styles);
        assert_eq!(text.foreground, Color::Rgb(Rgb::new(200, 0, 0)));
        assert!(text.attributes.contains(Attributes::BOLD));
        let selected = cell_style(
            &painted_cell(cherry::Style::Selection, Some(2)),
            &theme,
            &styles,
        );
        assert_eq!(selected.background, Color::Rgb(theme.selection));
        assert_eq!(selected.foreground, Color::Rgb(Rgb::new(0, 0, 200)));
        let past_the_end = cell_style(&painted_cell(cherry::Style::Text, Some(9)), &theme, &styles);
        assert_eq!(
            past_the_end,
            cell::Style::default(),
            "an id the host gave no style"
        );
    }

    #[test]
    fn test_a_wash_and_a_sign_of_the_hosts_own_kind_take_their_ids_colors() {
        let theme = Theme::dark();
        let styles = host_styles();
        let id = cherry::HighlightId::new(2);
        let wash = cell_style(
            &painted_cell(cherry::Style::Background(id), None),
            &theme,
            &styles,
        );
        assert_eq!(wash.background, Color::Rgb(Rgb::new(0, 90, 0)));
        let over_text = cell_style(
            &painted_cell(cherry::Style::Background(id), Some(1)),
            &theme,
            &styles,
        );
        assert_eq!(
            over_text.foreground,
            Color::Rgb(Rgb::new(0, 0, 200)),
            "the wash's text color over the text's own highlight"
        );
        let unstyled = cherry::Style::Background(cherry::HighlightId::new(7));
        let fallback = cell_style(&painted_cell(unstyled, None), &theme, &styles);
        assert_eq!(
            fallback.background,
            Color::Rgb(theme.raised),
            "no style: raised"
        );
        let sign = cherry::Style::Sign(DecorationKind::Background(id));
        let sign = cell_style(&painted_cell(sign, None), &theme, &styles);
        assert_eq!(sign.foreground, Color::Rgb(Rgb::new(0, 0, 200)));
    }

    #[test]
    fn test_an_inlay_is_muted_unless_its_highlight_colors_it() {
        let theme = Theme::dark();
        let styles = host_styles();
        let plain = cell_style(&painted_cell(cherry::Style::Inlay, None), &theme, &styles);
        assert_eq!(plain.foreground, Color::Rgb(theme.muted));
        assert!(plain.attributes.contains(Attributes::ITALIC));
        let colored = cell_style(
            &painted_cell(cherry::Style::Inlay, Some(1)),
            &theme,
            &styles,
        );
        assert_eq!(colored.foreground, Color::Rgb(Rgb::new(200, 0, 0)));
    }

    #[test]
    fn test_brackets_extra_cursors_and_preedits_stand_apart_from_text() {
        let theme = Theme::dark();
        let drawn = |style| cell_style(&painted_cell(style, None), &theme, &[]);
        let bracket = drawn(cherry::Style::MatchingBracket);
        assert_eq!(bracket.background, Color::Rgb(theme.border));
        let extra = drawn(cherry::Style::ExtraCursor);
        assert_eq!(extra.background, Color::Rgb(theme.palette.cursor));
        assert_eq!(
            drawn(cherry::Style::Preedit).underline,
            UnderlineStyle::Single
        );
    }

    #[test]
    fn test_a_view_in_a_font_of_its_own_takes_as_many_cells_as_that_font_fits() {
        let (mut editor, mut view) = drawn_view("text");
        let id = view.view();
        let font = Font::new(30.0);
        view.set_font(Some(font.clone()));
        let theme = Theme::dark();
        let drawn = TestWindow::new(600, 300).render(|frame| {
            let cell = frame.cell_metrics_of(&font);
            let area = Rect::new(0.0, 0.0, 7.0 * cell.width, 2.0 * cell.height);
            view.lend(&mut editor).draw(frame, area, &theme);
        });
        if drawn.is_none() {
            return;
        }
        let shown = editor.view(id).expect("the view");
        assert_eq!((shown.cols(), shown.rows()), (7, 2), "the big font's cells");
    }

    #[test]
    fn test_an_offset_rect_is_the_cell_its_char_was_drawn_in() {
        let (editor, view) = drawn_view("ab\ncd");
        assert_eq!(
            view.offset_rect(&editor, 4),
            Some(Rect::new(10.0, 20.0, 10.0, 20.0)),
            "d: the second cell of the second row"
        );
        assert_eq!(
            EditorView::new(view.view()).offset_rect(&editor, 0),
            None,
            "never drawn"
        );
    }

    #[test]
    fn test_a_preedit_in_insert_mode_is_laid_into_the_line_before_the_text_after_it() {
        let (mut editor, mut view) = drawn_view("ab");
        let id = view.view();
        editor.handle_key(
            id,
            cherry::keys::parse_key("a").expect("a key"),
            clock::now(),
        );
        let composition = Composition {
            cursor: None,
            text: "\u{1ea1}".to_owned(),
        };
        let input = Input::Ime(ImeInput::Preedit(composition));
        let mut window = TestWindow::new(200, 120);
        let response = window.feed(&mut view.lend(&mut editor), &input);
        assert_eq!(response, EditorResponse::Handled);
        let theme = Theme::dark();
        let cells = cells_of(&mut editor, id, &theme);
        let at = |col: usize| cells.cell(0, col).expect("in the grid").clone();
        assert_eq!(
            (at(0).ch, at(1).ch, at(2).ch),
            ('a', '\u{1ea1}', 'b'),
            "b moved right"
        );
        assert_eq!(at(1).style.underline, UnderlineStyle::Single);
        let document = editor.view(id).expect("the view").document();
        let text = editor
            .document(document)
            .expect("open")
            .text_with_line_endings();
        assert_eq!(text, "ab", "nothing typed");
    }

    #[cfg(feature = "syntax")]
    #[test]
    fn test_every_highlight_name_but_plain_variables_has_a_color_in_both_themes() {
        for theme in [Theme::dark(), Theme::light()] {
            let uncolored: Vec<&str> = cherry::syntax::HIGHLIGHT_NAMES
                .iter()
                .copied()
                .filter(|name| theme.syntax.color(name).is_none())
                .collect();
            assert_eq!(uncolored, ["variable"]);
        }
    }

    /// An editor with one view onto Rust `text`, highlighted.
    #[cfg(feature = "syntax")]
    fn rust_editor(text: &str) -> (Editor, ViewId) {
        let mut editor = Editor::new(EditorOptions::default());
        let document = editor.open_document(text);
        let language = cherry::syntax::Language::Rust;
        let source = editor.document(document).expect("just opened");
        let highlighter = cherry::syntax::SyntaxHighlighter::new(language, source).expect("loads");
        editor.set_highlighter(document, Some(Box::new(highlighter)));
        let view = editor.new_view(document).expect("a view");
        (editor, view)
    }

    #[cfg(feature = "syntax")]
    #[test]
    fn test_highlighted_text_takes_its_syntax_color_but_a_search_match_keeps_its_own() {
        let theme = Theme::dark();
        let keyword = theme.syntax.color("keyword").expect("a keyword color");
        let (mut editor, view) = rust_editor("fn x() {}\n");
        let cells = cells_of(&mut editor, view, &theme);
        let foreground =
            |cells: &CellBuffer, col: usize| cells.cell(0, col).map(|cell| cell.style.foreground);
        assert_eq!(foreground(&cells, 0), Some(Color::Rgb(keyword)));
        assert_ne!(foreground(&cells, 3), Some(Color::Rgb(keyword)));
        for spec in ["/", "f", "n", "RET"] {
            let key = cherry::keys::parse_key(spec).expect("a valid key");
            editor.handle_key(view, key, clock::now());
        }
        let cells = cells_of(&mut editor, view, &theme);
        assert_eq!(
            foreground(&cells, 0),
            Some(Color::Rgb(theme.palette.background)),
            "dark text on the match color"
        );
    }

    #[cfg(feature = "syntax")]
    #[test]
    fn test_a_keyword_is_drawn_in_the_keyword_color() {
        let theme = Theme::dark();
        let keyword = theme.syntax.color("keyword").expect("a keyword color");
        let (mut editor, view) = rust_editor("\nfn x() {}\n");
        let mut editor_view = EditorView::new(view);
        let Some(snapshot) = TestWindow::new(200, 120).render(|frame| {
            let metrics = frame.cell_metrics();
            let area = Rect::new(0.0, 0.0, 10.0 * metrics.width, 3.0 * metrics.height);
            editor_view.lend(&mut editor).draw(frame, area, &theme);
        }) else {
            return;
        };
        let metrics = snapshot.metrics();
        let row = |col: f32, cols: f32| {
            Rect::new(
                col * metrics.width,
                metrics.height,
                cols * metrics.width,
                metrics.height,
            )
        };
        assert!(snapshot.contains_color(row(0.0, 2.0), keyword), "fn");
        assert!(
            !snapshot.contains_color(row(3.0, 1.0), keyword),
            "the name is not a keyword"
        );
    }
}
