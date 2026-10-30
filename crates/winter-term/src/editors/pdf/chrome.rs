//! What the PDF viewer shows around its pages: the status, the toolbar, and
//! the outline, and what a click on them does.

use crate::model::editor::{DropList, EditorOutcome, PromptMode, PromptRequest};
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::paint::Point;

use super::document::Document;
use super::links::{Link, LinkTo};
use super::outline::Target;
use super::outline_list::RowHit;
use super::toolbar::Button;
use super::{
    outline_list, toolbar, Command, PdfEditor, Pending, Zoom, ASK_PAGE, ASK_ZOOM, FIT_HEIGHT_WORDS,
    FIT_PAGE_WORDS, FIT_WIDTH_WORDS, MAX_SCALE, MIN_SCALE, OUTLINE_COL, OUTLINE_SHARE, PERCENT,
    READING_LINE, TOOLBAR_ROWS,
};

// ========================================================================
// PdfEditor
// ========================================================================

impl PdfEditor {
    /// The name of the file, for the title.
    pub(super) fn file_name(&self) -> String {
        match self.path.file_name() {
            Some(name) => name.to_string_lossy().to_string(),
            None => self.path.display().to_string(),
        }
    }

    /// `page 3 / 18`, once the document has opened: blank before, since a
    /// confident "page 0 of 0" would be a lie about one still loading.
    pub(super) fn page_label(&self) -> String {
        match self.page_count() {
            0 => String::new(),
            pages => format!("page {} / {pages}", self.current_page() + 1),
        }
    }

    /// How far through the document the view has scrolled.
    pub(super) fn percent(&self) -> u32 {
        let Some(column) = self.column() else {
            return 0;
        };
        let (_, max_y) = column.max_scroll(self.pane);
        match max_y > 0.0 {
            true => (self.scroll.1 / max_y * 100.0).round() as u32,
            false => 100,
        }
    }

    /// The file, the page, and how far through, as `C-g` says them.
    pub(super) fn position(&self) -> String {
        format!(
            "{}, page {} of {}, {}%",
            self.file_name(),
            self.current_page() + 1,
            self.page_count(),
            self.percent()
        )
    }

    // --------------------------------------------------------------------
    // The toolbar and the outline
    // --------------------------------------------------------------------

    /// How tall the toolbar is in pixels.
    pub(super) fn toolbar_height(&self) -> f32 {
        TOOLBAR_ROWS as f32 * self.cell.1
    }

    /// What the toolbar shows.
    pub(super) fn bar_state(&self) -> toolbar::State<'_> {
        let entries = self.document.as_ref().map_or(&[][..], Document::outline);
        toolbar::State {
            has_outline: !entries.is_empty(),
            inverted: self.inverted,
            outline_open: self.outline.is_some(),
            page: (self.page_count() > 0).then(|| (self.current_page() + 1, self.page_count())),
            section: self
                .current_section()
                .map(|index| entries[index].title.as_str()),
            zoom: (self.scale() * PERCENT).round() as u32,
        }
    }

    /// The spot at the reading line: its page, and how far down it.
    pub(super) fn reading_spot(&self) -> Option<Target> {
        let column = self.column()?;
        let line = self.scroll.1 + self.pane.1 * READING_LINE;
        let page = column.page_at(line);
        Some(Target {
            page,
            y: (line - column.pages[page].origin().1) / self.scale(),
        })
    }

    /// The outline entry the reading spot is in.
    pub(super) fn current_section(&self) -> Option<usize> {
        let entries = self.document.as_ref()?.outline();
        outline_list::current(entries, self.reading_spot()?)
    }

    /// Scroll so outline entry `index`'s spot is at the top, and close the
    /// outline, the place left kept for `C-o`. An entry pointing nowhere
    /// leaves it open.
    pub(super) fn jump_to_section(&mut self, index: usize) -> EditorOutcome {
        let target = self
            .document
            .as_ref()
            .and_then(|doc| doc.outline().get(index))
            .and_then(|entry| entry.target);
        let Some(target) = target else {
            return EditorOutcome::Consumed;
        };
        self.outline = None;
        // Followed like a link, so `C-o` comes back from it.
        let to = LinkTo::Spot(target);
        self.follow(Link {
            bounds: (0.0, 0.0, 0.0, 0.0),
            to,
        })
    }

    /// How many words and characters the selection holds, or with none,
    /// how many words the whole document does, every page read for it.
    pub(super) fn word_count(&mut self) -> Option<String> {
        let doc = self.document.as_mut()?;
        let words = |text: &str| text.split_whitespace().count();
        let counted = |count: usize, one: &str| match count {
            1 => format!("1 {one}"),
            _ => format!("{count} {one}s"),
        };
        if let Some(text) = self
            .caret
            .selected_text(doc)
            .filter(|text| !text.is_empty())
        {
            let (words, chars) = (words(&text), text.chars().count());
            return Some(format!(
                "{}, {} selected",
                counted(words, "word"),
                counted(chars, "character")
            ));
        }
        let pages = doc.page_count();
        let mut total = 0;
        for page in 0..pages {
            if let Some(text) = doc.text(page) {
                total += text
                    .lines
                    .iter()
                    .map(|line| words(&line.text.iter().collect::<String>()))
                    .sum::<usize>();
            }
        }
        Some(format!(
            "{}, {}",
            counted(total, "word"),
            counted(pages, "page")
        ))
    }

    /// Ask which page to go to, the one being read filled in.
    pub(super) fn ask_page(&mut self) -> EditorOutcome {
        self.pending = Pending::default();
        if self.page_count() == 0 {
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Prompt(PromptRequest {
            initial: (self.current_page() + 1).to_string(),
            label: format!("Page (1-{}): ", self.page_count()),
            mode: PromptMode::Text,
            tag: ASK_PAGE,
        })
    }

    /// Ask what to zoom to, the zoom in force filled in.
    pub(super) fn ask_zoom(&mut self) -> EditorOutcome {
        self.pending = Pending::default();
        EditorOutcome::Prompt(PromptRequest {
            initial: format!("{}", (self.scale() * PERCENT).round()),
            label: "Zoom (%, or w width, h height, p page): ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_ZOOM,
        })
    }

    /// Do what toolbar button `button` does.
    pub(super) fn press(&mut self, button: Button) -> EditorOutcome {
        match button {
            Button::Invert => self.act(Command::Invert),
            Button::NextPage => self.act(Command::GoToPageBy(1)),
            Button::Outline => self.act(Command::ToggleOutline),
            Button::Page => self.ask_page(),
            Button::PreviousPage => self.act(Command::GoToPageBy(-1)),
            Button::Zoom => self.ask_zoom(),
            Button::ZoomIn => self.act(Command::Zoom(1)),
            Button::ZoomOut => self.act(Command::Zoom(-1)),
        }
    }

    /// A key while the outline is open: it moves the outline's cursor, and
    /// the keys it has no use for close it.
    pub(super) fn on_outline_key(&mut self, key: &Key) -> EditorOutcome {
        let Some(entries) = self.document.as_ref().map(|doc| doc.outline().to_vec()) else {
            return EditorOutcome::Ignored;
        };
        let Some(list) = self.outline.as_mut() else {
            return EditorOutcome::Ignored;
        };
        match key.chord() {
            (Mods::NONE, KeyCode::Down | KeyCode::Char('j')) | (Mods::CTRL, KeyCode::Char('n')) => {
                list.step(&entries, 1);
            }
            (Mods::NONE, KeyCode::Up | KeyCode::Char('k')) | (Mods::CTRL, KeyCode::Char('p')) => {
                list.step(&entries, -1);
            }
            (Mods::NONE, KeyCode::Right | KeyCode::Char('l')) => list.unfold(&entries),
            (Mods::NONE, KeyCode::Left | KeyCode::Char('h')) => list.fold(&entries),
            (Mods::NONE, KeyCode::Tab) => list.toggle(list.cursor),
            (Mods::NONE, KeyCode::Enter) => {
                let index = list.cursor;
                return self.jump_to_section(index);
            }
            (Mods::NONE, KeyCode::Escape | KeyCode::Char('q' | 'O'))
            | (Mods::CTRL, KeyCode::Char('g')) => {
                self.outline = None;
            }
            _ => {}
        }
        EditorOutcome::Consumed
    }

    /// A click on the toolbar or the open outline under it, at `at`:
    /// `None` for one on the pages.
    pub(super) fn on_chrome_mouse(&mut self, at: Point) -> Option<EditorOutcome> {
        // The window answers a press on a button of the toolbar itself (see
        // `on_toolbar`); anything else that lands on its row, a drag across
        // it included, does nothing.
        if at.row < TOOLBAR_ROWS {
            return Some(EditorOutcome::Consumed);
        }
        self.outline.as_ref()?;
        let drop = self.outline_drop.as_ref()?;
        let (top, left) = drop.cell;
        let on_card = (top..top + drop.rows.len()).contains(&at.row)
            && (left..left + drop.cols).contains(&at.col);
        if !on_card {
            // A click on the pages closes the outline and is spent on that.
            if !at.drag {
                self.outline = None;
                self.outline_drop = None;
            }
            return Some(EditorOutcome::Consumed);
        }
        if at.drag {
            return Some(EditorOutcome::Consumed);
        }
        let (row, col) = (at.row - top, at.col - left);
        let entries = self.document.as_ref()?.outline();
        let hit = self.outline.as_ref()?.hit(entries, row, col);
        Some(match hit {
            Some(RowHit::Fold(index)) => {
                if let Some(list) = self.outline.as_mut() {
                    list.toggle(index);
                }
                EditorOutcome::Consumed
            }
            Some(RowHit::Entry(index)) => self.jump_to_section(index),
            None => EditorOutcome::Consumed,
        })
    }

    /// The open outline as a drop-down list under the toolbar, as many of
    /// its rows as fit in a share of the pane's `rows`, `cols` wide.
    pub(super) fn outline_list(&mut self, rows: usize, cols: usize) -> Option<DropList> {
        let entries = self.document.as_ref()?.outline().to_vec();
        let list = self.outline.as_mut()?;
        let room = ((rows.saturating_sub(TOOLBAR_ROWS) as f32 * OUTLINE_SHARE) as usize).max(1);
        let height = list.shown(&entries).len().min(room);
        let width = list.width(&entries, cols.saturating_sub(OUTLINE_COL));
        let (rows, cursor) = list.rows(&entries, height);
        Some(DropList {
            cell: (TOOLBAR_ROWS, OUTLINE_COL),
            cols: width,
            rows,
            selected: Some(cursor),
        })
    }

    /// What a zoom typed at the prompt asks for: a fit by its word or first
    /// letter, or a percentage, `%` or not. `None` for anything else.
    pub(super) fn parse_zoom(answer: &str) -> Option<Zoom> {
        let answer = answer.trim().to_lowercase();
        let is = |words: &[&str]| words.contains(&answer.as_str());
        if is(FIT_WIDTH_WORDS) {
            return Some(Zoom::FitWidth);
        }
        if is(FIT_HEIGHT_WORDS) {
            return Some(Zoom::FitHeight);
        }
        if is(FIT_PAGE_WORDS) {
            return Some(Zoom::FitPage);
        }
        let percent: f32 = answer.trim_end_matches('%').trim().parse().ok()?;
        (percent.is_finite() && percent > 0.0)
            .then(|| Zoom::Scale((percent / PERCENT).clamp(MIN_SCALE, MAX_SCALE)))
    }
}
