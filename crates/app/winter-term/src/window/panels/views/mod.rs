//! The views a panel shows: narrow lists in a sidebar (Files, Git, Outline,
//! Todo) and wide ones in the bottom panel (Problems, Output, Search). Each
//! is drawn by misty in the UI font, never on the cell grid, and is a page
//! the window holds in no pane, named by the id its panel keeps.
//!
//! Most views are a [`PanelList`] of rows, a cursor, and the panel keys
//! (block `panel` of `keybindings.kdl`), wrapped as a page by
//! [`PanelPage`]; a view says what its rows are and what opening one does
//! ([`PanelView`]).

pub(in crate::window) mod files;
pub(in crate::window) mod git;
pub(in crate::window) mod outline;
pub(in crate::window) mod output;
pub(in crate::window) mod problems;
pub(in crate::window) mod search;
pub(in crate::window) mod todo;

use std::time::Instant;

use misty::view::{Press, Space, column, row, text};
use misty::{AccessAction, Element, EventCx, Frame as MistyFrame, HostedView, Input, Rect};
use misty::{Theme, ThemeToken};

use winter_model::editor::{
    EditorOutcome, EditorStatus, JobReply, Notice, Page, PageView, RowWindow,
};
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed, TableMatch};
use winter_model::paint::{Frame, Row};

// ========================================================================
// Constants
// ========================================================================

/// The panel keys, block `panel`, which every view's list answers.
const KEYS: KeyTable<ListAct> = KeyTable {
    block: "panel",
    bindings: &[
        Binding {
            action: ListAct::Down,
            id: "next_row",
            keys: &["j", "<down>"],
            label: "Next Row",
        },
        Binding {
            action: ListAct::Up,
            id: "previous_row",
            keys: &["k", "<up>"],
            label: "Previous Row",
        },
        Binding {
            action: ListAct::First,
            id: "first_row",
            keys: &["g g", "<home>"],
            label: "First Row",
        },
        Binding {
            action: ListAct::Last,
            id: "last_row",
            keys: &["G", "<end>"],
            label: "Last Row",
        },
        Binding {
            action: ListAct::Open,
            id: "open",
            keys: &["RET"],
            label: "Open in the Main Area",
        },
        Binding {
            action: ListAct::Expand,
            id: "expand",
            keys: &["l", "<right>"],
            label: "Open the Folder or Group",
        },
        Binding {
            action: ListAct::Collapse,
            id: "collapse",
            keys: &["h", "<left>"],
            label: "Close the Folder or Group",
        },
        Binding {
            action: ListAct::Refresh,
            id: "refresh",
            keys: &["r"],
            label: "Look Again",
        },
        Binding {
            action: ListAct::OpenTool,
            id: "open_tool",
            keys: &["t"],
            label: "Open the Tool in the Main Area",
        },
    ],
    configurable: true,
    title: "Panel",
};

/// The panels' key tables, for the Keys page and `keybindings.kdl`.
pub(crate) const KEY_TABLES: &[&dyn Listed] = &[&KEYS, &git::KEYS, &search::KEYS];

/// How many cell rows one row of a list takes: its words in the UI font and
/// the space around them.
const CELLS_PER_ROW: f32 = 1.5;

/// The space around a row's words, in logical pixels across and down.
const ROW_PADDING_PX: (f32, f32) = (8.0, 3.0);

/// How far a row is set in for each level of depth, in logical pixels.
const INDENT_PX: f32 = 12.0;

/// The space between the parts of a row, in logical pixels.
const ROW_GAP_PX: f32 = 6.0;

/// The marks on a folder or group's row, open and closed.
const OPEN_MARK: &str = "\u{25be}";
const CLOSED_MARK: &str = "\u{25b8}";

/// The clicks on a row that open it.
const DOUBLE_CLICK: u8 = 2;

// ========================================================================
// Data Structures
// ========================================================================

/// What a panel key or a click asks of a view's list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum ListAct {
    Collapse,
    Down,
    Expand,
    First,
    Last,
    Open,
    OpenTool,
    Refresh,
    Up,
}

/// A view's rows, the cursor among them, and which are built.
#[derive(Debug, Default)]
pub(in crate::window) struct PanelList {
    /// What a click asked, until the view takes it.
    clicked: Option<ListAct>,
    rows: Vec<ListRow>,
    /// The row the cursor is on, never a heading while any row is not one.
    selected: usize,
    /// The keys of a sequence typed so far (`g` of `g g`).
    typed: Vec<Key>,
    /// Which rows are built, around the cursor.
    window: RowWindow,
}

/// One row of a view's list.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::window) struct ListRow {
    /// How far it is set in: a folder's depth in a tree.
    pub(in crate::window) depth: usize,
    /// What it says at its right, muted: a count, a line number.
    pub(in crate::window) detail: Option<String>,
    /// Whether it is a folder or group, open or not.
    pub(in crate::window) fold: Option<bool>,
    /// Whether it heads the rows under it, and takes no cursor.
    pub(in crate::window) heading: bool,
    /// The view's own index for what it stands for.
    pub(in crate::window) key: usize,
    pub(in crate::window) label: String,
    /// A short colored mark before its detail: a git status, a level.
    pub(in crate::window) mark: Option<Mark>,
    /// Its full wording, or what it stands for, on hover.
    pub(in crate::window) tooltip: Option<String>,
}

/// A short colored mark on a row, and what its color means in words.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::window) struct Mark {
    pub(in crate::window) color: ThemeToken,
    pub(in crate::window) meaning: String,
    pub(in crate::window) text: String,
}

/// A view as a page: its state, which the view is built from.
pub(in crate::window) struct PanelPage<S: PanelView> {
    view: PageView<S>,
}

// ========================================================================
// Traits
// ========================================================================

/// What a view says beyond its list: its name, what opening a row does,
/// and the work it asks of the window.
pub(in crate::window) trait PanelView: HostedView + 'static {
    /// What the panel's switcher calls it.
    fn title(&self) -> String;

    /// Its list.
    fn list(&self) -> &PanelList;

    /// Its list, to change.
    fn list_mut(&mut self) -> &mut PanelList;

    /// Do `act` on the row the cursor is on: what the window is to do of it.
    fn act(&mut self, act: ListAct) -> EditorOutcome;

    /// Note that `rows` cell rows fit the panel, for what it shows beside
    /// its list.
    fn fit(&mut self, _rows: usize) {}

    /// A key of its own, ahead of the panel keys: `None` for one it has none
    /// for.
    fn own_key(&mut self, _key: &Key) -> Option<EditorOutcome> {
        None
    }

    /// A job it asked for has finished.
    fn on_job(&mut self, _reply: JobReply) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    /// When it next wants [`PanelView::on_tick`].
    fn next_tick(&self) -> Option<Instant> {
        None
    }

    /// Its tick has come: the work it waits to ask for.
    fn on_tick(&mut self) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    /// What it has to tell the user, taken so it is told once.
    fn take_notice(&mut self) -> Option<Notice> {
        None
    }
}

// ========================================================================
// PanelList Implementations
// ========================================================================

impl PanelList {
    /// The rows, for a test to read.
    #[cfg(test)]
    pub(in crate::window) fn rows(&self) -> &[ListRow] {
        &self.rows
    }

    /// Show `rows`, the cursor kept on the row of the same key where there
    /// is one, else where it was.
    pub(in crate::window) fn set_rows(&mut self, rows: Vec<ListRow>) {
        let was = self.selected_row().map(|row| (row.key, row.heading));
        self.rows = rows;
        let again = was.and_then(|(key, heading)| {
            self.rows
                .iter()
                .position(|row| row.key == key && row.heading == heading)
        });
        self.selected = again
            .unwrap_or(self.selected)
            .min(self.rows.len().saturating_sub(1));
        self.settle(1);
        self.follow();
    }

    /// The row the cursor is on.
    pub(in crate::window) fn selected_row(&self) -> Option<&ListRow> {
        self.rows.get(self.selected)
    }

    /// Put the cursor on the row of `key`, a heading's or not.
    pub(in crate::window) fn select_key(&mut self, key: usize, heading: bool) {
        if let Some(at) = self
            .rows
            .iter()
            .position(|row| row.key == key && row.heading == heading)
        {
            self.selected = at;
            self.follow();
        }
    }

    /// Note that `rows` cell rows fit the panel.
    fn set_fits(&mut self, rows: usize) {
        self.window.set_fits((rows as f32 / CELLS_PER_ROW) as usize);
        self.follow();
    }

    /// A panel key, a sequence (`g g`) waited out: the act it names, which
    /// the list carries out itself where it only moves the cursor.
    fn on_key(&mut self, key: &Key) -> KeyResult {
        self.window.release();
        self.typed.push(*key);
        match table::lookup(&KEYS, &self.typed) {
            TableMatch::Exact(act) => {
                self.typed.clear();
                match self.move_cursor(act) {
                    true => KeyResult::Moved,
                    false => KeyResult::Act(act),
                }
            }
            TableMatch::Prefix => KeyResult::Moved,
            TableMatch::Miss => {
                let ended = self.typed.len() > 1;
                self.typed.clear();
                match ended {
                    true => KeyResult::Moved,
                    false => KeyResult::Missed,
                }
            }
        }
    }

    /// Move the cursor as `act` asks: whether it was a move.
    fn move_cursor(&mut self, act: ListAct) -> bool {
        match act {
            ListAct::Down => self.step(1),
            ListAct::Up => self.step(-1),
            ListAct::First => {
                self.selected = 0;
                self.settle(1);
            }
            ListAct::Last => {
                self.selected = self.rows.len().saturating_sub(1);
                self.settle(-1);
            }
            ListAct::Collapse
            | ListAct::Expand
            | ListAct::Open
            | ListAct::OpenTool
            | ListAct::Refresh => return false,
        }
        self.follow();
        true
    }

    /// Move the cursor one row the way `step` goes, over headings.
    fn step(&mut self, step: isize) {
        let mut at = self.selected;
        while let Some(next) = at
            .checked_add_signed(step)
            .filter(|next| *next < self.rows.len())
        {
            at = next;
            if !self.rows[at].heading {
                self.selected = at;
                return;
            }
        }
    }

    /// Put the cursor off a heading, the way `step` goes, and back the other
    /// way at an end.
    fn settle(&mut self, step: isize) {
        for step in [step, -step] {
            let mut at = self.selected;
            loop {
                match self.rows.get(at) {
                    Some(row) if !row.heading => {
                        self.selected = at;
                        return;
                    }
                    Some(_) => {}
                    None => break,
                }
                match at.checked_add_signed(step) {
                    Some(next) if next < self.rows.len() => at = next,
                    _ => break,
                }
            }
        }
    }

    /// Keep the selected row among those built.
    fn follow(&mut self) {
        self.window.follow(self.selected, self.rows.len());
    }

    /// The row at `at` was pressed: select it, and ask to open it on a
    /// double click, or to open or close it when it is a folder or group.
    fn click(&mut self, at: usize, press: Press) {
        let Some(row) = self.rows.get(at) else {
            return;
        };
        self.clicked = match (row.fold, press.clicks >= DOUBLE_CLICK) {
            (Some(true), _) => Some(ListAct::Collapse),
            (Some(false), _) => Some(ListAct::Expand),
            (None, true) => Some(ListAct::Open),
            (None, false) => None,
        };
        if !row.heading || row.fold.is_some() {
            self.selected = at;
        }
    }
}

/// What a key did to a list.
enum KeyResult {
    /// Asked the view to act on the row the cursor is on.
    Act(ListAct),
    /// No panel key.
    Missed,
    /// Moved the cursor, or waits for the rest of a sequence.
    Moved,
}

// ========================================================================
// PanelPage Implementations
// ========================================================================

impl<S: PanelView> PanelPage<S> {
    /// The page of `view`.
    pub(in crate::window) fn new(view: S) -> Self {
        Self {
            view: PageView::new(view),
        }
    }

    /// The view, to change: what the window feeds it.
    pub(in crate::window) fn state_mut(&mut self) -> &mut S {
        self.view.state_mut()
    }

    /// What a click asked, carried out on the view.
    fn take_click(&mut self) -> Option<EditorOutcome> {
        let state = self.view.state_mut();
        let act = state.list_mut().clicked.take()?;
        Some(state.act(act))
    }
}

impl<S: PanelView> Page for PanelPage<S> {
    fn title(&self) -> String {
        self.view.state().title()
    }

    /// Misty draws the view; the grid under it is empty, its rows kept for
    /// how many of the list's fit.
    fn content(&mut self, rows: usize, _cols: usize, _wrap: bool) -> Frame {
        let state = self.view.state_mut();
        state.list_mut().set_fits(rows);
        state.fit(rows);
        Frame::new(vec![Row::new()])
    }

    fn is_view(&self) -> bool {
        true
    }

    fn draw_view(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        self.view.draw_view(frame, area, theme);
    }

    fn view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<EditorOutcome> {
        let used = self.view.view_input(input, cx);
        self.take_click()
            .or(used.then_some(EditorOutcome::Consumed))
    }

    fn view_access_action(&mut self, action: AccessAction) -> Option<EditorOutcome> {
        let used = self.view.access_action(action);
        self.take_click()
            .or(used.then_some(EditorOutcome::Consumed))
    }

    fn view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.view.view_about_to_wait(cx)
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        let state = self.view.state_mut();
        if let Some(outcome) = state.own_key(key) {
            return outcome;
        }
        match state.list_mut().on_key(key) {
            KeyResult::Act(act) => state.act(act),
            KeyResult::Missed => EditorOutcome::Ignored,
            KeyResult::Moved => EditorOutcome::Consumed,
        }
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        self.view.state_mut().on_job(reply)
    }

    fn next_tick(&self) -> Option<Instant> {
        self.view.state().next_tick()
    }

    fn on_tick(&mut self, _now: Instant) -> EditorOutcome {
        self.view.state_mut().on_tick()
    }

    fn keys(&self) -> Vec<KeyHelp> {
        table::help(&KEYS)
    }

    fn status(&self) -> EditorStatus {
        EditorStatus {
            mode: None,
            segments: Vec::new(),
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.view.state_mut().take_notice()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `state`'s list as misty draws it: the rows built around the cursor, or
/// `empty` in their place while there are none.
pub(in crate::window) fn list_view<'a, S: PanelView>(
    state: &'a S,
    empty: &'a str,
) -> Element<'a, S> {
    let list = state.list();
    if list.rows.is_empty() {
        return Element::from(text(empty).muted().proportional())
            .padding_px(ROW_PADDING_PX.0, ROW_PADDING_PX.1 * 2.0);
    }
    let total = list.rows.len();
    let built = list.window.range(total);
    column(built.map(|at| row_view(list, at)).collect::<Vec<_>>())
        .on_scroll(move |state: &mut S, wheel| state.list_mut().window.wheel(&wheel, total))
}

/// The row at `at` of `list`.
fn row_view<S: PanelView>(list: &PanelList, at: usize) -> Element<'_, S> {
    let shown = &list.rows[at];
    let mut parts: Vec<Element<'_, S>> = Vec::new();
    if let Some(open) = shown.fold {
        let mark = match open {
            true => OPEN_MARK,
            false => CLOSED_MARK,
        };
        parts.push(text(mark).muted().proportional().into());
    }
    let label = match shown.heading {
        true => text(shown.label.as_str())
            .muted()
            .emphasized()
            .proportional(),
        false => text(shown.label.as_str()).proportional(),
    };
    parts.push(Element::from(label).grow(1.0));
    if let Some(mark) = &shown.mark {
        parts.push(
            Element::from(text(mark.text.as_str()).color(mark.color).proportional())
                .tooltip(mark.meaning.as_str()),
        );
    }
    if let Some(detail) = &shown.detail {
        parts.push(text(detail.as_str()).muted().proportional().into());
    }
    let mut built = row(parts)
        .gap(Space::Px(ROW_GAP_PX))
        .padding_px(
            ROW_PADDING_PX.0 + INDENT_PX * shown.depth as f32,
            ROW_PADDING_PX.1,
        )
        .hover_background(ThemeToken::Hover)
        .on_pointer_press(move |state: &mut S, press: Press| state.list_mut().click(at, press));
    if at == list.selected && !shown.heading {
        built = built.background(ThemeToken::SelectedRow);
    }
    match &shown.tooltip {
        Some(tooltip) => built.tooltip(tooltip.as_str()),
        None => built,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: usize) -> ListRow {
        ListRow {
            key,
            label: format!("row {key}"),
            ..ListRow::default()
        }
    }

    fn heading(key: usize) -> ListRow {
        ListRow {
            heading: true,
            ..item(key)
        }
    }

    #[test]
    fn test_the_cursor_steps_over_headings_and_stops_at_the_ends() {
        let mut list = PanelList::default();
        list.set_rows(vec![heading(0), item(1), heading(2), item(3)]);
        assert_eq!(
            list.selected_row().map(|row| row.key),
            Some(1),
            "off the heading"
        );
        list.move_cursor(ListAct::Down);
        assert_eq!(
            list.selected_row().map(|row| row.key),
            Some(3),
            "over the heading"
        );
        list.move_cursor(ListAct::Down);
        assert_eq!(
            list.selected_row().map(|row| row.key),
            Some(3),
            "stops at the end"
        );
        list.move_cursor(ListAct::First);
        assert_eq!(list.selected_row().map(|row| row.key), Some(1));
    }

    #[test]
    fn test_new_rows_keep_the_cursor_on_the_same_key() {
        let mut list = PanelList::default();
        list.set_rows(vec![item(1), item(2), item(3)]);
        list.select_key(3, false);
        list.set_rows(vec![item(0), item(3), item(4)]);
        assert_eq!(list.selected_row().map(|row| row.key), Some(3));
    }
}
