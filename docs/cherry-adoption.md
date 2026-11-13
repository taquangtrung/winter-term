# Cherry Adoption Plan

Status: in progress. The decisions of section 5 are confirmed. Step 1 is built: `EditorView` draws a host's `HighlightStyle`s (highlighted text, inlays, virtual lines, `Background` washes, signs of a host's own kind), matching brackets, extra cursors, and a preedit laid into the line by `Editor::set_preedit`, in a font of the host's choosing (`set_font`), and gives a host's popups their anchors (`cursor_rect`, `offset_rect`). Step 2 is built: `winter-syntax`'s `SyntaxHighlighter`, its line states kept on the coloring thread, as the grammar engine's registers may not cross threads. Step 3 is built, as a swap rather than a build alongside: `TextEditor` is a view of a document in one Cherry editor per window thread, each tab on a buffer with a Cherry view of its own; the old core and every feature built on it are deleted, for steps 4 to 6 to rebuild. Step 4 is built: each document is synced with the server serving it (opened, its whole text handed over a moment after it changes, saved, closed with its last tab); every checker's findings are decorations and signs; the server is the document's completion provider, its asks noted during the key and sent after it; and go to, references, hover and signature cards (drawn by the tab from `cursor_rect`), rename, formatting (before a save too), and fixes run as before. Expand selection stays Cherry's own, as `SelectionRangeProvider` answers at once and a server cannot; inlay hints and lenses come with step 5. Step 5 is built: Markdown's markers concealed as the markup-markers setting asks; indent guides and sticky scroll on Cherry's options; git signs on a lane of their own; spelling on decorations, checked as the text changes; inlay hints as inlays and lenses after their lines; color swatches as washes, one host style per color, `EditorView` coloring a wash's text where its style sets a text color; and an extension's inline suggestions, ghost text as an inlay at the cursor with its later lines as virtual lines, a next edit as a card, the document handed to the extension's servers before each ask. Ghost text lies on the document, so a second tab on the same file shows it too. Step 6 is under way: a commit message and a rebase plan have keys of their own as Cherry host commands bound in their documents (a message finished, cancelled, its summary marked past 72 columns, earlier messages recalled; a plan's actions set by letter, its lines moved, refused where git would refuse it), and a file's merge conflicts are stepped through, their markers banded, and resolved by keys bound in the document once it holds any. The diff and merge editor is next. Grep's edit in place runs on a multi-row field, as wdired's renames do, so it moves with the fields in step 7.

Cherry's extraction plan (`docs/winter-extraction.md` in Cherry) moved every generic editing behavior of Winter into Cherry, and every step of it is built. Misty's extraction plan rebuilt Winter as a misty app, and every step of that is built too: Winter draws through misty on its `misty-adoption` branch. This plan is the last move: Winter's text editing runs on Cherry, drawn by `winter-cherry`'s `EditorView`, and Winter's own editing core is deleted. misty and Cherry know nothing of each other: `winter-cherry`, in Winter's workspace, is the one link between them, and anything either needs of the other is built there. Winter's rule holds throughout: subsystems are replaced in one pass from this design; the old code is read for behavior, never adapted.

## 1. Where Winter Stands

Winter edits text with:

- `winter-textedit` (9,000 lines): `TextBuffer` with its undo tree, `ModalEditor` (the Vim grammar, the Emacs keys of Insert mode, visual mode, operators, motions), `TextField` (the single-line and multi-row fields), `KillRing`, `Recall`, `Search`, and the `vim` helpers (`nav`, `objects`, `words`, `jumps`). Everything reaches it through `winter_model::textedit` and `winter_model::vim`.
- `winter-text` (37,000 lines): `TextEditor`, the editor tab, holding a `ModalEditor` and its documents. About a third of it is generic editing glue Cherry now has (the `:` line, multiple cursors, expand selection, sexps, reflow, query replace, occur, comment and list continuation, screen-row motions, recentering, undo branches, dabbrev). The rest is Winter's own: the language server client and what it shows (hints, lenses, hover, peek, fixes, problems, symbols), syntect highlighting through `winter-syntax`, the completion popup and its sources, conceal, indent guides, sticky scroll, swatches, git signs, diff and merge, the commit and rebase editors, LaTeX, BibTeX, Markdown, spelling, dictionary lookup, inline AI suggestions, local history, remote files, send to terminal, point actions, and per-format outlines.
- The fields: Winter's `TextField` backs the prompts, the overlays (rename, theme name, password), the terminal's search, the grep buffer's query, the keys page's filter, the remote file prompt, and `winter-dir`'s wdired renames (a multi-row field). Only the picker's query is misty's.

An editor tab draws as a `paint::Frame` (rows of styled spans, a gutter, icons) that the window lowers into the cell grid misty draws, with its popups (completion list, info card, toolbar, drop list) drawn by misty's widgets. Keys reach it through `Page::on_key` after the window's own sequences, and it answers an `EditorOutcome`.

What is not text editing, and stays on Winter's key engine (`winter-keys` and `winter_model::input`): `keybindings.kdl` with its `when=` conditions, the window's commands, the terminal's Normal and Visual modes (Vim motions over the emulator's grid, operators that edit the shell prompt through the PTY), and the list keys of the Dir, Git, Docker, and Monitor pages. None of them edits a document, and cherry-keys has neither a KDL reader nor conditions.

## 2. What EditorView Needs First

`EditorView` draws a Cherry view with misty, but colors only what Cherry itself names. Before Winter draws with it, it gains, in `winter-cherry` with headless tests:

1. Host colors: a host's `HighlightId`s (syntect scopes, a language server's semantic tokens), `Style::Background` washes (a diff's lines, conflicts, the reading wash), and signs by lane and kind (git signs, breakpoints) take colors the host hands the view, as a table it can change with the theme.
2. Inlays and virtual lines drawn: inlay hints, ghost text, code lenses, and inline diagnostics, each in a host style.
3. The styles it drops today: `MatchingBracket` and `ExtraCursor`.
4. Anchors: the screen rect of a document offset and of the cursor, so a host hangs its own popups (an info card, a toolbar, a drop list) from the text, as Winter does.
5. Input methods through `Editor::set_preedit`, so the composition is laid into the line as Cherry lays it, not drawn over it.

Each lands with `winter-cherry`'s tests and Winter's workspace suite.

## 3. The Shape In Winter

- One `cherry::Editor` per Winter process. A Winter Buffer is a Cherry document, shared by every tab that shows it and buried (closed) when its last tab goes; an editor tab is a Cherry view.
- `TextEditor` keeps its name and its place as a `Page`, but holds a document and a view instead of a `ModalEditor`. It draws through `EditorView` in its pane's body instead of answering `content()`. The window's frame leaves the pane to it, as it already does for the settings page.
- Highlighting: `winter-syntax` implements Cherry's `Highlighter` over syntect, preparing the lines in sight a budget at a time, its scopes numbered as `HighlightId`s and colored by Winter's theme through the host colors of section 2.
- Keys: the window's sequences run first, as today. A key they leave goes to `Editor::handle_key`, and `Handled::Ignored` goes on to the window. Winter's editor commands that are not editing (the language server's, git's, send to terminal) are host commands (`register_host_command`), bound with `bind`, and come back as `HostRequest::Command` for the tab to carry out. The user's editor keys in `keybindings.kdl` become `override_keys` on load and on every reload. The keys page and which-key read `help_rows` and `key_hints`.
- Requests: `take_requests` after each key maps onto `EditorOutcome` (save, quit, submit, open, notices) where the window acts on them.
- Options: Winter's choices are Cherry options: `unnamed_clipboard`, `literal_search`, `screen_row_motions`, `break_indent`, `edge_row_motions`, `scroll_margin`, `escape_drops_prefix`, and the rest from `settings.kdl`'s editor block. `C-/`, `C-s`, `M-i`, and `q` leaving Visual mode are Winter's bindings.
- Language servers: text goes to the server through `set_change_tracking` and `take_changes`; diagnostics are decorations; inlay hints are inlays; lenses are virtual lines; completion is a `CompletionProvider` answering `Later`; expand selection is a `SelectionRangeProvider`. Hover, peek, and fixes stay Winter's popups, hung from the anchors.
- Winter's other features sit on Cherry's API: conceal on `set_conceals` (Markdown's from `cherry-markup`), guides and sticky scroll on Cherry's options, git signs on `set_signs` lanes, spelling on decorations, inline AI suggestions on inlays and virtual lines, outlines on `cherry-markup` where it covers the format and Winter's own outlines elsewhere.
- Fields are Cherry single-line documents (`open_single_line_document`, `FieldEditing::Modal` as Winter's explicit modes ask), drawn by `EditorView`; wdired's renames are a multi-row field.

## 4. Steps

Each step leaves Winter building with its workspace tests passing, and is checked on screen on Xvfb.

1. `EditorView`'s needs (section 2), in winter-cherry.
2. Cherry in Winter's workspace (path dependencies on the branch, as misty is), and the syntect `Highlighter` in `winter-syntax`, tested against the scopes it colors today.
3. The editor tab: `TextEditor` on a Cherry document and view, drawn by `EditorView`; keys, requests, options, and the user's keys; Buffers as documents. Winter's generic editing glue in `winter-text` (section 1) is deleted, not ported.
4. The language server on Cherry's API.
5. The display features: conceal, guides, sticky scroll, swatches, git signs, spelling, hints, lenses, inline AI suggestions.
6. The editors built on the text editor: diff and merge, commit and rebase, and the grep and build buffers.
7. The fields, wdired's included.
8. `winter-textedit` deleted. The `vim` helpers the terminal and the lists still use (`nav`, `objects`, `words`, `jumps`) move into `winter-model`, beside the key engine that uses them.

## 5. Decisions

1. Highlighting stays syntect, as Winter has it, behind Cherry's `Highlighter`; `cherry-syntax`'s tree-sitter highlighter is not adopted. The alternative is tree-sitter, which gives the structural queries (sticky scroll, outlines, text objects) for the languages it has grammars for, but changes every color a user sees.
2. The terminal's Normal mode and the list pages stay on Winter's key engine. Moving them onto Cherry (the scrollback as a read-only document) is a later plan, if ever: the grid, the prompt-editing operators, and row actions have no Cherry counterpart.
3. `keybindings.kdl` stays Winter's format, read by Winter, which hands Cherry its editor keys; cherry-keys gains no KDL reader.
