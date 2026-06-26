//! The PDF viewer's tests, over documents made of text alone: the keys'
//! grammar, and what each command does to the view and the cursor.

use super::caret::Pos;
use super::text::tests::words;
use super::text::PageText;
use super::*;

/// A US Letter page, in points.
const LETTER: (f32, f32) = (612.0, 792.0);

/// A pane that fits a letter page's width at scale one, and shows most of
/// one page's height.
const PANE: (f32, f32) = (636.0, 600.0);

/// A viewer over `pages`, each a list of lines set down the page from the
/// top, laid out in [`PANE`].
fn reader(pages: &[&[&str]]) -> PdfTool {
    let texts = pages
        .iter()
        .map(|lines| {
            let glyphs = lines
                .iter()
                .enumerate()
                .flat_map(|(row, line)| words(line, 40.0, 60.0 + row as f32 * 14.0, 10.0))
                .collect();
            PageText::new(glyphs)
        })
        .collect();
    let mut pdf = PdfTool::opening(None, PathBuf::from("/tmp/paper.pdf"));
    pdf.take_document(Document::of_text(vec![LETTER; pages.len()], texts));
    pdf.set_pane_size(PANE);
    pdf
}

/// The rows and columns [`PANE`] is painted as.
const GRID: (usize, usize) = (30, 60);

/// Paint `pdf` and tell it its pane, as the host does each frame.
fn painted(pdf: &mut PdfTool) -> Frame {
    let frame = pdf.content(GRID.0, GRID.1, false);
    pdf.set_pane_size(PANE);
    frame
}

/// A viewer over ten pages of the same few lines.
fn long_reader() -> PdfTool {
    let page: &[&str] = &["first line here", "second line", "third one"];
    reader(&[page; 10])
}

fn press(code: KeyCode) -> Key {
    Key::plain(code)
}

/// Type `keys` as plain presses, and hand back what the last one did.
fn typed(pdf: &mut PdfTool, keys: &str) -> ToolOutcome {
    let mut last = ToolOutcome::Ignored;
    for key in keys.chars() {
        last = pdf.on_key(&press(KeyCode::Char(key)));
    }
    last
}

fn answer(pdf: &mut PdfTool, tag: &'static str, pattern: Option<&str>) -> ToolOutcome {
    pdf.on_prompt(PromptReply {
        answer: pattern.map(str::to_string),
        tag,
    })
}

/// What copying `text` comes back as.
fn copied(text: impl Into<String>) -> ToolOutcome {
    copy(text.into())
}

fn page_of(pdf: &PdfTool) -> String {
    pdf.page_label()
}

#[test]
fn test_only_pdf_paths_are_claimed() {
    // The host routes on this, so a miss sends a PDF to the editor, which
    // refuses it as binary.
    assert!(PdfTool::handles(Path::new("a/paper.pdf")));
    assert!(PdfTool::handles(Path::new("a/PAPER.PDF")));
    assert!(!PdfTool::handles(Path::new("a/paper.pdf.gz")));
    assert!(!PdfTool::handles(Path::new("a/paper")));
}

#[test]
fn test_a_count_multiplies_the_motion_it_precedes_and_is_spent_by_it() {
    let mut pdf = long_reader();
    typed(&mut pdf, "3j");
    assert_eq!(pdf.scroll.1, 3.0 * LINE_PX, "three lines, not line three");
    typed(&mut pdf, "j");
    assert_eq!(pdf.scroll.1, 4.0 * LINE_PX, "the next motion runs once");
}

#[test]
fn test_a_leading_zero_is_a_key_and_not_a_count() {
    let mut pdf = long_reader();
    pdf.on_key(&press(KeyCode::Char('0')));
    assert_eq!(pdf.pending.count, None);
    typed(&mut pdf, "10j");
    assert_eq!(pdf.scroll.1, 10.0 * LINE_PX);
}

#[test]
fn test_g_reads_its_count_as_a_page_and_not_a_repeat() {
    let mut pdf = long_reader();
    typed(&mut pdf, "7gg");
    assert_eq!(page_of(&pdf), "page 7 / 10");
    typed(&mut pdf, "gg");
    assert_eq!(page_of(&pdf), "page 1 / 10");
    typed(&mut pdf, "5G");
    assert_eq!(page_of(&pdf), "page 5 / 10");
    typed(&mut pdf, "G");
    assert_eq!(page_of(&pdf), "page 10 / 10", "a bare G is the end");
}

#[test]
fn test_a_prefix_owns_the_key_after_it() {
    // The `b` of `zb` is not a screen back.
    let mut pdf = long_reader();
    typed(&mut pdf, "3G");
    let page = pdf.column().expect("laid out").pages[2];
    typed(&mut pdf, "zb");
    assert_eq!(
        pdf.scroll.1,
        page.y - (PANE.1 - page.height - layout::GAP),
        "the page's foot at the pane's"
    );
    typed(&mut pdf, "zt");
    assert_eq!(
        pdf.scroll.1,
        page.y - layout::GAP,
        "and its head at the top"
    );
}

#[test]
fn test_an_unfinished_prefix_is_abandoned_rather_than_left_pending() {
    let mut pdf = long_reader();
    typed(&mut pdf, "zx");
    assert_eq!(pdf.pending, Pending::default());
    typed(&mut pdf, "j");
    assert_eq!(pdf.scroll.1, LINE_PX);
}

#[test]
fn test_escape_abandons_a_half_typed_command() {
    let mut pdf = long_reader();
    typed(&mut pdf, "12");
    pdf.on_key(&press(KeyCode::Escape));
    assert_eq!(pdf.pending, Pending::default());
    typed(&mut pdf, "j");
    assert_eq!(pdf.scroll.1, LINE_PX);
}

#[test]
fn test_the_half_and_full_screen_scrolls_differ() {
    let mut pdf = long_reader();
    pdf.on_key(&Key::with_ctrl(KeyCode::Char('d')));
    assert_eq!(pdf.scroll.1, PANE.1 * 0.5);
    pdf.on_key(&Key::with_ctrl(KeyCode::Char('f')));
    assert_eq!(pdf.scroll.1, PANE.1 * 1.4);
}

#[test]
fn test_every_reading_key_moves_the_document() {
    for code in [
        KeyCode::Space,
        KeyCode::PageDown,
        KeyCode::PageUp,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::Left,
        KeyCode::Right,
    ] {
        let mut pdf = long_reader();
        assert_eq!(
            pdf.on_key(&press(code)),
            ToolOutcome::Consumed,
            "{code:?} must move the document"
        );
    }
}

#[test]
fn test_i_puts_a_cursor_in_the_text_and_escape_takes_it_out() {
    let mut pdf = long_reader();
    assert_eq!(pdf.status().mode, None, "scrolling is no mode");
    typed(&mut pdf, "i");
    assert_eq!(
        pdf.caret.pos,
        Some(Pos::new(0, 0, 0)),
        "the first line in view"
    );
    assert_eq!(pdf.status().mode, Some(EditMode::Normal));
    pdf.on_key(&press(KeyCode::Escape));
    assert_eq!(pdf.caret.pos, None);
}

#[test]
fn test_the_same_key_means_different_things_in_the_two_modes() {
    // `b` is a screen back with no cursor and a word back with one.
    let mut pdf = long_reader();
    typed(&mut pdf, "3j");
    typed(&mut pdf, "b");
    assert_eq!(pdf.scroll.1, 0.0, "a screen back, as far as the top");
    typed(&mut pdf, "i$");
    typed(&mut pdf, "b");
    assert_eq!(pdf.caret.pos, Some(Pos::new(0, 0, 11)), "back to `here`");
}

#[test]
fn test_a_count_reaches_a_cursor_motion() {
    let mut pdf = long_reader();
    typed(&mut pdf, "i3w");
    assert_eq!(
        pdf.caret.pos,
        Some(Pos::new(0, 1, 0)),
        "two words on, then the next line"
    );
}

#[test]
fn test_yank_composes_with_the_motion_that_follows_it() {
    let mut pdf = long_reader();
    typed(&mut pdf, "i");
    assert_eq!(
        typed(&mut pdf, "yw"),
        copied("first ".to_string())
    );
    assert_eq!(
        typed(&mut pdf, "2ye"),
        copied("first line".to_string())
    );
    assert_eq!(
        typed(&mut pdf, "y$"),
        copied("first line here".to_string())
    );
    assert_eq!(
        pdf.caret.pos,
        Some(Pos::new(0, 0, 0)),
        "a yank leaves the cursor"
    );
}

#[test]
fn test_yank_doubled_takes_whole_lines() {
    let mut pdf = long_reader();
    typed(&mut pdf, "iw");
    assert_eq!(
        typed(&mut pdf, "yy"),
        copied("first line here".to_string()),
        "the whole line, however far in the cursor is"
    );
    assert_eq!(
        typed(&mut pdf, "2yy"),
        copied("first line here\nsecond line".to_string())
    );
    assert_eq!(
        typed(&mut pdf, "Y"),
        copied("first line here".to_string()),
        "Y acts at once"
    );
}

#[test]
fn test_find_waits_for_its_character_and_composes_with_yank() {
    let mut pdf = long_reader();
    typed(&mut pdf, "i");
    typed(&mut pdf, "f");
    assert_eq!(
        pdf.caret.pos,
        Some(Pos::new(0, 0, 0)),
        "f alone is not a motion"
    );
    typed(&mut pdf, "l");
    assert_eq!(pdf.caret.pos, Some(Pos::new(0, 0, 6)));
    assert_eq!(
        typed(&mut pdf, "yth"),
        copied("line ".to_string()),
        "up to the h and not over it"
    );
}

#[test]
fn test_semicolon_and_comma_repeat_the_last_find_either_way() {
    let mut pdf = long_reader();
    typed(&mut pdf, "ifi");
    assert_eq!(pdf.caret.pos, Some(Pos::new(0, 0, 1)));
    typed(&mut pdf, ";");
    assert_eq!(pdf.caret.pos, Some(Pos::new(0, 0, 7)));
    typed(&mut pdf, ",");
    assert_eq!(pdf.caret.pos, Some(Pos::new(0, 0, 1)));
}

#[test]
fn test_a_search_asks_the_host_and_lands_on_the_match() {
    let mut pdf = long_reader();
    let asked = pdf.on_key(&press(KeyCode::Char('/')));
    let ToolOutcome::Prompt(request) = asked else {
        panic!("`/` asks what to look for, got {asked:?}");
    };
    assert_eq!(request.label, "/");
    assert_eq!(request.tag, ASK_SEARCH);
    answer(&mut pdf, ASK_SEARCH, Some("third"));
    assert_eq!(
        pdf.caret.pos,
        Some(Pos::new(0, 2, 0)),
        "a search leaves a cursor"
    );
    let report = pdf.take_search().expect("the host shows the query");
    assert_eq!((report.query.as_str(), report.reverse), ("third", false));
    assert_eq!(
        (report.index, report.total),
        (1, 10),
        "the first of the ten the document holds, one a page"
    );
}

#[test]
fn test_a_cancelled_search_puts_the_cursor_back() {
    let mut pdf = long_reader();
    typed(&mut pdf, "ij");
    pdf.on_key(&press(KeyCode::Char('?')));
    pdf.on_prompt_input("first", ASK_SEARCH_BACK);
    assert_eq!(
        pdf.caret.pos,
        Some(Pos::new(0, 0, 0)),
        "it searches as you type"
    );
    answer(&mut pdf, ASK_SEARCH_BACK, None);
    assert_eq!(pdf.caret.pos, Some(Pos::new(0, 1, 0)), "back where it was");
    assert_eq!(pdf.caret.found, None);
    assert!(
        pdf.search.is_none(),
        "and what was typed is not kept for `n`"
    );
}

#[test]
fn test_typing_at_the_prompt_searches_from_where_it_opened() {
    // Each letter searches again from the same place, so a longer query
    // refines the match rather than skipping past it.
    let mut pdf = reader(&[&["the theme then"]]);
    typed(&mut pdf, "i");
    pdf.on_key(&press(KeyCode::Char('/')));
    pdf.on_prompt_input("th", ASK_SEARCH);
    assert_eq!(pdf.caret.pos, Some(Pos::new(0, 0, 4)));
    pdf.on_prompt_input("then", ASK_SEARCH);
    assert_eq!(pdf.caret.pos, Some(Pos::new(0, 0, 10)));
}

#[test]
fn test_n_needs_a_search_to_repeat_and_then_goes_either_way() {
    let mut pdf = long_reader();
    typed(&mut pdf, "n");
    assert!(
        pdf.take_notice().is_some(),
        "nothing to repeat yet, and says so"
    );

    pdf.on_key(&press(KeyCode::Char('/')));
    answer(&mut pdf, ASK_SEARCH, Some("line"));
    assert_eq!(pdf.caret.pos, Some(Pos::new(0, 0, 6)));
    typed(&mut pdf, "n");
    assert_eq!(pdf.caret.pos, Some(Pos::new(0, 1, 7)));
    typed(&mut pdf, "2N");
    assert_eq!(
        pdf.caret.pos,
        Some(Pos::new(9, 1, 7)),
        "`N` goes the other way, round the start, and a count multiplies it"
    );
}

#[test]
fn test_a_count_and_an_operator_survive_the_question() {
    // `y2/line` copies up to the second match.
    let mut pdf = long_reader();
    typed(&mut pdf, "iy2");
    pdf.on_key(&press(KeyCode::Char('/')));
    let outcome = answer(&mut pdf, ASK_SEARCH, Some("line"));
    assert_eq!(outcome, copied("first line here\nsecond ".to_string()));
    assert_eq!(pdf.pending, Pending::default(), "and are spent by it");
}

#[test]
fn test_visual_selects_and_yank_takes_what_is_selected() {
    let mut pdf = long_reader();
    typed(&mut pdf, "iv");
    assert_eq!(pdf.status().mode, Some(EditMode::Visual));
    typed(&mut pdf, "e");
    assert_eq!(typed(&mut pdf, "y"), copied("first".to_string()));
    assert_eq!(pdf.caret.visual, Visual::None, "a yank ends the selection");
}

#[test]
fn test_the_same_visual_key_again_drops_the_selection_and_the_other_switches() {
    let mut pdf = long_reader();
    typed(&mut pdf, "iv");
    typed(&mut pdf, "V");
    assert_eq!(pdf.caret.visual, Visual::Line, "V switches the kind");
    typed(&mut pdf, "V");
    assert_eq!(pdf.caret.visual, Visual::None);
}

#[test]
fn test_escape_steps_back_one_thing_at_a_time() {
    let mut pdf = long_reader();
    typed(&mut pdf, "iv12");
    pdf.on_key(&press(KeyCode::Escape));
    assert_eq!(pdf.pending, Pending::default(), "the count goes first");
    assert_eq!(pdf.caret.visual, Visual::Char, "the selection survives it");
    pdf.on_key(&press(KeyCode::Escape));
    assert_eq!(pdf.caret.visual, Visual::None);
    assert!(pdf.caret.pos.is_some(), "the cursor survives it");
    pdf.on_key(&press(KeyCode::Escape));
    assert_eq!(pdf.caret.pos, None);
}

#[test]
fn test_the_prefixes_mean_the_cursor_once_there_is_one() {
    let mut pdf = long_reader();
    typed(&mut pdf, "iG");
    assert_eq!(
        pdf.caret.pos,
        Some(Pos::new(9, 2, 8)),
        "the end of the text"
    );
    typed(&mut pdf, "gg");
    assert_eq!(pdf.caret.pos, Some(Pos::new(0, 0, 0)));
    typed(&mut pdf, "6gg");
    assert_eq!(
        pdf.caret.pos,
        Some(Pos::new(5, 0, 0)),
        "page six's first line"
    );
}

#[test]
fn test_the_window_keys_still_work_with_a_cursor_in_the_text() {
    let mut pdf = long_reader();
    typed(&mut pdf, "i]");
    assert_eq!(page_of(&pdf), "page 2 / 10");
    pdf.on_key(&Key::with_ctrl(KeyCode::Char('d')));
    assert!(pdf.caret.pos.is_some(), "and leave the cursor where it is");
}

#[test]
fn test_zoom_in_and_fit_width_are_told_apart() {
    let mut pdf = long_reader();
    typed(&mut pdf, "+");
    assert!(pdf.scale() > 1.0);
    typed(&mut pdf, "=");
    assert_eq!(pdf.scale(), 1.0, "fitted to the width again");
}

#[test]
fn test_an_unbound_key_falls_through_to_the_host() {
    // The chords that switch panes and tabs have to survive a document.
    let mut pdf = long_reader();
    assert_eq!(typed(&mut pdf, "Z"), ToolOutcome::Ignored);
    assert_eq!(
        pdf.on_key(&Key::with_alt(KeyCode::Char('h'))),
        ToolOutcome::Ignored
    );
}

#[test]
fn test_a_pending_prefix_offers_its_own_key_hints() {
    let mut pdf = long_reader();
    assert!(pdf.hint().is_none(), "nothing is pending yet");
    typed(&mut pdf, "z");
    let hint = pdf.hint().expect("z is waiting for its second key");
    let keys: Vec<String> = hint.items.into_iter().map(|(key, _)| key).collect();
    assert_eq!(keys, vec!["t", "z", "b"]);
}

#[test]
fn test_the_status_stays_quiet_until_the_document_has_opened() {
    let pdf = PdfTool::opening(None, PathBuf::from("/tmp/paper.pdf"));
    assert!(pdf.status().segments.is_empty(), "nothing is known yet");
}

#[test]
fn test_the_status_bar_says_where_the_reader_is_and_what_is_selecting() {
    let mut pdf = long_reader();
    assert_eq!(pdf.status().segments, ["page 1 / 10"]);
    typed(&mut pdf, "ijV2");
    let status = pdf.status();
    assert_eq!(status.mode, Some(EditMode::Visual));
    assert_eq!(status.segments, ["page 1 / 10", "line 2", "2"]);
}

#[test]
fn test_asking_where_you_are_names_the_file_page_and_progress() {
    let mut pdf = long_reader();
    pdf.on_key(&Key::with_ctrl(KeyCode::Char('g')));
    assert_eq!(
        pdf.take_notice().map(|notice| notice.text).as_deref(),
        Some("paper.pdf, page 1 of 10, 0%")
    );
}

#[test]
fn test_every_key_of_the_viewer_is_spelled_right() {
    for keys in [COMMON_KEYS, NORMAL_KEYS, WINDOW_KEYS, CURSOR_KEYS] {
        assert_eq!(table::invalid_specs(keys), Vec::<&str>::new());
    }
}

#[test]
fn test_a_restart_brings_the_document_back_at_the_page_it_was_on() {
    let mut pdf = long_reader();
    typed(&mut pdf, "7G");
    let saved = pdf.snapshot().expect("a PDF is saved with the session");
    assert_eq!(saved.path, PathBuf::from("/tmp/paper.pdf"));
    assert_eq!(saved.line, Some(7));

    let mut back = PdfTool {
        resume_page: saved.line,
        ..PdfTool::opening(None, saved.path)
    };
    assert_eq!(
        back.snapshot().and_then(|at| at.line),
        Some(7),
        "saved again before it opens, it keeps the page"
    );
    back.set_pane_size(PANE);
    let page: &[&str] = &["text"];
    back.take_document(Document::of_text(
        vec![LETTER; 10],
        vec![PageText::new(words(page[0], 40.0, 60.0, 10.0)); 10],
    ));
    assert_eq!(page_of(&back), "page 7 / 10");
}

#[test]
fn test_a_click_puts_the_cursor_on_the_character_and_a_drag_selects() {
    let mut pdf = long_reader();
    painted(&mut pdf);
    // Cells are 10.6 pixels across and 20 down in this pane, and the pages
    // start under the toolbar's row. The page stands a gap in from its
    // edges, its first line's letters 5 points each from 40 points in, on a
    // baseline 60 points down.
    let cell = |col: usize, row: usize, drag: bool| Point { col, drag, row };
    pdf.on_mouse(cell(5, 4, false));
    assert_eq!(pdf.caret.pos, Some(Pos::new(0, 0, 1)));
    let ToolOutcome::Copy { notice, text } = pdf.on_mouse(cell(7, 4, true)) else {
        panic!("a drag copies what it selects");
    };
    assert_eq!((notice.as_str(), text.as_str()), ("Copied 5 characters", "irst "));
    assert_eq!(pdf.caret.visual, Visual::Char, "and leaves it selected");
    assert_eq!(typed(&mut pdf, "y"), copied("irst ".to_string()));
}

#[test]
fn test_the_cursor_and_what_is_selected_are_drawn_over_the_page() {
    let mut pdf = long_reader();
    let blank = pdf.tiles().len();
    typed(&mut pdf, "ive");
    let tiles = pdf.tiles();
    assert_eq!(tiles.len(), blank + 2, "the selection and the cursor's bar");
    let bar = tiles.last().expect("the bar is drawn last, over the rest");
    assert_eq!(bar.width, CARET_WIDTH);
}

/// The toolbar's text as last painted.
fn bar_text(pdf: &PdfTool) -> String {
    pdf.bar.row.iter().map(|span| span.text.as_str()).collect()
}

/// A click on the toolbar's first column showing `label`.
fn click_bar(pdf: &mut PdfTool, label: &str) -> ToolOutcome {
    let text = bar_text(pdf);
    let byte = text.find(label).unwrap_or_else(|| panic!("{label} in {text}"));
    let col = text[..byte].chars().count();
    pdf.on_mouse(Point {
        col,
        drag: false,
        row: 0,
    })
}

/// [`long_reader`] with an outline of three sections, the second with two
/// under it, painted once.
fn outlined_reader() -> PdfTool {
    use outline_list::tests::entry;
    let mut pdf = long_reader();
    let doc = pdf.document.take().expect("the document");
    pdf.take_document(doc.with_outline(vec![
        entry(0, "One", 0, 0.0),
        entry(0, "Two", 3, 0.0),
        entry(1, "Two A", 4, 0.0),
        entry(1, "Two B", 5, 0.0),
        entry(0, "Three", 8, 0.0),
    ]));
    painted(&mut pdf);
    pdf
}

#[test]
fn test_the_toolbar_takes_the_top_row_and_the_pages_start_under_it() {
    let mut pdf = long_reader();
    let frame = painted(&mut pdf);
    assert_eq!(frame.rows.len(), 1);
    assert!(bar_text(&pdf).contains("1 / 10"), "{}", bar_text(&pdf));
    assert!(bar_text(&pdf).contains("No outline"));
    assert_eq!(pdf.header_rows(), 1);
    let cell_height = PANE.1 / GRID.0 as f32;
    let first = &pdf.tiles()[0];
    assert_eq!(first.y, cell_height + layout::GAP, "a gap under the bar");
}

#[test]
fn test_the_page_buttons_step_and_the_zoom_buttons_zoom() {
    let mut pdf = long_reader();
    painted(&mut pdf);
    click_bar(&mut pdf, "›");
    assert_eq!(page_of(&pdf), "page 2 / 10");
    click_bar(&mut pdf, "‹");
    assert_eq!(page_of(&pdf), "page 1 / 10");
    let before = pdf.scale();
    click_bar(&mut pdf, "−");
    assert!((pdf.scale() - before / ZOOM_STEP).abs() < 1e-4);
    painted(&mut pdf);
    click_bar(&mut pdf, " + ");
    assert!((pdf.scale() - before).abs() < 1e-4);
}

#[test]
fn test_a_drag_onto_the_toolbar_presses_nothing() {
    let mut pdf = long_reader();
    painted(&mut pdf);
    let col = bar_text(&pdf).chars().position(|ch| ch == '›').unwrap();
    pdf.on_mouse(Point { col, drag: true, row: 0 });
    assert_eq!(page_of(&pdf), "page 1 / 10");
}

#[test]
fn test_the_zoom_asks_for_a_percentage_or_a_fit() {
    let mut pdf = long_reader();
    painted(&mut pdf);
    let ToolOutcome::Prompt(ask) = click_bar(&mut pdf, "%") else {
        panic!("the zoom asks");
    };
    assert_eq!(ask.tag, ASK_ZOOM);
    answer(&mut pdf, ASK_ZOOM, Some("150%"));
    assert_eq!(pdf.zoom, Zoom::Scale(1.5));
    answer(&mut pdf, ASK_ZOOM, Some("p"));
    assert_eq!(pdf.zoom, Zoom::FitPage);
    assert!(pdf.scale() < layout::fit_width(&[LETTER], pdf.pane.0), "the height binds");
    answer(&mut pdf, ASK_ZOOM, Some("height"));
    assert_eq!(pdf.zoom, Zoom::FitHeight);
    answer(&mut pdf, ASK_ZOOM, Some("huge"));
    assert_eq!(pdf.zoom, Zoom::FitHeight, "what reads as no zoom changes none");
}

#[test]
fn test_colon_asks_for_a_page_and_goes_there() {
    let mut pdf = long_reader();
    painted(&mut pdf);
    let ToolOutcome::Prompt(ask) = typed(&mut pdf, ":") else {
        panic!("the page is asked for");
    };
    assert_eq!((ask.tag, ask.initial.as_str()), (ASK_PAGE, "1"));
    answer(&mut pdf, ASK_PAGE, Some("5"));
    assert_eq!(page_of(&pdf), "page 5 / 10");
    answer(&mut pdf, ASK_PAGE, Some("five"));
    assert_eq!(page_of(&pdf), "page 5 / 10", "no page, no jump");
}

#[test]
fn test_the_toolbar_names_the_section_being_read() {
    let mut pdf = outlined_reader();
    assert!(bar_text(&pdf).contains("One"), "{}", bar_text(&pdf));
    typed(&mut pdf, "5gg");
    painted(&mut pdf);
    assert!(bar_text(&pdf).contains("Two A"), "{}", bar_text(&pdf));
}

#[test]
fn test_the_outline_opens_under_the_toolbar_and_jumps_to_a_section() {
    let mut pdf = outlined_reader();
    typed(&mut pdf, "O");
    let frame = painted(&mut pdf);
    assert_eq!(frame.rows.len(), 1 + 3, "folded to the top level");
    assert_eq!(pdf.header_rows(), 4, "the pages are cut away under it");
    assert_eq!(frame.cursor_line, Some(1), "on the section being read");
    // Down to Two, unfolded, into Two A, and there.
    typed(&mut pdf, "jll");
    pdf.on_key(&press(KeyCode::Enter));
    assert!(pdf.outline.is_none(), "a jump closes it");
    assert_eq!(page_of(&pdf), "page 5 / 10");
    painted(&mut pdf);
    assert_eq!(pdf.header_rows(), 1);
}

#[test]
fn test_a_click_on_the_section_toggles_the_outline_and_one_on_a_row_jumps() {
    let mut pdf = outlined_reader();
    click_bar(&mut pdf, "One");
    painted(&mut pdf);
    assert!(pdf.outline.is_some());
    // The third row of the list is Three.
    pdf.on_mouse(Point { col: 10, drag: false, row: 3 });
    assert_eq!(page_of(&pdf), "page 9 / 10");
    assert!(pdf.outline.is_none());
}

#[test]
fn test_escape_closes_the_outline_and_keeps_the_keys_from_the_pages() {
    let mut pdf = outlined_reader();
    typed(&mut pdf, "O");
    typed(&mut pdf, "j");
    assert_eq!(pdf.scroll.1, 0.0, "j moved the outline's cursor, not the page");
    pdf.on_key(&press(KeyCode::Escape));
    assert!(pdf.outline.is_none());
}

#[test]
fn test_inverting_draws_the_pages_again_inverted() {
    let mut pdf = long_reader();
    painted(&mut pdf);
    let generation = pdf.generation;
    click_bar(&mut pdf, "◐");
    assert!(pdf.inverted);
    assert_eq!(pdf.generation, generation + 1, "drawn again");
    assert_eq!(
        pdf.tiles()[0].key,
        pdf.overlays.blank_inverted.key,
        "a page not drawn yet is dark"
    );
    typed(&mut pdf, "I");
    assert!(!pdf.inverted);
}

/// The labels of `pdf`'s menu, separators left out.
fn menu_labels(pdf: &PdfTool) -> Vec<String> {
    pdf.context_items()
        .into_iter()
        .filter(|item| !item.is_separator())
        .map(|item| item.label)
        .collect()
}

#[test]
fn test_the_menu_offers_copy_only_over_a_selection_and_a_right_click_keeps_it() {
    let mut pdf = long_reader();
    painted(&mut pdf);
    assert_eq!(
        menu_labels(&pdf),
        ["Find", "Go to Page", "Zoom In", "Zoom Out", "Fit Width", "Invert Colors"],
        "no outline to open, nothing to copy"
    );
    typed(&mut pdf, "ive");
    pdf.on_context_mouse(Point { col: 40, drag: false, row: 20 });
    assert!(pdf.caret.selection().is_some(), "the right-click moved nothing");
    let items = pdf.context_items();
    assert_eq!(items[0].label, "Copy");
    assert!(items[1].is_separator());
    let mut last = ToolOutcome::Ignored;
    for key in &items[0].keys {
        last = pdf.on_key(key);
    }
    assert_eq!(last, copied("first"));
}

#[test]
fn test_the_menu_opens_the_outline_when_there_is_one() {
    let mut pdf = outlined_reader();
    assert!(menu_labels(&pdf).contains(&"Outline".to_string()));
    let item = pdf
        .context_items()
        .into_iter()
        .find(|item| item.label == "Outline")
        .unwrap();
    pdf.on_key(&item.keys[0]);
    assert!(pdf.outline.is_some());
}

#[test]
fn test_every_pdf_menu_entry_is_a_key_the_viewer_answers() {
    let pdf = outlined_reader();
    for item in pdf.context_items().into_iter().filter(|item| !item.is_separator()) {
        let mut probe = outlined_reader();
        assert_ne!(probe.on_key(&item.keys[0]), ToolOutcome::Ignored, "{}", item.label);
    }
}
