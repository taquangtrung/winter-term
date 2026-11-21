# Misty And Cherry Integration Plan

Status: built. Step 1 is built: the input handlers take misty's buttons, wheel deltas, points, resize edges, and modifiers, and winit has left the app. Step 2 is built: an editor converts its key to Cherry's in one place, `winter_field::misty_key` then `winter_cherry::keys::to_cherry`, and `winter-field`'s own copy of the conversion is deleted. Step 3 is built: panes, viewports, and layouts are `misty::Rect`, `PaneRect`'s methods are the free functions `grid_area`, `text_area`, `cell_box`, and `cell_at`, and nothing copies one rectangle into another field by field. Step 4 is built: the renderer keeps misty's theme in `Themes` beside Winter's, rebuilt only by `set_theme`, and every draw reads the one kept. Step 5 is built: an info card's and the side panel's lines are `StyledText`, built in their colors, weights, and marks, and the renderer emits `misty::Cell`s that misty reads as they are; the status bar is built straight as misty's `StatusLine` and `StatusSegment`s, its quota icons held as SVG data with `with_svg_icon` and a quota that failed to refresh marked `with_stale`, and the renderer's rectangles of one color are misty's `SolidFill`, all of which (the status line's getters, `with_stale`, `with_svg_icon`, and `SolidFill`) arrive in misty 0.0.2, which Winter now asks for. Step 6 is built: each menubar item shows the keys the keymap binds its command to, none for an unbound one, read as the menu opens and again when the keybindings change; and a `keybindings.kdl` line with `in="editor"` (or `editor.insert`, `editor.visual`, and the other modes' blocks) names a Cherry command, which until now was reported unknown and dropped, its keys read in Winter's notation, written in Cherry's, and put in force with `override_keys` on the window thread's editor on load and on every reload, a command no longer bound getting its own keys back. Every step of this plan is built.

Winter runs on misty and edits text with Cherry, as the misty extraction and Cherry adoption plans built. Both now come from crates.io (misty 0.0.1, Cherry 0.0.1). What is left are the layers Winter kept from before: winit types its input handlers still take, a second key hop into Cherry, rectangles and spans of its own that are copied field by field into misty's, a misty theme rebuilt on every draw, and two command surfaces that ignore the user's keys. This plan removes those layers with misty and Cherry as published, so neither needs a new release.

## 1. Where Winter Stands

Input: misty's event loop delivers misty's `Input`, and `window/input/misty.rs` turns its buttons, wheel deltas, positions, and modifiers back into winit's for handlers written before the move. `layout/geometry.rs` answers a window edge as winit's `ResizeDirection` only for `edge_of` to turn it into misty's `ResizeEdge`. winit is a dependency of the app for these alone; misty already depends on it, and gives everything Winter reads from it.

Keys: an editor's key goes from misty's `Key` to Winter's (`winter_key`), then to Cherry's (`winter_field::to_cherry`), though `winter_cherry::to_cherry` goes from misty's to Cherry's in one step. `winter-field` keeps its own copy of that conversion.

Geometry: `winter_model::layout::Rect` and the app's `PaneRect` hold the same four fields as `misty::Rect`, and 13 places copy one into the other field by field.

Theme: `misty_theme` builds a fresh `misty::Theme` from Winter's on each of its 32 calls, every draw.

Views: `InfoSpan` and `PanelSpan` are a subset of misty's `StyledText` spans; the app's `StatusBar` and `StatusSegment` mirror misty's status line; and an editor's `Frame` reaches misty in three hops, the renderer's `PaintCell` the second.

Commands: the menubar writes its shortcuts as literals, so a rebound command still shows its default keys. The Cherry adoption plan has the user's editor keys reach Cherry through `override_keys`, which nothing calls.

## 2. What Changes, What Stays

Changing: the handlers take misty's types and winit leaves the app; editors convert keys to Cherry's in one place; `layout::Rect` and `PaneRect` become `misty::Rect`, their methods free functions; the misty theme is built when Winter's theme changes and kept; spans and the status line are misty's; the renderer emits misty's cells; the menubar's shortcuts come from the keymap; the user's editor keys reach Cherry.

Staying, each for a reason: Winter's `Key` and its notation (`F5`, a bare `Enter`, `SPC`), which users' `keybindings.kdl` files are written in; the semantic `paint::Style` and `Frame`, which carry git, syntax, and selection meaning that misty's colors cannot; the Vim vocabulary of the terminal and the lists, which works on grids and rows, and on purpose differs from Cherry's (`w` skips punctuation); the `BUILTIN` commands and Winter's theme, which misty's `CommandRegistry` and `Theme` cannot yet hold (runtime ids, `when` conditions, git and scope colors). The last two would need misty changes first.

## 3. Steps

Each step is one commit, passing `make rust-lint` and `make rust-test`.

1. Input on misty's types: the mouse, wheel, and resize handlers take `PointerButton`, `WheelDelta`, `Point`, `ResizeEdge`, and `Mods`; the conversions in `input/misty.rs`, `edge_of`, and the winit key mapping no longer used are deleted, and winit leaves the app's dependencies.
2. Editor keys through one conversion: `winter_field::to_cherry` is deleted, and editors convert with `winter_cherry::keys::to_cherry`. An editor still takes Winter's `Key` (its `on_key`, on 26 editors and about 195 calls, after Winter's keymap, pending sequences, and replays), so its key goes from Winter's to misty's with the shared `misty_key` first.
3. Shared geometry: `layout::Rect` and `PaneRect` become `misty::Rect`, with their methods as free functions, and the field by field copies go.
4. The theme built once: the misty theme is built when Winter's theme changes, and every draw reads the one kept.
5. Views straight to misty: `InfoSpan` and `PanelSpan` become `StyledText`, the status bar types misty's, and the renderer emits misty's cells.
6. Commands and keys: the menubar's shortcuts come from the keymap, and the user's editor keys reach Cherry.
