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
| Launcher | What a new split pane opens with, offering a terminal, a file, a directory, or git. |
| Workspace | A project the window switches to and back from. |
| Picker | A list on the picker framework (the command palette, the file picker), fed by **sources**, made of **rows** (never "candidates"), each with **actions**. One opening of a picker is a **picker session**. |
| Tool | A feature with its own guide page, such as Dir, Git, or Compile. Never a kind of tab. |

A design doc may add terms for what it plans (the panels design's main area, panel, sidebar, and bottom panel); use its words for that work, and add them here once built. Stored names kept for compatibility, such as `tool` in the status bar segment or `editor` as the text editor's kind in `keybindings.kdl`, stay as they are.

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
