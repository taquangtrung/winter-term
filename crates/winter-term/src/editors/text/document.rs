//! One open file: reading it in, joining a buffer another tab has open, and
//! learning its grammar and colors as the window asks for them.

use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::model::paint::Style;
use crate::model::textedit::buffer::TextBuffer;
use crate::model::textedit::modal::Marks;

use crate::editors::highlight::{self, LineState};
use crate::editors::text_file::{FileShape, FileStamp};
use super::{buffers, commit, grammar_of, remote, spelling, Document};

// ========================================================================
// Document
// ========================================================================

impl Document {
    /// Whether the file can be written without a question first: on this
    /// machine, and not changed on disk since it was read. A save that
    /// cannot ask (a build's, or one as the keyboard leaves) skips the rest.
    pub(super) fn writes_unasked(&self) -> bool {
        crate::remote::parse(&self.path).is_none() && !remote::is_stale(&self.path, &self.stamp())
    }

    /// Read `path` in, landing on `line` where whatever asked for the file
    /// knew which one mattered. A file only root can read becomes a buffer
    /// of root's, and where sudo wants a password first it waits unread.
    pub(super) fn open(path: PathBuf, line: Option<usize>) -> io::Result<Self> {
        // A file another tab has open is that tab's buffer, not read again.
        if let Some(file) = buffers::open_buffer(&path) {
            return Ok(Self::joining(path, line, file));
        }
        let (path, loaded) = remote::read_for_open(path)?;
        let mut doc = Self::unread(path, line);
        if let Some(loaded) = loaded {
            doc.buffer = TextBuffer::new(loaded.lines);
            if let Some(line) = doc.land_on.take() {
                doc.buffer.move_to_line(line.saturating_sub(1));
            }
            doc.file = buffers::BufferState::new(&doc.buffer, loaded.shape, loaded.stamp);
            buffers::keep_buffer(&doc.path, &doc.file);
            doc.unread = false;
            doc.learn_grammar();
        }
        Ok(doc)
    }

    /// A file whose name says nothing of its language may say it on its
    /// first line: a script's shebang, a modeline.
    pub(super) fn learn_grammar(&mut self) {
        if self.grammar.is_none() {
            self.grammar = self
                .buffer
                .lines()
                .first()
                .and_then(|line| highlight::of_first_line(line));
        }
    }

    /// Another tab on `file`, the buffer some tab already has `path` open
    /// in, landing on `line`.
    pub(super) fn joining(path: PathBuf, line: Option<usize>, file: Rc<buffers::BufferState>) -> Self {
        let mut doc = Self::unread(path, None);
        doc.buffer = buffers::view_of(&file);
        if let Some(line) = line {
            doc.buffer.move_to_line(line.saturating_sub(1));
        }
        doc.file = file;
        doc.unread = false;
        doc.learn_grammar();
        doc
    }

    /// A buffer for `path` that has not read it yet, to land on `line` once
    /// it does.
    pub(super) fn unread(path: PathBuf, line: Option<usize>) -> Self {
        let buffer = TextBuffer::new(vec![String::new()]);
        let file = buffers::BufferState::new(&buffer, FileShape::default(), FileStamp::default());
        Self {
            buffer,
            conflicts: std::cell::RefCell::new(None),
            hscroll: 0,
            land_on: line,
            marks: Marks::default(),
            scroll: 0,
            scroll_skip: 0,
            file,
            attached: false,
            color_upto: 0,
            colors: Vec::new(),
            grammar: grammar_of(&path),
            spelling: spelling::Spelling::from_the_start(&path),
            states: Vec::new(),
            path,
            unread: true,
            diagnostics: Vec::new(),
        }
    }

    /// Whether lines the window wants colored are still to be worked out.
    pub(super) fn learning(&self) -> bool {
        let wanted = self
            .color_upto
            .min(self.buffer.lines().len().saturating_sub(1));
        self.grammar.is_some() && self.states.len() <= wanted
    }

    /// Work out what every line up to the one the window wants colored
    /// leaves open, as far as it is not already known and `budget` allows.
    /// A line's color depends on every line above it, so this is the price
    /// of the first look at a part of the file; what it learns is kept until
    /// an edit makes it untrue.
    pub(super) fn learn_states(&mut self, budget: Duration) {
        if let Some(changed) = self.buffer.take_change() {
            self.states.truncate(changed + 1);
            self.colors.truncate(changed);
        }
        let Some(grammar) = self.grammar else {
            return;
        };
        if self.states.is_empty() {
            self.states.push(LineState::start(grammar));
        }
        let started = Instant::now();
        let lines = self.buffer.lines();
        while self.states.len() <= self.color_upto && self.states.len() < lines.len() {
            let index = self.states.len() - 1;
            let next = highlight::next_state(&lines[index], &self.states[index]);
            self.states.push(next);
            if started.elapsed() >= budget {
                break;
            }
        }
    }

    /// Work out the styles of lines `from..to`, as far as their states are
    /// known and their styles are not.
    ///
    /// A commit message's summary is marked past the width it reads well at,
    /// which is git's convention rather than anything its grammar says.
    pub(super) fn color_lines(&mut self, from: usize, to: usize) {
        let known = self.buffer.lines().iter().zip(&self.states);
        for (index, (line, state)) in known.enumerate().take(to).skip(from) {
            if self.colors.get(index).is_some_and(Option::is_some) {
                continue;
            }
            let (mut styles, _) = highlight::highlight(line, state);
            if index == 0 && commit::is_message_file(&self.path) {
                for style in styles.iter_mut().skip(commit::SUMMARY_WIDTH) {
                    *style = Style::Removed;
                }
            }
            if self.colors.len() <= index {
                self.colors.resize(index + 1, None);
            }
            self.colors[index] = Some(styles);
        }
    }

    /// Start the file's coloring over, for text that has replaced the old.
    pub(super) fn forget_colors(&mut self) {
        self.states = Vec::new();
        self.colors = Vec::new();
    }
}
