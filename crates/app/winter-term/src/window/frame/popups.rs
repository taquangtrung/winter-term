//! The popups composed over the panes each frame: the picker, and the
//! which-key hint for a pending prefix.

use std::ops::Range;

use crate::render::{PanelView, PickerItem, PickerView, Theme, ThemeRgb};
use misty::picker::{MenuView, PreviewKind, SessionView, Status};
use misty::{Rgba, Span, StyledText, TextColor};
use winter_model::paint::Style;

use crate::window::frame::frame_grid::{span_rgb, span_style};
use crate::window::input::sequence::WHICH_KEY_DELAY;
use crate::window::picker::PanelState;

// ========================================================================
// Constants
// ========================================================================

/// What a picker says in place of rows while they are still coming, and
/// once none match.
const STILL_WORKING: &str = "Working…";
const NOTHING_MATCHES: &str = "Nothing matches";

/// What the action menu is headed by, before the row it acts on.
const MENU_TITLE: &str = "Actions";

/// What joins the key hints on their line.
const HINT_SEPARATOR: &str = "  ·  ";

// ========================================================================
// Data Structures
// ========================================================================

/// How the picker is drawn, past what it holds.
pub(in crate::window) struct PickerLook<'a> {
    pub(in crate::window) caret_visible: bool,
    pub(in crate::window) match_underline: bool,
    /// The theme the side panel's text is coloured by.
    pub(in crate::window) theme: &'a Theme,
}

// ========================================================================
// Functions
// ========================================================================

/// The renderer's view of a picker session: its rows on screen, its query,
/// its title with its counts, its key hints, and its side panel. While the
/// action menu is open, the list shows the selected row's actions.
pub(in crate::window) fn session_view(
    view: &SessionView,
    panel: &PanelState,
    look: &PickerLook,
) -> PickerView {
    let items = view
        .rows
        .iter()
        .map(|shown| PickerItem {
            action: String::new(),
            buttons: shown
                .buttons
                .iter()
                .filter_map(|action| action.button.as_deref())
                .filter_map(crate::icons::named_svg)
                .collect(),
            description: shown.row.description.clone(),
            description_colors: Vec::new(),
            description_matches: shown.lit.description.clone(),
            detail: shown.row.detail.clone(),
            detail_matches: shown.lit.detail.clone(),
            hint: shown.row.hint.clone(),
            icon: shown.row.icon.as_deref().and_then(crate::icons::named_svg),
            label: shown.row.label.clone(),
            match_positions: shown.lit.label.clone(),
        })
        .collect();
    let empty_message = match (&view.error, view.status) {
        (Some(error), _) => error.clone(),
        (None, Status::Working) => STILL_WORKING.to_string(),
        (None, Status::Capped | Status::Done) => NOTHING_MATCHES.to_string(),
        // A status misty adds later reads as finished.
        (None, _) => NOTHING_MATCHES.to_string(),
    };
    let shown_panel = view.panel && view.preview_kind == PreviewKind::Panel;
    PickerView {
        // Drawn at the query's end until misty's quick pick draws the
        // picker, and its field the caret.
        caret: view.query.chars().count(),
        caret_visible: look.caret_visible,
        empty_message,
        hints: view
            .hints
            .iter()
            .filter_map(|act| crate::window::picker::keys::hint(*act))
            .collect::<Vec<_>>()
            .join(HINT_SEPARATOR),
        items,
        match_underline: look.match_underline,
        panel: shown_panel.then(|| panel_view(panel, look.theme)),
        placeholder: view.placeholder.clone(),
        query: view.query.clone(),
        query_selected: false,
        selected: view.selected.unwrap_or(usize::MAX),
        title: view.heading(),
        title_count: view.count(&misty::Strings::english()),
        menu: view
            .menu
            .as_ref()
            .map(|menu| Box::new(menu_view(menu, view, look))),
        ..PickerView::default()
    }
}

/// The action menu over a picker's selected row, as the list at the cursor
/// is drawn: the actions the filter keeps, headed by the row's name.
fn menu_view(menu: &MenuView, view: &SessionView, look: &PickerLook) -> PickerView {
    let row = view
        .selected
        .and_then(|at| view.rows.get(at))
        .map(|shown| shown.row.label.clone())
        .unwrap_or_default();
    PickerView {
        caret: menu.filter.chars().count(),
        caret_visible: look.caret_visible,
        empty_message: NOTHING_MATCHES.to_string(),
        items: menu
            .actions
            .iter()
            .map(|action| PickerItem {
                action: action.id.0.to_string(),
                label: action.title.clone(),
                ..PickerItem::default()
            })
            .collect(),
        query: menu.filter.clone(),
        selected: menu.selected,
        title: format!("{MENU_TITLE}: {row}"),
        ..PickerView::default()
    }
}

/// The side panel as the renderer draws it: the text read, each run in the
/// colour its language gives it, the query's marks on each line bold.
fn panel_view(panel: &PanelState, theme: &Theme) -> PanelView {
    let Some(text) = &panel.shown else {
        return PanelView::default();
    };
    PanelView {
        focus: text.focus,
        lines: text
            .lines
            .iter()
            .enumerate()
            .map(|(at, line)| {
                let styles = text.styles.get(at).map(Vec::as_slice);
                let marks = text.marks.get(at).map_or(&[][..], Vec::as_slice);
                coloured_runs(line, styles, marks, theme)
            })
            .collect(),
    }
}

/// `line` split into runs of one style each, coloured by `theme`, and apart
/// again where `marks`, the char ranges the query matched, begin and end,
/// those drawn bold; in the foreground where its chars have no styles.
fn coloured_runs(
    line: &str,
    styles: Option<&[Style]>,
    marks: &[Range<usize>],
    theme: &Theme,
) -> StyledText {
    let styles = styles.filter(|styles| !styles.is_empty());
    let mut runs = StyledText::new();
    let mut run = String::new();
    let mut last: Option<(Option<Style>, bool)> = None;
    for (at, c) in line.chars().enumerate() {
        let style = styles.map(|styles| styles.get(at).copied().unwrap_or(Style::Normal));
        let look = (style, marks.iter().any(|mark| mark.contains(&at)));
        if let Some(previous) = last.filter(|&previous| previous != look) {
            runs.push(marked_span(std::mem::take(&mut run), previous, theme));
        }
        last = Some(look);
        run.push(c);
    }
    if let Some(look) = last {
        runs.push(marked_span(run, look, theme));
    }
    runs
}

/// A run of a panel line in `style`'s colour, or the foreground for none,
/// bold where the query `marked` it.
fn marked_span(text: String, (style, marked): (Option<Style>, bool), theme: &Theme) -> Span {
    let color = style.map_or(theme.foreground, |style| span_rgb(style, theme));
    let span = Span::new(text, opaque(color));
    match marked {
        true => span.bold(),
        false => span,
    }
}

/// `line` split into runs of one style each, drawn as `theme` draws them:
/// a line of an info card, its code in colour and its docs' emphasis in
/// weight and slant. Chars past `styles` are left to the card's plain look.
pub(in crate::window) fn card_runs(line: &str, styles: &[Style], theme: &Theme) -> StyledText {
    let mut runs = StyledText::new();
    let mut run = String::new();
    let mut last: Option<Style> = None;
    for (c, &style) in line.chars().zip(styles) {
        if let Some(previous) = last.filter(|&previous| previous != style) {
            runs.push(card_span(std::mem::take(&mut run), previous, theme));
        }
        last = Some(style);
        run.push(c);
    }
    if let Some(style) = last {
        runs.push(card_span(run, style, theme));
    }
    runs
}

/// A run of an info card line in `style`'s colour, weight, and slant.
fn card_span(text: String, style: Style, theme: &Theme) -> Span {
    let look = span_style(style, theme);
    let span = Span::new(text, opaque(span_rgb(style, theme)));
    let span = match look.bold {
        true => span.bold(),
        false => span,
    };
    match look.italic {
        true => span.italic(),
        false => span,
    }
}

/// Misty's text color for Winter's `color`, drawn opaque.
fn opaque(color: ThemeRgb) -> TextColor {
    TextColor::Rgba(Rgba::new(color.r, color.g, color.b, u8::MAX))
}

/// The which-key popup, once a pending prefix has been held long enough to be
/// worth explaining. `None` until then, or when the prefix has no hint.
pub(in crate::window) fn which_key_view(
    pending: &winter_model::input::PendingPrefix,
    since: Option<std::time::Instant>,
) -> Option<crate::render::WhichKeyView> {
    since.filter(|s| s.elapsed() >= WHICH_KEY_DELAY)?;
    pending
        .hint()
        .map(|(title, items)| crate::render::WhichKeyView {
            anchor_rect: None,
            items: items
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            title: key_hint_title(title),
        })
}

/// The key-hint popup's title for a pending `prefix`.
pub(in crate::window) fn key_hint_title(prefix: &str) -> String {
    format!("Key hints: ({prefix}) was pressed, wait for the next key stroke…")
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn test_a_pending_prefix_is_explained_only_once_it_has_sat_unresolved() {
        let pending = winter_model::input::PendingPrefix::G;
        assert!(which_key_view(&pending, Some(Instant::now())).is_none());
        assert!(which_key_view(&pending, None).is_none(), "nothing pending");
        let view = which_key_view(&pending, Some(Instant::now() - WHICH_KEY_DELAY))
            .expect("shown once the delay is up");
        assert!(view.title.contains("(g)"), "title: {}", view.title);
        assert!(!view.items.is_empty());
    }

    #[test]
    fn test_a_panel_line_is_split_into_runs_of_one_style_each() {
        let theme = Theme::dark();
        let styles = [
            Style::Normal,
            Style::Normal,
            Style::Dim,
            Style::Dim,
            Style::Normal,
        ];
        let runs = coloured_runs("ab cd", Some(&styles), &[], &theme);
        let texts: Vec<&str> = runs.spans().iter().map(|run| run.text.as_str()).collect();
        assert_eq!(texts, ["ab", " c", "d"]);
        let plain = coloured_runs("ab", None, &[], &theme);
        assert_eq!(plain.spans().len(), 1, "an uncoloured line is one run");
        assert_eq!(plain.spans()[0].color, opaque(theme.foreground));
    }
}
