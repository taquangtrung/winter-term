# Misty Extraction Plan

Status: built. Steps 1 to 3 are built: GPU interop (`Gpu`, `GpuTexture`, `Frame::draw_texture`), window controls (`AppContext::set_title`, `set_icon`, `set_transparent`, `set_font`), terminal-grade key input (`Input::KeyReleased`, `EventCx::is_key_repeat`, unnamed letter and digit keys named by where they sit), and cell grid parity (blink, conceal, overline, a hollow cursor, `Highlight`s through `CellSource::highlights`, braille drawn as dots, a smooth curly underline, fallback glyphs shrunk to their cells, and `Font::with_ligatures`). Step 4 is built: `Palette::background_alpha` and the editor and chrome colors (`cursor_line`, `raised`, `scrollbar`, `search_current`, `search_match`), each read from a theme file; mapping Winter's own color names onto them names Winter, so it is built in Winter with step 8. Step 5 is built: the toast upgrade (corner, source, hover close), `CompletionList`, `InfoCard` (with `PopupSide::Above`), `Transient`, the filterable menu as `QuickPick::with_anchor`, and `DropList`. Step 6 is built: the picker framework in `misty::picker` (section 4), drawn by `QuickPick`, the command palette one of its sources. Step 7 is built: a menubar and tabs in `TitleBar`, icons, a busy spinner, and a modified dot on `TabStrip` tabs, clickable `StatusSegment`s with icons on `StatusBar`, and scopes on `SettingsPage`; the settings page's structured rows (lists of names) wait for step 8, where Winter's rows show what they need. Step 8 is under way on Winter's `misty-adoption` branch: Winter runs on misty's event loop (sub-step 1), and its pickers on `misty::picker`, drawn by `QuickPick` (sub-step 2), with `winter-picker` and the raster picker deleted. Sub-step 3 is under way: Winter's notifications and long work run on `misty-notify` (now misty's `notify` module), one engine for toasts, status bar messages, and progress tasks, with its toasts drawn by misty's `Toasts` and `winter-notify` deleted, its status bar is misty's `StatusBar` (left segments, a prompt, short texts, priorities), and its completion list (with sources), info card, drop list, which-key popup, transient, menus (the menubar's, with their submenus, and the context menu, on misty's `Menu`), title bar (`WindowButtons` on either side or none, `TitleBar::height` for the layout below it), tooltips, pane strips (a `TabStrip` per pane, kept in step with Winter's layout, `TabStrip::set_focused` for the focused pane's), and the settings page (misty's `SettingsPage` drawn in its tab's pane, with settings named at runtime, settings of names, and Esc keeping an edit) are misty's. Sub-step 3 is built. Sub-step 4 is built: every pane's cells are drawn by misty in the terminal's font (`Frame::draw_cells_in`, `Font::with_line_height`), from what Winter's renderer resolves of them, its pictures from textures it makes on misty's GPU, and its dialogs, find card, token inspector, and pane toolbars by misty; the renderer's GPU passes and every raster cache are deleted. Step 8 is built. Step 9 is built: `winter-render` is removed; Winter's theme, theme files, and syntax scopes are its `winter-theme` crate, the views other crates build are in `winter_model::view`, and what resolves panes for misty, the grid font, and the picture textures are in the app. Every step is built.

Winter (this repository) draws its own GUI: one renderer (`winter-render`'s `GpuRenderer`) and a window runtime in `winter-term/src/window`. This plan moves the generic parts of that GUI into misty, so misty becomes a library other apps can adopt, and then rebuilds Winter on `misty::run`. Misty's rules in `AGENTS.md` decide what moves: a part that names a terminal, a file, or one of Winter's commands stays in Winter, built on misty.

## 1. Where The Two Stand

Both use wgpu 30, winit 0.30, glyphon 0.12, and resvg 0.48, so no dependency has to change.

Winter draws most chrome and every popup on the CPU into an RGBA image, caches it, and uploads it as a texture. Misty draws the same things natively on the GPU from one `Frame` draw list. Moving to misty's widgets removes Winter's raster caches, its largest per-frame cost.

Misty is ahead on: several windows on one GPU, typed `Waker` messages, deadline redraws, accessibility, focus traps, smooth scrolling, the headless test harness, menus with checks and radios, and toolbars.

Winter is ahead on: GPU interop (app textures, render-to-texture for Vello), terminal-grade key input, the cell grid's finer attributes, the picker, and a handful of popups misty has no widget for.

## 2. What Moves, What Stays

Generic, moving to misty:

- GPU interop: app-owned textures, cropped texture draws, device and queue access.
- Window controls: title, icon, opacity, runtime font changes.
- Key input a terminal can encode: releases, repeats, and keys named by their place when the layout names none. Key text, keypad location, and lone modifier keys wait until an app needs them; Winter does not.
- Cell grid parity: overline, blink, conceal, hollow cursor, font weights, a ligature switch, fitted fallback glyphs, braille dots, a smooth curly underline, and a highlight overlay API for selections and bands.
- Widgets: a completion list, a rich info card, a transient (switches plus commands), a filterable menu, a cell-aligned drop list.
- Upgrades: the quick pick (preview panel, detail lines, icons, row buttons, action menu, query syntax, streamed sources, history), the title bar (menubar, tabs in the bar), the tab strip (icons, spinner), the status bar (icons, clickable segments), toasts (corner, hover close, source), and the settings page (scopes, structured rows).

Mixed, split between the two:

- The theme: misty takes background alpha and the chrome tokens; Winter keeps its git colours and TextMate scope table.
- The runtime: misty's event loop replaces Winter's; Winter's PTY, socket, job, and LSP polling become threads that send `Waker` messages.
- Pane splits: misty's `SplitTree` takes zoom; Winter keeps tabs-in-pane and docking.

Winter-specific, staying:

- The terminal emulator and its `CellSource` adapter, the token inspector, inline terminal blocks, the PDF and web page editors, the bundled icon map, and every command and picker source.

## 3. Steps

Each step lands in misty first with headless tests, passing `make check` and `make test`; Winter adopts a part only once misty's version does all Winter's does.

1. GPU interop: a public texture handle, `Frame::draw_texture` with a source crop, and device and queue access so an app can render into a texture (Vello). Window title, icon, opacity, and runtime font changes.
2. Terminal-grade key input on `Input`.
3. Cell grid parity in `render/grid.rs`.
4. A theme bridge from Winter's theme files to misty's `Theme`, with background alpha.
5. The new popup widgets, and the toast upgrade.
6. The quick pick upgrade, taking the toolkit-free core of `winter-picker` (session, matcher, query, rows, sources, history).
7. Chrome upgrades: menubar, title bar tabs, tab strip icons, status bar segments, settings scopes.
8. Winter rebuilt as a misty `Application`: input routed through `FocusTree`, everything drawn through `Frame`, `GpuRenderer` and the raster caches deleted.
9. `winter-render` removed, its Winter-specific remains moved into the app.

Step 8 runs in sub-steps on a local Winter branch (`misty-adoption`) with a path dependency on misty, kept unpushed until misty is published; each leaves Winter building and working:

1. Winter on misty's event loop. `GpuRenderer` renders into a `GpuTexture` on misty's GPU (no surface of its own), drawn over the window by `Frame::draw_texture`; Winter's `App` is a misty `Application`, turning misty's `Input` into Winter's keys and pointer events, and its window requests (title, cursor, drag, maximize) into `EventCx` and `AppContext` calls. Misty gains what the window needs on the way: an sRGB view of a `GpuTexture` (Winter blends text in linear light), and macOS Option as Alt (`WindowOptions::with_option_as_alt`, `AppContext::set_option_as_alt`). Winter keeps pinning the integrated GPU itself: it sets the Vulkan loader's environment before `misty::run`, which misty, denying unsafe code, cannot.
2. The picker: Winter's sources on `misty::picker`, drawn by `QuickPick`; `winter-picker` deleted. Before Winter draws with it, `QuickPick` gains what Winter's picker does that it does not: `QuickPickPlacement` (a live picker docked along the bottom of its pane, a list centered in a pane or filling a transient's card), descriptions in colors of their own (`set_description_colors`, Scout's lines in their syntax colors), and a heading that moves the picker and a grip that resizes it. The side panel's matches are lit in the `StyledText` the app hands it.
3. The popups and chrome, one at a time, each deleting its raster cache: toasts, the completion list, the info card, the transient, the which-key popup, the menus, the title bar and tabs, the pane strips, the status bar, the settings page.
4. The panes: terminal and editor grids through `Frame::draw_cells`, the last of `GpuRenderer`.

## 4. The Picker

Step 6 moves `winter-picker` (`crates/libs/winter-picker`, designed in its `docs/picker-framework-design.md`) into misty as `widgets/picker`, and `QuickPick` becomes its view: one picker engine, as misty's rules ask. Winter keeps its own crate until step 8, when it switches to misty's; its repository is public and misty is not published, so it cannot depend on misty before then.

What moves as it is: rows (label, description, detail, hint, icon) and the fields a query meets (`MatchOn`); the query (words in any order, `/regex/`, `!` words, quoted phrases) and its scoring; the background matcher (generations, narrowing, merged batches, a tracked row, a pinned row); the session (`Picker<O>` over a `Source<O>`, its acts, the action menu, query history, recently used first, resume, saving rows, answer pickers); and the testing harness.

What changes on the way:

- Keys: misty resolves no key presses, so the session answers `PickerAct`s, and keys no act binds go to the query. `PickerAct::default_keys` gives the defaults an app binds; Winter's `picker` key table stays in Winter.
- Waking: the matcher wakes the event loop through a `Waker`, not Winter's global.
- Icons: a row names its icon in the app's `IconSet`.
- Previews: a source's preview is a type of the app's own; the picker says when the selected row's preview is due, and draws a side panel of `StyledText` lines the app hands it.
- The query field is misty's `TextField`.
- Matching: word matching replaces `QuickPick`'s scattered-letter fuzzy match, as Winter has it.

Sub-steps, each committed with its tests:

1. Rows, the query, and the matcher, with their tests.
2. The source trait, the session, history and recency, and the harness.
3. `QuickPick` rebuilt as the session's view: two-line rows, descriptions, icons, the hints line, the action menu hung from the row, row buttons, the side panel, errors and the working mark in the header; the command palette a source; the old matcher deleted.
