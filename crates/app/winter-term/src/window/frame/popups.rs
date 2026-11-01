//! The popups composed over the panes each frame: the picker, and the
//! which-key hint for a pending prefix.

use std::collections::HashMap;
use std::sync::Arc;

use winter_model::paint::IconKind;
use winter_model::paint::Style;
use winter_picker::{MenuView, PreviewKind, SessionView, Status};
use winter_render::{PanelLine, PanelSpan, PanelView, PickerItem, PickerView, Theme};

use crate::window::frame::frame_grid::span_rgb;
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
///
/// `icons` holds what each icon kind resolved to on earlier frames, so a long
/// list is not walked through the icon tables again every frame.
pub(in crate::window) fn session_view(
    view: &SessionView,
    panel: &PanelState,
    look: &PickerLook,
    icons: &mut HashMap<IconKind, Option<Arc<[u8]>>>,
) -> PickerView {
    let items = view
        .rows
        .iter()
        .map(|shown| PickerItem {
            action: String::new(),
            buttons: shown
                .buttons
                .iter()
                .filter_map(|action| action.button.as_ref())
                .filter_map(|kind| palette_icon(icons, kind))
                .collect(),
            description: shown.row.description.clone(),
            description_matches: shown.lit.description.clone(),
            detail: shown.row.detail.clone(),
            detail_matches: shown.lit.detail.clone(),
            hint: shown.row.hint.clone(),
            icon: shown
                .row
                .icon
                .as_ref()
                .and_then(|kind| palette_icon(icons, kind)),
            label: shown.row.label.clone(),
            match_positions: shown.lit.label.clone(),
        })
        .collect();
    let empty_message = match (&view.error, view.status) {
        (Some(error), _) => error.clone(),
        (None, Status::Working) => STILL_WORKING.to_string(),
        (None, Status::Capped | Status::Done) => NOTHING_MATCHES.to_string(),
    };
    let shown_panel = view.panel && view.preview_kind == PreviewKind::Panel;
    PickerView {
        caret: view.caret,
        caret_visible: look.caret_visible,
        empty_message,
        hints: view
            .hints
            .iter()
            .map(|hint| format!("{} {}", hint.key, hint.label))
            .collect::<Vec<_>>()
            .join(HINT_SEPARATOR),
        items,
        match_underline: look.match_underline,
        panel: shown_panel.then(|| panel_view(panel, look.theme)),
        query: view.query.clone(),
        selected: view.selected.unwrap_or(usize::MAX),
        title: view.heading(),
        title_count: view.count(),
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
/// colour its language gives it, the query's marks on each line.
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
            .map(|(at, line)| PanelLine {
                marks: text.marks.get(at).cloned().unwrap_or_default(),
                spans: coloured_runs(line, text.styles.get(at).map(Vec::as_slice), theme),
            })
            .collect(),
    }
}

/// `line` split into runs of one style each, coloured by `theme`; one run in
/// the foreground where its chars have no styles.
fn coloured_runs(line: &str, styles: Option<&[Style]>, theme: &Theme) -> Vec<PanelSpan> {
    let Some(styles) = styles.filter(|styles| !styles.is_empty()) else {
        return vec![PanelSpan {
            color: theme.foreground,
            text: line.to_string(),
        }];
    };
    let mut runs: Vec<PanelSpan> = Vec::new();
    let mut last: Option<Style> = None;
    for (at, c) in line.chars().enumerate() {
        let style = styles.get(at).copied().unwrap_or(Style::Normal);
        match runs.last_mut() {
            Some(run) if last == Some(style) => run.text.push(c),
            Some(_) | None => runs.push(PanelSpan {
                color: span_rgb(style, theme),
                text: c.to_string(),
            }),
        }
        last = Some(style);
    }
    runs
}

/// The SVG for `kind`, from `icons` when an earlier frame resolved it.
pub(in crate::window) fn palette_icon(
    icons: &mut HashMap<IconKind, Option<Arc<[u8]>>>,
    kind: &IconKind,
) -> Option<Arc<[u8]>> {
    if let Some(hit) = icons.get(kind) {
        return hit.clone();
    }
    let svg = crate::icons::shared_svg(kind);
    icons.insert(kind.clone(), svg.clone());
    svg
}

/// The which-key popup, once a pending prefix has been held long enough to be
/// worth explaining. `None` until then, or when the prefix has no hint.
pub(in crate::window) fn which_key_view(
    pending: &winter_model::input::PendingPrefix,
    since: Option<std::time::Instant>,
) -> Option<winter_render::WhichKeyView> {
    since.filter(|s| s.elapsed() >= WHICH_KEY_DELAY)?;
    pending
        .hint()
        .map(|(title, items)| winter_render::WhichKeyView {
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
        let runs = coloured_runs("ab cd", Some(&styles), &theme);
        let texts: Vec<&str> = runs.iter().map(|run| run.text.as_str()).collect();
        assert_eq!(texts, ["ab", " c", "d"]);
        let plain = coloured_runs("ab", None, &theme);
        assert_eq!(plain.len(), 1, "an uncoloured line is one run");
        assert_eq!(plain[0].color, theme.foreground);
    }
}
