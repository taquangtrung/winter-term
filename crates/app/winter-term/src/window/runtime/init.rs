//! GPU/window/PTY bootstrap: the heavy init that runs once on `resumed`.

use misty::{AppContext, Decorations, OptionAsAlt, Position, Size, WindowId, WindowOptions};
use winit::dpi::PhysicalSize;

use crate::config::{load_state, ThemeSetting, TitleBarStyle};
use crate::session::Session;
use winter_render::renderer::GpuRenderer;
use winter_render::{FontConfig, Theme};
use winter_terminal::session::Terminal;

use crate::window::appearance::{terminal_presentation, theme_for};
use crate::window::input::ime::enable_ime;
use crate::window::runtime::handle::WindowHandle;
use crate::window::{
    content_rows, App, APPROX_CELL_HEIGHT, APPROX_CELL_WIDTH, DEFAULT_COLS, DEFAULT_ROWS, MIN_COLS,
    MIN_ROWS,
};

// ========================================================================
// Constants
// ========================================================================

/// How much smaller Winter's chrome text is than its terminal text: a 13.5
/// pixel status line under a 15 pixel grid.
const CHROME_FONT_RATIO: f32 = 13.5 / 15.0;

// ========================================================================
// App: window initialization
// ========================================================================

impl App {
    /// How misty opens Winter's window: its title, frame, translucency,
    /// icon, Option keys, and the size and place it had when it last closed.
    /// Stays hidden until the first frame, so the OS never shows it blank.
    pub fn window_options(&self) -> WindowOptions {
        // Only request a transparent (alpha-channel) window when the user actually
        // wants translucency. A needlessly transparent window is recomposited (and
        // often blurred) by many compositors even when its content is fully opaque.
        let want_transparency = self.config.opacity < 1.0
            || self
                .config
                .colors
                .background
                .is_some_and(|background| !background.is_opaque());
        // Modern style is borderless: the tab strip is the title bar and carries
        // its own window controls. System style keeps the native OS decorations.
        let decorations = match self.config.title_bar_style {
            TitleBarStyle::System => Decorations::System,
            TitleBarStyle::Modern => Decorations::Frameless,
        };
        let state = load_state();
        let options = WindowOptions::new(self.render_window_title())
            .with_decorations(decorations)
            .with_transparent(want_transparency)
            .with_min_size(min_size())
            .with_size(opening_size(state.window_size))
            .with_option_as_alt(misty_option_as_alt(self.config.option_as_alt))
            .with_font(self.misty_font());
        let options = match load_window_icon() {
            Some(icon) => options.with_icon(icon),
            None => options,
        };
        match state.window_position {
            Some((x, y)) => options.with_position(Position::new(x, y)),
            None => options,
        }
    }

    /// The font misty's widgets (the status bar, the picker, the cards) draw
    /// in: the terminal's family, at the size Winter's chrome text takes.
    pub(in crate::window) fn misty_font(&self) -> misty::Font {
        self.misty_font_at(self.config.font_size)
    }

    /// [`App::misty_font`] for a terminal font of `size` logical pixels.
    pub(in crate::window) fn misty_font_at(&self, size: f32) -> misty::Font {
        let size = size * CHROME_FONT_RATIO;
        let font = misty::Font::new(size);
        match &self.config.font_family {
            Some(family) => font.with_family(family.clone()),
            None => font,
        }
    }

    /// Full one-time setup once misty has opened `window`: font scan
    /// (background), PTY spawn (background), the renderer on misty's GPU,
    /// theme, and the initial pane. Called from misty's `init`.
    pub(in crate::window) fn init_window(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId) {
        // Kick off the system-font scan first so it runs concurrently with
        // window creation and the whole wgpu instance/adapter/device setup.
        let font_load = winter_render::start_font_load();

        // The side-channel directory must exist before the shell is spawned so
        // the child inherits WINTER_SIDECHANNEL_DIR via the environment.
        // It lives in this user's private directory: a predictable path in
        // the shared temp directory could be planted first by another user.
        // Without one, side-channel payloads are simply off.
        let side_channel_name = format!("sidechannel-{}", std::process::id());
        if let Some(side_channel_dir) = winter_base::paths::private_subdir(&side_channel_name) {
            std::env::set_var("WINTER_SIDECHANNEL_DIR", &side_channel_dir);
        }

        // Spawn the PTY early so fork/exec overlaps with the entire GPU init
        // sequence (instance → adapter → device), which dominates startup time.
        // The pane is created with default dimensions and resized to the
        // renderer's precise cell grid after the GPU is ready.
        let pane_init_cols = DEFAULT_COLS as usize;
        let pane_init_rows = content_rows(DEFAULT_ROWS as usize);
        let configured_shell = self.config.active_shell().map(String::from);
        let max_scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_emulator::MAX_SCROLLBACK);
        // Lets a launcher (e.g. a file manager's "Open Terminal Here") pin the
        // initial pane to a specific directory. Takes priority over
        // restore-session below, since asking for a directory means "start
        // fresh here", not "bring back my last layout".
        let cwd_override = std::env::var("WINTER_CWD")
            .ok()
            .filter(|s| !s.trim().is_empty());
        // A saved session fills the window itself, so the bootstrap shell is
        // started only without one: started anyway, it would run the shell's
        // startup files only to be thrown away. A reload that kept the
        // programs running restores whatever the configuration says, since
        // what it restores is not a fresh start.
        let session = (crate::session::handover_announced()
            || (cwd_override.is_none() && self.config.restore_session))
            .then(Session::load)
            .flatten();
        let pane_handle = session.is_none().then(|| {
            let cwd_override = cwd_override.clone();
            std::thread::spawn(move || {
                Terminal::new_with_cwd(
                    pane_init_cols,
                    pane_init_rows,
                    configured_shell.as_deref(),
                    max_scrollback,
                    cwd_override.as_deref(),
                )
            })
        });

        let Some(gpu) = cx.gpu().cloned() else {
            eprintln!("winter: misty opened the window without a GPU");
            cx.exit();
            return;
        };
        let decorated = self.config.title_bar_style == TitleBarStyle::System;
        let scale_factor = 1.0;
        // The size it opened at, which the first frame corrects.
        let opening = opening_size(load_state().window_size);
        let size = PhysicalSize::new(opening.width, opening.height);
        let handle = WindowHandle::new(window, size, scale_factor);
        handle.note_color_scheme(cx.appearance().color_scheme);
        enable_ime(&handle);

        let font = FontConfig {
            family: std::env::var("WINTER_FONT")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| self.config.font_family.clone()),
            size: std::env::var("WINTER_FONT_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(self.config.font_size),
            normal_weight: std::env::var("WINTER_FONT_WEIGHT")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| self.config.font_weight.clone()),
            bold_weight: std::env::var("WINTER_FONT_WEIGHT_BOLD")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| self.config.font_weight_bold.clone()),
        };
        let mut renderer = GpuRenderer::new(
            gpu.clone(),
            size.width,
            size.height,
            scale_factor,
            font,
            font_load,
        );
        self.gpu = Some(gpu);

        let mut initial_theme = match &self.config.theme {
            ThemeSetting::Auto => theme_for(handle.color_scheme()),
            ThemeSetting::Dark => Theme::dark(),
            ThemeSetting::Light => Theme::light(),
            // A user theme file; the dark preset, and a word saying why, when
            // it cannot be read.
            ThemeSetting::Named(name) => match crate::config::load_named_theme(name) {
                Ok(theme) => theme,
                Err(problem) => {
                    self.set_error(problem);
                    Theme::dark()
                }
            },
        };
        self.config.colors.apply(&mut initial_theme);
        renderer.set_window_opacity(self.config.opacity);
        renderer.set_theme(initial_theme);
        renderer.set_ligatures(self.config.ligatures);
        renderer.set_divider_width(self.config.pane_border_width);
        renderer.set_popup_input_height(self.config.popup_input_height);
        renderer.set_decorated(decorated);

        let (cols, rows) = renderer.grid_size();
        let want_rows = content_rows(rows);
        if let Some(pane_handle) = pane_handle {
            let spawned = pane_handle.join().expect("PTY spawn thread panicked");
            let mut terminal = match spawned {
                Ok(terminal) => terminal,
                Err(e) => {
                    // The configured shell failed even before the window opened
                    // (bad path, fd exhaustion, ...); retry once with the OS
                    // default rather than leaving the app with no terminal at
                    // all. The notice is queued now and paints once the window
                    // (made visible below) draws its first frame.
                    self.set_error(format!(
                        "shell failed to start ({e}), trying the default shell"
                    ));
                    match Terminal::new_with_cwd(
                        cols.max(1),
                        want_rows.max(1),
                        None,
                        max_scrollback,
                        cwd_override.as_deref(),
                    ) {
                        Ok(terminal) => terminal,
                        Err(e2) => {
                            eprintln!("winter: could not start a shell: {e2}");
                            cx.exit();
                            return;
                        }
                    }
                }
            };
            if cols != pane_init_cols || want_rows != pane_init_rows {
                terminal.resize(cols, want_rows);
            }
            terminal.set_presentation(terminal_presentation(&renderer));
            let focused = self.layout().focused();
            self.open_tabs.insert_terminal(focused, terminal);
        }

        self.window = Some(handle);
        self.renderer = Some(renderer);
        // Now that a window exists to paint it, raise any config diagnostic
        // held from startup.
        if let Some(message) = self.runtime.config_watch.pending_error.take() {
            self.set_error(message);
        }
        for problem in crate::extensions::load_problems() {
            self.set_error(problem.as_str());
        }
        // Winter never sizes its own window once it is up: the size belongs to
        // whoever set it (the window manager, a tiling editor, a drag), and a
        // terminal that rounded it to whole cells would push past a tile's
        // edge. Space short of a whole row or column is left as padding
        // inside the panes. No resize increments are set either:
        // a height holding whole rows is the chrome plus a multiple of the
        // cell, which X11 spells as a base size the increment alone cannot
        // carry, so a manager honouring a bare cell increment would floor off
        // the chrome's pixels.

        // The saved session, when there is one, becomes the window's layout.
        // One that brings nothing back leaves the bootstrap pane empty, so
        // the shell it would have had starts now.
        let reloaded = crate::session::handover_announced();
        let restored = session.is_some_and(|session| self.restore_saved_session(session));
        if reloaded {
            self.note_strays();
        }
        if !restored {
            let focused = self.layout().focused();
            if !self.open_tabs.is_terminal(&focused) {
                let (cols, rows) = self
                    .renderer
                    .as_ref()
                    .map_or((DEFAULT_COLS as usize, DEFAULT_ROWS as usize), |r| {
                        r.grid_size()
                    });
                let shell = self.config.active_shell().map(String::from);
                let Some(mut terminal) = self.spawn_terminal_or_notify(
                    cols.max(1),
                    content_rows(rows).max(1),
                    shell.as_deref(),
                    max_scrollback,
                    cwd_override.as_deref(),
                ) else {
                    eprintln!("winter: could not start a shell");
                    cx.exit();
                    return;
                };
                if let Some(renderer) = &self.renderer {
                    terminal.set_presentation(terminal_presentation(renderer));
                }
                self.open_tabs.insert_terminal(focused, terminal);
            }
            self.resize_all_panes();
        }
        self.dirty = true;

        // Now that the theme and initial layout are both in place, draw the
        // first real frame, which misty shows the window with.
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

// ========================================================================
// GPU selection
// ========================================================================

/// Vulkan ICD filename prefixes that only ever describe an integrated GPU.
/// Pinning the loader to these never disables a machine's sole device: a system
/// without an integrated GPU simply has none of these ICDs and is left untouched.
#[cfg(target_os = "linux")]
const INTEGRATED_ICD_PREFIXES: [&str; 2] = ["intel", "asahi"];

/// Standard directories the Vulkan loader reads ICD manifests from.
#[cfg(target_os = "linux")]
const VULKAN_ICD_DIRS: [&str; 2] = ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"];

/// On a hybrid-GPU Linux laptop, `wgpu::Instance::new` makes the Vulkan loader
/// enumerate every installed ICD; loading the discrete NVIDIA driver powers up
/// the dGPU and costs ~1.7s before the first frame (it dominates startup). A
/// terminal never needs the discrete GPU, so when the user has not pinned one we
/// restrict the loader to an integrated-GPU ICD via `VK_DRIVER_FILES`.
///
/// Returns `true` when it pinned the loader.
///
/// A no-op (returns `false`) when: not on Linux; the user set any Vulkan
/// driver-selection variable, `WGPU_BACKEND`, or `WINTER_GPU=discrete`; or no
/// integrated ICD is found (so machines without an Intel/Asahi iGPU keep full
/// enumeration). Must run before the wgpu instance is created and before any
/// other thread reads the environment.
#[cfg(target_os = "linux")]
pub fn prefer_integrated_gpu() -> bool {
    const OVERRIDES: [&str; 4] = [
        "VK_DRIVER_FILES",
        "VK_ICD_FILENAMES",
        "VK_LOADER_DRIVERS",
        "WGPU_BACKEND",
    ];
    if OVERRIDES.iter().any(|var| std::env::var_os(var).is_some()) {
        return false;
    }
    if std::env::var("WINTER_GPU").is_ok_and(|v| v.eq_ignore_ascii_case("discrete")) {
        return false;
    }

    let icds: Vec<String> = VULKAN_ICD_DIRS
        .iter()
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    INTEGRATED_ICD_PREFIXES
                        .iter()
                        .any(|prefix| name.starts_with(prefix))
                })
        })
        .filter_map(|path| path.to_str().map(str::to_string))
        .collect();

    if icds.is_empty() {
        return false;
    }
    // `VK_DRIVER_FILES` is the modern loader variable; set the legacy
    // `VK_ICD_FILENAMES` alias too so older loaders honor the same pin.
    let joined = icds.join(":");
    std::env::set_var("VK_DRIVER_FILES", &joined);
    std::env::set_var("VK_ICD_FILENAMES", &joined);
    true
}

#[cfg(not(target_os = "linux"))]
pub fn prefer_integrated_gpu() -> bool {
    false
}

/// The winter-terminal-64.png icon embedded into the binary, as misty shows
/// a window's icon.
fn load_window_icon() -> Option<misty::Image> {
    let icon_bytes = include_bytes!("../../../assets/icons/winter-terminal-64.png");
    misty::Image::decode(icon_bytes).ok()
}

/// The smallest size the window may be made, in logical pixels.
fn min_size() -> Size {
    Size::new(MIN_COLS * APPROX_CELL_WIDTH, MIN_ROWS * APPROX_CELL_HEIGHT)
}

/// The size the window opens at, in logical pixels: the `saved` size it
/// had when it last closed, held to the smallest, or a default one.
fn opening_size(saved: Option<(u32, u32)>) -> Size {
    let least = min_size();
    match saved {
        Some((w, h)) => Size::new(w.max(least.width), h.max(least.height)),
        None => Size::new(
            DEFAULT_COLS * APPROX_CELL_WIDTH,
            DEFAULT_ROWS * APPROX_CELL_HEIGHT,
        ),
    }
}

/// What misty calls the Option keys `setting` makes Alt.
pub(in crate::window) fn misty_option_as_alt(setting: crate::config::OptionAsAlt) -> OptionAsAlt {
    use crate::config::OptionAsAlt as Setting;
    match setting {
        Setting::Both => OptionAsAlt::Both,
        Setting::Left => OptionAsAlt::Left,
        Setting::None => OptionAsAlt::None,
        Setting::Right => OptionAsAlt::Right,
    }
}
