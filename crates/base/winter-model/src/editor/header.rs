//! A tab's header: the line across the top of a tab, under its strip, for
//! the tabs that need one. A page builds it of segments, each drawn where
//! its kind goes: the window draws it over an editor's grid, and a page
//! misty draws puts it in its page's header through [`header_view`].

use std::rc::Rc;

use misty::layout::Align;
use misty::view::{Corners, Extent, Space, Text, column, menu, row, text};
use misty::widgets::{ButtonVariant, MenuEntry, MenuItem};
use misty::{Element, ThemeToken};

use crate::paint::ColumnName;
use crate::view::ToolbarTone;

// ========================================================================
// Constants
// ========================================================================

/// How tall a header is, in rows of cells: a little taller than a row, so
/// it reads as the pane's furniture rather than a line of it.
pub const HEADER_ROWS: f32 = 1.2;

/// What stands between two parts of a path.
pub const PATH_SEPARATOR: &str = "\u{203a}";

/// What stands between the parts of a path in a tooltip.
pub const PATH_TRAIL: &str = " \u{203a} ";

/// The cell columns between two parts of a header.
const PART_GAP_COLS: f32 = 1.0;

/// The cell columns at each side of a toggle's text, inside its fill.
const TOGGLE_PADDING_COLS: f32 = 0.5;

// ========================================================================
// Data Structures
// ========================================================================

/// What a tab's header shows: its segments, each drawn where its kind goes,
/// several of a side in the order given. Empty for a tab with no header.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Header {
    /// What it is made of.
    pub segments: Vec<HeaderSegment>,
}

/// One segment of a [`Header`].
#[derive(Clone, Debug, PartialEq)]
pub enum HeaderSegment {
    /// The names of the tab's columns, each over the content columns it
    /// names, kept in place as the rows scroll.
    Columns(Vec<ColumnName>),
    /// Where the tab is, outermost first (a directory's path, one part to a
    /// folder, or `System > Port Monitor`), each part a press away and the
    /// last in the tab's icon: at the left.
    Path(Vec<PathPart>),
    /// Short facts about what the tab shows, such as a count, a sort, or a
    /// filter: at the right.
    Summary(Vec<HeaderPart>),
}

/// One part of a header's path: a folder, or a group of pages.
#[derive(Clone, Debug, PartialEq)]
pub struct PathPart {
    /// What it is called.
    pub name: String,
    /// The places a press lists to go to instead, such as the other pages of
    /// its group; empty for a part a press goes to, as a folder is. Only a
    /// page's header ([`header_view`]) lists them.
    pub places: Vec<HeaderPlace>,
}

/// A place a [`PathPart`] lists.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderPlace {
    /// What choosing it is told back as, as a [`HeaderPart`]'s press is.
    pub action: u16,
    /// Whether it is where the tab is, marked in the list.
    pub current: bool,
    /// What it is called.
    pub name: String,
}

/// One part of a header's summary.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderPart {
    /// What a press on it is told back as, through
    /// [`Page::on_header_part`](crate::editor::Page::on_header_part), below
    /// [`HeaderPart::MAX_ACTION`]; `None` for a part with nothing to press.
    pub action: Option<u16>,
    /// For one of a set of choices of how the tab lays out what it shows
    /// (`Flat` or `Tree`), whether it is the one chosen, drawn lit; `None`
    /// for a part that is no choice.
    pub chosen: Option<bool>,
    /// What it says.
    pub text: String,
    /// How it is drawn.
    pub tone: ToolbarTone,
    /// What it means, and the key that does what a press does.
    pub tooltip: Option<String>,
}

// ========================================================================
// Header
// ========================================================================

impl Header {
    /// A header of `segments`.
    pub fn new(segments: Vec<HeaderSegment>) -> Self {
        Self { segments }
    }

    /// Whether there is nothing to show, so the tab gets no header.
    pub fn is_empty(&self) -> bool {
        self.segments.iter().all(HeaderSegment::is_empty)
    }

    /// The parts of its path, empty for none.
    pub fn path(&self) -> &[PathPart] {
        self.segments
            .iter()
            .find_map(|segment| match segment {
                HeaderSegment::Path(parts) => Some(parts.as_slice()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// The names of its columns, empty for none.
    pub fn columns(&self) -> &[ColumnName] {
        self.segments
            .iter()
            .find_map(|segment| match segment {
                HeaderSegment::Columns(names) => Some(names.as_slice()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// The parts of its summaries, in order.
    pub fn summary(&self) -> impl Iterator<Item = &HeaderPart> {
        self.segments
            .iter()
            .filter_map(|segment| match segment {
                HeaderSegment::Summary(parts) => Some(parts),
                _ => None,
            })
            .flatten()
    }

    /// The part of its summary a press tells back as `action`.
    pub fn part(&self, action: u16) -> Option<&HeaderPart> {
        self.summary().find(|part| part.action == Some(action))
    }
}

// ========================================================================
// HeaderSegment
// ========================================================================

impl HeaderSegment {
    /// Whether it has nothing to show.
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Columns(names) => names.is_empty(),
            Self::Path(parts) => parts.is_empty(),
            Self::Summary(parts) => parts.is_empty(),
        }
    }
}

// ========================================================================
// HeaderPart
// ========================================================================

impl HeaderPart {
    /// The highest action a part may be told back as, so the window can
    /// tell a press on one from a press on a path or a toolbar.
    pub const MAX_ACTION: u16 = 0x3fff;

    /// A part saying `text` in `tone`, with nothing to press.
    pub fn label(text: impl Into<String>, tone: ToolbarTone) -> Self {
        Self {
            action: None,
            chosen: None,
            text: text.into(),
            tone,
            tooltip: None,
        }
    }

    /// A part saying `text` in `tone` that a press tells back as `action`,
    /// its tooltip `tooltip`.
    pub fn button(
        text: impl Into<String>,
        tone: ToolbarTone,
        action: u16,
        tooltip: impl Into<String>,
    ) -> Self {
        Self {
            action: Some(action),
            chosen: None,
            text: text.into(),
            tone,
            tooltip: Some(tooltip.into()),
        }
    }

    /// One of a set of choices saying `text`, lit when `chosen`, a press on
    /// it told back as `action`, its tooltip `tooltip`. The chosen one needs
    /// no action, since pressing it again changes nothing.
    pub fn choice(
        text: impl Into<String>,
        chosen: bool,
        action: Option<u16>,
        tooltip: impl Into<String>,
    ) -> Self {
        let tone = match chosen {
            true => ToolbarTone::Accent,
            false => ToolbarTone::Dim,
        };
        Self {
            action,
            chosen: Some(chosen),
            text: text.into(),
            tone,
            tooltip: Some(tooltip.into()),
        }
    }
}

// ========================================================================
// PathPart
// ========================================================================

impl PathPart {
    /// A part called `name` that a press goes to.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            places: Vec::new(),
        }
    }

    /// A part called `name` that a press lists `places` from.
    pub fn listing(name: impl Into<String>, places: Vec<HeaderPlace>) -> Self {
        Self {
            name: name.into(),
            places,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `header` as misty draws it in a page's header
/// ([`misty::view::Page::header`]), [`HEADER_ROWS`] tall: its path at the
/// left, each part a button running `on_path` with its index, or a flat
/// menu of its places running `on_part` with the one chosen, its folders
/// muted; and its summary at the right, each part with an action a button
/// running `on_part` with it, a choice lit when chosen. The names of
/// columns stand over an editor's grid, which a page has none of, so they
/// are left out.
pub fn header_view<'a, A: 'static>(
    header: &Header,
    on_path: impl Fn(&mut A, usize) + 'static,
    on_part: impl Fn(&mut A, u16) + 'static,
) -> Element<'a, A> {
    let on_path = Rc::new(on_path);
    let on_part = Rc::new(on_part);
    let path = header.path();
    let mut parts: Vec<Element<'a, A>> = Vec::new();
    for (index, part) in path.iter().enumerate() {
        if index > 0 {
            parts.push(text(PATH_SEPARATOR).muted().into());
        }
        let trail = path[..=index]
            .iter()
            .map(|part| part.name.as_str())
            .collect::<Vec<_>>()
            .join(PATH_TRAIL);
        if !part.places.is_empty() {
            let entries = part
                .places
                .iter()
                .map(|place| {
                    MenuEntry::from(
                        MenuItem::new(place.action, place.name.clone()).with_radio(place.current),
                    )
                })
                .collect();
            let on_part = Rc::clone(&on_part);
            let listed = menu(part.name.clone(), entries)
                .variant(ButtonVariant::Ghost)
                .on_choose(move |page: &mut A, action| on_part(page, action));
            parts.push(Element::from(listed).tooltip(trail));
            continue;
        }
        let shown = match index + 1 == path.len() {
            true => text(part.name.clone()),
            false => text(part.name.clone()).muted(),
        };
        let on_path = Rc::clone(&on_path);
        parts.push(
            Element::from(shown)
                .tooltip(trail)
                .on_press(move |page: &mut A, _| on_path(page, index)),
        );
    }
    parts.push(column(Vec::new()).grow(1.0));
    for part in header.summary() {
        let shown = Element::from(tinted(&part.text, part.tone));
        let shown = match part.chosen {
            Some(true) => shown
                .padding(TOGGLE_PADDING_COLS, 0.0)
                .background(ThemeToken::Selection)
                .corners(Corners::Control),
            Some(false) => shown.padding(TOGGLE_PADDING_COLS, 0.0),
            None => shown,
        };
        let shown = match part.action {
            Some(action) => {
                let on_part = Rc::clone(&on_part);
                shown.on_press(move |page: &mut A, _| on_part(page, action))
            }
            None => shown,
        };
        let shown = match &part.tooltip {
            Some(tooltip) => shown.tooltip(tooltip.clone()),
            None => shown,
        };
        parts.push(shown);
    }
    row(parts)
        .gap(Space::Cells(PART_GAP_COLS))
        .align(Align::Center)
        .height(Extent::Cells(HEADER_ROWS))
}

/// `words` in the color `tone` gives them.
fn tinted(words: &str, tone: ToolbarTone) -> Text<'static> {
    let shown = text(words.to_string());
    match tone {
        ToolbarTone::Accent => shown.color(ThemeToken::Accent),
        ToolbarTone::Danger => shown.color(ThemeToken::Danger),
        ToolbarTone::Dim => shown.muted(),
        ToolbarTone::Normal => shown,
        ToolbarTone::Strong => shown.emphasized(),
        ToolbarTone::Warning => shown.color(ThemeToken::Warning),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use misty::accesskit::Role;

    use super::*;

    #[test]
    fn test_a_header_of_empty_segments_is_empty() {
        let header = Header::new(vec![
            HeaderSegment::Path(Vec::new()),
            HeaderSegment::Summary(Vec::new()),
        ]);
        assert!(header.is_empty(), "no line for nothing to show");
        assert!(Header::default().is_empty());
    }

    /// A page showing a header, and what its presses ran.
    struct Shown {
        header: Header,
        paths: Vec<usize>,
        parts: Vec<u16>,
        theme: misty::Theme,
    }

    impl misty::App for Shown {
        fn view(&self, _window: misty::WindowId) -> Element<'_, Self> {
            misty::view::untitled_page()
                .header(header_view(
                    &self.header,
                    |page: &mut Self, at| page.paths.push(at),
                    |page: &mut Self, action| page.parts.push(action),
                ))
                .filled(column(Vec::new()))
                .into()
        }

        fn theme(&self) -> &misty::Theme {
            &self.theme
        }
    }

    #[test]
    fn test_a_page_s_header_shows_its_path_and_summary_and_runs_their_presses() {
        let header = Header::new(vec![
            HeaderSegment::Path(vec![
                PathPart::listing(
                    "System",
                    vec![
                        HeaderPlace {
                            action: 20,
                            current: false,
                            name: "Process Monitor".to_string(),
                        },
                        HeaderPlace {
                            action: 21,
                            current: true,
                            name: "Port Monitor".to_string(),
                        },
                    ],
                ),
                PathPart::new("Port Monitor"),
            ]),
            HeaderSegment::Summary(vec![
                HeaderPart::label("12 processes", ToolbarTone::Normal),
                HeaderPart::choice("Flat", true, None, "Listed flat"),
                HeaderPart::choice("Tree", false, Some(7), "List as a tree (=)"),
            ]),
        ]);
        let shown = Shown {
            header,
            paths: Vec::new(),
            parts: Vec::new(),
            theme: misty::Theme::dark(),
        };
        let mut window = misty::testing::TreeWindow::new(shown, "header", 600, 200);
        let drawn = window.drawn_text();
        for words in ["System", "Port Monitor", "12 processes", "Flat", "Tree"] {
            assert!(
                drawn.iter().any(|line| line == words),
                "{words:?} in {drawn:?}"
            );
        }
        window.click(Role::Button, "Port Monitor");
        window.click(Role::Button, "Tree");
        assert_eq!(window.app().paths, [1]);
        assert_eq!(window.app().parts, [7]);
        // "System" lists the other pages; choosing one tells back its action.
        window.click(Role::Button, "System");
        window.click(Role::MenuItemRadio, "Process Monitor");
        assert_eq!(window.app().parts, [7, 20]);
        let unnamed = misty::testing::unnamed_controls(window.frame());
        assert!(unnamed.is_empty(), "{unnamed:?}");
    }

    #[test]
    fn test_a_press_finds_the_part_told_back_as_its_action() {
        let header = Header::new(vec![
            HeaderSegment::Path(vec![PathPart::new("home")]),
            HeaderSegment::Summary(vec![
                HeaderPart::label("12 processes", ToolbarTone::Normal),
                HeaderPart::button("tree", ToolbarTone::Dim, 7, "Listed as a tree"),
            ]),
        ]);
        assert!(!header.is_empty());
        assert_eq!(header.path(), [PathPart::new("home")]);
        assert_eq!(header.part(7).map(|part| part.text.as_str()), Some("tree"));
        assert_eq!(header.part(0), None, "a label is told back as nothing");
    }
}
