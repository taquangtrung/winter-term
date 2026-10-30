//! Tests for terminal navigation: cursor motions, jumps and marks, yanks and text objects, find-char.

use crate::model::input::{FindChar, TextObject, TextObjectSpec, VisualKind};
use crate::model::textedit::modal::shared;
use crate::window::test_support::*;

use super::*;

/// How long a test waits for a child process to produce what it was asked
/// for. Generous, because every wait below breaks out as soon as the output
/// arrives: the ceiling only has to survive a machine under load.
const PTY_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long to sleep between polls, so waiting does not spin a core.
const POLL_STEP: std::time::Duration = std::time::Duration::from_millis(20);

/// A pane whose visible grid holds `lines`, one per row from the top, written
/// straight into the grid (no shell, no timing).
fn pane_with_lines(lines: &[&str]) -> crate::terminal::session::Terminal {
    let cols = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(20)
        .max(20)
        + 5;
    let rows = lines.len().max(8);
    let mut terminal = crate::terminal::session::Terminal::with_command(
        cols,
        rows,
        portable_pty::CommandBuilder::new("cat"),
        winter_core::MAX_SCROLLBACK,
    )
    .expect("test pane spawn");
    let grid = terminal.grid_mut();
    for (row, line) in lines.iter().enumerate() {
        grid.move_to(row, 0);
        for ch in line.chars() {
            grid.print(ch);
        }
    }
    grid.move_to(0, 0);
    terminal
}

/// Row `row`'s visible text with trailing blanks trimmed, for asserting on
/// what the fixture's `cat` echoed back into the grid.
fn row_text(app: &App, terminal: TabId, row: usize) -> String {
    let grid = app.open_tabs.terminal(&terminal).unwrap().grid();
    (0..grid.cols())
        .map(|col| grid.visible_cell(row, col).map(|c| c.ch).unwrap_or(' '))
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// `["alpha", "beta", "", "gamma", ...blank]`: one paragraph of two lines, a
/// blank separator, then a one-line paragraph.
fn app_with_paragraphs(cursor_row: usize) -> (App, TabId) {
    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = app.layout().shown_tabs()[0];
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["alpha", "beta", "", "gamma"]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (cursor_row, 0));
    (app, id)
}

#[test]
fn test_visual_shift_y_selects_whole_lines_and_leaves_visual() {
    let (mut app, id) = app_with_paragraphs(0);
    app.set_nav_cursor(id, (0, 2));
    app.handle_action(input::Action::EnterVisual(input::VisualKind::Char), id);
    assert_eq!(app.selection.visual_kind, VisualKind::Char);

    app.handle_action(input::Action::YankSelectionLines, id);

    assert_eq!(app.selection.visual_kind, VisualKind::Line);
    assert_eq!(
        app.open_tabs.mode(&id),
        Some(Mode::Normal),
        "yank leaves Visual"
    );
}

#[test]
fn test_shift_y_yanks_from_the_cursor_not_the_whole_line() {
    let (mut app, id) = app_with_paragraphs(0);
    app.set_nav_cursor(id, (0, 2));

    let mut pending = input::PendingPrefix::None;
    let key = input::Key::plain(input::KeyCode::Char('Y'));
    let action = input::resolve(Mode::Normal, &key, &mut pending, 0);
    let input::Action::YankMotion { motion, .. } = action else {
        panic!("Y should be a motion yank, got {action:?}");
    };
    app.handle_action(
        input::Action::YankMotion {
            motion,
            register: Some('r'),
        },
        id,
    );

    assert_eq!(shared::register_text('r').as_deref(), Some("pha"));
}

#[test]
fn test_the_yank_operator_takes_a_word_a_paragraph_and_a_line() {
    // What `y` is for in a terminal: copying out of the output without
    // going through Visual first.
    let (mut app, id) = app_with_paragraphs(0);
    let yank = |app: &mut App, action: input::Action| -> Option<String> {
        app.handle_action(action, id);
        shared::register_text('r')
    };

    assert_eq!(
        yank(
            &mut app,
            input::Action::YankTextObject {
                spec: TextObjectSpec::new(false, TextObject::Word),
                register: Some('r'),
            }
        )
        .as_deref(),
        Some("alpha")
    );
    assert_eq!(
        yank(
            &mut app,
            input::Action::YankTextObject {
                spec: TextObjectSpec::new(false, TextObject::Paragraph),
                register: Some('r'),
            }
        )
        .as_deref(),
        Some("alpha\nbeta"),
        "the paragraph is the run of lines up to the blank one"
    );
    assert_eq!(
        yank(
            &mut app,
            input::Action::YankLine {
                register: Some('r')
            }
        )
        .as_deref(),
        Some("alpha")
    );
}

#[test]
fn test_a_yank_lights_what_it_took_until_the_flash_burns_out() {
    let (mut app, id) = app_with_paragraphs(0);

    app.handle_action(
        input::Action::YankTextObject {
            spec: TextObjectSpec::new(false, TextObject::Word),
            register: Some('r'),
        },
        id,
    );
    let lit = app.selection.span.clone().expect("the yank's own span");
    assert_eq!((lit.start_col, lit.end_col), (0, 4), "the word it took");
    assert!(app.term.yank_flash.is_some(), "and it is lit");

    // Nothing expires while the deadline holds; once it passes, the
    // highlight goes and the selection with it.
    assert!(app.expire_yank_flash().is_some());
    assert!(app.selection.span.is_some());
    app.term.yank_flash = Some((lit, std::time::Instant::now()));
    assert!(app.expire_yank_flash().is_none());
    assert!(app.selection.span.is_none(), "the flash cleared it");
}

#[test]
fn test_a_selection_made_after_a_yank_outlives_its_flash() {
    // The flash only ever puts out its own light: a drag or a Visual
    // selection started since owns the span, and clearing it under the
    // reader would be worse than a highlight that overstays.
    let (mut app, id) = app_with_paragraphs(0);
    app.handle_action(
        input::Action::YankTextObject {
            spec: TextObjectSpec::new(false, TextObject::Word),
            register: Some('r'),
        },
        id,
    );
    let flashed = app.term.yank_flash.clone().expect("a flash").0;
    app.term.yank_flash = Some((flashed, std::time::Instant::now()));
    app.handle_action(
        input::Action::SelectTextObject(TextObjectSpec::new(false, TextObject::Paragraph)),
        id,
    );

    assert!(app.expire_yank_flash().is_none());
    assert!(
        app.selection.span.is_some(),
        "the newer selection is still there"
    );
}

#[test]
fn test_the_marks_winter_keeps_on_its_own_hold_the_jump_and_the_yank() {
    let (mut app, id) = app_with_paragraphs(3);

    // A jump records where it started under `'`, which is how `` ` `` and
    // `''` get back without walking the jumplist.
    app.handle_action(input::Action::MoveCursor(CursorMove::Top), id);
    assert_eq!(
        app.vim.tab(id).and_then(|vim| vim.marks.get(&'\'')),
        Some(&(3, 0))
    );

    // A yank records its own ends under `[` and `]`.
    app.handle_action(
        input::Action::YankTextObject {
            spec: TextObjectSpec::new(false, TextObject::Paragraph),
            register: Some('r'),
        },
        id,
    );
    assert_eq!(
        app.vim.tab(id).and_then(|vim| vim.marks.get(&'[')),
        Some(&(0, 0))
    );
    assert_eq!(
        app.vim
            .tab(id)
            .and_then(|vim| vim.marks.get(&']'))
            .map(|&(row, _)| row),
        Some(1),
        "the last row the paragraph covered"
    );
}

#[test]
fn test_select_paragraph_covers_the_run_of_lines_around_the_cursor() {
    // `M-i` from the second line of the first paragraph selects both its
    // lines linewise and stops at the blank separator: vim's `vip`.
    let (mut app, id) = app_with_paragraphs(1);

    app.handle_action(input::Action::SelectParagraph, id);

    assert_eq!(app.open_tabs.mode(&id), Some(Mode::Visual));
    assert_eq!(
        app.selection.visual_kind,
        VisualKind::Line,
        "paragraph selection is linewise"
    );
    let sel = app.selection.span.as_ref().expect("a selection");
    assert_eq!((sel.start_row, sel.end_row), (0, 1));
    assert_eq!(app.selected_text().as_deref(), Some("alpha\nbeta"));
    assert_eq!(
        app.nav_cursor(id),
        Some((1, 0)),
        "cursor ends on the last line"
    );
}

#[test]
fn test_select_paragraph_from_a_one_line_paragraph_selects_only_that_line() {
    let (mut app, id) = app_with_paragraphs(3);

    app.handle_action(input::Action::SelectParagraph, id);

    let sel = app.selection.span.as_ref().expect("a selection");
    assert_eq!((sel.start_row, sel.end_row), (3, 3));
    assert_eq!(app.selected_text().as_deref(), Some("gamma"));
}

#[test]
fn test_select_paragraph_on_a_blank_line_selects_the_blank_run() {
    // Vim's `ip` on a blank line takes the blank run instead; here that's the
    // single separator row.
    let (mut app, id) = app_with_paragraphs(2);

    app.handle_action(input::Action::SelectParagraph, id);

    let sel = app.selection.span.as_ref().expect("a selection");
    assert_eq!((sel.start_row, sel.end_row), (2, 2));
}

/// A pane whose grid has `lines` printed one per line-feed, so that with more
/// lines than rows the earlier ones end up in scrollback.
fn pane_with_history(lines: &[&str]) -> crate::terminal::session::Terminal {
    let mut terminal = crate::terminal::session::Terminal::with_command(
        20,
        4,
        portable_pty::CommandBuilder::new("cat"),
        winter_core::MAX_SCROLLBACK,
    )
    .expect("test pane spawn");
    {
        let grid = terminal.grid_mut();
        for (i, line) in lines.iter().enumerate() {
            for ch in line.chars() {
                grid.print(ch);
            }
            if i + 1 < lines.len() {
                grid.line_feed();
                grid.move_to_column(0);
            }
        }
    }
    terminal
}

#[test]
fn test_find_overlay_labels_every_candidate_and_jumps_to_the_picked_one() {
    // `f a` on "banana" has three landing spots, so the overlay goes up with a
    // lowercase label on each; the label key jumps and takes it down.
    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = TabId(1);
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["banana"]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 0));

    app.handle_action(
        input::Action::FindChar(FindChar {
            ch: 'a',
            forward: true,
            till: false,
        }),
        id,
    );

    let labels = app.chrome.labels.find.clone().expect("overlay is up");
    assert_eq!(
        labels.iter().map(|l| (l.row, l.col)).collect::<Vec<_>>(),
        vec![(0, 1), (0, 3), (0, 5)]
    );
    assert!(
        labels.iter().all(|l| l.label.is_ascii_lowercase()),
        "labels are lowercase"
    );
    assert_eq!(app.input.keys.prefix, input::PendingPrefix::FindLabel);
    assert_eq!(app.nav_cursor(id), Some((0, 0)), "nothing moves yet");

    let second = labels[1].label;
    app.handle_action(input::Action::FindJump(second), id);

    assert_eq!(app.nav_cursor(id), Some((0, 3)));
    assert!(
        app.chrome.labels.find.is_none(),
        "overlay taken down after the jump"
    );
}

#[test]
fn test_find_overlay_is_skipped_for_a_single_candidate() {
    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = TabId(1);
    app.open_tabs.insert_terminal(id, pane_with_lines(&["abc"]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 0));

    app.handle_action(
        input::Action::FindChar(FindChar {
            ch: 'c',
            forward: true,
            till: false,
        }),
        id,
    );

    assert!(app.chrome.labels.find.is_none(), "one candidate needs no overlay");
    assert_eq!(app.nav_cursor(id), Some((0, 2)), "it just jumps");
    assert_eq!(app.input.keys.prefix, input::PendingPrefix::None);
}

#[test]
fn test_find_overlay_spans_the_visible_rows_and_respects_till() {
    // Easymotion-style: candidates come from the whole screen in the search
    // direction, and `t` lands one cell short of each target.
    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = TabId(1);
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["--a--", "--a--"]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 0));

    app.handle_action(
        input::Action::FindChar(FindChar {
            ch: 'a',
            forward: true,
            till: true,
        }),
        id,
    );

    let labels = app.chrome.labels.find.clone().expect("overlay is up");
    assert_eq!(
        labels.iter().map(|l| (l.row, l.col)).collect::<Vec<_>>(),
        vec![(0, 1), (1, 1)],
        "one landing spot per row, each one cell before the 'a'"
    );
}

#[test]
fn test_find_overlay_jump_extends_a_visual_selection() {
    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = TabId(1);
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["banana"]));
    app.open_tabs.set_mode(&id, Mode::Visual);
    app.selection.visual_anchor = Some((0, 0));
    app.set_nav_cursor(id, (0, 0));
    app.update_visual_selection(id);

    app.handle_action(
        input::Action::FindChar(FindChar {
            ch: 'a',
            forward: true,
            till: false,
        }),
        id,
    );
    let label = app.chrome.labels.find.clone().expect("overlay is up")[2].label;
    app.handle_action(input::Action::FindJump(label), id);

    assert_eq!(app.nav_cursor(id), Some((0, 5)));
    assert_eq!(app.selected_text().as_deref(), Some("banana"));
}

#[test]
fn test_find_overlay_cancel_leaves_the_cursor_alone() {
    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = TabId(1);
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["banana"]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 0));

    app.handle_action(
        input::Action::FindChar(FindChar {
            ch: 'a',
            forward: true,
            till: false,
        }),
        id,
    );
    app.handle_action(input::Action::FindCancel, id);

    assert!(app.chrome.labels.find.is_none());
    assert_eq!(app.nav_cursor(id), Some((0, 0)));
}

#[test]
fn test_matching_bracket_reaches_a_partner_in_scrollback() {
    // Regression: `%` searched only the visible rows, so a partner scrolled off
    // the top was unreachable. It now scans the whole buffer and scrolls.
    let terminal = pane_with_history(&["f(x", "a", "b", "c", "d", ")end"]);
    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = TabId(1);
    app.open_tabs.insert_terminal(id, terminal);
    assert!(
        app.open_tabs.terminal(&id).unwrap().grid().scrollback_len() > 0,
        "fixture needs the opening bracket above the viewport"
    );
    // The `)` sits on the last visible row, at column 0.
    let last = app
        .open_tabs
        .terminal(&id)
        .unwrap()
        .grid()
        .last_content_row();
    app.set_nav_cursor(id, (last, 0));

    app.move_nav_cursor(input::CursorMove::MatchingBracket, id);

    let (row, col) = app.nav_cursor(id).unwrap();
    let grid = app.open_tabs.terminal(&id).unwrap().grid();
    assert!(
        grid.scroll_offset() > 0,
        "the view scrolled up to the partner"
    );
    assert_eq!(
        grid.visible_cell(row, col).map(|c| c.ch),
        Some('('),
        "the cursor sits on the opening bracket"
    );
}

#[test]
fn test_word_end_back_wraps_onto_the_previous_line_in_scrollback() {
    // `ge` at the start of the top visible line steps to the previous line's
    // last word end, scrolling history in like `b` does.
    let terminal = pane_with_history(&["aa", "bb", "cc", "dd", "ee"]);
    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = TabId(1);
    app.open_tabs.insert_terminal(id, terminal);
    assert_eq!(
        app.open_tabs.terminal(&id).unwrap().grid().scrollback_len(),
        1,
        "one line above"
    );
    app.set_nav_cursor(id, (0, 0));

    app.move_nav_cursor(input::CursorMove::WordEndBack, id);

    let (row, col) = app.nav_cursor(id).unwrap();
    let grid = app.open_tabs.terminal(&id).unwrap().grid();
    assert_eq!(grid.scroll_offset(), 1, "scrolled one line into history");
    assert_eq!(row, 0);
    assert_eq!(
        grid.visible_cell(row, col).map(|c| c.ch),
        Some('a'),
        "landed on the end of the previous line's word"
    );
}

#[test]
fn test_paragraph_motion_scrolls_past_the_viewport_edge() {
    // Regression: `{`/`}` only scanned the visible rows and clamped to the
    // screen's first/last row, so they stopped dead at the viewport edge
    // instead of scrolling into scrollback like `k`/`j` do.
    let mut terminal = crate::terminal::session::Terminal::with_command(
        20,
        4,
        portable_pty::CommandBuilder::new("cat"),
        winter_core::MAX_SCROLLBACK,
    )
    .expect("test pane spawn");
    // Six paragraphs of "text / blank", pushed through a 4-row grid so most
    // of them end up in scrollback.
    {
        let grid = terminal.grid_mut();
        for i in 0..6 {
            for ch in format!("para{i}").chars() {
                grid.print(ch);
            }
            grid.line_feed();
            grid.move_to_column(0);
            grid.line_feed();
            grid.move_to_column(0);
        }
    }
    assert!(
        terminal.grid().scrollback_len() > 0,
        "fixture needs paragraphs above the viewport"
    );

    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = TabId(1);
    app.open_tabs.insert_terminal(id, terminal);
    // Start at the top of the visible screen: everything earlier is off screen.
    app.set_nav_cursor(id, (0, 0));
    let abs_before = app
        .open_tabs
        .terminal(&id)
        .unwrap()
        .grid()
        .to_absolute_row(0);

    app.move_nav_cursor(input::CursorMove::ParagraphBack, id);

    let (row, _) = app.nav_cursor(id).unwrap();
    let abs_after = app
        .open_tabs
        .terminal(&id)
        .unwrap()
        .grid()
        .to_absolute_row(row);
    assert!(
        abs_after < abs_before,
        "backward paragraph should reach a boundary above the viewport \
         (was at absolute row {abs_before}, landed on {abs_after})"
    );
    assert!(
        app.open_tabs.terminal(&id).unwrap().grid().scroll_offset() > 0,
        "and scroll the view up to show it"
    );

    // Forward again returns down the buffer, past the viewport it scrolled to.
    app.move_nav_cursor(input::CursorMove::ParagraphForward, id);
    let (row_fwd, _) = app.nav_cursor(id).unwrap();
    let abs_fwd = app
        .open_tabs
        .terminal(&id)
        .unwrap()
        .grid()
        .to_absolute_row(row_fwd);
    assert!(abs_fwd > abs_after, "forward paragraph moves back down");
}

#[test]
fn test_paragraph_motions_stop_on_the_blank_separators() {
    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = TabId(1);
    app.open_tabs.insert_terminal(
        id,
        pane_with_lines(&["alpha", "beta", "", "gamma", "delta"]),
    );
    app.set_nav_cursor(id, (0, 0));

    app.move_nav_cursor(input::CursorMove::ParagraphForward, id);
    assert_eq!(
        app.nav_cursor(id).unwrap().0,
        2,
        "forward paragraph lands on the blank row"
    );

    app.set_nav_cursor(id, (4, 0));
    app.move_nav_cursor(input::CursorMove::ParagraphBack, id);
    assert_eq!(
        app.nav_cursor(id).unwrap().0,
        2,
        "backward paragraph lands on the blank row"
    );
}

#[test]
fn test_screen_motions_jump_to_top_middle_and_bottom_of_the_viewport() {
    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = TabId(1);
    // Fill every row so `L`/`M` aren't clamped by the content's last row.
    let lines: Vec<String> = (0..8).map(|i| format!("line{i}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    app.open_tabs.insert_terminal(id, pane_with_lines(&refs));
    let rows = app.open_tabs.terminal(&id).unwrap().grid().rows();
    app.set_nav_cursor(id, (3, 2));

    app.move_nav_cursor(input::CursorMove::ScreenTop, id);
    assert_eq!(app.nav_cursor(id).unwrap().0, 0);
    app.move_nav_cursor(input::CursorMove::ScreenBottom, id);
    assert_eq!(app.nav_cursor(id).unwrap().0, rows - 1);
    app.move_nav_cursor(input::CursorMove::ScreenMiddle, id);
    assert_eq!(app.nav_cursor(id).unwrap().0, rows / 2);
}

#[test]
fn test_matching_bracket_hops_between_partners_counting_nesting() {
    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = TabId(1);
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["f(g(x))"]));

    // From the outer `(` at col 1 to its partner at col 6, and back.
    app.set_nav_cursor(id, (0, 1));
    app.move_nav_cursor(input::CursorMove::MatchingBracket, id);
    assert_eq!(app.nav_cursor(id), Some((0, 6)));
    app.move_nav_cursor(input::CursorMove::MatchingBracket, id);
    assert_eq!(app.nav_cursor(id), Some((0, 1)));

    // Vim starts at the first bracket at or right of the cursor: from col 0
    // that's the outer `(` again.
    app.set_nav_cursor(id, (0, 0));
    app.move_nav_cursor(input::CursorMove::MatchingBracket, id);
    assert_eq!(app.nav_cursor(id), Some((0, 6)));
}

#[test]
fn test_last_non_blank_and_word_end_back_motions() {
    let mut app = App::new();
    app.config.status_bar.enabled = true;
    let id = TabId(1);
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["foo bar baz"]));

    app.set_nav_cursor(id, (0, 0));
    app.move_nav_cursor(input::CursorMove::LastNonBlank, id);
    assert_eq!(app.nav_cursor(id), Some((0, 10)), "g_ lands on the final z");

    // `ge` from inside "baz" lands on the end of "bar".
    app.set_nav_cursor(id, (0, 9));
    app.move_nav_cursor(input::CursorMove::WordEndBack, id);
    assert_eq!(app.nav_cursor(id), Some((0, 6)));
}

#[test]
fn test_line_to_top_scrolls_the_cursor_line_to_the_first_row() {
    // `zt` keeps the cursor on its buffer line and scrolls the view so that
    // line sits at the top. Started from inside history: at the live bottom
    // the view can't scroll further down, so a line near the bottom cannot be
    // lifted to the top (a terminal has no blank space past the last row).
    let mut terminal = pane_with_scrollback();
    terminal.grid_mut().scroll_up_history(20);
    let mut app = App::new();
    let id = TabId(1);
    app.open_tabs.insert_terminal(id, terminal);
    app.set_nav_cursor(id, (5, 0));
    let abs_before = app
        .open_tabs
        .terminal(&id)
        .unwrap()
        .grid()
        .to_absolute_row(5);

    app.move_nav_cursor(input::CursorMove::LineToTop, id);

    let (row, _) = app.nav_cursor(id).unwrap();
    let grid = app.open_tabs.terminal(&id).unwrap().grid();
    assert_eq!(row, 0, "the cursor's line is now the top row");
    assert_eq!(
        grid.to_absolute_row(row),
        abs_before,
        "and it is still the same buffer line"
    );
}

#[test]
fn test_page_up_snaps_cursor_to_top_of_page() {
    // Editor convention: PageUp (vim's `C-f`/`C-b` analog) scrolls a
    // full page and repositions the cursor to the new page's top row,
    // rather than leaving it on whatever screen row it was on before.
    let terminal = pane_with_scrollback();
    let mut app = App::new();
    let id = TabId(1);
    app.open_tabs.insert_terminal(id, terminal);
    app.set_nav_cursor(id, (5, 0));

    app.move_nav_cursor(input::CursorMove::PageUp, id);

    let (row, _) = app.nav_cursor(id).expect("cursor still tracked");
    assert_eq!(
        row, 0,
        "PageUp should land the cursor on the new page's top row"
    );
}

#[test]
fn test_page_down_snaps_cursor_to_bottom_of_page() {
    // Same convention in the other direction: paging down through
    // history (not yet back at the live bottom) should land the cursor
    // on the new page's bottom row, not wherever it happened to be.
    let terminal = pane_with_scrollback();
    let mut app = App::new();
    let id = TabId(1);
    app.open_tabs.insert_terminal(id, terminal);

    app.move_nav_cursor(input::CursorMove::Top, id);
    app.move_nav_cursor(input::CursorMove::PageDown, id);

    let rows = app.open_tabs.terminal(&id).unwrap().grid().rows();
    let (row, _) = app.nav_cursor(id).expect("cursor still tracked");
    assert_eq!(
        row,
        rows - 1,
        "PageDown should land the cursor on the new page's bottom row"
    );
}

#[test]
fn test_page_down_from_history_does_not_strand_cursor_past_last_prompt() {
    // Regression: PageDown/PageUp only changed the grid's scroll offset
    // and left `row` untouched, so returning from scrolled-back history
    // to a pane holding just a few lines of content could leave the
    // traversal cursor stranded below the last printed row. `Down`'s own
    // clamp only stops further increments, so it could never pull an
    // already-too-large row back up.
    // One content row and blank rows under it, written straight into the
    // grid: what the shell used to be asked for, without the wait.
    let terminal = pane_with_lines(&["hi"]);
    let last_row = terminal.grid().last_content_row();
    let bottom_row = terminal.grid().rows() - 1;
    assert!(
        bottom_row > last_row,
        "fixture needs blank rows below the last prompt to reproduce the bug"
    );

    let mut app = App::new();
    let id = TabId(1);
    app.open_tabs.insert_terminal(id, terminal);
    // Simulate a cursor stranded past the content, as returning from a
    // scrolled-back page would leave it.
    app.set_nav_cursor(id, (bottom_row, 0));

    app.move_nav_cursor(input::CursorMove::PageDown, id);

    let (row, _) = app.nav_cursor(id).expect("cursor still tracked");
    assert!(
        row <= last_row,
        "cursor at row {row} sits past the last prompt line (row {last_row})"
    );
}

#[test]
fn test_mouse_tracking_parks_nav_cursor_in_normal_and_visual_modes() {
    // A click (or drag) parks the traversal cursor on the pointer's cell so
    // the block cursor follows the mouse while selecting.
    let (mut app, id) = app_with_paragraphs(0);

    app.track_nav_cursor_to_mouse(id, 2, 4);
    assert_eq!(app.nav_cursor(id), Some((2, 4)));

    app.open_tabs.set_mode(&id, Mode::Visual);
    app.track_nav_cursor_to_mouse(id, 3, 1);
    assert_eq!(app.nav_cursor(id), Some((3, 1)));
}

#[test]
fn test_mouse_tracking_leaves_nav_cursor_alone_in_insert_mode() {
    // Insert mode hides the traversal cursor and re-derives it from the
    // shell cursor on entering Normal, so a click must not overwrite the
    // stored position.
    let (mut app, id) = app_with_paragraphs(1);
    app.open_tabs.set_mode(&id, Mode::Insert);

    app.track_nav_cursor_to_mouse(id, 2, 4);

    assert_eq!(
        app.nav_cursor(id),
        Some((1, 0)),
        "Insert-mode click must not move the traversal cursor"
    );
}

#[test]
fn test_count_moves_the_cursor_multiple_rows() {
    // `3j` lands three rows down in one stroke; the count is spent on the
    // motion (a count that resolved to Ignore, or moved once, fails this).
    let (mut app, id) = app_with_paragraphs(0);

    app.handle_action(
        input::Action::MoveCursorN {
            count: 3,
            mv: input::CursorMove::Down,
        },
        id,
    );

    assert_eq!(
        app.nav_cursor(id),
        Some((3, 0)),
        "3j from row 0 lands on row 3"
    );
}

#[test]
fn test_changelist_appends_instead_of_forking() {
    // Unlike the jumplist (whose new jump from the past discards the
    // abandoned future), a changelist is a log: a change made while
    // stepped back becomes the newest entry while the older ones stay
    // reachable behind it.
    let mut changes = ChangeList::default();
    changes.push((1, 0));
    changes.push((2, 0));
    assert_eq!(changes.older((9, 9)), Some((2, 0)));
    changes.push((3, 0));
    assert_eq!(
        changes.older((9, 9)),
        Some((3, 0)),
        "a change made while stepped back becomes the newest entry"
    );
    assert_eq!(
        changes.older((9, 9)),
        Some((2, 0)),
        "older entries survive the push: the changelist is a log, not a fork"
    );
}

#[test]
fn test_changelist_walk_returns_to_the_live_position_last() {
    let mut changes = ChangeList::default();
    changes.push((1, 0));
    changes.push((2, 0));
    assert_eq!(changes.older((9, 9)), Some((2, 0)));
    assert_eq!(changes.older((9, 9)), Some((1, 0)));
    assert_eq!(changes.newer(), Some((2, 0)));
    assert_eq!(
        changes.newer(),
        Some((9, 9)),
        "g, walks all the way back to where it started"
    );
    assert_eq!(
        changes.newer(),
        None,
        "at the live position, g, has nowhere to go"
    );
}

#[test]
fn test_g_semicolon_returns_to_where_a_change_began() {
    // An edit records where it began; `g;` lands the cursor back there
    // after it moved on, and `g,` walks forward to the live spot again.
    let (mut app, id) = app_with_paragraphs(0);
    let (prompt_row, prompt_col) = app.open_tabs.terminal(&id).unwrap().grid().cursor();
    app.set_nav_cursor(id, (prompt_row, prompt_col));

    app.handle_action(input::Action::DeleteCharForward, id);

    app.handle_action(input::Action::MoveCursor(input::CursorMove::Down), id);
    app.handle_action(input::Action::ChangeOlder, id);
    assert_eq!(
        app.nav_cursor(id),
        Some((prompt_row, prompt_col)),
        "g; lands where the delete began"
    );
    app.handle_action(input::Action::ChangeNewer, id);
    assert_eq!(
        app.nav_cursor(id),
        Some((prompt_row + 1, 0)),
        "g, returns to the position the walk started from"
    );
}

#[test]
fn test_dot_redispatches_the_last_normal_mode_change() {
    // `x` then `.`: the repeat re-runs the delete at the cursor, which a
    // no-op `.` (no recorded change, or a stale one) would leave behind.
    let (mut app, id) = app_with_paragraphs(0);
    let (prompt_row, prompt_col) = app.open_tabs.terminal(&id).unwrap().grid().cursor();
    app.set_nav_cursor(id, (prompt_row, prompt_col));

    app.handle_action(input::Action::DeleteCharForward, id);
    assert!(matches!(
        app.vim.tab(id).and_then(|vim| vim.last_change.as_ref()),
        Some(LastChange::Action(input::Action::DeleteCharForward))
    ));

    // `delete_on_prompt` flags the nav cursor for resync on success; reset
    // and re-check it to prove the repeat really re-dispatched the delete.
    app.term.nav_resync_pending = false;
    app.handle_action(input::Action::RepeatLastChange, id);
    assert!(
        app.term.nav_resync_pending,
        "`.` must re-run the recorded delete, not just remember it"
    );
}

#[test]
fn test_dot_replays_the_last_insert_typing_run() {
    // Typing "hi" in Insert and leaving for Normal records the run as the
    // pane's last change; `.` re-sends it, so the echo pane shows it twice.
    // A blank fixture row so the echo is the only text to assert on.
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs.insert_terminal(id, pane_with_lines(&[""]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 0));

    app.handle_action(input::Action::EnterInsert(input::InsertAt::Cursor), id);
    app.handle_action(input::Action::SendBytes(b"hi".to_vec()), id);
    // The key handler accumulates forwarded bytes into the session run.
    app.vim
        .tab_mut(id)
        .insert_session
        .get_or_insert_with(Default::default)
        .run
        .extend_from_slice(b"hi");
    app.handle_action(input::Action::SwitchMode(Mode::Normal), id);
    assert_eq!(
        app.vim.tab(id).and_then(|vim| vim.last_change.as_ref()),
        Some(&LastChange::Typed(b"hi".to_vec()))
    );

    app.handle_action(input::Action::RepeatLastChange, id);
    // `cat` echoes both writes back; pump until both arrive.
    let deadline = std::time::Instant::now() + PTY_WAIT;
    while std::time::Instant::now() < deadline {
        app.open_tabs.terminal_mut(&id).unwrap().drain_output();
        if row_text(&app, id, 0) == "hihi" {
            break;
        }
        std::thread::sleep(POLL_STEP);
    }
    assert_eq!(
        row_text(&app, id, 0),
        "hihi",
        "`.` must re-send the recorded typing run"
    );
}

#[test]
fn test_dot_without_a_recorded_change_is_a_no_op() {
    let (mut app, id) = app_with_paragraphs(0);
    let before = app.nav_cursor(id);

    app.handle_action(input::Action::RepeatLastChange, id);

    assert_eq!(app.nav_cursor(id), before);
    assert!(app.vim.tab(id).is_none_or(|vim| vim.last_change.is_none()));
}

#[test]
fn test_dot_session_through_the_real_resolver_and_key_path() {
    // Drives the same pipeline the keyboard handler runs: `resolve_with`
    // for each key, `record_insert_key` for forwarded bytes, `handle_action`
    // for dispatch, so the Insert session opens, accumulates, and closes
    // through production code, not hand-built actions. Entry chord
    // (M-ESC) leaves Insert deterministically, without the
    // bare-Escape foreground-process branch.
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs.insert_terminal(id, pane_with_lines(&[""]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 0));
    let keymap = input::Keymap::default();
    let mut pending = input::PendingPrefix::None;
    let mut dispatch = |app: &mut App, key: input::Key| {
        let mode = app.open_tabs.mode(&id).unwrap_or_default();
        let action = input::resolve_with(mode, &key, &mut pending, &keymap, 0, None, false);
        if mode == Mode::Insert && crate::window::input::keys::forwarded_to_pty(&action) {
            let at_prompt = app
                .open_tabs
                .terminal(&id)
                .is_some_and(|p| p.is_at_prompt());
            app.record_insert_key(id, &key, &action, at_prompt);
        }
        app.handle_action(action, id);
    };

    dispatch(
        &mut app,
        input::Key {
            alt: false,
            code: input::KeyCode::Char('i'),
            ctrl: false,
            shift: false,
            sup: false,
        },
    );
    for ch in ['h', 'i'] {
        dispatch(
            &mut app,
            input::Key {
                alt: false,
                code: input::KeyCode::Char(ch),
                ctrl: false,
                shift: false,
                sup: false,
            },
        );
    }
    dispatch(
        &mut app,
        input::Key {
            alt: true,
            code: input::KeyCode::Escape,
            ctrl: false,
            shift: false,
            sup: false,
        },
    );

    assert_eq!(
        app.open_tabs.mode(&id),
        Some(Mode::Normal),
        "entry chord left Insert"
    );
    assert_eq!(
        app.vim.tab(id).and_then(|vim| vim.last_change.as_ref()),
        Some(&LastChange::Typed(b"hi".to_vec())),
        "the real key path accumulated the typed run and finalized it on leaving Insert"
    );

    app.handle_action(input::Action::RepeatLastChange, id);
    let deadline = std::time::Instant::now() + PTY_WAIT;
    while std::time::Instant::now() < deadline {
        app.open_tabs.terminal_mut(&id).unwrap().drain_output();
        if row_text(&app, id, 0) == "hihi" {
            break;
        }
        std::thread::sleep(POLL_STEP);
    }
    assert_eq!(
        row_text(&app, id, 0),
        "hihi",
        "`.` re-sends the run recorded through the real path"
    );
}

#[test]
fn test_jumplist_returns_to_the_position_before_the_jump() {
    // `gg` records its origin, so `C-o` comes back to it and `C-i`
    // walks forward again: in absolute terms, so the round trip holds
    // even once the view has scrolled.
    let (mut app, id) = app_with_paragraphs(2);
    let abs_before = app
        .open_tabs
        .terminal(&id)
        .unwrap()
        .grid()
        .to_absolute_row(2);

    app.handle_action(input::Action::MoveCursor(input::CursorMove::Top), id);
    assert_eq!(
        app.nav_cursor(id),
        Some((0, 0)),
        "gg lands on the oldest row"
    );

    app.handle_action(input::Action::JumpOlder, id);
    let (row, _) = app.nav_cursor(id).expect("cursor restored");
    assert_eq!(
        app.open_tabs
            .terminal(&id)
            .unwrap()
            .grid()
            .to_absolute_row(row),
        abs_before,
        "C-o returns to the pre-jump line"
    );

    app.handle_action(input::Action::JumpNewer, id);
    assert_eq!(
        app.nav_cursor(id),
        Some((0, 0)),
        "C-i walks forward to the post-jump position"
    );
}

#[test]
fn test_jumplist_forks_when_a_new_jump_leaves_the_past() {
    // Stepping back then jumping again discards the abandoned entries, so
    // the next C-o lands on the new jump, not on a forked-away future.
    let mut jumps = JumpList::default();
    jumps.push((1, 0));
    jumps.push((2, 0));
    assert_eq!(jumps.older((9, 9)), Some((2, 0)));
    jumps.push((3, 0));
    assert_eq!(
        jumps.older((8, 8)),
        Some((3, 0)),
        "the entry stepped back past is forked away, not kept"
    );
    assert_eq!(
        jumps.newer(),
        Some((8, 8)),
        "walking all the way forward returns to the live position"
    );
    assert_eq!(jumps.newer(), None, "already at the live position");
}

#[test]
fn test_leaving_normal_records_a_jump_origin() {
    // Pressing `i` from a browsed position is itself a jump in vim's
    // jumplist: after the next Escape (which re-derives the cursor at the
    // prompt), C-o returns to where the browsing left off.
    let (mut app, id) = app_with_paragraphs(2);
    let abs_before = app
        .open_tabs
        .terminal(&id)
        .unwrap()
        .grid()
        .to_absolute_row(2);

    app.handle_action(input::Action::SwitchMode(Mode::Insert), id);
    // The next Escape re-enters Normal and re-derives the cursor at the
    // prompt: the state C-o is actually pressed from.
    app.handle_action(input::Action::SwitchMode(Mode::Normal), id);

    app.handle_action(input::Action::JumpOlder, id);
    let (row, _) = app.nav_cursor(id).expect("cursor restored");
    assert_eq!(
        app.open_tabs
            .terminal(&id)
            .unwrap()
            .grid()
            .to_absolute_row(row),
        abs_before,
        "the pre-Insert browsing position is on the jumplist"
    );
}

#[test]
fn test_visual_o_swaps_the_cursor_to_the_anchor() {
    // The cursor block moves to the selection's other end while the
    // highlighted span is unchanged: it always runs anchor..cursor in
    // either order.
    let (mut app, id) = app_with_paragraphs(2);
    app.open_tabs.set_mode(&id, Mode::Visual);
    app.selection.visual_anchor = Some((0, 2));
    app.set_nav_cursor(id, (2, 1));
    app.update_visual_selection(id);
    // `update_visual_selection` stores anchor->cursor order, and the swap
    // flips which end is which: compare the covered span (normalized, the
    // same way the renderer resolves it) instead of the raw field order.
    let span_before = app
        .selection
        .span
        .as_ref()
        .map(|s| (s.start_row.min(s.end_row), s.start_row.max(s.end_row)));

    app.handle_action(input::Action::SwapVisualEnds, id);

    assert_eq!(
        app.nav_cursor(id),
        Some((0, 2)),
        "cursor sits on the anchor"
    );
    assert_eq!(
        app.selection.visual_anchor,
        Some((2, 1)),
        "anchor moved to the old cursor"
    );
    assert_eq!(
        app.selection
            .span
            .as_ref()
            .map(|s| (s.start_row.min(s.end_row), s.start_row.max(s.end_row))),
        span_before,
        "the selection span itself is unchanged"
    );
}

#[test]
fn test_gv_restores_the_last_visual_selection() {
    // Every exit from Visual snapshots the selection; `gv` brings back the
    // same span, the same kind (linewise here), and the same cursor end.
    let (mut app, id) = app_with_paragraphs(0);
    app.handle_action(input::Action::EnterVisual(input::VisualKind::Line), id);
    app.handle_action(
        input::Action::MoveCursorN {
            count: 2,
            mv: input::CursorMove::Down,
        },
        id,
    );
    let span = app
        .selection
        .span
        .as_ref()
        .map(|s| (s.start_row, s.end_row))
        .expect("selection live");

    // Leaving Visual (same `V` again) clears it; `gv` restores it.
    app.handle_action(input::Action::EnterVisual(input::VisualKind::Line), id);
    assert!(
        app.selection.span.is_none(),
        "exiting Visual cleared the selection"
    );

    app.handle_action(input::Action::RestoreVisual, id);

    assert_eq!(
        app.open_tabs.mode(&id),
        Some(Mode::Visual),
        "gv re-enters Visual"
    );
    assert_eq!(
        app.selection.visual_kind,
        VisualKind::Line,
        "the linewise kind is preserved"
    );
    assert_eq!(
        app.selection
            .span
            .as_ref()
            .map(|s| (s.start_row, s.end_row)),
        Some(span),
        "the same span is selected again"
    );
}

#[test]
fn test_mark_set_and_goto_exact_and_first_non_blank() {
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["alpha", "   beta", "gamma"]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (1, 4));

    // Set mark 'a' at row 1, col 4
    app.handle_action(input::Action::SetMark('a'), id);

    // Move elsewhere
    app.set_nav_cursor(id, (0, 0));

    // Goto exact (`) lands on col 4
    app.handle_action(input::Action::GotoMark(input::GotoMark::new('a', true)), id);
    assert_eq!(app.nav_cursor(id), Some((1, 4)));

    // Move elsewhere
    app.set_nav_cursor(id, (0, 0));

    // Goto first non-blank (') lands on col 3 (start of "beta")
    app.handle_action(
        input::Action::GotoMark(input::GotoMark::new('a', false)),
        id,
    );
    assert_eq!(app.nav_cursor(id), Some((1, 3)));
}

#[test]
fn test_goto_mark_records_jump_origin_for_ctrl_o() {
    let (mut app, id) = app_with_paragraphs(0);
    app.set_nav_cursor(id, (3, 0));
    app.handle_action(input::Action::SetMark('z'), id);

    app.set_nav_cursor(id, (0, 2));

    // Jumping to mark 'z' records the origin (0, 2)
    app.handle_action(input::Action::GotoMark(input::GotoMark::new('z', true)), id);
    assert_eq!(app.nav_cursor(id), Some((3, 0)));

    // C-o returns to (0, 2)
    app.handle_action(input::Action::JumpOlder, id);
    assert_eq!(app.nav_cursor(id), Some((0, 2)));
}

#[test]
fn test_goto_mark_in_visual_mode_extends_selection() {
    let (mut app, id) = app_with_paragraphs(0);
    app.set_nav_cursor(id, (3, 0));
    app.handle_action(input::Action::SetMark('b'), id);

    app.set_nav_cursor(id, (0, 0));
    app.handle_action(input::Action::EnterVisual(input::VisualKind::Char), id);

    app.handle_action(input::Action::GotoMark(input::GotoMark::new('b', true)), id);
    assert_eq!(app.nav_cursor(id), Some((3, 0)));
    assert!(
        app.selection.span.is_some(),
        "selection extends to the mark"
    );
    let sel = app.selection.span.as_ref().unwrap();
    assert_eq!((sel.start_row, sel.end_row), (0, 3));
}

#[test]
fn test_select_text_object_word_in_normal_enters_visual() {
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["hello world foo"]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 7)); // on 'o' in 'world'

    app.handle_action(
        input::Action::SelectTextObject(input::TextObjectSpec::new(false, input::TextObject::Word)),
        id,
    );

    assert_eq!(app.open_tabs.mode(&id), Some(Mode::Visual));
    assert_eq!(app.selection.visual_kind, VisualKind::Char);
    assert_eq!(app.selected_text().as_deref(), Some("world"));
}

#[test]
fn test_select_text_object_quotes() {
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["let s = \"hello world\";"]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 11)); // inside quotes

    // Inner quotes: `i"`
    app.handle_action(
        input::Action::SelectTextObject(input::TextObjectSpec::new(
            false,
            input::TextObject::Quotes('"'),
        )),
        id,
    );
    assert_eq!(app.selected_text().as_deref(), Some("hello world"));

    // Around quotes: `a"`
    app.handle_action(
        input::Action::SelectTextObject(input::TextObjectSpec::new(
            true,
            input::TextObject::Quotes('"'),
        )),
        id,
    );
    assert_eq!(app.selected_text().as_deref(), Some("\"hello world\""));
}

#[test]
fn test_blockwise_visual_selection_and_yank() {
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["abcdef", "123456", "ghijkl"]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 1)); // 'b'

    // Enter blockwise visual
    app.handle_action(input::Action::EnterVisual(input::VisualKind::Block), id);
    assert_eq!(app.open_tabs.mode(&id), Some(Mode::Visual));
    assert_eq!(app.selection.visual_kind, VisualKind::Block);

    // Move down 2 rows, right 2 cols (cols 1..=3 on rows 0..=2)
    app.set_nav_cursor(id, (2, 3));
    app.update_visual_selection(id);

    let sel = app.selection.span.as_ref().expect("block selection active");
    assert!(sel.block, "selection is marked block");
    assert_eq!(app.selected_text().as_deref(), Some("bcd\n234\nhij"));
}

#[test]
fn test_named_register_yank_and_paste_in_app() {
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["echo \"hello\"", ""]));
    app.open_tabs.set_mode(&id, Mode::Visual);
    app.selection.visual_anchor = Some((0, 5));
    app.set_nav_cursor(id, (0, 11));
    app.update_visual_selection(id);

    // Yank into register 'a'
    app.handle_action(input::Action::YankSelectionRegister('a'), id);
    assert_eq!(
        shared::register_text('a').as_deref(),
        Some("\"hello\"")
    );
    assert_eq!(app.open_tabs.mode(&id), Some(Mode::Normal));
}

#[test]
fn test_buffer_swoop_opens_navigates_and_confirms_with_jump_history() {
    use winit::keyboard::{Key, NamedKey, PhysicalKey};

    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs.insert_terminal(
        id,
        pane_with_lines(&["alpha line", "beta target", "gamma last"]),
    );
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 0));

    // Open Swoop
    app.open_swoop(id);
    let palette = app.chrome.picker().expect("palette is active");
    assert_eq!(palette.mode, crate::model::picker::PickerMode::Swoop);
    assert_eq!(palette.entries.len(), 3);

    // Press Down Arrow to select "beta target" (row 1)
    let key_down = Key::Named(NamedKey::ArrowDown);
    let phys = PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
    app.press_palette_key(&key_down, &phys, id);

    // Check live preview jumped nav cursor to row 1
    assert_eq!(app.nav_cursor(id), Some((1, 0)));

    // Press Enter to confirm
    let key_enter = Key::Named(NamedKey::Enter);
    app.press_palette_key(&key_enter, &phys, id);
    // Picker closed
    assert!(app.chrome.picker().is_none());
    assert_eq!(app.nav_cursor(id), Some((1, 0)));

    // Jump list has origin (0, 0)
    let jl = &mut app.vim.tabs.get_mut(&id).expect("jumplist entry").jump_list;
    assert_eq!(jl.older((1, 0)), Some((0, 0)));
}

#[test]
fn test_buffer_swoop_cancel_restores_initial_cursor() {
    use winit::keyboard::{Key, NamedKey, PhysicalKey};

    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs.insert_terminal(
        id,
        pane_with_lines(&["alpha line", "beta target", "gamma last"]),
    );
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 2));

    // Open Swoop
    app.open_swoop(id);

    // Press Down Arrow
    let key_down = Key::Named(NamedKey::ArrowDown);
    let phys = PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
    app.press_palette_key(&key_down, &phys, id);

    // Preview is at row 1
    assert_eq!(app.nav_cursor(id), Some((1, 0)));

    // Press Escape to cancel
    let key_esc = Key::Named(NamedKey::Escape);
    app.press_palette_key(&key_esc, &phys, id);
    // Picker closed and cursor restored to (0, 2)
    assert!(app.chrome.picker().is_none());
    assert_eq!(app.nav_cursor(id), Some((0, 2)));
}

#[test]
fn test_buffer_jump_terminal_opens_navigates_and_confirms() {
    use winit::keyboard::{Key, NamedKey, PhysicalKey};

    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs.insert_terminal(
        id,
        pane_with_lines(&["alpha line", "beta target", "gamma last"]),
    );
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 0));

    // Open Jump on a terminal tab
    app.open_jump(id);
    let palette = app.chrome.picker().expect("palette is active");
    assert_eq!(palette.mode, crate::model::picker::PickerMode::Jump);
    assert_eq!(palette.title.as_deref(), Some("Jump: Terminal"));
    assert_eq!(palette.entries.len(), 3);

    // Move down to row 1
    let key_down = Key::Named(NamedKey::ArrowDown);
    let phys = PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
    app.press_palette_key(&key_down, &phys, id);

    // Check preview
    assert_eq!(app.nav_cursor(id), Some((1, 0)));

    // Confirm
    let key_enter = Key::Named(NamedKey::Enter);
    app.press_palette_key(&key_enter, &phys, id);
    assert!(app.chrome.picker().is_none());
    assert_eq!(app.nav_cursor(id), Some((1, 0)));
    let jl = &mut app.vim.tabs.get_mut(&id).expect("jumplist entry").jump_list;
    assert_eq!(jl.older((1, 0)), Some((0, 0)));
}

#[test]
fn test_buffer_jump_cancel_restores_position() {
    use winit::keyboard::{Key, NamedKey, PhysicalKey};

    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs.insert_terminal(
        id,
        pane_with_lines(&["alpha line", "beta target", "gamma last"]),
    );
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 3));

    app.open_jump(id);

    let key_down = Key::Named(NamedKey::ArrowDown);
    let phys = PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
    app.press_palette_key(&key_down, &phys, id);
    assert_eq!(app.nav_cursor(id), Some((1, 0)));

    // Cancel with Escape
    let key_esc = Key::Named(NamedKey::Escape);
    app.press_palette_key(&key_esc, &phys, id);
    assert!(app.chrome.picker().is_none());
    assert_eq!(app.nav_cursor(id), Some((0, 3)));
}

#[test]
fn test_buffer_jump_on_an_editor_previews_confirms_and_cancels() {
    use crate::editors::dir::DirEditor;
    use crate::window::editor::EditorKind;
    use winit::keyboard::{Key, NamedKey, PhysicalKey};

    let mut app = App::new();
    let dir = std::env::temp_dir().join("winter_jump_page_test");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("a.txt"), "hello");
    let _ = std::fs::write(dir.join("b.txt"), "world");

    let editor = Box::new(DirEditor::new(dir.clone()));
    let id = app.show_editor(EditorKind::Dir, editor);

    // Open Jump over the editor
    app.open_jump(id);
    let palette = app.chrome.picker().expect("palette active");
    assert_eq!(palette.mode, crate::model::picker::PickerMode::Jump);
    assert!(palette.title.as_ref().unwrap().starts_with("Jump: "));

    // Preview down to item 1
    let key_down = Key::Named(NamedKey::ArrowDown);
    let phys = PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
    app.press_palette_key(&key_down, &phys, id);

    assert_eq!(
        app.open_tabs.editor(&id).unwrap().editor.jump_cursor(),
        Some(1)
    );

    // Escape cancels and restores to 0
    let key_esc = Key::Named(NamedKey::Escape);
    app.press_palette_key(&key_esc, &phys, id);
    assert!(app.chrome.picker().is_none());
    assert_eq!(
        app.open_tabs.editor(&id).unwrap().editor.jump_cursor(),
        Some(0)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_action_jump_toggles_palette() {
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["alpha line", "beta target"]));

    // Action::Jump opens jump palette
    app.handle_action(crate::model::input::Action::Jump, id);
    assert!(app.chrome.picker().is_some());
    assert_eq!(
        app.chrome.picker().unwrap().mode,
        crate::model::picker::PickerMode::Jump
    );

    // Second Action::Jump toggles it closed and cancels jump
    app.handle_action(crate::model::input::Action::Jump, id);
    assert!(app.chrome.picker().is_none());
}

#[test]
fn test_reveal_position_centered_vertically_centers_in_pane() {
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    let lines: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
    let str_lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&str_lines));
    app.open_tabs.set_mode(&id, Mode::Normal);

    // Center on row 10 in a 20-row pane: view_row lands on 10 (20 / 2)
    app.reveal_position_centered(id, (10, 0));
    assert_eq!(app.nav_cursor(id), Some((10, 0)));
}

#[test]
fn test_change_operator_deletes_and_enters_insert() {
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["echo \"hello world\""]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 6)); // on 'h'

    // `ci"` (Change inside quotes)
    app.handle_action(
        input::Action::ChangeTextObject(input::TextObjectSpec::new(
            false,
            input::TextObject::Quotes('"'),
        )),
        id,
    );

    // Pane should transition to Mode::Insert
    assert_eq!(app.open_tabs.mode(&id), Some(Mode::Insert));

    // Return to Normal mode and test `s` (substitute)
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 2));
    app.handle_action(input::Action::SubstituteChar, id);
    assert_eq!(app.open_tabs.mode(&id), Some(Mode::Insert));

    // Return to Normal mode and test `S` (change whole line)
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.handle_action(input::Action::ChangeLine, id);
    assert_eq!(app.open_tabs.mode(&id), Some(Mode::Insert));
}

#[test]
fn test_replace_char_and_toggle_case_on_prompt() {
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    app.open_tabs
        .insert_terminal(id, pane_with_lines(&["Hello World"]));
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 0)); // 'H'

    // `rX` (Replace char with 'X')
    app.handle_action(input::Action::ReplaceChar('X'), id);
    // Stays in Mode::Normal
    assert_eq!(app.open_tabs.mode(&id), Some(Mode::Normal));

    // `~` (Toggle case)
    app.handle_action(input::Action::ToggleCaseChar, id);
    // Cursor moved to column 1 and stays in Normal mode
    assert_eq!(app.nav_cursor(id), Some((0, 1)));
    assert_eq!(app.open_tabs.mode(&id), Some(Mode::Normal));
}

#[test]
fn test_prompt_edits_refuse_the_shells_own_prompt_text() {
    // With the B mark recorded, a Vim operator aimed left of the command
    // line - at the shell's \"$ \" decoration - must be refused rather
    // than mistranslated into readline chords that edit the wrong
    // characters.
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    let mut terminal = pane_with_lines(&[]);
    terminal.feed_program_output(b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\ls -l");
    app.open_tabs.insert_terminal(id, terminal);
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 0)); // the '$' of the decoration

    app.handle_action(input::Action::ReplaceChar('X'), id);
    assert!(
        app.chrome.notice_text().is_some(),
        "refused with a dedicated error"
    );
    assert_eq!(app.nav_cursor(id), Some((0, 0)), "nothing moved");

    app.handle_action(input::Action::DeleteCharForward, id);
    assert!(app.chrome.notice_text().is_some());

    // The same edit on the command text itself still works.
    app.set_nav_cursor(id, (0, 3)); // 'l' of \"ls\"
    app.handle_action(input::Action::ToggleCaseChar, id);
    assert_eq!(
        app.nav_cursor(id),
        Some((0, 4)),
        "edit past the decoration proceeds"
    );
}

#[test]
fn test_jump_to_prompt_gp() {
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    let mut terminal = pane_with_lines(&["first line", "second line", "prompt line"]);
    terminal.grid_mut().move_to_row(2);
    terminal.grid_mut().move_to_column(5);
    app.open_tabs.insert_terminal(id, terminal);
    app.open_tabs.set_mode(&id, Mode::Normal);
    app.set_nav_cursor(id, (0, 0));

    // Jump to prompt with `gp`
    app.handle_action(input::Action::JumpToPrompt, id);
    assert_eq!(app.nav_cursor(id), Some((2, 5)));
}

#[test]
fn test_vertical_band_stop_treats_a_band_as_one_stop_each_way() {
    // A band spanning viewport rows [4, 9).
    let spans = [(4, 9)];
    // Down: entering parks on the band's first row, leaving from anywhere
    // inside jumps past the whole band.
    assert_eq!(vertical_band_stop(&spans, 3, true), 4);
    assert_eq!(vertical_band_stop(&spans, 4, true), 9);
    assert_eq!(vertical_band_stop(&spans, 6, true), 9);
    // Up: entering from below parks on the first row, leaving from inside
    // lands just above the band.
    assert_eq!(vertical_band_stop(&spans, 9, false), 4);
    assert_eq!(vertical_band_stop(&spans, 4, false), 3);
    // Rows outside a band still move one row at a time.
    assert_eq!(vertical_band_stop(&spans, 1, true), 2);
    assert_eq!(vertical_band_stop(&spans, 2, false), 1);
    assert_eq!(vertical_band_stop(&spans, 10, true), 11);
    assert_eq!(vertical_band_stop(&spans, 12, false), 11);
}

#[test]
fn test_vertical_band_stop_without_bands_moves_one_row() {
    assert_eq!(vertical_band_stop(&[], 5, true), 6);
    assert_eq!(vertical_band_stop(&[], 5, false), 4);
    // Row 0 cannot go further up.
    assert_eq!(vertical_band_stop(&[], 0, false), 0);
}

#[test]
fn test_j_and_k_cross_a_block_band_in_one_stop_each() {
    // Regression: a reserved band cost `j` one keypress per blank row
    // behind the image, and the 1-cell cursor parked inside it was
    // invisible against the block.
    let mut app = App::new();
    let id = app.layout().shown_tabs()[0];
    let terminal = pane_with_lines(&["above", "more", "", "", "", "below"]);
    app.open_tabs.insert_terminal(id, terminal);
    app.open_tabs.set_mode(&id, Mode::Normal);
    // A block band over the blank rows 2..5 (absolute rows 2..5, no
    // scrollback in the fixture, so viewport rows line up with them).
    app.term.image_blocks.push(crate::window::ImageBlock {
        abs_row: 2,
        block_index: 0,
        closed: false,
        content: crate::model::picture::Content::Raster(std::sync::Arc::from(&[][..])),
        drawn: None,
        id: 1,
        max_rows: 3,
        natural: Some((80.0, 40.0)),
        segment_index: 1,
        tab_id: id,
    });
    app.set_nav_cursor(id, (1, 0));

    app.handle_action(input::Action::MoveCursor(input::CursorMove::Down), id);
    assert_eq!(app.nav_cursor(id), Some((2, 0)), "j parks on the band");

    app.handle_action(input::Action::MoveCursor(input::CursorMove::Down), id);
    assert_eq!(
        app.nav_cursor(id),
        Some((5, 0)),
        "the next j crosses the whole band in one stop"
    );

    app.handle_action(input::Action::MoveCursor(input::CursorMove::Up), id);
    assert_eq!(app.nav_cursor(id), Some((2, 0)), "k parks on the band");

    app.handle_action(input::Action::MoveCursor(input::CursorMove::Up), id);
    assert_eq!(app.nav_cursor(id), Some((1, 0)), "and back above it");
}

#[test]
fn test_nav_cursor_is_isolated_per_pane() {
    // Regression: the Normal-mode traversal cursor used to be a single shared
    // field, so switching focus between two panes leaked the source pane's
    // cursor coordinates into the destination, landing the cursor before the
    // destination pane's first typeable column. Now it is kept with its
    // tab, so each tab's cursor is read only for that tab.
    let mut app = App::new();
    let a = TabId(1);
    let b = TabId(2);
    app.open_tabs.insert_terminal(a, idle_terminal());
    app.open_tabs.insert_terminal(b, idle_terminal());
    app.set_nav_cursor(a, (5, 3));
    app.set_nav_cursor(b, (1, 9));
    // Pane A's cursor never bleeds into pane B and vice versa.
    assert_eq!(app.nav_cursor(a), Some((5, 3)));
    assert_eq!(app.nav_cursor(b), Some((1, 9)));
    // A pane with no cursor reads as None, regardless of other panes' cursors.
    assert_eq!(app.nav_cursor(TabId(99)), None);
    // Clearing one tab's cursor does not disturb the other's.
    app.clear_nav_cursor(a);
    assert_eq!(app.nav_cursor(a), None);
    assert_eq!(app.nav_cursor(b), Some((1, 9)));
}
