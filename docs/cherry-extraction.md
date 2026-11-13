# Cherry Extraction Plan

Status: built. Steps 1 and 2 are built: `Highlighter::prepare` colors the lines in sight a budget at a time; host commands come back as `HostRequest::Command`; notices carry a `NoticeLevel`; a host selects a range (`Editor::select`), finds the char under a cell (`Editor::offset_at_cell`), and sets wrap and line numbers per view (`Editor::set_view_options`). Step 3 is built: `EditorOptions::unnamed_clipboard`, `literal_search`, `screen_row_motions`, `break_indent`, `edge_row_motions`, and `escape_drops_prefix`. Step 4 is built: `g_`, `g0 g^ gm g$`, the changelist (`g; g,`), the special marks (`. [ ] ^ < >`), the sentence objects, surround (`ys yss cs ds`), `gn` and `gN`, and in Insert mode `C-x C-t`, `M-SPC`, `M-m`, `M-< M->`, and `C-l`. Step 5 is built: `relative_line_numbers` and `Style::CursorLineNumber`, `GutterLane`s with `Sign::in_lane`, and `Layers` on each cell for `indent_guides` and `light_occurrences`; washes (a reading wash, a diff's lines) are host `Background` decorations. Step 6 is built: `set_completion_providers` merging several providers with `CompletionKind`s and sources, `CompletionAnswer::Later` answered by `CompletionTicket` through `answer_completion`, `auto_complete` with `trigger_characters`, and snippets (`insert_snippet`, `CompletionItem::with_snippet`). Step 7 is built: expand and shrink selection with a `SelectionRangeProvider`, sexps (`C-M-f` and the rest, slurp, barf, splice, raise), `[m ]M` and `zj zk`, `gq` with `text_width`, `continue_comments` and `continue_lists`, `:s///c` and `M-%`, `C-;` changing every occurrence, and phrases with `M-s h .`. Step 8 is built: `Editor::bind_in_document`, and an unnamed register and a visual selection of each extra cursor's own. Step 9 is built: `VirtualLine`s with `set_virtual_lines`, `sticky_scroll` pinning with `Layers::PINNED`, and `ConcealMode`. Step 10 is built: the undo tree with `g-` and `g+` (`History::earlier`, `later`, `step_toward`), and fields that wrap over several rows when their view asks. Step 11 is built: the `markup` module (then the `cherry-markup` crate), with `markdown_conceals`, `outline` for each `Format`, and `register_markdown_commands`. Every step is built.

Winter (this repository) edits text with its own crates: `winter-textedit` (the buffer, the Vim grammar, Emacs keys, fields), `winter-keys`, the modal key engine in `winter-model`, and the editor tab in `winter-text`. This plan moves every generic editing behavior of Winter into Cherry, so Winter's editor can be rebuilt on Cherry. Cherry's rules in `AGENTS.md` hold throughout: Winter is a reference for what a behavior should be, never code to copy; each behavior is built here test first.

## 1. What Moves, What Stays

Generic, moving to Cherry:

- The editing core Winter has and Cherry lacks: `g_`, `g0 g^ gm g$`, the changelist (`g; g,`), the special marks, sentence objects (`is`, `as`), surround, `gn`, transpose lines, cycle spacing, `M-m`, `M-< M->`, recentering that cycles.
- Fields: undo a word at a time, `C-/ C-\ M-\`, and fields of several fixed rows (renaming in place).
- Structural editing: expand selection (through a `SelectionRangeProvider` a host can back with a language server), sexp navigation and editing, definition start and end, indentation block steps, reflow (`gq`), comment and list continuation, query-replace (`:s///c` and an interactive replace), editing every occurrence at once, phrase highlighting.
- Paint: gutter lanes, line number styles, separate layers for occurrence lighting, indent guides and washes, virtual lines between text lines (multi-line ghost text, conflict bands), pinned rows (sticky scroll), and conceal modes.
- Completion: an asynchronous, ticketed provider, several providers merged with kinds and sources, automatic triggering, snippets with tab stops.
- An undo tree, buffer-local key layers, and per-cursor registers and visual selections.
- Optional modules: Markdown helpers, per-format outlines, markup conceal detection.

Winter-specific, staying in Winter on Cherry's host traits: the language server client, git changes, diff and merge, commit and rebase editors, LaTeX and BibTeX, spelling dictionaries, Docker, and the editor tab's plumbing.

## 2. Winter's Choices As Options

Where Winter and Cherry act differently, the difference is Winter's taste, so Cherry gains an option and keeps its Vim defaults:

- The unnamed register is the system clipboard (`unnamed_clipboard`).
- Search is literal text, not a regular expression (`literal_search`).
- With word wrap on, `j` and `k` move by screen rows and `gj` and `gk` by lines (`screen_row_motions`); wrapped lines keep their indent (`break_indent`).
- The scroll margin (`scroll_margin`), and `H` and `L` going to the very top and bottom rows inside it, the view left where it is (`edge_row_motions`).
- `Esc` drops a pending count or register first (`escape_drops_prefix`).

Insert mode is entered with Vim's whole set of keys (`i a I A o O R s S C c`) in Winter too, so it needs no option.

Single-chord bindings Winter prefers (`C-/` to comment, `C-s` to save, `M-i` to select a paragraph) are key bindings the host makes, not options; so is `q` leaving Visual mode, a binding of `q` to `exit-visual` in the visual modes.

## 3. Steps

Each step lands with tests and passes `make check` and `make test`.

1. Highlighter hardening: highlighting that prepares the lines in sight a budget of lines at a time and says more work is pending (through `Editor::deadline` and `tick`); a host's own styles numbered as `HighlightId`s. A test double that colors line by line, as syntect does, proves it. The queries a grammar can answer (where definitions start, the scope at an offset) come with the steps that use them: sticky scroll and outlines.
2. Host plumbing: a host-handled command request, notice severity, a public selection setter, hit testing from a cell to an offset, and per-view options (wrap, line numbers).
3. The options of section 2.
4. The small generic gaps of section 1.
5. Paint model v2: gutter lanes, line number styles, the extra layers; then indent guides and occurrence lighting on them.
6. Completion v2.
7. Structural editing.
8. Buffer-local key layers, and per-cursor registers and visual selections.
9. Virtual lines, pinned rows, and conceal modes.
10. The undo tree, and fields of several rows.
11. The optional modules.
