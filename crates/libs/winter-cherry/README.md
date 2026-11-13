# winter-cherry

Winter's link between the [misty](../../../../misty) GUI framework and the [Cherry](../../../../cherry) editor, which know nothing of each other. Misty owns its key values and resolves no key presses; this crate resolves them with Cherry's keymap and edits text with Cherry's modal editor:

- `keys`: converts misty's keys to Cherry's and back, without loss.
- `keymap`: binds a misty `CommandRegistry` into a `cherry::keys::Keymap`, and turns a pending prefix's continuations into which-key rows.
- `EditorView`: one view of a Cherry `Editor` as a misty widget, with mouse handling, an input method, decorations, signs, and Insert-mode completion. `EditorClipboard` hands the editor misty's system clipboard.

Cherry is re-exported as `winter_cherry::cherry`.

## Features

| Feature | What it adds |
| --- | --- |
| `syntax` | Syntax highlighting in `EditorView` through Cherry's `syntax` module, colored by the theme's syntax colors; turns on `cherry/syntax` and builds the `editor` example. |

## Examples

The `chrome`, `palette`, and `settings` examples resolve their keys with Cherry's keymap; `editor` is a small text editor. From the repository root:

```bash
cargo run -p winter-cherry --example palette
cargo run -p winter-cherry --features syntax --example editor -- notes.txt
```
