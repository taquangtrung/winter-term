//! Tests for composing a frame.

use super::*;
use crate::window::editor;
use crate::window::input::overlay::Overlay;
use winter_render::Theme;

#[test]
fn test_a_lit_search_marks_an_editors_matches_and_the_one_its_cursor_is_on() {
    let dir = std::env::temp_dir().join(format!("winter-lit-tool-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut app = App::new();
    let tab = app.show_editor(
        crate::window::editor::EditorKind::Dir,
        Box::new(winter_dir::DirEditor::new(dir.clone())),
    );
    let mut grid = Grid::new(10, 3);
    for (row, line) in ["a mod", "mod mod", "x"].iter().enumerate() {
        grid.move_to(row, 0);
        line.chars().for_each(|ch| grid.print(ch));
    }
    let slot = app.open_tabs.page_mut(&tab).expect("the editor");
    slot.view.painted = Some(grid);
    slot.view.cursor_line = Some(1);
    app.term.search.query = Some("mod".to_string());

    let (matches, current) = app.search_highlight_cells(tab, tab);
    let row = |row, cols: std::ops::Range<usize>| cols.map(move |col| (row, col));
    let every: Vec<_> = row(0, 2..5)
        .chain(row(1, 0..3))
        .chain(row(1, 4..7))
        .collect();
    assert_eq!(matches, every);
    assert_eq!(
        current,
        row(1, 0..3).collect::<Vec<_>>(),
        "first on the cursor's row"
    );
    let (elsewhere, unfocused) = app.search_highlight_cells(tab, TabId(u64::MAX));
    assert!(
        elsewhere.is_empty() && unfocused.is_empty(),
        "a search lights the focused tab alone, not the window"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_the_current_match_is_under_the_caret_or_else_nearest_it() {
    let spans = [(0, 2, 5), (1, 0, 3), (1, 8, 11)];
    assert_eq!(
        current_span(&spans, 1, Some(9)),
        Some(&(1, 8, 11)),
        "under it"
    );
    assert_eq!(
        current_span(&spans, 1, Some(6)),
        Some(&(1, 8, 11)),
        "nearest"
    );
    assert_eq!(current_span(&spans, 1, None), Some(&(1, 0, 3)), "the first");
    assert_eq!(current_span(&spans, 2, Some(0)), None, "none on the row");
}

#[test]
fn test_an_editor_caret_without_the_keyboard_is_a_hollow_block() {
    let grid = Grid::new(10, 3);
    let paint = EditorPaint {
        caret_cell: None,
        completion: None,
        drop_list: None,
        cursor_line: Some(0),
        jump_target: None,
        icons: Vec::new(),
        info: None,
        outlines: Vec::new(),
        scroll: None,
        key_moved: false,
        tab: TabId(1),
        toolbar: None,
    };
    let mut config = crate::config::Config::default();
    config.cursor.insert = CursorShape::Bar;
    let rect = Rect::new(0.0, 0.0, 100.0, 60.0);
    let typing = Some(PaneCaret {
        col: 1,
        insert: true,
        row: 0,
    });
    let view = |focused: bool, window_focused: bool, caret: Option<PaneCaret>| {
        let view = editor_pane_view(
            &paint,
            &grid,
            Vec::new(),
            rect,
            rect,
            focused,
            window_focused,
            &config,
            true,
            None,
            caret,
            &[],
            &[],
            None,
            false,
            false,
        );
        (view.cursor_unfocused, view.cursor_shape)
    };

    assert_eq!(
        view(true, true, typing),
        (false, CursorShape::Bar),
        "has the keyboard"
    );
    assert_eq!(
        view(false, true, typing),
        (true, CursorShape::Block),
        "another pane has it"
    );
    assert_eq!(
        view(true, false, typing),
        (true, CursorShape::Block),
        "the window lost it"
    );
    assert!(
        !view(false, true, None).0,
        "a page with no caret draws none"
    );
}

#[test]
fn test_an_editor_caret_blinks_with_the_global_setting() {
    let grid = Grid::new(10, 3);
    let paint = EditorPaint {
        caret_cell: None,
        completion: None,
        drop_list: None,
        cursor_line: Some(0),
        jump_target: None,
        icons: Vec::new(),
        info: None,
        outlines: Vec::new(),
        scroll: None,
        key_moved: false,
        tab: TabId(1),
        toolbar: None,
    };
    let rect = Rect::new(0.0, 0.0, 100.0, 60.0);
    let caret = Some(PaneCaret::at(0, 1));
    let mut config = crate::config::Config::default();
    let shown = |config: &crate::config::Config, focused, phase, caret| {
        editor_pane_view(
            &paint,
            &grid,
            Vec::new(),
            rect,
            rect,
            focused,
            true,
            config,
            phase,
            None,
            caret,
            &[],
            &[],
            None,
            false,
            false,
        )
        .nav_cursor_visible
    };

    assert!(config.cursor.blink, "blinking is the default");
    assert!(shown(&config, true, true, caret), "on phase");
    assert!(!shown(&config, true, false, caret), "off phase");
    assert!(
        shown(&config, false, false, caret),
        "hollow outline holds steady"
    );
    assert!(
        !shown(&config, true, true, None),
        "no caret, nothing to show"
    );

    config.cursor.blink = false;
    assert!(
        shown(&config, true, false, caret),
        "steady when blink is off"
    );
}

#[test]
fn test_cursor_line_row_is_kept_for_unfocused_panes() {
    // The band follows the pane's mode and cursor only, no focus term, so a
    // pane you navigated stays banded after you switch to another pane.
    assert_eq!(cursor_line_row(Mode::Normal, Some((4, 2))), Some(4));
    assert_eq!(cursor_line_row(Mode::Visual, Some((0, 0))), Some(0));
    // Insert (or a pane that was never navigated) shows no band.
    assert_eq!(cursor_line_row(Mode::Insert, Some((4, 2))), None);
    assert_eq!(cursor_line_row(Mode::Normal, None), None);
}

#[test]
fn test_alt_screen_app_reports_shape_from_its_first_frame() {
    // Regression: vim/nvim opens straight into Normal mode and reports
    // Block immediately, before the user ever presses `i`. The shape must
    // come from that first report, not fall back to the host's Insert
    // config shape while waiting for a Bar to appear.
    assert_eq!(
        effective_cursor_shape(true, Some(CursorShape::Block), CursorShape::Bar),
        CursorShape::Block
    );
}

#[test]
fn test_alt_screen_app_with_no_report_yet_falls_back_to_config() {
    assert_eq!(
        effective_cursor_shape(true, None, CursorShape::Bar),
        CursorShape::Bar
    );
}

#[test]
fn test_shell_prompt_always_uses_configured_shape() {
    // Outside the alt screen, a shell's stray DECSCUSR (e.g. zsh
    // re-emitting Block on a resize) must never override the host's
    // configured per-mode shape.
    assert_eq!(
        effective_cursor_shape(false, Some(CursorShape::Block), CursorShape::Bar),
        CursorShape::Bar
    );
    assert_eq!(
        effective_cursor_shape(false, None, CursorShape::Bar),
        CursorShape::Bar
    );
}

#[test]
fn test_the_palette_caret_stays_lit_in_a_window_without_the_keyboard() {
    // Regression: the blink clock pauses when the window loses focus, and
    // a caret that paused on its dark phase stayed hidden until it came
    // back.
    assert!(is_caret_lit(true, false, false));
    assert!(
        !is_caret_lit(true, true, false),
        "blinking with the keyboard"
    );
    assert!(
        is_caret_lit(false, true, false),
        "steady when blinking is off"
    );
}

#[test]
fn test_an_inactive_pane_shows_a_hollow_steady_cursor() {
    let cursor = crate::config::CursorConfig::default();
    let inactive = PaneFocus {
        pane: false,
        window: true,
    };
    // Hollow, and shown whatever the blink phase: it marks where the
    // pane stands without claiming the keys.
    for blink_phase in [true, false] {
        assert_eq!(
            pane_cursor(inactive, false, &cursor, blink_phase),
            PaneCursor {
                unfocused: true,
                visible: true,
            }
        );
    }
    // Unless the user asked for none there.
    let hiding = crate::config::CursorConfig {
        hide_in_inactive: true,
        ..cursor
    };
    assert!(!pane_cursor(inactive, false, &hiding, true).visible);
    // The focused pane keeps its filled, blinking cursor.
    let focused = PaneFocus {
        pane: true,
        window: true,
    };
    assert_eq!(
        pane_cursor(focused, false, &cursor, false),
        PaneCursor {
            unfocused: false,
            visible: false,
        }
    );
}

#[test]
fn test_questions_are_asked_in_the_dialog_and_searches_in_the_find_popup() {
    let mut app = App::new();
    assert!(app.find_target().is_none());
    assert!(app.input_view().is_none());

    let prompt = editor::ActivePrompt {
        origin: None,
        buttons: None,
        query_buttons: None,
        field: winter_model::textedit::field::TextField::line("hello"),
        recall: winter_model::textedit::recall::Recall::default(),
        tab: TabId(0),
        request: winter_model::editor::PromptRequest {
            initial: String::new(),
            label: "find".to_string(),
            mode: winter_model::editor::PromptMode::Text,
            tag: "search",
            title: None,
        },
    };
    app.chrome.overlay = Some(Overlay::EditorPrompt(Box::new(prompt)));
    assert!(app.find_target().is_none(), "a question is not a search");
    let dialog = app.input_view().expect("an editor's question");
    assert_eq!(dialog.label, "find");
    assert_eq!(dialog.input.as_deref(), Some("hello"));
    assert_eq!(dialog.caret, 5);

    app.chrome.overlay = Some(Overlay::ThemeName(
        winter_model::textedit::field::TextField::line("dracula"),
    ));
    assert!(app.find_target().is_none());
    let dialog = app.input_view().expect("the theme name question");
    assert_eq!(dialog.label, "New theme name");
    assert_eq!(dialog.input.as_deref(), Some("dracula"));

    // An editor's search is typed in the find popup, not a dialog.
    app.chrome.overlay = Some(Overlay::EditorPrompt(Box::new(editor::ActivePrompt {
        origin: None,
        buttons: None,
        query_buttons: None,
        field: winter_model::textedit::field::TextField::line("needle"),
        recall: winter_model::textedit::recall::Recall::default(),
        tab: TabId(3),
        request: winter_model::editor::PromptRequest {
            initial: String::new(),
            label: "?".to_string(),
            mode: winter_model::editor::PromptMode::Search { reverse: true },
            tag: "search-back",
            title: None,
        },
    })));
    assert!(app.input_view().is_none(), "no dialog for a search");
    let target = app.find_target().expect("the editor's search");
    assert_eq!(
        (
            target.tab,
            target.label.as_str(),
            target.query.as_str(),
            target.caret
        ),
        (TabId(3), "?", "needle", 6)
    );

    // And so is the terminal's own `/` line.
    app.chrome.overlay = None;
    app.input.keys.prefix = PendingPrefix::SearchInput;
    app.term.search.reverse = false;
    app.term.search.query = Some("term".to_string());
    let target = app.find_target().expect("forward search");
    assert_eq!(target.label, "/");
    assert_eq!(target.query, "term");
    assert_eq!(target.caret, 4);

    app.term.search.reverse = true;
    assert_eq!(app.find_target().expect("reverse search").label, "?");
}

#[test]
fn test_token_inspect_popup_works_on_terminal_and_editors() {
    let (mut app, tab) = crate::window::test_support::app_pane_and_id();
    let rects = [(tab, Rect::new(0.0, 0.0, 800.0, 600.0))];
    let theme = Theme::default();

    // Initially inactive
    assert!(token_inspect_popup(
        &mut app.open_tabs,
        app.host.editor_cursor.as_ref(),
        app.host.token_inspect,
        &rects,
        (10.0, 20.0),
        tab,
        &theme,
    )
    .is_none());

    // Toggle inspection on terminal
    app.toggle_token_inspect(tab);
    assert_eq!(app.host.token_inspect, Some(tab));

    let view = token_inspect_popup(
        &mut app.open_tabs,
        app.host.editor_cursor.as_ref(),
        app.host.token_inspect,
        &rects,
        (10.0, 20.0),
        tab,
        &theme,
    )
    .expect("terminal inspect view");
    assert_eq!(view.language, "Terminal");
    assert_eq!(view.scopes, vec!["terminal".to_string()]);

    // Escape dismisses inspection
    app.dismiss_token_inspect();
    assert!(app.host.token_inspect.is_none());
    assert!(token_inspect_popup(
        &mut app.open_tabs,
        app.host.editor_cursor.as_ref(),
        app.host.token_inspect,
        &rects,
        (10.0, 20.0),
        tab,
        &theme,
    )
    .is_none());
}

/// An editor waiting on a prefix, hinting at its keys, or a menu.
struct HintingEditor {
    menu: bool,
}

impl winter_model::editor::Page for HintingEditor {
    fn title(&self) -> String {
        "Hinting".to_string()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> winter_model::paint::Frame {
        winter_model::paint::Frame::new(Vec::new())
    }

    fn on_key(&mut self, _key: &winter_model::input::Key) -> winter_model::editor::EditorOutcome {
        winter_model::editor::EditorOutcome::Ignored
    }

    fn hint(&self) -> Option<winter_model::editor::EditorHint> {
        Some(winter_model::editor::EditorHint {
            items: vec![("g".to_string(), "to the top".to_string())],
            menu: self.menu,
            title: "g".to_string(),
        })
    }
}

impl winter_model::editor::Editor for HintingEditor {}

/// Whether the key hints show for an editor hinting as `menu` says, at
/// once and once the delay has passed.
fn hint_shown(menu: bool) -> (bool, bool) {
    let mut app = App::new();
    let tab = app.show_editor(editor::EditorKind::Grep, Box::new(HintingEditor { menu }));
    app.sync_editor_hint();
    let at_once = app.which_key_popup(tab).is_some();
    app.input.keys.editor_hint_since = Some(Instant::now() - WHICH_KEY_DELAY);
    let later = app.which_key_popup(tab).is_some();
    (at_once, later)
}

#[test]
fn test_an_editor_s_prefix_hint_waits_the_delay_and_a_menu_does_not() {
    assert_eq!(hint_shown(false), (false, true), "a prefix's keys wait");
    assert_eq!(hint_shown(true), (true, true), "a menu shows at once");
}

#[test]
fn test_the_window_wakes_when_an_editor_s_prefix_hint_is_due() {
    let mut app = App::new();
    app.show_editor(
        editor::EditorKind::Grep,
        Box::new(HintingEditor { menu: false }),
    );
    app.sync_editor_hint();
    let since = app
        .input
        .keys
        .editor_hint_since
        .expect("the prefix is waited on");
    assert_eq!(app.hint_deadline(), Some(since + WHICH_KEY_DELAY));
    app.sync_editor_hint();
    assert_eq!(
        app.input.keys.editor_hint_since,
        Some(since),
        "a prefix still held keeps its start"
    );
}

#[test]
fn test_editor_pane_view_scrollbar_params() {
    let grid = Grid::new(10, 20);
    let mut paint = EditorPaint {
        caret_cell: None,
        completion: None,
        drop_list: None,
        cursor_line: None,
        jump_target: None,
        icons: Vec::new(),
        info: None,
        outlines: Vec::new(),
        scroll: None,
        key_moved: false,
        tab: TabId(1),
        toolbar: None,
    };
    let rect = Rect::new(0.0, 0.0, 100.0, 200.0);
    let config = crate::config::Config::default();

    // No scroll position -> 0 scrollback
    let view = editor_pane_view(
        &paint,
        &grid,
        Vec::new(),
        rect,
        rect,
        true,
        true,
        &config,
        true,
        None,
        None,
        &[],
        &[],
        None,
        false,
        false,
    );
    assert_eq!(view.scroll_offset, 0);
    assert_eq!(view.scrollback_len, 0);
    assert!(!view.scrollbar_active);
    assert!(!view.scrollbar_hover);
    assert_eq!(view.jump_target_row, None);

    // Jump target preview line is transferred to PaneView
    paint.jump_target = Some(7);
    let jump_view = editor_pane_view(
        &paint,
        &grid,
        Vec::new(),
        rect,
        rect,
        true,
        true,
        &config,
        true,
        None,
        None,
        &[],
        &[],
        None,
        false,
        false,
    );
    assert_eq!(jump_view.jump_target_row, Some(7));
    assert_eq!(jump_view.cursor_line_row, None);
    paint.jump_target = None;

    // Long file with scroll position
    paint.scroll = Some(winter_model::editor::ScrollPosition::new(10, 20, 100));
    let view = editor_pane_view(
        &paint,
        &grid,
        Vec::new(),
        rect,
        rect,
        true,
        true,
        &config,
        true,
        None,
        None,
        &[],
        &[],
        None,
        true,
        true,
    );
    assert_eq!(view.scrollback_len, 80);
    assert_eq!(view.scroll_offset, 70);
    assert!(view.scrollbar_active);
    assert!(view.scrollbar_hover);
}

#[test]
fn test_only_one_scrollbar_active_where_mouse_hovers() {
    let mut app = App::new();
    let main = app.layout().focused();
    app.open_tabs
        .insert_terminal(main, crate::window::test_support::idle_terminal());
    app.split_pane(winter_model::layout::Direction::Vertical);
    let second = app.layout().focused();
    assert_ne!(main, second);

    // Focus is on second pane; position cursor over main pane.
    let rects = app.pane_rects();
    let (_, r_main) = rects.iter().find(|(id, _)| *id == main).expect("main rect");
    let pr_main = App::layout_rect_to_pane(*r_main);
    app.input.pointer.cursor_pos = (pr_main.x + 5.0, pr_main.y + 5.0);

    let active = app.active_pane();
    let hovered = app.hovered_pane(&rects);
    assert_eq!(hovered, Some(main));

    // Active scrollbar resolves to main (where mouse hovers), NOT second (focused).
    let active_scrollbar_tab = app
        .input
        .pointer
        .scrollbar_drag
        .map(|d| d.tab)
        .or(app.input.pointer.scrollbar_hover)
        .or(hovered)
        .or(active)
        .or(Some(app.layout().focused()));
    assert_eq!(active_scrollbar_tab, Some(main));
    assert_ne!(active_scrollbar_tab, Some(second));

    // Move cursor over second pane.
    let (_, r_sec) = rects
        .iter()
        .find(|(id, _)| *id == second)
        .expect("sec rect");
    let pr_sec = App::layout_rect_to_pane(*r_sec);
    app.input.pointer.cursor_pos = (pr_sec.x + 5.0, pr_sec.y + 5.0);

    let hovered = app.hovered_pane(&rects);
    assert_eq!(hovered, Some(second));

    let active_scrollbar_tab = app
        .input
        .pointer
        .scrollbar_drag
        .map(|d| d.tab)
        .or(app.input.pointer.scrollbar_hover)
        .or(hovered)
        .or(active)
        .or(Some(app.layout().focused()));
    assert_eq!(active_scrollbar_tab, Some(second));
    assert_ne!(active_scrollbar_tab, Some(main));
}

#[test]
fn test_the_scrollbar_the_keyboard_scrolled_is_lit_wherever_the_pointer_rests() {
    let (editor, other) = (TabId(1), TabId(2));
    // The pointer rests on the other pane: without the keyboard's scrolling,
    // that pane's scrollbar is the active one.
    let lit = LitScrollbars::resolve(None, None, Some(other), Some(editor), editor);
    assert_eq!((lit.active, lit.hovered), (Some(other), None));
    // Scrolled by the keyboard, the focused editor's scrollbar is lit as
    // though hovered.
    let lit = LitScrollbars::resolve(None, Some(editor), Some(other), Some(editor), editor);
    assert_eq!((lit.active, lit.hovered), (Some(editor), Some(editor)));
    // A scrollbar under the pointer still wins.
    let lit = LitScrollbars::resolve(Some(other), Some(editor), Some(other), Some(editor), editor);
    assert_eq!((lit.active, lit.hovered), (Some(other), Some(other)));
    // Once the keyboard is elsewhere, the editor it scrolled is not lit.
    let lit = LitScrollbars::resolve(None, Some(editor), Some(other), Some(other), other);
    assert_eq!((lit.active, lit.hovered), (Some(other), None));
}
