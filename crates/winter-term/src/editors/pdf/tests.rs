//! The PDF viewer's tests, over documents made of text alone: the keys'
//! grammar, and what each command does to the view and the cursor.

use crate::model::editor::{CommandOutput, Editor, JobReply, JobRequest, PromptReply};
use crate::model::input::table;
use crate::model::input::{format_key, KeyCode};
use crate::model::paint::{Frame, Point};
use crate::model::textedit::modal::EditMode;

use super::caret::Pos;
use super::keys::{COMMON_KEYS, CURSOR_KEYS, NORMAL_KEYS, WINDOW_KEYS};
use super::links::LinkTo;
use super::text::tests::words;
use super::text::PageText;
use super::*;
use crate::editors::synctex;

/// A US Letter page, in points.
const LETTER: (f32, f32) = (612.0, 792.0);

/// A pane that fits a letter page's width at scale one, and shows most of
/// one page's height.
const PANE: (f32, f32) = (636.0, 600.0);

/// A viewer over `pages`, each a list of lines set down the page from the
/// top, laid out in [`PANE`].
fn reader(pages: &[&[&str]]) -> PdfEditor {
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
    let mut pdf = PdfEditor::opening(None, PathBuf::from("/tmp/paper.pdf"));
    pdf.take_document(Document::of_text(vec![LETTER; pages.len()], texts));
    pdf.set_pane_size(PANE);
    pdf
}

/// The rows and columns [`PANE`] is painted as.
const GRID: (usize, usize) = (30, 60);

/// Paint `pdf` and tell it its pane, as the host does each frame.
fn painted(pdf: &mut PdfEditor) -> Frame {
    let frame = pdf.content(GRID.0, GRID.1, false);
    pdf.set_pane_size(PANE);
    frame
}

/// A viewer over ten pages of the same few lines.
fn long_reader() -> PdfEditor {
    let page: &[&str] = &["first line here", "second line", "third one"];
    reader(&[page; 10])
}

fn press(code: KeyCode) -> Key {
    Key::plain(code)
}

/// Type `keys` as plain presses, and hand back what the last one did.
fn typed(pdf: &mut PdfEditor, keys: &str) -> EditorOutcome {
    let mut last = EditorOutcome::Ignored;
    for key in keys.chars() {
        last = pdf.on_key(&press(KeyCode::Char(key)));
    }
    last
}

fn answer(pdf: &mut PdfEditor, tag: &'static str, pattern: Option<&str>) -> EditorOutcome {
    pdf.on_prompt(PromptReply {
        answer: pattern.map(str::to_string),
        tag,
    })
}

/// What copying `text` comes back as.
fn copied(text: impl Into<String>) -> EditorOutcome {
    EditorOutcome::copied(text.into())
}

fn page_of(pdf: &PdfEditor) -> String {
    pdf.page_label()
}

#[test]
fn test_only_pdf_paths_are_claimed() {
    // The host routes on this, so a miss sends a PDF to the editor, which
    // refuses it as binary.
    assert!(PdfEditor::handles(Path::new("a/paper.pdf")));
    assert!(PdfEditor::handles(Path::new("a/PAPER.PDF")));
    assert!(!PdfEditor::handles(Path::new("a/paper.pdf.gz")));
    assert!(!PdfEditor::handles(Path::new("a/paper")));
}

/// Where the view lands, once the glide on its way there has finished.
fn landed(pdf: &mut PdfEditor) -> f32 {
    pdf.step_glide(Instant::now() + GLIDE);
    pdf.scroll.1
}

#[test]
fn test_a_tab_s_view_comes_back_where_it_was_after_another_tab_moved_on() {
    // Two tabs on one document: the window keeps each one's view while the
    // other's is loaded.
    let mut pdf = long_reader();
    painted(&mut pdf);
    let start = landed(&mut pdf);
    let first = pdf.save_view().expect("a view to keep");
    typed(&mut pdf, "G");
    assert_ne!(landed(&mut pdf), start, "the other tab went to the end");
    pdf.restore_view(first);
    assert_eq!(landed(&mut pdf), start, "and this one is where it was");
}

#[test]
fn test_a_count_multiplies_the_motion_it_precedes_and_is_spent_by_it() {
    let mut pdf = long_reader();
    typed(&mut pdf, "3j");
    assert_eq!(
        landed(&mut pdf),
        3.0 * LINE_PX,
        "three lines, not line three"
    );
    typed(&mut pdf, "j");
    assert_eq!(landed(&mut pdf), 4.0 * LINE_PX, "the next motion runs once");
}

#[test]
fn test_a_leading_zero_is_a_key_and_not_a_count() {
    let mut pdf = long_reader();
    pdf.on_key(&press(KeyCode::Char('0')));
    assert_eq!(pdf.pending.count, None);
    typed(&mut pdf, "10j");
    assert_eq!(landed(&mut pdf), 10.0 * LINE_PX);
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
    assert_eq!(landed(&mut pdf), LINE_PX);
}

#[test]
fn test_escape_abandons_a_half_typed_command() {
    let mut pdf = long_reader();
    typed(&mut pdf, "12");
    pdf.on_key(&press(KeyCode::Escape));
    assert_eq!(pdf.pending, Pending::default());
    typed(&mut pdf, "j");
    assert_eq!(landed(&mut pdf), LINE_PX);
}

#[test]
fn test_the_half_and_full_screen_scrolls_differ() {
    let mut pdf = long_reader();
    pdf.on_key(&Key::with_ctrl(KeyCode::Char('d')));
    assert_eq!(landed(&mut pdf), PANE.1 * 0.5);
    pdf.on_key(&Key::with_ctrl(KeyCode::Char('f')));
    assert_eq!(landed(&mut pdf), PANE.1 * 1.4);
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
            EditorOutcome::Consumed,
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
    assert_eq!(typed(&mut pdf, "yw"), copied("first ".to_string()));
    assert_eq!(typed(&mut pdf, "2ye"), copied("first line".to_string()));
    assert_eq!(typed(&mut pdf, "y$"), copied("first line here".to_string()));
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
    let EditorOutcome::Prompt(request) = asked else {
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
fn test_p_fits_the_page_and_t_fits_the_text_s_width_from_its_left() {
    let mut pdf = long_reader();
    typed(&mut pdf, "P");
    assert_eq!(pdf.zoom, Zoom::FitPage);
    assert!(pdf.scale() < 1.0, "a letter page is taller than the pane");
    typed(&mut pdf, "T");
    assert_eq!(pdf.zoom, Zoom::FitContent);
    let (left, _, width, _) = pdf.text_box.expect("measured");
    assert!(pdf.scale() > 1.0, "the text is narrower than the page");
    let column = pdf.column().expect("laid out");
    let text_left = column.pages[0].origin().0 + left * pdf.scale() - pdf.scroll.0;
    assert_eq!(text_left.round(), layout::GAP, "the margin is off the side");
    assert!((width * pdf.scale() - (PANE.0 - 2.0 * layout::GAP)).abs() < 1.0);
}

#[test]
fn test_c_crops_the_pages_to_their_text_and_back_keeping_the_page_read() {
    let mut pdf = long_reader();
    typed(&mut pdf, "5gg");
    let whole = pdf.column().expect("laid out").pages[0];
    typed(&mut pdf, "C");
    let (_, _, width, height) = pdf.crop().expect("cropped");
    assert!(width < LETTER.0 && height < LETTER.1, "the margins are cut off");
    let cut = pdf.column().expect("laid out").pages[0];
    assert_eq!((cut.width, cut.height), (width * pdf.scale(), height * pdf.scale()));
    assert!(pdf.scale() > 1.0, "fitted to the width, what is left is drawn larger");
    assert_eq!(page_of(&pdf), "page 5 / 10", "the page read stays in view");
    typed(&mut pdf, "C");
    assert!(pdf.crop().is_none());
    assert_eq!(pdf.column().expect("laid out").pages[0], whole);
    assert_eq!(page_of(&pdf), "page 5 / 10");
}

#[test]
fn test_e_centers_a_page_wider_than_the_pane() {
    let mut pdf = long_reader();
    typed(&mut pdf, "++");
    typed(&mut pdf, "E");
    let column = pdf.column().expect("laid out");
    assert_eq!(pdf.scroll.0, (column.width - PANE.0) / 2.0);
}

#[test]
fn test_an_unbound_key_falls_through_to_the_host() {
    // The chords that switch panes and tabs have to survive a document.
    let mut pdf = long_reader();
    assert_eq!(typed(&mut pdf, "Z"), EditorOutcome::Ignored);
    assert_eq!(
        pdf.on_key(&Key::with_alt(KeyCode::Char('h'))),
        EditorOutcome::Ignored
    );
}

#[test]
fn test_a_pending_prefix_offers_its_own_key_hints() {
    let mut pdf = long_reader();
    assert!(pdf.hint().is_none(), "nothing is pending yet");
    typed(&mut pdf, "z");
    let hint = pdf.hint().expect("z is waiting for its second key");
    let keys: Vec<String> = hint.items.into_iter().map(|(key, _)| key).collect();
    assert_eq!(keys, vec!["t", "z", "b", "g"]);
}

#[test]
fn test_the_status_stays_quiet_until_the_document_has_opened() {
    let pdf = PdfEditor::opening(None, PathBuf::from("/tmp/paper.pdf"));
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
fn test_g_c_g_counts_the_words_selected_or_with_none_the_document_s() {
    let mut pdf = long_reader();
    let count = |pdf: &mut PdfEditor| {
        typed(pdf, "g");
        pdf.on_key(&Key::with_ctrl(KeyCode::Char('g')));
        pdf.take_notice().map(|notice| notice.text)
    };
    // Ten pages of "first line here", "second line", "third one".
    assert_eq!(count(&mut pdf).as_deref(), Some("70 words, 10 pages"));
    // The first word, `first`, selected.
    typed(&mut pdf, "ive");
    assert_eq!(count(&mut pdf).as_deref(), Some("1 word, 5 characters selected"));
}

#[test]
fn test_m_s_s_tints_each_sentence_in_view_and_again_takes_the_tints_off() {
    let mut pdf = reader(&[&["One here. Two runs", "on over a line."]]);
    let tint = Key {
        alt: true,
        shift: true,
        ..press(KeyCode::Char('s'))
    };
    let column = pdf.column().expect("laid out");
    assert!(pdf.sentence_tiles(&column).is_empty(), "none until asked");
    pdf.on_key(&tint);
    // One tint for the first sentence, one each for the two lines the
    // second covers, in the next color.
    let tiles = pdf.sentence_tiles(&column);
    assert_eq!(tiles.len(), 3);
    assert_ne!(tiles[0].key, tiles[1].key);
    assert_eq!(tiles[1].key, tiles[2].key);
    pdf.on_key(&tint);
    assert!(pdf.sentence_tiles(&column).is_empty());
}

#[test]
fn test_v_shows_the_pages_small_down_the_left_and_a_click_on_one_goes_there() {
    let mut pdf = long_reader();
    painted(&mut pdf);
    let whole = pdf.pane.0;
    typed(&mut pdf, "V");
    painted(&mut pdf);
    let strip = pdf.thumbs_width();
    assert!(strip >= THUMB_MIN_WIDTH);
    assert_eq!(pdf.pane.0, whole - strip, "the pages are laid out right of it");
    // A thumbnail for each page in the strip, and the frame's four sides
    // round the one being read.
    let (column, scroll) = pdf.thumbs_column().expect("laid out");
    let shown = column.visible(scroll, pdf.pane.1).len();
    assert_eq!(pdf.thumb_tiles().len(), shown + 4);
    // The cell over the middle of page two's thumbnail.
    let rect = column.pages[1];
    let row = (rect.y + rect.height / 2.0 - scroll + pdf.toolbar_height()) / pdf.cell.1;
    let at = Point {
        col: 1,
        drag: false,
        row: row as usize,
    };
    assert_eq!(pdf.on_mouse(at), EditorOutcome::Consumed);
    assert_eq!(page_of(&pdf), "page 2 / 10");
    typed(&mut pdf, "V");
    painted(&mut pdf);
    assert_eq!(pdf.pane.0, whole);
    assert!(pdf.thumb_tiles().is_empty());
}

/// Ctrl, Alt, and `c`.
fn ctrl_alt(c: char) -> Key {
    Key {
        alt: true,
        ctrl: true,
        ..press(KeyCode::Char(c))
    }
}

#[test]
fn test_misspelled_words_are_underlined_and_stepped_through_round_the_end() {
    let mut pdf = reader(&[&["We recieve the letter."], &["And teh reply."]]);
    painted(&mut pdf);
    let column = pdf.column().expect("laid out");
    assert!(pdf.spelling_tiles(&column).is_empty(), "none until asked");
    pdf.on_key(&Key {
        alt: true,
        shift: true,
        ..press(KeyCode::Char('i'))
    });
    painted(&mut pdf);
    assert!(!pdf.spelling_tiles(&column).is_empty(), "the first page's in view");
    let said = |pdf: &mut PdfEditor| pdf.take_notice().map(|notice| notice.text).unwrap_or_default();
    pdf.on_key(&ctrl_alt('n'));
    let first = said(&mut pdf);
    assert!(first.starts_with("recieve: ") && first.contains("receive"), "{first}");
    assert_eq!(pdf.mark.map(|mark| mark.page), Some(0), "marked where it is");
    pdf.on_key(&ctrl_alt('n'));
    assert!(said(&mut pdf).starts_with("teh: "));
    assert_eq!(page_of(&pdf), "page 2 / 2", "brought into view");
    pdf.on_key(&ctrl_alt('n'));
    assert!(said(&mut pdf).starts_with("recieve: "), "round past the end");
    pdf.on_key(&ctrl_alt('p'));
    assert!(said(&mut pdf).starts_with("teh: "), "and back");
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

    let mut back = PdfEditor {
        resume_page: saved.line,
        ..PdfEditor::opening(None, saved.path)
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
fn test_a_click_puts_no_cursor_down_but_a_drag_selects() {
    let mut pdf = long_reader();
    painted(&mut pdf);
    // Cells are 10.6 pixels across and 20 down in this pane, and the pages
    // start under the toolbar's row. The page stands a gap in from its
    // edges, its first line's letters 5 points each from 40 points in, on a
    // baseline 60 points down.
    let cell = |col: usize, row: usize, drag: bool| Point { col, drag, row };
    pdf.on_mouse(cell(5, 4, false));
    assert_eq!(pdf.caret.shown(), None, "the cursor is only asked for with i");
    assert_eq!(pdf.mode(), Mode::Normal);
    let EditorOutcome::Copy { notice, text } = pdf.on_mouse(cell(7, 4, true)) else {
        panic!("a drag copies what it selects");
    };
    assert_eq!(
        (notice.as_str(), text.as_str()),
        ("Copied 5 characters", "irst ")
    );
    assert_eq!(pdf.caret.visual, Visual::Char, "and leaves it selected");
    assert_eq!(pdf.caret.shown(), None, "still with no cursor drawn");
    let dragged_to = pdf.caret.pos;
    typed(&mut pdf, "i");
    assert_eq!(pdf.caret.shown(), dragged_to, "i starts it where the drag ended");
    pdf.on_mouse(cell(5, 4, false));
    assert_eq!(pdf.caret.shown(), Some(Pos::new(0, 0, 1)), "with the cursor on, a click moves it");
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

/// The toolbar's items as the window draws them, each set off by a space.
fn bar_text(pdf: &PdfEditor) -> String {
    let bar = pdf.toolbar().expect("a toolbar");
    let texts: Vec<String> = bar.left.iter().chain(&bar.right).map(|item| item.text.clone()).collect();
    texts.join(" ")
}

/// A press on the toolbar button saying `label`, as the window reports one.
fn click_bar(pdf: &mut PdfEditor, label: &str) -> EditorOutcome {
    let bar = pdf.toolbar().expect("a toolbar");
    let button = bar
        .left
        .iter()
        .chain(&bar.right)
        .find(|item| item.text.contains(label))
        .and_then(|item| item.button)
        .unwrap_or_else(|| panic!("a button saying {label}"));
    pdf.on_toolbar(button)
}

/// [`long_reader`] with an outline of three sections, the second with two
/// under it, painted once.
fn outlined_reader() -> PdfEditor {
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
    click_bar(&mut pdf, "+");
    assert!((pdf.scale() - before).abs() < 1e-4);
}

#[test]
fn test_the_toolbar_row_answers_no_click_of_its_own() {
    // The window tells a press on a button by where it drew it, and hands
    // the viewer the button; a click or a drag on the row itself does nothing.
    let mut pdf = long_reader();
    painted(&mut pdf);
    for drag in [false, true] {
        let at = Point { col: 70, drag, row: 0 };
        assert_eq!(pdf.on_mouse(at), EditorOutcome::Consumed);
    }
    assert_eq!(page_of(&pdf), "page 1 / 10");
    let frame = painted(&mut pdf);
    assert!(frame.rows[0].is_empty(), "the row is left blank for the bar");
}

#[test]
fn test_the_zoom_asks_for_a_percentage_or_a_fit() {
    let mut pdf = long_reader();
    painted(&mut pdf);
    let EditorOutcome::Prompt(ask) = click_bar(&mut pdf, "%") else {
        panic!("the zoom asks");
    };
    assert_eq!(ask.tag, ASK_ZOOM);
    answer(&mut pdf, ASK_ZOOM, Some("150%"));
    assert_eq!(pdf.zoom, Zoom::Scale(1.5));
    answer(&mut pdf, ASK_ZOOM, Some("p"));
    assert_eq!(pdf.zoom, Zoom::FitPage);
    assert!(
        pdf.scale() < layout::fit_width(&[LETTER], pdf.pane.0),
        "the height binds"
    );
    answer(&mut pdf, ASK_ZOOM, Some("height"));
    assert_eq!(pdf.zoom, Zoom::FitHeight);
    answer(&mut pdf, ASK_ZOOM, Some("huge"));
    assert_eq!(
        pdf.zoom,
        Zoom::FitHeight,
        "what reads as no zoom changes none"
    );
}

#[test]
fn test_colon_asks_for_a_page_and_goes_there() {
    let mut pdf = long_reader();
    painted(&mut pdf);
    let EditorOutcome::Prompt(ask) = typed(&mut pdf, ":") else {
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
    painted(&mut pdf);
    let list = pdf.popups().drop_list.expect("a drop-down under the toolbar");
    assert_eq!(list.rows.len(), 3, "folded to the top level");
    assert_eq!(list.cell.0, 1, "under the toolbar");
    assert_eq!(pdf.header_rows(), 1, "it drops down over the pages, which stay");
    assert_eq!(list.selected, Some(0), "on the section being read");
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
    pdf.on_mouse(Point {
        col: 10,
        drag: false,
        row: 3,
    });
    assert_eq!(page_of(&pdf), "page 9 / 10");
    assert!(pdf.outline.is_none());
}

#[test]
fn test_a_click_on_the_pages_beside_the_outline_closes_it_and_goes_nowhere() {
    let mut pdf = outlined_reader();
    click_bar(&mut pdf, "One");
    painted(&mut pdf);
    let list = pdf.popups().drop_list.expect("open");
    let before = page_of(&pdf);
    // The row of Three, but right of the card.
    pdf.on_mouse(Point {
        col: list.cell.1 + list.cols + 2,
        drag: false,
        row: 3,
    });
    assert!(pdf.outline.is_none());
    assert_eq!(page_of(&pdf), before);
}

#[test]
fn test_escape_closes_the_outline_and_keeps_the_keys_from_the_pages() {
    let mut pdf = outlined_reader();
    typed(&mut pdf, "O");
    typed(&mut pdf, "j");
    assert_eq!(
        pdf.scroll.1, 0.0,
        "j moved the outline's cursor, not the page"
    );
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
fn menu_labels(pdf: &PdfEditor) -> Vec<String> {
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
        [
            "Find",
            "Go to Page",
            "Zoom In",
            "Zoom Out",
            "Fit Width",
            "Invert Colors",
            "Show Links"
        ],
        "no outline to open, nothing to copy"
    );
    typed(&mut pdf, "ive");
    pdf.on_context_mouse(Point {
        col: 40,
        drag: false,
        row: 20,
    });
    assert!(
        pdf.caret.selection().is_some(),
        "the right-click moved nothing"
    );
    let items = pdf.context_items();
    assert_eq!(items[0].label, "Copy");
    assert!(items[1].is_separator());
    let mut last = EditorOutcome::Ignored;
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
    for item in pdf
        .context_items()
        .into_iter()
        .filter(|item| !item.is_separator())
    {
        let mut probe = outlined_reader();
        assert_ne!(
            probe.on_key(&item.keys[0]),
            EditorOutcome::Ignored,
            "{}",
            item.label
        );
    }
}

/// What `synctex edit` says when it names `input` at `line`.
fn synctex_said(input: &str, line: usize) -> JobReply {
    JobReply::Command(CommandOutput {
        id: 0,
        code: Some(0),
        stderr: String::new(),
        stdout: format!("SyncTeX result begin\nInput:{input}\nLine:{line}\nColumn:-1\n"),
        tag: synctex::TAG_EDIT,
    })
}

#[test]
fn test_synctex_backward_narrows_the_line_by_the_text_around_the_cursor() {
    let mut pdf = reader(&[&["We prove the theorem here", "second line"]]);
    // A cursor on "prove", then back to the source from there.
    let EditorOutcome::Job(JobRequest::Command(request)) = typed(&mut pdf, "iwS") else {
        panic!("S runs synctex");
    };
    assert_eq!(request.tag, synctex::TAG_EDIT);
    assert!(request.args[2].starts_with("1:") && request.args[2].ends_with(":/tmp/paper.pdf"));

    // SyncTeX names the paragraph's start; the text is three lines down.
    let read = pdf.on_job(synctex_said("/tmp/./paper.tex", 2));
    let source = PathBuf::from("/tmp/paper.tex");
    assert_eq!(
        read,
        EditorOutcome::Job(JobRequest::ReadFiles(vec![source.clone()]))
    );
    let content = "\\begin{document}\n\\section{A}\nIntro.\n\nWe prove the theorem here\n";
    assert_eq!(
        pdf.on_job(JobReply::Files(vec![(source.clone(), content.to_string())])),
        EditorOutcome::OpenBeside(OpenTarget::at_line(source, 5))
    );
}

#[test]
fn test_s_labels_each_line_in_view_and_a_label_goes_back_from_its_start() {
    let mut pdf = reader(&[&["We prove the theorem here", "second line"]]);
    painted(&mut pdf);
    typed(&mut pdf, "s");
    let hints = pdf.hints.as_ref().expect("labels up");
    assert_eq!(hints.links.len(), 2, "one a line");
    let second = Pos {
        char: 0,
        line: 1,
        page: 0,
    };
    let (spot, _) = pdf.spot_and_context(second).expect("text there");
    let EditorOutcome::Job(JobRequest::Command(request)) = typed(&mut pdf, "s") else {
        panic!("a label runs synctex");
    };
    assert_eq!(request.tag, synctex::TAG_EDIT);
    assert_eq!(request.args, synctex::edit_request(&pdf.path, spot).args);
    assert!(pdf.hints.is_none());
}

#[test]
fn test_synctex_backward_looks_in_the_files_included_near_its_line() {
    // SyncTeX names the `\input` line; the text is in the file it brings in.
    let mut pdf = reader(&[&["We prove the theorem here"]]);
    typed(&mut pdf, "iS");
    pdf.on_job(synctex_said("/tmp/paper.tex", 2));
    let main = "\\begin{document}\n\\input{body}\n\\end{document}\n";
    let includes = pdf.on_job(JobReply::Files(vec![(
        PathBuf::from("/tmp/paper.tex"),
        main.to_string(),
    )]));
    let body = PathBuf::from("/tmp/body.tex");
    assert_eq!(
        includes,
        EditorOutcome::Job(JobRequest::ReadFiles(vec![
            body.clone(),
            PathBuf::from("/tmp/body"),
        ]))
    );
    assert_eq!(
        pdf.on_job(JobReply::Files(vec![(
            body.clone(),
            "% body\nWe prove the theorem here\n".to_string(),
        )])),
        EditorOutcome::OpenBeside(OpenTarget::at_line(body, 2))
    );
}

#[test]
fn test_a_synctex_mark_waits_for_the_document_then_comes_to_the_reading_line() {
    let page: &[&str] = &["first line here", "second line"];
    let mut pdf = PdfEditor::opening(None, PathBuf::from("/tmp/paper.pdf"));
    let mark = PdfMark {
        height: 10.0,
        left: 40.0,
        page: 3,
        top: 200.0,
        width: 100.0,
    };
    // Shown before the document has opened, it waits for it.
    pdf.show_mark(mark);
    assert_eq!(pdf.mark, None);
    pdf.set_pane_size(PANE);
    let texts = (0..6).map(|_| PageText::new(Vec::new())).collect();
    pdf.take_document(Document::of_text(vec![LETTER; 6], texts));
    assert_eq!(pdf.mark, Some(mark));
    let column = pdf.column().expect("laid out");
    let top = column.pages[3].y + 200.0 * pdf.scale() - pdf.pane.1 * READING_LINE;
    assert!(
        (pdf.scroll.1 - top).abs() < 0.5,
        "{} vs {top}",
        pdf.scroll.1
    );
    assert_eq!(page_of(&pdf), "page 4 / 6");

    // The next key puts the mark out.
    let mut read = reader(&[page]);
    read.show_mark(PdfMark { page: 0, ..mark });
    assert!(read.mark.is_some());
    typed(&mut read, "j");
    assert_eq!(read.mark, None);
}

/// A PDF of `pages` empty pages, 200 by 100 points, for a viewer to read
/// from disk.
fn pdf_of(pages: usize) -> String {
    let kids: Vec<String> = (0..pages).map(|page| format!("{} 0 R", page + 3)).collect();
    let mut pdf = format!(
        "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [{}] /Count {pages} >> endobj\n",
        kids.join(" ")
    );
    for page in 0..pages {
        pdf.push_str(&format!(
            "{} 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] >> endobj\n",
            page + 3
        ));
    }
    pdf.push_str("trailer << /Root 1 0 R >>\n%%EOF\n");
    pdf
}

/// Tick `pdf` until `done` says so, or panic after a while.
fn tick_until(pdf: &mut PdfEditor, what: &str, done: impl Fn(&PdfEditor) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(pdf) {
        assert!(Instant::now() < deadline, "never {what}");
        pdf.on_tick(Instant::now());
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn test_a_rebuilt_pdf_is_read_again_keeping_the_page_being_read() {
    let path = std::env::temp_dir().join(format!("winter-reload-{}.pdf", std::process::id()));
    fs::write(&path, pdf_of(4)).expect("a scratch PDF");
    let mut pdf = PdfEditor::new(path.clone());
    pdf.set_pane_size(PANE);
    tick_until(&mut pdf, "opened", |pdf| pdf.document.is_some());
    pdf.go_to_page(2);
    let page = page_of(&pdf);

    // The build writes a longer document; the viewer reads it once it holds
    // still, and stays on the page it was on.
    fs::write(&path, pdf_of(6)).expect("rebuilt");
    tick_until(&mut pdf, "read the rebuild", |pdf| pdf.page_count() == 6);
    assert_eq!(page_of(&pdf), page.replace("/ 4", "/ 6"));

    // A file that will not open keeps the document on screen, and is given
    // up on after a few tries.
    fs::write(&path, "not a pdf at all").expect("broken");
    tick_until(&mut pdf, "gave up", |pdf| pdf.notice.is_some());
    assert_eq!(pdf.page_count(), 6, "the last good document stays");
    assert!(pdf
        .notice
        .as_ref()
        .is_some_and(|notice| notice.text.contains("could not read it again")));
    let _ = fs::remove_file(&path);
}

/// `C-` and the letter `c`.
fn ctrl(c: char) -> Key {
    Key {
        ctrl: true,
        ..press(KeyCode::Char(c))
    }
}

/// A ten-page reader whose first page's first line links to the top of
/// page four, and whose second line links out to a URL.
fn linked_reader() -> PdfEditor {
    let mut pdf = long_reader();
    let doc = pdf.document.take().expect("a document");
    let links = vec![
        Link {
            bounds: (40.0, 50.0, 100.0, 12.0),
            to: LinkTo::Spot(Target { page: 3, y: 0.0 }),
        },
        Link {
            bounds: (40.0, 64.0, 100.0, 12.0),
            to: LinkTo::Uri("https://example.org".to_string()),
        },
    ];
    pdf.take_document(doc.with_links(0, links));
    painted(&mut pdf);
    pdf
}

#[test]
fn test_a_click_on_a_link_follows_it_and_c_o_comes_back() {
    let mut pdf = linked_reader();
    assert_eq!(page_of(&pdf), "page 1 / 10");
    // The cell over the first line's link.
    let at = Point {
        col: 5,
        drag: false,
        row: 4,
    };
    assert_eq!(pdf.on_mouse(at), EditorOutcome::Consumed);
    assert_eq!(page_of(&pdf), "page 4 / 10", "followed to page four");
    assert_eq!(pdf.on_key(&ctrl('o')), EditorOutcome::Consumed);
    assert_eq!(page_of(&pdf), "page 1 / 10", "and back");
    pdf.on_key(&ctrl('i'));
    assert_eq!(page_of(&pdf), "page 4 / 10", "and forward again");
}

#[test]
fn test_the_page_jumps_are_kept_and_every_back_key_comes_back() {
    let mut pdf = long_reader();
    typed(&mut pdf, "5gg");
    typed(&mut pdf, "G");
    assert_eq!(page_of(&pdf), "page 10 / 10");
    pdf.on_key(&Key::with_alt(KeyCode::Left));
    assert_eq!(page_of(&pdf), "page 5 / 10", "back from the last page");
    assert_eq!(pdf.on_side_button(true), EditorOutcome::Consumed);
    assert_eq!(page_of(&pdf), "page 1 / 10", "the mouse's back button too");
    pdf.on_key(&Key::with_alt(KeyCode::Right));
    assert_eq!(page_of(&pdf), "page 5 / 10");
    pdf.on_side_button(false);
    assert_eq!(page_of(&pdf), "page 10 / 10", "and forward to the last");
}

#[test]
fn test_a_jump_from_where_the_last_one_left_keeps_no_twin_of_it() {
    let mut pdf = long_reader();
    // Both jumps leave the top of page one: `C-o` has one place to go
    // back to, not the same one twice.
    typed(&mut pdf, "gggg");
    assert_eq!(pdf.jumps_back.len(), 1);
}

/// The pointer resting on the cell at `row`, `col`.
fn hover(pdf: &mut PdfEditor, row: usize, col: usize) -> EditorOutcome {
    pdf.on_hover(Some(Point {
        col,
        drag: false,
        row,
    }))
}

#[test]
fn test_resting_on_a_link_previews_it_and_leaving_the_link_closes_it() {
    let mut pdf = linked_reader();
    // The cell over the first line's link, as a click lands on it.
    assert_eq!(hover(&mut pdf, 4, 5), EditorOutcome::Consumed);
    assert_eq!(hover(&mut pdf, 4, 6), EditorOutcome::Ignored, "still the same link");
    pdf.on_tick(Instant::now());
    assert!(pdf.preview.is_none(), "a pointer passing over is let be");
    pdf.on_tick(Instant::now() + HOVER_DELAY);
    let preview = pdf.preview.as_ref().expect("a preview");
    assert_eq!(preview.target, Target { page: 3, y: 0.0 });
    assert_eq!(page_of(&pdf), "page 1 / 10", "previewing moves nothing");
    hover(&mut pdf, 20, 5);
    assert!(pdf.preview.is_none(), "off the link, it closes");
    hover(&mut pdf, 4, 5);
    pdf.on_hover(None);
    pdf.on_tick(Instant::now() + HOVER_DELAY);
    assert!(pdf.preview.is_none(), "nor shows once the pointer has gone");
}

#[test]
fn test_resting_on_a_url_says_it_in_the_status_bar() {
    let mut pdf = linked_reader();
    // The URL's line is thinner than a row, so the pointer is put on it
    // rather than on a cell.
    let url = pdf.document.as_mut().expect("a document").links(0)[1].clone();
    pdf.hover_link(Some((0, url)));
    pdf.on_tick(Instant::now() + HOVER_DELAY);
    assert!(pdf.preview.is_none(), "a URL has no page to show");
    assert!(pdf.status().segments.contains(&"https://example.org".to_string()));
    hover(&mut pdf, 20, 5);
    assert!(!pdf.status().segments.contains(&"https://example.org".to_string()));
}

/// Alt, Shift, and `o`, as the host hands it on.
fn show_links_key() -> Key {
    Key {
        alt: true,
        shift: true,
        ..press(KeyCode::Char('o'))
    }
}

#[test]
fn test_m_s_o_outlines_every_link_in_view_and_again_only_the_hovered_one() {
    let mut pdf = linked_reader();
    let column = pdf.column().expect("laid out");
    assert_eq!(format_key(&show_links_key()), "M-S-o");
    assert!(pdf.link_box_tiles(&column).is_empty(), "none until asked");
    assert_eq!(pdf.on_key(&show_links_key()), EditorOutcome::Consumed);
    painted(&mut pdf);
    // A frame of four lines around the link to page four, a line under the
    // URL.
    assert_eq!(pdf.link_box_tiles(&column).len(), 4 + 1);
    pdf.on_key(&show_links_key());
    assert!(pdf.link_box_tiles(&column).is_empty());
    hover(&mut pdf, 4, 5);
    assert_eq!(pdf.link_box_tiles(&column).len(), 4, "the link under the pointer");
}

#[test]
fn test_a_preview_asked_for_with_a_key_outlasts_the_pointer() {
    let mut pdf = linked_reader();
    typed(&mut pdf, "Fa");
    hover(&mut pdf, 4, 5);
    hover(&mut pdf, 20, 5);
    assert!(pdf.preview.is_some(), "only `Enter` or another key puts it away");
}

#[test]
fn test_a_citation_the_pdf_names_no_spot_for_leads_to_its_bibliography_entry() {
    // Prose citing [2] and [9] on page one, the references on page three.
    let mut pdf = reader(&[
        &["as [2] shows, and [9]"],
        &["[1] An early line"],
        &["[1] A. One.", "[2] B. Two.", "[3] C. Three."],
    ]);
    let doc = pdf.document.take().expect("a document");
    // `words` sets each character 5 points wide from x = 40: `[2]` is the
    // fourth to sixth, `[9]` the eighteenth to twentieth.
    let cite = |at: f32| (40.0 + at * 5.0, 50.0, 15.0, 14.0);
    pdf.take_document(doc.with_citations(0, vec![cite(3.0), cite(17.0)]));
    let links = pdf.document.as_mut().expect("a document").links(0).to_vec();
    assert_eq!(links.len(), 1, "no entry [9]: the link leads nowhere");
    let LinkTo::Spot(target) = links[0].to else {
        panic!("a spot");
    };
    assert_eq!(target.page, 2, "the list, not the stray [1] before it");
    assert!(target.y > 60.0, "the second entry, under the first");
}

#[test]
fn test_f_labels_the_links_in_view_and_a_label_follows_its_link() {
    let mut pdf = linked_reader();
    typed(&mut pdf, "f");
    let labels: Vec<String> = pdf
        .hints
        .as_ref()
        .expect("labels up")
        .links
        .iter()
        .map(|hint| hint.label.clone())
        .collect();
    assert_eq!(labels, ["a", "s"]);
    assert_eq!(
        typed(&mut pdf, "s"),
        EditorOutcome::OpenExternal(PathBuf::from("https://example.org"))
    );
    assert!(pdf.hints.is_none());

    typed(&mut pdf, "f");
    typed(&mut pdf, "x");
    assert!(
        pdf.hints.is_none(),
        "a letter no label starts with puts them away"
    );
    assert_eq!(page_of(&pdf), "page 1 / 10");
}

#[test]
fn test_a_preview_shows_where_a_link_leads_and_enter_follows_it() {
    let mut pdf = linked_reader();
    typed(&mut pdf, "Fa");
    let preview = pdf.preview.as_ref().expect("a preview");
    assert_eq!(preview.target, Target { page: 3, y: 0.0 });
    assert_eq!(page_of(&pdf), "page 1 / 10", "previewing moves nothing");
    pdf.on_key(&press(KeyCode::Enter));
    assert_eq!(page_of(&pdf), "page 4 / 10");
    assert!(pdf.preview.is_none());

    // Any other key puts it away and goes on to mean what it means.
    pdf.on_key(&ctrl('o'));
    typed(&mut pdf, "Fa");
    typed(&mut pdf, "]");
    assert!(pdf.preview.is_none());
    assert_eq!(page_of(&pdf), "page 2 / 10");
}

#[test]
fn test_the_cursor_follows_and_previews_the_link_it_is_on() {
    let mut pdf = linked_reader();
    typed(&mut pdf, "iK");
    assert!(
        pdf.preview.is_some(),
        "K previews the link under the cursor"
    );
    pdf.on_key(&press(KeyCode::Escape));
    assert_eq!(pdf.on_key(&press(KeyCode::Enter)), EditorOutcome::Consumed);
    assert_eq!(page_of(&pdf), "page 4 / 10");
}

#[test]
fn test_labels_are_as_short_as_the_links_in_view_allow_and_each_its_own() {
    assert_eq!(hint_labels(3), ["a", "s", "d"]);
    let many = hint_labels(HINT_KEYS.len() + 1);
    assert!(many.iter().all(|label| label.len() == 2));
    assert_eq!(&many[..2], ["aa", "as"]);
    let unique: HashSet<&String> = many.iter().collect();
    assert_eq!(unique.len(), many.len());
}

#[test]
fn test_a_crop_takes_the_rect_asked_for_and_stops_at_the_edges() {
    // Four by four, each pixel's bytes its row times ten and its column.
    let pixels = Pixels {
        height: 4,
        rgba: (0..4u8)
            .flat_map(|row| (0..4u8).flat_map(move |col| [row * 10 + col; 4]))
            .collect(),
        width: 4,
    };
    let crop = crop_rect(&pixels, (1, 1, 2, 2));
    assert_eq!((crop.width, crop.height), (2, 2));
    let firsts: Vec<u8> = crop.rgba.chunks(4).map(|pixel| pixel[0]).collect();
    assert_eq!(firsts, [11, 12, 21, 22]);
    let edge = crop_rect(&pixels, (3, 3, 10, 10));
    assert_eq!((edge.width, edge.height), (1, 1));
}

#[test]
fn test_a_scroll_glides_to_where_it_lands_and_scrolls_in_a_row_add_up() {
    let mut pdf = long_reader();
    typed(&mut pdf, "j");
    assert_eq!(pdf.scroll.1, 0.0, "nothing moves until the glide is drawn");
    let started = pdf.glide.expect("gliding").started;
    pdf.step_glide(started + GLIDE / 2);
    assert!(
        pdf.scroll.1 > 0.0 && pdf.scroll.1 < LINE_PX,
        "part of the way: {}",
        pdf.scroll.1
    );
    // Another line while the first glides goes on from where it lands.
    typed(&mut pdf, "j");
    assert_eq!(landed(&mut pdf), 2.0 * LINE_PX);
    assert!(pdf.glide.is_none(), "landed, it stops");
    assert!(pdf
        .next_tick()
        .is_some_and(|at| at > Instant::now() + GLIDE_FRAME));
}

#[test]
fn test_a_jump_lands_at_once_over_a_glide_and_a_touchpad_moves_by_its_pixels() {
    let mut pdf = long_reader();
    typed(&mut pdf, "j");
    typed(&mut pdf, "]");
    assert!(pdf.glide.is_none(), "the page key lands at once");
    assert_eq!(page_of(&pdf), "page 2 / 10");
    let before = pdf.scroll.1;
    pdf.on_scroll_pixels(-7.5);
    assert_eq!(pdf.scroll.1, before + 7.5, "a touchpad's own pixels, down");
}
