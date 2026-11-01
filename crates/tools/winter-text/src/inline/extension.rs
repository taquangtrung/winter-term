//! An extension's inline source (see `winter_ext_api::InlineProvider`): asked
//! through the window, which hands the extension each request and its
//! answer back, by ticket.
//!
//! What the extension offers, and which servers are its own, come in with
//! the settings. Before each ask its servers are handed every local
//! document as it now reads, so a request names text the server has; the
//! answer comes in the server's terms (lines, UTF-16 columns) and is put in
//! the editor's here, against the text as it stands when it comes.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use winter_ext_api::{InlineAnswer, InlineEdit, InlineKind, InlineRequest, TextPosition};
use winter_lsp::protocol::{self, Position};
use winter_model::paint::TAB_WIDTH;

use super::super::settings::{EditorSettings, InlineOffered};
use super::{Answer, Edit, GhostAnswer, InlineSource, OfferKind, Suggestion, Want};
use crate::lsp::hand_over;
use crate::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The ticket the next request goes under, unique among every editor's.
static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);

// ========================================================================
// Data Structures
// ========================================================================

/// One extension's inline source, as one editor asks it.
#[derive(Debug)]
pub(super) struct ExtensionSource {
    /// The request in flight.
    asked: Option<Asked>,
    offered: InlineOffered,
}

/// What a request in flight asked, and of what.
#[derive(Clone, Copy, Debug)]
struct Asked {
    /// Whether `C-\` asked for it.
    invoked: bool,
    kind: OfferKind,
    /// The text's revision as asked: a next edit for another is put away.
    revision: usize,
    /// The cursor's row as asked.
    row: usize,
    ticket: u64,
}

/// A request for an extension's inline source, or what to tell it, for the
/// window to hand on with the extension's host.
#[derive(Clone, Debug, PartialEq)]
pub enum InlineCall {
    /// Ask it, as the request says.
    Ask(InlineAsk),
    /// Its suggestion shows.
    Shown(InlineTold),
    /// Its suggestion was taken.
    Taken(InlineTold),
}

/// A request, and the source, by its full id, it is for.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineAsk {
    pub request: InlineRequest,
    pub source: String,
}

/// What a source is told of one of its suggestions: its kind, its note of
/// it, and the source, by its full id.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineTold {
    pub kind: InlineKind,
    pub receipt: String,
    pub source: String,
}

// ========================================================================
// ExtensionSource
// ========================================================================

impl ExtensionSource {
    /// The source `offered` describes, asked nothing yet.
    pub(super) fn new(offered: InlineOffered) -> Self {
        Self {
            asked: None,
            offered,
        }
    }

    /// Hand the extension's servers every local document as it now reads.
    /// The version the first of them has of the document in front.
    fn sync(&self, editor: &mut TextEditor) -> Option<i64> {
        let ports = editor.ready_ports(&self.offered.servers);
        for doc in &mut editor.docs {
            if doc.unread || winter_base::remote::parse(&doc.path).is_some() {
                continue;
            }
            doc.buffer.catch_up();
            for port in &ports {
                let synced = hand_over(port, doc, doc.everywhere_synced(port.id()));
                doc.set_everywhere_synced(port.id(), synced);
            }
        }
        let first = ports.first()?;
        let synced = editor.doc().everywhere_synced(first.id())?;
        Some(i64::from(synced.version))
    }

    /// Tell the extension `kind`'s suggestion noted by `receipt` shows or
    /// was taken, as `call` makes the call.
    fn tell(
        &self,
        editor: &mut TextEditor,
        kind: OfferKind,
        receipt: &Value,
        call: fn(InlineTold) -> InlineCall,
    ) {
        let Some(receipt) = receipt.as_str() else {
            return;
        };
        editor.inline.calls.push(call(InlineTold {
            kind: kind_of(kind),
            receipt: receipt.to_string(),
            source: self.offered.id.clone(),
        }));
    }
}

impl InlineSource for ExtensionSource {
    fn name(&self) -> &str {
        &self.offered.title
    }

    fn extension_id(&self) -> Option<&str> {
        Some(&self.offered.id)
    }

    fn configure(&mut self, settings: &EditorSettings) -> bool {
        let now = settings
            .inline
            .iter()
            .find(|offered| offered.id == self.offered.id)
            .cloned()
            .unwrap_or_else(|| InlineOffered {
                id: self.offered.id.clone(),
                ..InlineOffered::default()
            });
        let changed = now.offer != self.offered.offer;
        self.offered = now;
        changed
    }

    fn offers(&self, editor: &TextEditor, kind: OfferKind) -> bool {
        self.offered.offer.offers(kind_of(kind))
            && !editor.doc().unread
            && winter_base::remote::parse(&editor.doc().path).is_none()
    }

    fn ask(&mut self, editor: &mut TextEditor, want: Want) -> Option<Answer> {
        let version = self.sync(editor);
        let (row, col) = (editor.buffer().row(), editor.buffer().col());
        let at = protocol::position_in(editor.buffer().lines(), row, col);
        let (kind, invoked) = match want {
            Want::Edit => (OfferKind::Edit, false),
            Want::Ghost { invoked } => (OfferKind::Ghost, invoked),
        };
        let ticket = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
        let mut request = InlineRequest::new(
            ticket,
            kind_of(kind),
            editor.doc().path.clone(),
            TextPosition::new(at.line, at.character),
        );
        request.insert_spaces = !editor
            .buffer()
            .lines()
            .iter()
            .any(|line| line.starts_with('\t'));
        request.invoked = invoked;
        request.tab_size = TAB_WIDTH as u32;
        request.uri = protocol::uri_of(&editor.doc().path);
        request.version = version;
        editor.inline.calls.push(InlineCall::Ask(InlineAsk {
            request,
            source: self.offered.id.clone(),
        }));
        self.asked = Some(Asked {
            invoked,
            kind,
            revision: editor.buffer().revision(),
            row,
            ticket,
        });
        None
    }

    fn answer(
        &mut self,
        editor: &mut TextEditor,
        ticket: u64,
        answer: &InlineAnswer,
    ) -> Option<Answer> {
        let asked = self.asked.filter(|asked| asked.ticket == ticket)?;
        self.asked = None;
        let lines = editor.buffer().lines();
        let place = |at: TextPosition| {
            protocol::place_of(
                lines,
                Position {
                    character: at.column,
                    line: at.line,
                },
            )
        };
        let receipt = |edit: &InlineEdit| Value::String(edit.receipt.clone());
        Some(match (asked.kind, answer) {
            (OfferKind::Ghost, InlineAnswer::Ghost(offered)) => {
                let row = asked.row as u32;
                let suggestions = offered
                    .iter()
                    .filter(|edit| edit.range.start.line == row && edit.range.end.line == row)
                    .map(|edit| Suggestion {
                        end: place(edit.range.end).1,
                        insert: edit.text.clone(),
                        receipt: receipt(edit),
                        start: place(edit.range.start).1,
                    })
                    .collect();
                Answer::Ghost(GhostAnswer {
                    invoked: asked.invoked,
                    row: asked.row,
                    suggestions,
                })
            }
            (OfferKind::Edit, InlineAnswer::Edit(edit))
                if editor.buffer().revision() == asked.revision =>
            {
                Answer::Edit(Edit {
                    end: place(edit.range.end),
                    receipt: receipt(edit),
                    start: place(edit.range.start),
                    text: edit.text.clone(),
                })
            }
            // Ghost text said to be nothing still says so where asked for.
            (OfferKind::Ghost, _) => Answer::Ghost(GhostAnswer {
                invoked: asked.invoked,
                row: asked.row,
                suggestions: Vec::new(),
            }),
            (OfferKind::Edit, _) => Answer::Nothing,
        })
    }

    fn shown(&mut self, editor: &mut TextEditor, kind: OfferKind, receipt: &Value) {
        self.tell(editor, kind, receipt, InlineCall::Shown);
    }

    fn taken(&mut self, editor: &mut TextEditor, kind: OfferKind, receipt: &Value) {
        self.tell(editor, kind, receipt, InlineCall::Taken);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `kind` as the API names it.
fn kind_of(kind: OfferKind) -> InlineKind {
    match kind {
        OfferKind::Edit => InlineKind::Edit,
        OfferKind::Ghost => InlineKind::Ghost,
    }
}
