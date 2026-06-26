//! GPU/window/PTY bootstrap — the heavy init that runs once on `resumed`.

use std::sync::Arc;

use winit::event_loop::ActiveEventLoop;
use winit::window::WindowAttributes;

use crate::config::{load_state, ThemeSetting, TitleBarStyle};
use crate::terminal::pane::Pane;
use winter_render::renderer::GpuRenderer;
use winter_render::{FontConfig, Theme};

use super::{
    content_rows, App, APPROX_CELL_HEIGHT, APPROX_CELL_WIDTH, DEFAULT_COLS, DEFAULT_ROWS, MIN_COLS,
    MIN_ROWS,
};

// ========================================================================
// App — window initialization
// ========================================================================

impl App {
    /// Full one-time setup: font scan (background), PTY spawn (background), GTK
    /// init, window creation, wgpu instance/adapter/device, renderer, theme, and
    /// the initial pane. Called from `ApplicationHandler::resumed`.
    pub(crate) fn init_window(&mut self, event_loop: &ActiveEventLoop) {
        // Pin the Vulkan loader to the integrated GPU before anything (the wgpu
        // instance, or another thread) reads the environment. On a hybrid laptop
        // enumerating the discrete GPU's driver costs ~1.7s at instance creation.
        let pinned_gpu = prefer_integrated_gpu();

        // Kick off the system-font scan first so it runs concurrently with
        // window creation and the whole wgpu instance/adapter/device setup.
        let font_load = winter_render::start_font_load();

        // The side-channel directory must exist before the shell is spawned so
        // the child inherits WINTER_SIDECHANNEL_DIR via the environment.
        let side_channel_dir =
            std::env::temp_dir().join(format!("winter-sidechannel-{}", std::process::id()));
        std::fs::create_dir_all(&side_channel_dir).ok();
        std::env::set_var("WINTER_SIDECHANNEL_DIR", &side_channel_dir);

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
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        // Lets a launcher (e.g. a file manager's "Open Terminal Here") pin the
        // initial pane to a specific directory. Takes priority over
        // restore-session below, since asking for a directory means "start
        // fresh here", not "bring back my last layout".
        let cwd_override = std::env::var("WINTER_CWD")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let pane_handle = {
            let cwd_override = cwd_override.clone();
            std::thread::spawn(move || {
                Pane::new_with_cwd(
                    pane_init_cols,
                    pane_init_rows,
                    configured_shell.as_deref(),
                    max_scrollback,
                    cwd_override.as_deref(),
                )
            })
        };

        #[cfg(target_os = "linux")]
        {
            let _ = gtk::init();
        }

        // Only request a transparent (alpha-channel) window when the user actually
        // wants translucency. A needlessly transparent window is recomposited (and
        // often blurred) by many compositors even when its content is fully opaque.
        let want_transparency = self.config.opacity < 1.0;
        // Modern style is borderless: the tab strip is the title bar and carries
        // its own window controls. System style keeps the native OS decorations.
        let decorated = self.config.title_bar_style == TitleBarStyle::System;
        // Stay hidden until the theme, panes, and (if restored) the saved
        // layout are all in place — otherwise the OS shows the window with
        // whatever was in the freshly-configured swapchain (blank or garbage)
        // and it visibly "flashes" to the real content a moment later.
        let min_width = MIN_COLS * APPROX_CELL_WIDTH;
        let min_height = MIN_ROWS * APPROX_CELL_HEIGHT;
        let base_attrs = WindowAttributes::default()
            .with_title(self.render_window_title())
            .with_decorations(decorated)
            .with_transparent(want_transparency)
            .with_window_icon(load_window_icon())
            .with_visible(false)
            .with_min_inner_size(winit::dpi::LogicalSize::new(min_width, min_height));
        // An undecorated (Modern title-bar) window drops the whole native frame
        // on Windows, including its border and drop shadow, so it renders as a
        // flat borderless rectangle. Re-add the DWM drop shadow (which also
        // draws the thin 1px top edge) so the window keeps a visible border
        // while the tab strip stands in for the title bar. No effect when
        // decorated: the System style already carries the native frame.
        #[cfg(target_os = "windows")]
        let base_attrs = {
            use winit::platform::windows::WindowAttributesExtWindows;
            base_attrs.with_undecorated_shadow(!decorated)
        };
        let window_attrs = match load_state().window_size {
            Some((w, h)) => base_attrs.with_inner_size(winit::dpi::PhysicalSize::new(
                w.max(min_width),
                h.max(min_height),
            )),
            None => base_attrs.with_inner_size(winit::dpi::LogicalSize::new(
                DEFAULT_COLS * APPROX_CELL_WIDTH,
                DEFAULT_ROWS * APPROX_CELL_HEIGHT,
            )),
        };
        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .expect("create window"),
        );

        let size = window.inner_size();

        // GPU validation layers (enabled by `InstanceFlags::default()` in debug
        // builds) cost ~100ms at startup and slow every draw call. Off by
        // default; opt in with WINTER_GPU_DEBUG=1 when debugging the renderer.
        let gpu_flags = if std::env::var_os("WINTER_GPU_DEBUG").is_some() {
            wgpu::InstanceFlags::debugging()
        } else {
            wgpu::InstanceFlags::empty()
        };
        // Acquire the GPU, instance, surface, and a surface-compatible adapter.
        // Pinning a single integrated ICD can occasionally enumerate no
        // surface-compatible adapter (some driver/suspend states); when that
        // happens, drop the pin and retry with the full set of GPUs so startup
        // degrades to "slower" instead of crashing.
        let mut gpu = acquire_gpu(&window, gpu_flags);
        if gpu.is_none() && pinned_gpu {
            clear_integrated_gpu_pin();
            gpu = acquire_gpu(&window, gpu_flags);
        }
        // Keep the instance alive for the rest of setup (the surface depends on it).
        let (_instance, surface, adapter) = gpu.expect("no compatible GPU adapter found");

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
        let scale_factor = window.scale_factor();
        let mut renderer = GpuRenderer::new(
            surface,
            adapter,
            size.width,
            size.height,
            scale_factor,
            font,
            font_load,
        );

        let mut initial_theme = match &self.config.theme {
            ThemeSetting::Auto => window
                .theme()
                .map(|t| match t {
                    winit::window::Theme::Dark => Theme::dark(),
                    winit::window::Theme::Light => Theme::light(),
                })
                .unwrap_or_default(),
            ThemeSetting::Dark => Theme::dark(),
            ThemeSetting::Light => Theme::light(),
            // A user theme file; fall back to the dark preset if it is missing.
            ThemeSetting::Named(name) => {
                crate::config::load_named_theme(name).unwrap_or_else(Theme::dark)
            }
        };
        self.config.colors.apply(&mut initial_theme);
        renderer.set_theme(initial_theme);
        renderer.set_ligatures(self.config.ligatures);
        renderer.set_divider_width(self.config.pane_border_width);
        renderer.set_decorated(decorated);

        let (cols, rows) = renderer.grid_size();
        let want_rows = content_rows(rows);
        let spawned = pane_handle.join().expect("PTY spawn thread panicked");
        let mut pane = match spawned {
            Ok(pane) => pane,
            Err(e) => {
                // The configured shell failed even before the window opened
                // (bad path, fd exhaustion, ...); retry once with the OS
                // default rather than leaving the app with no terminal at
                // all. The notice is queued now and paints once the window
                // (made visible below) draws its first frame.
                self.set_error(format!(
                    "shell failed to start ({e}), trying the default shell"
                ));
                match Pane::new_with_cwd(
                    cols.max(1),
                    want_rows.max(1),
                    None,
                    max_scrollback,
                    cwd_override.as_deref(),
                ) {
                    Ok(pane) => pane,
                    Err(e2) => {
                        eprintln!("winter: could not start a shell: {e2}");
                        event_loop.exit();
                        return;
                    }
                }
            }
        };
        if cols != pane_init_cols || want_rows != pane_init_rows {
            pane.resize(cols, want_rows);
        }
        let (cell_w, cell_h) = renderer.cell_size();
        pane.set_cell_size(cell_w, cell_h);
        let focused = self.tab().focused();
        self.panes.insert(focused, pane);

        self.window = Some(window);
        self.renderer = Some(renderer);
        // Now that a window exists to paint it, raise any config diagnostic
        // held from startup.
        if let Some(message) = self.pending_config_error.take() {
            self.set_error(message);
        }
        // Hints the WM to snap interactive drags to whole cell rows/columns,
        // so `snap_window_to_cell_grid` rarely has to correct anything after
        // the fact. Best-effort: unsupported platforms just ignore the hint.
        if let Some(window) = &self.window {
            window.set_resize_increments(Some(winit::dpi::PhysicalSize::new(
                cell_w.round() as u32,
                cell_h.round() as u32,
            )));
        }
        // Restore previous session layout if configured and a session file exists.
        // This replaces the single bootstrap pane with the saved split tree.
        let restored = cwd_override.is_none()
            && self.config.restore_session
            && self.restore_session_if_present();
        if !restored {
            self.resize_all_panes();
        }
        self.snap_window_to_cell_grid();
        self.dirty = true;

        // Now that the theme and initial layout are both in place, reveal the
        // window and draw its first real frame.
        if let Some(window) = &self.window {
            window.set_visible(true);
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
/// Returns `true` when it pinned the loader, so the caller can undo it via
/// [`clear_integrated_gpu_pin`] if the pinned GPU yields no usable adapter.
///
/// A no-op (returns `false`) when: not on Linux; the user set any Vulkan
/// driver-selection variable, `WGPU_BACKEND`, or `WINTER_GPU=discrete`; or no
/// integrated ICD is found (so machines without an Intel/Asahi iGPU keep full
/// enumeration). Must run before the wgpu instance is created and before any
/// other thread reads the environment.
#[cfg(target_os = "linux")]
fn prefer_integrated_gpu() -> bool {
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
fn prefer_integrated_gpu() -> bool {
    false
}

/// Undo the [`prefer_integrated_gpu`] pin so a subsequent `wgpu::Instance::new`
/// re-enumerates every GPU. Only removes the variables this module sets.
#[cfg(target_os = "linux")]
fn clear_integrated_gpu_pin() {
    std::env::remove_var("VK_DRIVER_FILES");
    std::env::remove_var("VK_ICD_FILENAMES");
}

#[cfg(not(target_os = "linux"))]
fn clear_integrated_gpu_pin() {}

/// Create a wgpu instance, window surface, and surface-compatible low-power
/// adapter, or `None` if no adapter is available (so the caller can retry with a
/// different GPU pin). Returns all three so the instance outlives the surface.
fn acquire_gpu(
    window: &Arc<winit::window::Window>,
    flags: wgpu::InstanceFlags,
) -> Option<(wgpu::Instance, wgpu::Surface<'static>, wgpu::Adapter)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
        flags,
        backend_options: wgpu::BackendOptions::default(),
        display: None,
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
    });
    let surface = instance.create_surface(Arc::clone(window)).ok()?;
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .ok()?;
    Some((instance, surface, adapter))
}

/// Load the winter-terminal-64.png icon embedded into the binary and convert it
/// into a winit-compatible window icon.
fn load_window_icon() -> Option<winit::window::Icon> {
    let icon_bytes = include_bytes!("../../assets/icons/winter-terminal-64.png");
    let img = image::load_from_memory(icon_bytes).ok()?;
    let rgba = img.into_rgba8();
    let (width, height) = rgba.dimensions();
    winit::window::Icon::from_rgba(rgba.into_raw(), width, height).ok()
}
