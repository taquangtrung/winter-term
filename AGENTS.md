# Agent instructions

Rules every agent working in this repository follows.

## Vocabulary

Winter uses one set of words from the UI to the code to the docs. Use them in everything you write: code, comments, docs, commit messages, and replies.

| Term | Means |
| --- | --- |
| Window | One window on the desktop. Never a pane, as Vim and Emacs use the word. |
| Pane | One leaf of the window's split tree: a strip of tabs, one shown. A split is the divider between panes, never a pane. |
| Tab | One entry in a pane's strip. It is a terminal, a page, or an editor. |
| Terminal | A tab running a program on a PTY. Its screen and scrollback are not a buffer. |
| Page | A tab drawn by Winter showing Winter's own state (Settings, Notifications, System). One of each per window, never buried. |
| Editor | A page whose contents are a buffer: a file, a directory, a repository, a build, a picker's rows saved as a list. The one that edits files is the **text editor**. |
| Buffer | An editor's contents, shared by every tab showing it and buried when its last tab closes. Never a kind of tab, never a terminal. |
| Buried | A closed tab kept alive out of sight: an editor's buffer, or a terminal with a program running. |
| Shown | Visible on screen: the front tab of a pane in sight, or the view a shown panel shows. A tab behind another in its strip, zoomed out of sight, or buried is not shown. A panel is shown when the user has it open. |
| Launcher | What a new split pane opens with, offering a terminal, a file, a directory, or git. |
| Main area | The window's split tree of panes, where every tab lives. |
| Panel | A sidebar or the bottom panel: outside the split tree, never split, showing views rather than tabs. |
| Sidebar | The panel at the window's left or right, showing sidebar views (Files, Git, Outline, Todo). |
| Bottom panel | The panel under the main area, showing bottom panel views (Output, Problems, Search). |
| View | What a panel shows, one at a time: a list drawn by misty, never on the cell grid, built for its panel. Never a tab. A view belongs to a tool and shares its data, not its layout. |
| Workspace | A project the window switches to and back from. |
| Picker | A list on the picker framework (the command palette, the file picker), fed by **sources**, made of **rows** (never "candidates"), each with **actions**. One opening of a picker is a **picker session**. |
| Tool | A feature with its own guide page, such as Dir, Git, or Compile. Never a kind of tab. |

A design doc may add terms for what it plans; use its words for that work, and add them here once built. Stored names kept for compatibility, such as `tool` in the status bar segment or `editor` as the text editor's kind in `keybindings.kdl`, stay as they are.

When the user, or any text you are working from, uses a term loosely, work from what is meant and write the right term. If the difference matters, say once which term you used and why ("the saved list is the buffer; its tab is an editor"). Ask only when the meaning is unclear.

## The user guide

The user guide lives in [`docs/guide/`](docs/guide/README.md), one page per tool, as Vim's help is split by topic. Keep it simple: it tells people how to use Winter, not how Winter is built.

- **One page per tool.** A new tool gets its own page, listed in `docs/guide/README.md`. Do not grow a page into a second tool's.
- **Open with what it is and how to reach it.** The first lines say what the tool is for and its key or command.
- **Keys go in a table.** One row per key, a few words for what it does. Then only the notes that change what the user does.
- **Say what people do and see.** Leave out threads, engines, file formats, timers, and other internals; they belong in [`architecture.md`](docs/architecture.md).
- **Plain words, short sentences.** One idea to a bullet. Leave out edge cases unless they would surprise someone.
- **Do not repeat.** Link to the page that covers a thing instead of explaining it twice.
- **Keep it true.** When a change alters a key, a command name, or what a page shows, update its guide page in the same change.

## Tooltips

misty gives cut text its full wording as a tooltip, and drops a tooltip that only repeats its element. Each page's test asserts `misty::testing::unnamed_controls` is empty, so a control with only an icon fails until it has a name. The rest is judgement:

- **Explain what a reader can't decode.** An abbreviation (`RSS`), a bare number (`12:5`, a load average), or a glyph gets a tooltip that says it in words.
- **Say what color alone means.** A state or a level shown only by color gets a tooltip naming it.
- **Name the key.** A control a key also runs says it, in Winter's style: `New Tab (C-S-t)`.
- **Case follows what it is.** A tooltip naming a command is in Title Case, as the command is (`Zoom In`); one describing something is a sentence, with no closing period (`Changed since the last commit`).
- **Never only in a tooltip.** Keyboard users never see tooltips, so anything they need must also show on screen or be reachable another way.
- **Use the shared hooks.** `Element::tooltip` or `item(..).tooltip(..)` on pages, `Page::status_tooltip` and `Page::gutter_tooltip` for editors, `Sign::with_note` in Cherry. Do not draw a tooltip by hand.

## Passive and lazy

Winter does no work nobody can see the result of. A quiet window costs nothing.

- **Passive: never poll.** React to events: file notifications (`FileWatch` in `winter-base` for one file, Dir's `DirWatch` for a folder, the shared `TreeWatch` over a repository), a job's answer, a key. A timer that reads something again to see whether it changed is a bug; use a watch, and debounce its bursts as those two do.
- **Lazy: nothing not shown.** A page, editor, or panel view that is not shown sets up no watch, reads nothing, and asks for no tick. The window tells it each time it is shown or stops being shown (`Page::set_shown`, once a frame from `sync_shown`), and it catches up once when shown again. Work already running, such as a build or a search, goes on.
- **Cheap to make.** A constructor stores what it is given; the first read and the first watch wait until it is first shown.
- **Feed only what is shown.** What the window hands a view each frame (an outline, a build's lines) goes only to views shown.
- **The exceptions are live by nature.** The Process and System monitors refresh on a timer while shown, and a job in flight is checked until it answers (`background::POLL`). A new exception needs the same reason.
- **Test it.** Anything that watches or ticks has a test that its `next_tick` is `None` while nothing changed and while not shown.
