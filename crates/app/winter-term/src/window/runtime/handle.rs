//! The window as Winter's code asks things of it, between misty's callbacks:
//! what is asked (a redraw, a pointer shape, a drag, a title) is kept until
//! the callback running hands it to misty, and what misty last said of the
//! window (its size, its scale, whether it is maximized) is kept for the
//! code that asks.

use std::cell::{Cell, RefCell};

use misty::{
    AppContext, ColorScheme, CursorIcon, ImeFocus, OptionAsAlt, Rect, ResizeEdge, WindowGeometry,
    WindowId,
};
use winit::dpi::PhysicalSize;
use winit::window::ResizeDirection;

// ========================================================================
// Data Structures
// ========================================================================

/// Winter's one window, as its code asks things of it. Asking takes `&self`,
/// so code holding other parts of the app borrowed can still ask; the asks
/// are kept in order until [`WindowHandle::flush`].
pub(in crate::window) struct WindowHandle {
    /// Whether the system is set to light or dark, as misty last said.
    color_scheme: Cell<Option<ColorScheme>>,
    /// Where the window is and how large, in logical pixels, as misty last
    /// said.
    geometry: Cell<Option<WindowGeometry>>,
    /// The window misty knows it by.
    id: WindowId,
    /// Whether the input method composes, and where its candidates go.
    ime: RefCell<ImeWanted>,
    /// What misty was last told of the input method.
    ime_focus: RefCell<ImeFocus>,
    /// Whether the window was maximized when misty last said.
    maximized: Cell<bool>,
    /// What is asked of the window and not yet handed to misty.
    requests: RefCell<Vec<Request>>,
    /// Physical pixels per logical pixel, as the last frame drew.
    scale_factor: Cell<f64>,
    /// The window's inner size in physical pixels, as the last frame drew.
    size: Cell<PhysicalSize<u32>>,
}

/// What the window's input method should do.
#[derive(Clone, Copy, Debug, Default)]
struct ImeWanted {
    /// Whether it composes.
    allowed: bool,
    /// Where the cursor is that its candidates go beside.
    area: Rect,
}

/// One thing asked of the window.
#[derive(Clone, Debug)]
enum Request {
    /// Move the window with the pointer.
    Drag,
    /// Minimize the window.
    Minimize,
    /// Make these Option keys Alt on macOS.
    OptionAsAlt(OptionAsAlt),
    /// Draw a new frame.
    Redraw,
    /// Resize the window from this edge with the pointer.
    Resize(ResizeEdge),
    /// Show this pointer shape.
    SetCursor(CursorIcon),
    /// Name the window this.
    SetTitle(String),
    /// Let the background show the desktop through it.
    SetTransparent(bool),
    /// Maximize the window, or restore it.
    ToggleMaximize,
}

// ========================================================================
// WindowHandle Implementations
// ========================================================================

impl WindowHandle {
    /// The handle of window `id`, `size` physical pixels inside at
    /// `scale_factor`.
    pub(in crate::window) fn new(id: WindowId, size: PhysicalSize<u32>, scale_factor: f64) -> Self {
        Self {
            color_scheme: Cell::new(None),
            geometry: Cell::new(None),
            id,
            ime: RefCell::new(ImeWanted::default()),
            ime_focus: RefCell::new(ImeFocus::new()),
            maximized: Cell::new(false),
            requests: RefCell::new(Vec::new()),
            scale_factor: Cell::new(scale_factor),
            size: Cell::new(size),
        }
    }

    /// The window misty knows it by.
    pub(in crate::window) fn id(&self) -> WindowId {
        self.id
    }

    // --------------------------------------------------------------------
    // What misty said of it
    // --------------------------------------------------------------------

    /// The window's inner size in physical pixels.
    pub(in crate::window) fn inner_size(&self) -> PhysicalSize<u32> {
        self.size.get()
    }

    /// Whether the window is maximized.
    pub(in crate::window) fn is_maximized(&self) -> bool {
        self.maximized.get()
    }

    /// Whether the system is set to light or dark, as misty last said.
    pub(in crate::window) fn color_scheme(&self) -> Option<ColorScheme> {
        self.color_scheme.get()
    }

    /// Notes whether the system is set to light or dark.
    pub(in crate::window) fn note_color_scheme(&self, scheme: Option<ColorScheme>) {
        self.color_scheme.set(scheme);
    }

    /// Where the window is and how large, in logical pixels, as misty last
    /// said: what is saved for the next start to open it as.
    pub(in crate::window) fn geometry(&self) -> Option<WindowGeometry> {
        self.geometry.get()
    }

    /// Notes what misty says of the window's place and size.
    pub(in crate::window) fn note_geometry(&self, geometry: Option<WindowGeometry>) {
        self.geometry.set(geometry);
    }

    /// Notes what a frame says of the window: its size, scale, and whether
    /// it is maximized. Whether the size or the scale changed.
    pub(in crate::window) fn note_frame(
        &self,
        size: PhysicalSize<u32>,
        scale_factor: f64,
        maximized: bool,
    ) -> bool {
        self.maximized.set(maximized);
        let changed = self.size.get() != size || self.scale_factor.get() != scale_factor;
        self.size.set(size);
        self.scale_factor.set(scale_factor);
        changed
    }

    // --------------------------------------------------------------------
    // Asking
    // --------------------------------------------------------------------

    /// Asks for a new frame.
    pub(in crate::window) fn request_redraw(&self) {
        self.ask(Request::Redraw);
    }

    /// Shows `icon` over the window.
    pub(in crate::window) fn set_cursor(&self, icon: CursorIcon) {
        self.ask(Request::SetCursor(icon));
    }

    /// Lets the user move the window with the pointer held down.
    pub(in crate::window) fn drag_window(&self) {
        self.ask(Request::Drag);
    }

    /// Lets the user resize the window from `direction`.
    pub(in crate::window) fn drag_resize_window(&self, direction: ResizeDirection) {
        self.ask(Request::Resize(edge_of(direction)));
    }

    /// Minimizes the window; `false` asks nothing, since a minimized window
    /// is restored by the window system.
    pub(in crate::window) fn set_minimized(&self, minimized: bool) {
        if minimized {
            self.ask(Request::Minimize);
        }
    }

    /// Maximizes the window, or restores it, when it is not so already.
    pub(in crate::window) fn set_maximized(&self, maximized: bool) {
        if maximized != self.maximized.get() {
            self.maximized.set(maximized);
            self.ask(Request::ToggleMaximize);
        }
    }

    /// Names the window `title`.
    pub(in crate::window) fn set_title(&self, title: &str) {
        self.ask(Request::SetTitle(title.to_owned()));
    }

    /// Lets the background show the desktop through it, or not.
    pub(in crate::window) fn set_transparent(&self, transparent: bool) {
        self.ask(Request::SetTransparent(transparent));
    }

    /// Makes the Option keys `option_as_alt` names Alt on macOS.
    pub(in crate::window) fn set_option_as_alt(&self, option_as_alt: OptionAsAlt) {
        self.ask(Request::OptionAsAlt(option_as_alt));
    }

    /// Lets the input method compose, or passes keys through.
    pub(in crate::window) fn set_ime_allowed(&self, allowed: bool) {
        self.ime.borrow_mut().allowed = allowed;
    }

    /// Anchors the input method's candidates beside the cursor at `area`,
    /// in physical pixels.
    pub(in crate::window) fn set_ime_cursor_area(&self, area: Rect) {
        self.ime.borrow_mut().area = area;
    }

    /// Hands what was asked of the window to misty, in the order asked.
    pub(in crate::window) fn flush(&self, cx: &mut AppContext<'_, ()>) {
        let requests = std::mem::take(&mut *self.requests.borrow_mut());
        let id = self.id;
        for request in requests {
            match request {
                Request::Drag => cx.event_cx(id).drag_window(),
                Request::Minimize => cx.event_cx(id).minimize(),
                Request::OptionAsAlt(option_as_alt) => cx.set_option_as_alt(id, option_as_alt),
                Request::Redraw => cx.request_redraw(id),
                Request::Resize(edge) => cx.event_cx(id).drag_resize_window(edge),
                Request::SetCursor(icon) => cx.event_cx(id).set_cursor(icon),
                Request::SetTitle(title) => cx.set_title(id, title),
                Request::SetTransparent(transparent) => cx.set_transparent(id, transparent),
                Request::ToggleMaximize => cx.event_cx(id).toggle_maximize(),
            }
        }
        let ime = *self.ime.borrow();
        let area = ime.allowed.then_some(ime.area);
        self.ime_focus
            .borrow_mut()
            .follow(&mut cx.event_cx(id), area);
    }

    /// Keeps `request` until the next flush.
    fn ask(&self, request: Request) {
        self.requests.borrow_mut().push(request);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The window edge misty resizes from for winit's `direction`.
fn edge_of(direction: ResizeDirection) -> ResizeEdge {
    match direction {
        ResizeDirection::East => ResizeEdge::East,
        ResizeDirection::North => ResizeEdge::North,
        ResizeDirection::NorthEast => ResizeEdge::NorthEast,
        ResizeDirection::NorthWest => ResizeEdge::NorthWest,
        ResizeDirection::South => ResizeEdge::South,
        ResizeDirection::SouthEast => ResizeEdge::SouthEast,
        ResizeDirection::SouthWest => ResizeEdge::SouthWest,
        ResizeDirection::West => ResizeEdge::West,
    }
}
