# Picker framework design

This document is the agreed design for rebuilding Winter's picker as a framework of its own, in the manner of Emacs' Helm but native to Winter: none of its code comes from another editor. It describes a plan, decided on 2026-11-04, not the code as it stands; [architecture.md](architecture.md) describes the code.

## Problem

The picker is one monolith. `PickerMode` has 27 variants, each row's action is a string, and what every picker does lives in one large `match` in `crates/app/winter-term/src/window/picker/mod.rs`. Some pickers load on the window's thread before they appear (the project file picker runs `git ls-files`, then filters up to 50,000 files on every key), and no picker has an action menu, a preview beside its list, a way to resume it, or a way to save what it found.

## Approach

A new crate, `winter-picker`, holds the framework. Sources return outcomes and the window carries them out, as an editor returns an `EditorOutcome` (`crates/base/winter-model/src/editor/mod.rs`) for the window to act on. The framework knows nothing of the window, so it is tested on its own.

## Vocabulary

| Term           | Meaning                                                    |
|----------------|------------------------------------------------------------|
| Picker         | The framework.                                             |
| A picker       | One list on it: the file picker, the command palette.      |
| Picker session | One opening of a picker: query, rows, selection, scroll.   |
| Source         | Where a picker's rows come from.                           |
| Row            | One choice in the list.                                    |
| Action         | What a row can do. `Enter` runs the first.                 |
| Buffer         | Any editor's contents, such as rows saved from a picker.   |
| Live picker    | A picker about its pane, previewing in it: Scout, outline. |
| Side panel     | A preview drawn inside a picker, beside or under its list. |

The command palette keeps its name in the UI; it is a picker like the others. Users meet "pickers"; only contributors and extension authors meet the framework.

## The `winter-picker` crate

The crate depends on `winter-model` for the query field (`TextField`), the edit history, keys, icons, and `Background` work. The scorer, `score_and_positions`, moves into it; its one other user, the Keys editor in `winter-term`, imports it from there.

- `Picker<O>` is one picker session: its title, the query field and the history of its picker (each picker keeps its own, so `M-p` in project search never brings back a command), its sources, the rows on screen and the counts the matcher last sent, the selection and scroll, and whether it is working, done, or stopped at its cap. It holds a list of sources, one for now, so several can share a list later.
- A row is what is drawn (see Rows) and a key naming it to its source. The matcher holds the text of each row's matched fields to score; the typed data behind it (a path and line, a tab) stays in the source, so rows stay plain and actions still get typed data.
- A picker is built with a `match_on` flag for each of its sources, naming the fields of the source's rows that the query is matched against and highlighted in: any of `Label`, `Description`, and `Detail`, `Label` when it is not set. It is set per source so that sources sharing one list later can have rows of different shapes. The one flag decides both, so a picker can never highlight where it did not match. Project search is built with `Detail` and `Label`, its pattern meeting the detail and its `@` file filter the label; Scout over text or a terminal is built with `Description`; and the table under Rows lists the rest.
- `Source<O>` declares how its rows come: read when the picker opens, streamed from background work, or streamed again on each query. It declares how they are matched: by words (the default, see Matching), by the source itself, or not at all where the query is the answer (a mux session's name, a build command, a workspace's name).
- `Source<O>` declares whether it is a listing or a search, which decides what resume does, and whether its picker session can be resumed at all.
- `Source<O>` declares its preview: none, live (the picker is a live picker), or a side panel.
- `Source<O>` may declare whether its rows rank recently used first (see Recently used first), and which of a row's actions show as buttons on it (see Row buttons).
- `Source<O>` offers `actions(row)`, the first of them the default; `run(action, row, query)`, which returns an `O`; `preview(row)`, which describes what to show (a file at a line, a buffer, a tab, text) and never opens it; `export(rows)`, optional; and `refresh()`, run on resume.
- `Picker::on_key` answers consumed, close, run(O), preview (the description, for the window to show), save(O), or ignored.
- A picker that is a fixed list with one thing to do on a row (panes, Open With, workspaces, mux sessions, an editor's question) is built in one call from its rows and that one action, with no `Source` of its own to write. `pick_from` for extensions is this, underneath.
- The action menu is a second list in the same picker session: typing filters it, `Enter` runs the action, `Esc` goes back to the rows.

## Rows

A row has a label, and may have a description, a detail, a hint, and an icon. A source chooses which it fills, so its rows take the shape that suits them.

- The label is what the row is, drawn first.
- The description follows the label on its line, dimmer: a file's directory, a buffer's kind.
- The detail is a second line under the label, for text too long or too uneven to share the label's line. A row with no detail takes one line, as today.
- The hint is drawn dim at the line's right, for a key chord or a pane's digit only. Workspaces, which put their folders in the hint today (`window/workspace.rs`), move them to the description.
- The rows on screen are counted in lines, so scrolling, paging, and the selected row's band suit rows of one line and of two alike.

Rows of the searches take two lines, since file names are of uneven length and text after them would start at a different place on every row:

- The first line is the place: the file's name and its line and column (`mod.rs:184:13`), with its directory dimmed after them.
- The second line is the matched text, its leading indentation dropped, and cut around the match when it is longer than the line so the match is always in view.
- Project search and the Todo picker have rows like this. Project search runs ripgrep with `--column` for the column, which it does not ask for today (`window/picker/live_search.rs`); that comes with its move in step 1. The Todo picker runs ripgrep too (`window/picker/todo.rs`) and gains `--column` with its move in step 3.

Rows that are a line number and its text take one line, since line numbers line up: Scout's label is the line's number and its description the line's text.

The `match_on` flag a picker is built with (see The `winter-picker` crate) names the fields of its rows the query is matched against: the label, the description, the detail, or more than one of them. Matched letters are highlighted in those fields and never in the others, so a search's place is never lit by a word typed to find its text. A picker built without it matches its label.

| Picker                    | Matched against                                    |
|---------------------------|----------------------------------------------------|
| Command palette           | The label: the command's title, never its chord.   |
| Project files, find file  | The label: the path.                               |
| Recent, buffers, panes    | The label.                                         |
| Outline                   | The label: the heading.                            |
| Scout over text, terminal | The description: the line's text, not its number.  |
| Scout over a listing      | The label: the entry's name, as in Dir.            |
| Project search            | The detail by pattern, the label by `@` filter.    |
| Todo                      | The detail: the comment.                           |
| Locate                    | The label: the path.                               |
| History, clipboard ring   | The label.                                         |
| Workspaces                | The label: the name, not its folders.              |

Project search's pattern is matched by ripgrep, not the picker, but it is highlighted by the same rule: in the detail, its second line. The Todo picker's keyword is highlighted in the detail too, beside the words typed to filter it.

## Matching

The query is words separated by spaces, matched in any order: `pick rs` finds `src/picker/mod.rs`. This is how the picker matches today, and it keeps its scoring.

- A word matches where it appears, in any case.
- A word written `/like this/` is a regular expression, as today.
- A word starting with `!` leaves out the rows it matches: `!test` hides every row holding "test", and `!/^tmp/` every row a regular expression matches.
- Text in double quotes is matched as written, spaces included and `/` read as itself: `"fn main"`.
- A row's matched letters are highlighted (see Rows); a word that leaves rows out highlights nothing.

These rules are for the pickers that match by words. A source that matches by itself keeps its own query, as it does today: project search reads its pattern as written with `*` for anything, ignores case until the pattern has a capital, and takes a file filter after ` @ ` (`docs/usage-guide.md`). A Grep buffer saved from it searches again (`C-r`) by those same rules, so a saved search finds what the picker found.

## Errors

A source that fails says so in the picker's header and keeps the rows it has: ripgrep or git not installed, a regular expression that does not parse, a directory that cannot be read, an extension that stops answering. The picker stays open, so the query can be fixed; a failure never closes it or reaches the window as a panic. A source that cannot start at all, such as project search without ripgrep, opens its picker with the reason in place of rows, as the file pickers say "no files listed" today.

## Concurrency

The picker never waits on its source, and the window never waits on the picker.

- Every picker opens at once, with a quiet working mark in its header until its rows are in.
- A source that reads the disk or runs a program streams its rows in batches from background work: files, project search, locate, Todo, and extensions. A source listing what the window already holds (commands, tabs, panes, the clipboard ring) reads it when the picker opens, as does one handed its rows by an editor (the Git editor's branches), since that is instant.
- Typing before the rows are in filters the rows so far, and each batch is filtered as it lands.
- Closing a picker stops its work; resuming it goes on with it (see Resume).

### The background matcher

Each picker session that matches its rows has one worker thread that owns their text and does all the scoring, with Winter's own scorer, so ranking stays as it is today.

- Every query has a generation. Work on a query that a newer one replaced is dropped, and an answer from an older generation is ignored, so a slow match on "fo" never lands over the one for "foo".
- A query that only narrows the last one (a word added, a plain or quoted word lengthened, a `!` word added) scores only the rows that matched before; any other change scores every row, a regular expression lengthened included, since `/a|/` to `/a|b/` matches more.
- A new batch is scored against the current query and merged into the sorted matches; the rows already scored are not scored again.
- Rows that score the same keep the order they came in, so a later batch never shuffles what is already shown.
- The worker scores only the fields each source names (see Rows).
- The worker sends back only the rows on screen, with their matched letters, and the counts matched and in all, and whether it is still working. Scrolling asks for another slice.
- A source that matches by itself (project search, which runs ripgrep on each query) runs on the same worker and generations. A picker whose query is the answer has no rows to match and no worker.

### Silent updates

- The selected row stays selected while rows arrive, and the view does not scroll under it.
- Before the selection has been moved, it stays on the best match.
- The header's count rises as rows come in ("120 of 4,532").
- Redraws are coalesced to one a frame.

## Preview

There are two kinds, and a source declares which it offers. Both follow the selection, and neither opens anything, so a preview never leaves a buffer behind in `C-x C-b`.

### Live preview

A live picker is a special kind of picker about the pane it is in: Scout and the outline. It sits at the bottom of the focused pane, as Jump does today, and the pane above it shows the selected row's target at full size, scrolled so the target is in view above the picker. Its live preview is always on: it is what the picker is for, so it cannot be turned off.

- A live picker only ever shows the pane's own tab, so its preview moves that tab's view and reads nothing.
- When the picker closes, the pane shows what it showed before, a terminal included, unless `Enter` chose the row, which goes to the target for real.
- While Scout is open, its pane highlights what the query matches: every match on screen in the search colour, and the selected row's match in the current match's colour, as a `/` search does. It uses the window's search highlighting (`search_matches` and `search_current` in `window/frame/overlays.rs`), drawn over what the tab paints, so every editor and a terminal get it without drawing a match themselves.
- The matches on screen follow the picker's word rules (see Matching): each word, quoted phrase, and `/regular expression/` is highlighted where it is found, and a `!` word highlights nothing. Only the rows on screen are matched, as the view moves.
- The highlights go when the picker closes, whether `Enter` chose a row or `Esc` dismissed it.

### Side panel

Every other picker sits where it does today, most in the middle of the window. A source with a side panel shows the selected row's preview in a panel inside the picker: beside the list where the window is wide enough, under it where it is not. The panel is hidden until `F6` shows it; `F6` shows or hides it, and each picker remembers which. The panel follows the selection debounced, so a quick run down the list does not read every file, and it reads files in the background.

- The panel is a read-only view of text drawn inside the popup with Winter's own highlighting: a file centred on the row's line with the match marked, a buffer, or a tab. Images and PDF pages in the panel come later.
- A row with nothing to show leaves the panel empty.
- Project files, project search, buffers, recent files, and Todo offer the panel.

## Recently used first

A source may ask for its rows to rank recently used first: the command palette and the file pickers do.

- With an empty query, the rows used most often and most lately come first.
- With a query, rows that score the same rank by the same measure, before the order they came in.
- What was used is kept per picker with the window's other recent state, across restarts.

## Resume

The window keeps the last closed picker session that can be resumed, and `C-M-r` resumes it with its query, selection, and scroll.

- A listing (commands, tabs, buffers, files) reads its rows again, so a buffer killed or a file deleted since is gone; the selection goes back to the same row if it is still there.
- A search (project search, locate, Todo) keeps its rows and goes on past them where it stopped early, closed while working or stopped at its cap, leaving out the rows it has already shown. Each resume raises the cap by a batch.
- A picker session that answered a question cannot be resumed: an editor's question (a branch to check out), every extension's picker in the first release, and the pickers whose query is the answer. Once answered or dismissed there is nothing left for them to answer, so `C-M-r` passes over them to the last session that can be resumed.

## Save to a buffer

`C-S-s` copies the rows matching the current query, in the order shown, into a new Grep buffer, and closes the picker. It searches no further: resuming the picker session is what continues a search.

- Project search and the Todo picker can save: their rows are lines of files, and their sources export them. Scout does not save in this design. Any other picker says there is nothing to save.
- The Grep editor gains a constructor taking the rows as `SearchHit`s and the query, so the buffer has `C-x C-q` editing, `M-%` replacing, and the rest of Grep.
- The buffer is named after the query, its header says when the picker had stopped at its cap, and it shows in a tab beside the one the picker was opened from. Closing that tab buries it, as every buffer is; `C-x C-b` and `C-S-z` bring it back.
- Each save makes a buffer of its own; saving the same query twice gives two.
- A buffer saved from the Todo picker is the Grep editor's Todo list (`GrepEditor::todo`), so `C-r` searches its keywords again. The words the picker filtered its rows by do not carry over, so `C-r` there finds every keyword again, not only the rows saved; the header says what was filtered.

## Keys

Every key is one chord. `Tab` stays with completion, and `C-z` with the terminal. The keys a picker has today stay as they are: `C-n` and `C-p` and the arrows move, `M-p` and `M-n` bring back earlier queries, `Esc` closes, and the query takes the Emacs keys of every input field. The table names what is new.

| Key                 | Where          | Does                                         |
|---------------------|----------------|----------------------------------------------|
| `Enter`             | In a picker    | Run the row's first action.                  |
| `F4`, `C-Enter`     | In a picker    | Open the action menu.                        |
| `F6`                | In a picker    | Show or hide the side panel.                 |
| `C-S-s`             | In a picker    | Save the rows to a buffer.                   |
| `C-M-r`             | Anywhere       | Resume the last picker session.              |

### Defined like every editor's keys

A picker's keys are defined the way every editor's are, so they are found and rebound where all of Winter's keys are. Today they are written into `window/picker/mod.rs` and can be neither listed nor rebound.

- The picker's own keys are a `KeyTable` (`winter-model`'s `input::table`) with the block `picker`: each a `Binding` with an id, its default keys, and what it does (`action_menu`, `toggle_panel`, `save`, `next`, `previous`, `history_previous`, `history_next`, `close`).
- Each source with actions has a table of its own, under the picker's block as a dotted name, as `dir.actions` and `grep.edit` are under theirs (`picker.buffers`, `picker.project_search`), so no picker's table can share a name with an editor's. It has a binding for each action the source offers, and a binding there runs its action at once, with no menu: `commands "picker.buffers" { "kill_buffer" "C-k" }` kills the selected buffer. A source's actions have no keys by default, so the action menu stays the way to them until one is bound.
- The tables join `KEY_TABLE_GROUPS` (`editors/mod.rs`), the one list the Keys editor shows and `keybindings.kdl` rebinds by, so `F1 k` names a picker key, the Keys editor lists them under the picker's title, and `commands "picker" { "toggle_panel" "F5" }` rebinds one, the same as `commands "dir" { "toggle_archive" "M-z" }`.
- `C-M-r` is the window's `picker_resume`, in the `window` block with the window's other chords.
- A source's own key table is how an extension's actions get keys too, once extensions can add actions in a later release.

`F6` is Winter's preview key already (Markdown preview, SyncTeX forward); in a live picker, whose preview is always on, it does nothing. `C-S-s` is Winter's save key (the text editor writes its file). `C-M-r` is `picker_resume`, titled "Picker: Resume Last Session", and like `C-M-z` it is the window's, never the shell's. The window-wide `C-S-s` that opens Grep goes; the command "Grep: Search Files" stays.

## Row buttons

A source may mark some of a row's actions as buttons: the row under the pointer, and the selected row, show them as icons at its right, and a click runs one. The buffer picker shows × to kill a buffer; the file pickers show a split to open the file beside. Buttons repeat what the action menu offers, so the keyboard never needs them.

## Key hints

A dim line under the rows names the keys the picker offers now, and only those: `F4 actions · F6 panel · C-S-s save`. A picker with no action menu, panel, or save leaves them out, and a picker offering none of them shows no line.

## Testing harness

The crate ships what a source's tests need, so the tests of every picker stay quick and certain however much of the picker runs in the background.

- A fake source whose rows, batches, failures, and pauses a test scripts.
- Keys fed in as a script, and the replies of `on_key` collected in order.
- A synchronous mode, in which the matcher and a source's batches run on the calling thread and are done before the call returns, so a test never sleeps or waits on a thread.
- The background matcher's own tests run it threaded as well, since what they check is the threading.

## Speed targets

The concurrency is held to numbers, checked by benchmarks that run from step 1 on, before the other pickers come to depend on it.

- A picker appears in the frame of the key that opens it.
- With 50,000 files listed, and with a terminal's 1,000,000 lines of scrollback in Scout, the rows on screen follow a key within one frame.
- Opening Scout over that scrollback copies its lines, the one part done on the window's thread, within one frame.
- A key typed while a batch is being scored is never delayed by it.

## Changes to `winter-term`

- `WindowOutcome` is the window's `O`: open a path at a line, go to a tab, run a command, paste text, answer an editor's question, call an extension back, and save rows to a buffer.
- Each picker of today becomes a source under `window/picker/sources/`. `PickerMode` and the large `match` go once the last has moved.
- Jump and Swoop become one live picker, **Scout** ("Scout: This Tab", titled "Scout: notes.md"): it searches the tab's text as you type and scouts each match in place before you go to it. Today both list a terminal's lines with `extract_swoop_lines` and preview them alike; only Jump keeps the line clear of the picker (`reveal_position_in_uncovered`). Scout has one source per kind of tab: an editor's own jump targets, a terminal's lines. `C-j` opens it, and `gs` in a terminal's Normal mode stays as a second key. The stored action id `jump` stays, so keybinding files keep working, with `swoop` read as it; `PickerMode::Swoop`, `update_swoop_preview`, and the names Jump and Swoop go. Going to a row is still a jump, recorded so `C-o` comes back.
- `Overlay::Picker` holds a `Picker<WindowOutcome>`. The editor's question and the extension's list it carries now become two sources.
- Where a picker sits follows its source: a live picker at the bottom of the focused pane (`PickerPlacement::EditorBottom`); an editor's question where the editor asks for it (centred, at the cursor, or at the bottom, as `PickPlacement` says today); every other picker in the middle of the window (`PickerPlacement::WindowCenter`). `PickerKind` is replaced by what the source declares.
- `PickerView` is drawn as it is, with rows of two lines, the description, and the side panel added to it. The action menu reuses the list at the cursor (`widgets/popup.rs`), hung from the selected row.

## Extension API

The first release gives extensions their own rows over time and their own actions; the rest waits until the `Source` trait has settled.

- `Host::open_picker` takes fixed rows, or a stream the extension feeds and then marks done, and can ask for the query as it changes (debounced) to compute its rows again.
- A row has the shape of every row (see Rows): a label, and a description, a detail, and an icon if the extension gives them. It can carry several named actions and a side-panel preview. The extension is called back with the action chosen and the row's value.
- `pick_from` stays, built on `open_picker`.
- WebAssembly extensions get the same, through `winter-wasm` and `winter-ext-guest`.

## Order of work

Each step is one commit or more, and the tests pass after each.

1. The crate, the background matcher, the testing harness, the speed benchmarks, and the rows of Rows, with Commands, Project Files, and project search moved onto it.
2. The action menu, the live preview, the side panel, saving to a buffer, resume, the history of each picker, the `!` and quoted words, the key hints, and the picker's key tables. It ends with project search saving to a Grep buffer, the window-wide `C-S-s` gone, and both described in the usage guide, so the change this design began from is in use before the rest moves.
3. The other 23 pickers, Jump and Swoop made Scout, then `PickerMode` and `PickerKind` deleted.
4. Recently used first and row buttons, once their sources have moved.
5. The extension API, compiled-in and WebAssembly.
6. The rest of the usage guide, with the vocabulary above.

## Tests

- The background matcher: a stale generation never lands over a newer one; narrowing a query matches scoring every row afresh; merging a batch matches scoring every row at once; rows that score the same keep their order; the window thread never waits on the worker.
- The picker session: a refresh selects the same row again; a resumed search leaves out the rows already shown; `C-M-r` passes over a session that cannot be resumed; the action menu runs the action chosen; a picker whose query is the answer hands the query on.
- Preview: Scout highlights every match on screen and the selected row's in the current match's colour, a `!` word highlighting nothing, and the highlights go when it closes; closing a live picker gives the pane back as it was; a file previewed and not chosen leaves no buffer; a stale preview read never lands over the row selected since.
- Matching: `!` words leave rows out and highlight nothing; a quoted phrase matches with its spaces and its `/`; narrowing with a `!` word matches scoring every row afresh; lengthening a regular expression scores every row again; a Grep buffer saved from project search finds the same lines again with `C-r`.
- History: `M-p` in one picker never brings back another picker's query.
- Recently used first: an empty query lists the most used first; equal scores rank by use.
- Key hints: a picker shows only the keys it offers.
- Keys: every picker key and every source's action is listed in the Keys editor and rebinds through `keybindings.kdl`; a source's action bound to a key runs without the menu.
- Speed: the benchmarks above hold.
- Errors: a failing source keeps its rows and names the failure in the header; a regular expression that does not parse leaves the picker open.
- Rows: a word matches and lights only the fields its source names, so a project search word never lights the place and a Scout word never lights the line number; a search row shows its place on one line and its text on the next; a long line is cut so its match stays in view; paging moves by rows of two lines as by rows of one.
- Each source keeps what its picker's tests check today.

## Not in this design

The design leaves room for these, but they come later:

- Marking several rows and running one action on all of them.
- Several sources in one list, under headers.
- Extensions adding sources to Winter's own pickers.
- Extensions adding actions to the rows of other sources.
- Images and PDF pages in the side panel.
- Choosing per picker which kind of preview it has.
- Saving Scout's rows over a text file to a Grep buffer, since they are lines of a file there.
- Stepping through the last search session's rows after closing it, with `F8` and `S-F8`, as through a build's errors.
- Richer descriptions: a file's size and git status, a command's description.
