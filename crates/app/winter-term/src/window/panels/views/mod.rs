//! The views a panel shows: narrow lists in a sidebar (Files, Git, Outline,
//! Todo) and wide ones in the bottom panel (Problems, Output, Search). Each
//! is drawn by misty in the UI font, never on the cell grid, and is a page
//! the window holds in no pane, named by the id its panel keeps.
//!
//! A view out of sight does no work of its own: no watch, no read, no tick.
//! The window says when one comes into sight, and it catches up once then.
//!
//! Most views are a [`PanelList`] of rows, a cursor, and the panel keys
//! (block `panel` of `keybindings.kdl`), wrapped as a page by
//! [`PanelPage`]; a view says what its rows are and what opening one does
//! ([`PanelView`]).

pub(in crate::window) mod agents;
pub(in crate::window) mod files;
pub(in crate::window) mod git;
pub(in crate::window) mod outline;
pub(in crate::window) mod output;
pub(in crate::window) mod problems;
pub(in crate::window) mod search;
pub(in crate::window) mod terminals;
pub(in crate::window) mod todo;

use std::time::Instant;

use misty::layout::Justify;
use misty::view::{
    Corners, Extent, LazySource, Press, Space, canvas, column, lazy_column, row, section_header,
    text,
};
use misty::{AccessAction, Element, EventCx, Frame as MistyFrame, HostedView, IconSet, Input};
use misty::{Rect, Svg, Theme, ThemeToken};

use winter_model::editor::{EditorOutcome, EditorStatus, JobReply, Notice, Page, PageView};
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed, TableMatch};
use winter_model::paint::{Frame, IconKind, Row};

use crate::icons;

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
            keys: &["r", "C-r", "M-S-r"],
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
pub(crate) const KEY_TABLES: &[&dyn Listed] = &[
    &KEYS,
    &agents::KEYS,
    &files::KEYS,
    &git::KEYS,
    &output::KEYS,
    &problems::KEYS,
];

/// The space around a row's words, in logical pixels across and down.
const ROW_PADDING_PX: (f32, f32) = (8.0, 3.0);

/// How far a row is set in for each level of depth, in logical pixels.
const INDENT_PX: f32 = 12.0;

/// The space between the parts of a row, in logical pixels.
const ROW_GAP_PX: f32 = 6.0;

/// The marks on a folder or group's row, open and closed.
const OPEN_MARK: &str = "\u{25be}";
const CLOSED_MARK: &str = "\u{25b8}";

/// How wide and tall an icon's slot is, in logical pixels: a row square.
const ICON_PX: f32 = 16.0;

/// How many cells an icon takes in a column it shares with words.
const ICON_CELLS: f32 = 2.0;

/// How wide the slot of a folder's open or closed mark is, in logical
/// pixels.
const FOLD_PX: f32 = 10.0;

/// How wide a trailing fold toggle button is, in logical pixels.
const FOLD_BUTTON_PX: f32 = 18.0;

/// What a folder or group's open or closed mark does when clicked.
const FOLD_TIP: &str = "Collapse";
const UNFOLD_TIP: &str = "Expand";

/// What the bundled codicons put before a code symbol's kind
/// (`symbol-class`), left out of the words it is called by.
const SYMBOL_PREFIX: &str = "symbol-";

/// The bundled codicons' set, drawn in the text's color as symbols are.
const SYMBOL_SET: &str = "symbols/";

/// The widest a detail's column grows, in cells; a longer detail is cut.
const MAX_DETAIL_CELLS: f32 = 30.0;

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

/// A view's rows and the cursor among them; misty builds the rows that
/// show.
#[derive(Debug, Default)]
pub(in crate::window) struct PanelList {
    /// What a click asked, until the view takes it.
    clicked: Option<ListAct>,
    /// The slots every row keeps, so the rows' parts line up.
    columns: Columns,
    /// The artwork of the rows' icons, by name.
    icons: IconSet,
    rows: Vec<ListRow>,
    /// The row the cursor is on, never a heading while any row is not one.
    selected: usize,
    /// The view's title, which keeps its rows' ids apart from another
    /// view's.
    title: String,
    /// The keys of a sequence typed so far (`g` of `g g`).
    typed: Vec<Key>,
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
    /// Whether its words are in the cell grid's font, as data is: a line of
    /// a build's output.
    pub(in crate::window) cells: bool,
    /// Whether it heads the rows under it, and takes no cursor.
    pub(in crate::window) heading: bool,
    /// What it shows before its label: a file's icon, a problem's level.
    pub(in crate::window) icon: Option<RowIcon>,
    /// The view's own index for what it stands for.
    pub(in crate::window) key: usize,
    pub(in crate::window) label: String,
    /// A short colored mark before its detail: a git status, a level.
    pub(in crate::window) mark: Option<Mark>,
    /// Whether its words are dimmed: an entry git ignores, shown anyway.
    pub(in crate::window) muted: bool,
    /// Whether a heading is a note instead: dimmed words under a section's
    /// heading, as an empty section says so, taking no cursor as a heading
    /// does.
    pub(in crate::window) note: bool,
    /// A second line under its words, dimmed and in the cell grid's font,
    /// as data is: where an agent works.
    pub(in crate::window) subtitle: Option<String>,
    /// Its full wording, or what it stands for, on hover.
    pub(in crate::window) tooltip: Option<String>,
    /// Whether the fold toggle sits at the right of the first line instead of
    /// reserving a left column.
    pub(in crate::window) trailing_fold: bool,
}

/// A short colored mark on a row, and what its color means in words.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::window) struct Mark {
    pub(in crate::window) color: ThemeToken,
    /// The icon drawn in place of its text, where the artwork is bundled.
    pub(in crate::window) icon: Option<IconKind>,
    pub(in crate::window) meaning: String,
    pub(in crate::window) text: String,
}

/// An icon on a row, and what it stands for in words.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::window) struct RowIcon {
    /// The color a one-color icon is drawn in; a colored one keeps its own.
    color: ThemeToken,
    kind: IconKind,
    /// What a screen reader calls it.
    name: String,
}

/// Which slots every row of a list keeps, so that its parts line up in
/// columns: a slot no row fills is left out, and one some rows fill is
/// kept empty on the rest.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Columns {
    /// How many cells the details' column takes, where any row has one.
    detail: Option<f32>,
    /// Whether any row is a folder or group.
    fold: bool,
    /// Whether any row has an icon before its label.
    icon: bool,
    /// How many cells the marks' column takes, where any row has one.
    mark: Option<f32>,
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

    /// Note that `rows` cell rows, `cols` cells across, fit the panel, for
    /// what it shows beside its list or how much of its words fit.
    fn fit(&mut self, _rows: usize, _cols: usize) {}

    /// A key of its own, ahead of the panel keys: `None` for one it has none
    /// for.
    fn own_key(&mut self, _key: &Key) -> Option<EditorOutcome> {
        None
    }

    /// Whether one click on the row of `key` opens it, as a double click
    /// does: a row that only shows what is already there, such as a running
    /// agent's terminal.
    fn opens_on_click(&self, _key: usize) -> bool {
        false
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

    /// It came into sight, or went out of it (see [`Page::set_shown`]):
    /// out of sight it does no work of its own, back in sight it catches up
    /// once.
    fn set_shown(&mut self, _shown: bool) -> EditorOutcome {
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
        load_row_icons(&mut self.icons, &self.rows);
        self.columns = Columns::of(&self.rows, &self.icons);
        let again = was.and_then(|(key, heading)| {
            self.rows
                .iter()
                .position(|row| row.key == key && row.heading == heading)
        });
        self.selected = again
            .unwrap_or(self.selected)
            .min(self.rows.len().saturating_sub(1));
        self.settle(1);
    }

    /// Add `rows` at the end, as a stream of them grows: the cursor on the
    /// last row before follows to the new last, and otherwise stays where
    /// it is, as do the rows built.
    pub(in crate::window) fn extend_rows(&mut self, rows: Vec<ListRow>) {
        let following = self.selected + 1 >= self.rows.len();
        load_row_icons(&mut self.icons, &rows);
        self.columns.widen(&rows, &self.icons);
        self.rows.extend(rows);
        if following {
            self.selected = self.rows.len().saturating_sub(1);
        }
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
        }
    }

    /// A panel key, a sequence (`g g`) waited out: the act it names, which
    /// the list carries out itself where it only moves the cursor.
    fn on_key(&mut self, key: &Key) -> KeyResult {
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

    /// The row at `at` was pressed: select it, and ask to open it on a
    /// double click, or on one when `at_once`, or else to open or close it
    /// when it is a folder or group.
    fn click(&mut self, at: usize, press: Press, at_once: bool) {
        let Some(row) = self.rows.get(at) else {
            return;
        };
        self.clicked = match (row.fold, at_once, press.clicks >= DOUBLE_CLICK) {
            (_, true, _) | (None, false, true) => Some(ListAct::Open),
            (Some(true), false, _) => Some(ListAct::Collapse),
            (Some(false), false, _) => Some(ListAct::Expand),
            (None, false, false) => None,
        };
        if !row.heading || row.fold.is_some() {
            self.selected = at;
        }
    }

    /// The fold mark of the row at `at` was pressed: select the row and
    /// open or close it, whatever a click on the rest of the row does.
    fn click_fold(&mut self, at: usize) {
        let Some(open) = self.rows.get(at).and_then(|row| row.fold) else {
            return;
        };
        self.clicked = Some(match open {
            true => ListAct::Collapse,
            false => ListAct::Expand,
        });
        self.selected = at;
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
    pub(in crate::window) fn new(mut view: S) -> Self {
        view.list_mut().title = view.title();
        Self {
            view: PageView::new(view),
        }
    }

    /// The view.
    pub(in crate::window) fn state(&self) -> &S {
        self.view.state()
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
    /// how many of the list's fit, its columns for how wide its words may
    /// be.
    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        self.view.state_mut().fit(rows, cols);
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

    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        self.view.state_mut().set_shown(shown)
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
// RowIcon Implementations
// ========================================================================

impl RowIcon {
    /// The icon of `kind`, called `name`, tinted `color` where it takes one.
    pub(in crate::window) fn new(
        kind: IconKind,
        name: impl Into<String>,
        color: ThemeToken,
    ) -> Self {
        Self {
            color,
            kind,
            name: name.into(),
        }
    }

    /// The icon of the file or folder named `name`: a folder, open or not,
    /// where `open` says, else a file.
    pub(in crate::window) fn of_path(name: &str, open: Option<bool>) -> Self {
        let (kind, said) = match open {
            Some(expanded) => (
                IconKind::Dir {
                    expanded,
                    name: name.to_string(),
                },
                "Folder",
            ),
            None => (
                IconKind::File {
                    name: name.to_string(),
                },
                "File",
            ),
        };
        Self::new(kind, said, ThemeToken::Muted)
    }

    /// The icon of `kind`, called by what it depicts.
    pub(in crate::window) fn of_kind(kind: IconKind) -> Self {
        let said = match &kind {
            IconKind::Dir { .. } => "Folder",
            IconKind::Editor { .. } => "Editor",
            IconKind::File { .. } => "File",
            IconKind::Git { .. } => "Git state",
            IconKind::Gutter { .. } => "Mark",
            IconKind::Named { .. } => "Icon",
            IconKind::Program { .. } => "Program",
            IconKind::Spinner => "Working",
            IconKind::Symbol { name } => name.strip_prefix(SYMBOL_PREFIX).unwrap_or(name),
            IconKind::Terminal => "Terminal",
        };
        Self::new(kind, said, ThemeToken::Muted)
    }
}

// ========================================================================
// Columns Implementations
// ========================================================================

impl Columns {
    /// The slots `rows` need, their icons' artwork loaded in `icons`.
    fn of(rows: &[ListRow], icons: &IconSet) -> Self {
        let mut columns = Self::default();
        columns.widen(rows, icons);
        columns
    }

    /// Widen these slots to what `rows` need as well: rows added to a list,
    /// measured without the ones it held.
    fn widen(&mut self, rows: &[ListRow], icons: &IconSet) {
        let widest =
            |cells: Option<f32>, wide: f32| Some(cells.map_or(wide, |cells| cells.max(wide)));
        for row in rows {
            self.fold |= row.fold.is_some() && !row.trailing_fold && !row.heading;
            self.icon |= row.icon.is_some();
            if let Some(mark) = &row.mark {
                let drawn = mark.icon.as_ref().and_then(|kind| art_name(icons, kind));
                let wide = match drawn {
                    Some(_) => ICON_CELLS,
                    None => cells_of(&mark.text),
                };
                self.mark = widest(self.mark, wide);
            }
            if let Some(detail) = &row.detail {
                self.detail = widest(self.detail, cells_of(detail).min(MAX_DETAIL_CELLS));
            }
        }
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
    lazy_column(list, list.selected).into()
}

impl<S: PanelView> LazySource<S> for PanelList {
    fn count(&self) -> usize {
        self.rows.len()
    }

    /// Row `at`, known by what it stands for, so its icons keep their own
    /// state as rows come and go; the view's title keeps two views' apart.
    fn row(&self, at: usize) -> Element<'_, S> {
        let row = &self.rows[at];
        row_view(self, at).id(format!("{}/{}/{}", self.title, row.key, row.heading))
    }
}

/// The row at `at` of `list`, its parts in the list's columns.
fn row_view<S: PanelView>(list: &PanelList, at: usize) -> Element<'_, S> {
    let shown = &list.rows[at];
    if shown.heading && !shown.note {
        return heading_view(list, at);
    }
    let columns = list.columns;
    let mut parts: Vec<Element<'_, S>> = Vec::new();
    if columns.fold {
        let mark = match shown.fold.filter(|_| !shown.trailing_fold) {
            Some(open) => fold_button(open, at, FOLD_PX),
            None => blank(Extent::Px(FOLD_PX)),
        };
        parts.push(mark);
    }
    if columns.icon {
        let icon = shown.icon.as_ref().and_then(|icon| {
            let art = art_name(&list.icons, &icon.kind)?;
            Some(icon_view(&list.icons, art, icon.color, &icon.name))
        });
        parts.push(icon.unwrap_or_else(|| blank(Extent::Px(ICON_PX))));
    }
    let label = match shown.heading {
        true if shown.note => text(shown.label.as_str()).muted().proportional(),
        // In capitals, so it never reads as one of the rows under it.
        true => text(shown.label.to_uppercase())
            .muted()
            .emphasized()
            .proportional(),
        false if shown.cells => text(shown.label.as_str()),
        false if shown.muted => text(shown.label.as_str()).muted().proportional(),
        false => text(shown.label.as_str()).proportional(),
    };
    let trailing_mark = columns.mark.map(|cells| {
        let mark = shown.mark.as_ref().map(|mark| mark_view(&list.icons, mark));
        let slot = mark.unwrap_or_else(|| blank(Extent::Px(ICON_PX)));
        row([slot]).width(Extent::Cells(cells))
    });
    let trailing_detail = columns.detail.map(|cells| {
        let detail = shown.detail.as_deref().unwrap_or_default();
        let detail = Element::from(text(detail).muted().proportional());
        row([detail])
            .justify(Justify::End)
            .width(Extent::Cells(cells))
    });
    let trailing_fold = shown
        .fold
        .filter(|_| shown.trailing_fold)
        .map(|open| fold_button(open, at, FOLD_BUTTON_PX));
    match (&shown.subtitle, shown.trailing_fold) {
        (Some(subtitle), true) => {
            let mut first_line = vec![Element::from(label).grow(1.0)];
            first_line.extend(trailing_mark);
            first_line.extend(trailing_detail);
            first_line.extend(trailing_fold);
            let stack = column([
                row(first_line).gap(Space::Px(ROW_GAP_PX)),
                Element::from(text(subtitle).muted()),
            ]);
            parts.push(stack.grow(1.0));
        }
        (Some(subtitle), false) => {
            parts.push(
                column([Element::from(label), Element::from(text(subtitle).muted())]).grow(1.0),
            );
            parts.extend(trailing_mark);
            parts.extend(trailing_detail);
        }
        (None, _) => {
            parts.push(Element::from(label).grow(1.0));
            parts.extend(trailing_mark);
            parts.extend(trailing_detail);
            parts.extend(trailing_fold);
        }
    }
    let mut built = row(parts)
        .gap(Space::Px(ROW_GAP_PX))
        .padding_px(
            ROW_PADDING_PX.0 + INDENT_PX * shown.depth as f32,
            ROW_PADDING_PX.1,
        )
        // Outlined rather than filled under the pointer, so the selected
        // row's fill still shows and the two never read alike.
        .hover_border(ThemeToken::Border)
        .on_pointer_press(move |state: &mut S, press: Press| {
            let at_once = state
                .list()
                .rows
                .get(at)
                .is_some_and(|row| !row.heading && state.opens_on_click(row.key));
            state.list_mut().click(at, press, at_once);
        });
    if at == list.selected && !shown.heading {
        built = built.background(ThemeToken::SelectedRow);
    }
    match &shown.tooltip {
        Some(tooltip) => built.tooltip(tooltip.as_str()),
        None => built,
    }
}

/// A section header row at `at` of `list`, rendered with misty's section header.
fn heading_view<S: PanelView>(list: &PanelList, at: usize) -> Element<'_, S> {
    let shown = &list.rows[at];
    let mut header = section_header(shown.label.as_str()).separated(at > 0);
    if let Some(open) = shown.fold {
        header = header
            .open(open)
            .on_pointer_press(move |state: &mut S, _| state.list_mut().click_fold(at));
    }
    if let Some(icon) = &shown.icon
        && let Some(art) = art_name(&list.icons, &icon.kind)
    {
        header = header.leading(icon_view(&list.icons, art, icon.color, &icon.name));
    }
    if let Some(detail) = &shown.detail {
        header = header.count(detail.as_str());
    }
    if let Some(mark) = &shown.mark {
        header = header.trailing(mark_view(&list.icons, mark));
    }
    if let Some(tooltip) = &shown.tooltip {
        header = header.tooltip(tooltip.as_str());
    }
    header.into()
}

/// Clickable fold chevron for the row at `at`, separate from the row's own
/// click target so a single-click row can still be folded.
fn fold_button<S: PanelView>(open: bool, at: usize, width: f32) -> Element<'static, S> {
    let glyph = match open {
        true => OPEN_MARK,
        false => CLOSED_MARK,
    };
    let tip = match open {
        true => FOLD_TIP,
        false => UNFOLD_TIP,
    };
    row([Element::from(text(glyph).muted().proportional())])
        .justify(Justify::Center)
        .width(Extent::Px(width))
        .corners(Corners::Control)
        .hover_background(ThemeToken::Hover)
        .hover_border(ThemeToken::Border)
        .tooltip(tip)
        .on_pointer_press(move |state: &mut S, _| state.list_mut().click_fold(at))
}

/// `mark` as its icon where the artwork is bundled, else as its words, in
/// its color either way.
fn mark_view<'a, S: PanelView>(icons: &'a IconSet, mark: &'a Mark) -> Element<'a, S> {
    let art = mark.icon.as_ref().and_then(|kind| art_name(icons, kind));
    match art {
        Some(art) => icon_view(icons, art, mark.color, &mark.meaning),
        None => Element::from(text(mark.text.as_str()).color(mark.color).proportional())
            .tooltip(mark.meaning.as_str()),
    }
}

/// The icon `art` of `icons`, a row square, tinted `color` where it takes
/// one, called `name` to a screen reader and to the pointer.
fn icon_view<'a, S: PanelView>(
    icons: &'a IconSet,
    art: String,
    color: ThemeToken,
    name: &'a str,
) -> Element<'a, S> {
    let drawn = canvas(move |frame: &mut MistyFrame, area: Rect, theme: &Theme| {
        let side = area.width.min(area.height);
        let square = Rect::new(
            area.x + (area.width - side) / 2.0,
            area.y + (area.height - side) / 2.0,
            side,
            side,
        );
        frame.draw_named_icon(icons, &art, square, theme.color(color));
    })
    .name(name);
    Element::from(drawn)
        .width(Extent::Px(ICON_PX))
        .height(Extent::Px(ICON_PX))
        .tooltip(name)
}

/// An empty slot `width` wide, holding a column's place on a row with
/// nothing in it.
fn blank<'a, S: PanelView>(width: Extent) -> Element<'a, S> {
    Element::from(text("")).width(width)
}

/// The name `kind`'s artwork has in `icons`, where it was loaded.
fn art_name(icons: &IconSet, kind: &IconKind) -> Option<String> {
    icons::name_for(kind).filter(|name| icons.get(name).is_some())
}

/// Put the artwork of every icon `rows` show in `icons`.
fn load_row_icons(icons: &mut IconSet, rows: &[ListRow]) {
    for row in rows {
        let mark = row.mark.as_ref().and_then(|mark| mark.icon.as_ref());
        for kind in row.icon.iter().map(|icon| &icon.kind).chain(mark) {
            load_icon(icons, kind);
        }
    }
}

/// Put the bundled artwork of `kind` in `icons` under its name, the first
/// time it is asked for: a symbol tinted as words are, the rest in their
/// own colors.
fn load_icon(icons: &mut IconSet, kind: &IconKind) {
    let Some(name) = icons::name_for(kind) else {
        return;
    };
    if icons.get(&name).is_some() {
        return;
    }
    let Some(svg) = icons::named_svg(&name).and_then(|bytes| Svg::parse(&bytes).ok()) else {
        return;
    };
    match kind {
        IconKind::Symbol { .. } => icons.insert(name, svg),
        IconKind::Named { name: named } if named.starts_with(SYMBOL_SET) => icons.insert(name, svg),
        IconKind::Named { .. }
        | IconKind::Dir { .. }
        | IconKind::Editor { .. }
        | IconKind::File { .. }
        | IconKind::Git { .. }
        | IconKind::Gutter { .. }
        | IconKind::Program { .. }
        // Drawn from the font, so never named, and never reaches here.
        | IconKind::Spinner
        | IconKind::Terminal => icons.insert_colored(name, svg),
    }
}

/// How many cells `words` take, near enough for a column of them.
fn cells_of(words: &str) -> f32 {
    words.chars().count() as f32
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use misty::testing::{TestWindow, access_tree};

    use crate::editors::view_tests::drawn;

    use super::*;

    /// The size of the window a view is drawn over to find its controls:
    /// a sidebar's shape, narrow and tall.
    const WINDOW_PX: (u32, u32) = (320, 600);

    /// `view` drawn over a window of a sidebar's shape.
    pub(super) fn drawn_view<S: PanelView>(view: S) -> MistyFrame {
        let mut page = PanelPage::new(view);
        drawn(&mut page, &TestWindow::new(WINDOW_PX.0, WINDOW_PX.1))
    }

    /// The controls drawn with nothing to call them by, and those the
    /// pointer does not light, when `view` fills a window: what AGENTS.md
    /// asks each page's test to find none of.
    pub(super) fn control_faults_in<S: PanelView>(view: S) -> (Vec<String>, Vec<String>) {
        let mut page = PanelPage::new(view);
        let mut window = TestWindow::new(WINDOW_PX.0, WINDOW_PX.1);
        let unnamed = misty::testing::unnamed_controls(&drawn(&mut page, &window));
        let unlit = crate::editors::view_tests::unlit(&mut page, &mut window);
        (unnamed, unlit)
    }

    /// The left edge of every control `frame` calls `name`, top to bottom.
    pub(super) fn lefts_of(frame: &MistyFrame, name: &str) -> Vec<f64> {
        let tree = access_tree(frame, "view");
        let mut found: Vec<(f64, f64)> = tree
            .nodes
            .iter()
            .filter(|(_, node)| node.label() == Some(name))
            .filter_map(|(_, node)| node.bounds())
            .map(|bounds| (bounds.y0, bounds.x0))
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.into_iter().map(|(_, left)| left).collect()
    }

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
