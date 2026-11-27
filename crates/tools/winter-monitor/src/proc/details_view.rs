//! How the process monitor shows one process looked into: what it is in a
//! line by the heading, then a section for each part (its open files,
//! sockets, libraries, memory, environment, threads), each saying why where
//! the system would not tell it.

use misty::view::{Extent, Space, column, item, page, row, section, text};
use misty::{Element, HostedView, ThemeToken};

use winter_model::monitor::{
    FileKind, MemoryRegion, OpenFile, ProcessDetails, Protocol, Socket, ThreadInfo,
};

use crate::units::format_kb;

// ========================================================================
// Constants
// ========================================================================

/// The id of the page's scrolling body, which the keys scroll.
pub const BODY: &str = "proc.details.body";

/// The ids of the sections, the stops `]` and `[` move between.
pub const SECTIONS: [&str; 6] = [
    "proc.details.files",
    "proc.details.sockets",
    "proc.details.libraries",
    "proc.details.memory",
    "proc.details.environment",
    "proc.details.threads",
];

/// What the page says before the first answer.
const READING: &str = "reading the process…";

/// The most rows a section lists; a process with thousands of descriptors
/// would otherwise build them all.
const MAX_ROWS: usize = 500;

/// How wide the narrow columns are, in text columns: a descriptor's number
/// and kind, a mapping's sizes, a thread's id and state.
const NUMBER_COLS: f32 = 7.0;
const KIND_COLS: f32 = 7.0;
const SIZE_COLS: f32 = 9.0;

/// The room around a row, as a page's items have.
const ROW_PADDING: (f32, f32) = (1.0, 0.1);

/// Gap between a row's cells.
const GAP_COLS: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// What the page knows of the process it looks into.
#[derive(Debug)]
pub struct Details {
    /// Its command line, from the list.
    pub command: String,
    /// What was read, once it is.
    pub info: Option<ProcessDetails>,
    /// Its name, from the list.
    pub name: String,
    /// Its id.
    pub pid: u32,
}

// ========================================================================
// Details
// ========================================================================

impl HostedView for Details {
    /// The process in the heading, over its parts.
    fn view(&self) -> Element<'_, Self> {
        let heading = page(format!("{} (pid {})", self.name, self.pid))
            .action(text(self.command.as_str()).muted());
        let Some(info) = &self.info else {
            return heading
                .scrolling(text(READING).muted())
                .scroll_id(BODY)
                .into();
        };
        let sections = vec![
            part(SECTIONS[0], "Open Files", &info.files, file_row),
            part(SECTIONS[1], "Sockets", &info.sockets, socket_row),
            part(
                SECTIONS[2],
                "Libraries",
                &info.libraries,
                |path: &String| item(path.as_str()).into(),
            ),
            part(SECTIONS[3], "Memory", &info.memory, memory_row),
            part(SECTIONS[4], "Environment", &info.environment, env_row),
            part(SECTIONS[5], "Threads", &info.threads, thread_row),
        ];
        heading.scrolling(column(sections)).scroll_id(BODY).into()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A section headed `title` with its count, known as `id`, a row for each of
/// `read` as `row_of` draws it, or why it could not be read.
fn part<'a, T>(
    id: &'static str,
    title: &str,
    read: &'a Result<Vec<T>, String>,
    row_of: impl Fn(&'a T) -> Element<'a, Details>,
) -> Element<'a, Details> {
    let shown = match read {
        Err(why) => section(title.to_string())
            .item(Element::from(text(why.as_str()).color(ThemeToken::Warning))),
        Ok(rows) if rows.is_empty() => {
            section(format!("{title} (0)")).item(Element::from(text("none").muted()))
        }
        Ok(rows) => {
            let mut shown = section(format!("{title} ({})", rows.len()))
                .items(rows.iter().take(MAX_ROWS).map(row_of));
            if rows.len() > MAX_ROWS {
                shown = shown.item(Element::from(
                    text(format!("and {} more", rows.len() - MAX_ROWS)).muted(),
                ));
            }
            shown
        }
    };
    Element::from(shown).id(id)
}

/// A row of cells: `fixed` each of its width, then the rest taking the room
/// left.
fn cells<'a>(fixed: Vec<(String, f32)>, rest: String) -> Element<'a, Details> {
    let mut shown: Vec<Element<'a, Details>> = fixed
        .into_iter()
        .map(|(value, cols)| {
            Element::from(text(value).muted())
                .width(Extent::Cells(cols))
                .shrink(0.0)
        })
        .collect();
    shown.push(Element::from(text(rest)).grow(1.0));
    row(shown)
        .gap(Space::Cells(GAP_COLS))
        .padding(ROW_PADDING.0, ROW_PADDING.1)
}

/// An open file: its descriptor, its kind, and what it is.
fn file_row(file: &OpenFile) -> Element<'_, Details> {
    let fd = file.fd.map_or_else(String::new, |fd| fd.to_string());
    let kind = match file.kind {
        FileKind::File => "file",
        FileKind::Pipe => "pipe",
        FileKind::Socket => "socket",
        FileKind::Other => "other",
    };
    cells(
        vec![(fd, NUMBER_COLS), (kind.to_string(), KIND_COLS)],
        file.target.clone(),
    )
    .tooltip(format!(
        "Descriptor {}: {}",
        file.fd.unwrap_or_default(),
        file.target
    ))
}

/// A socket: its protocol and state, and where it is bound and connected.
fn socket_row(socket: &Socket) -> Element<'_, Details> {
    let protocol = match socket.protocol {
        Protocol::Tcp => "tcp",
        Protocol::Udp => "udp",
    };
    let state = match socket.listening {
        true => "listening".to_string(),
        false => socket.state.to_lowercase(),
    };
    let reach = match &socket.remote {
        Some(remote) => format!("{} \u{2192} {remote}", socket.local),
        None => socket.local.clone(),
    };
    cells(
        vec![(protocol.to_string(), KIND_COLS), (state, SIZE_COLS + 3.0)],
        reach,
    )
}

/// A source of memory: what is in memory of it, the space it takes, and
/// what it is.
fn memory_row(region: &MemoryRegion) -> Element<'_, Details> {
    let resident = region.resident_kb.map_or_else(String::new, format_kb);
    let mut about = format!(
        "{} of address space in {} mapping{}",
        format_kb(region.size_kb),
        region.mappings,
        if region.mappings == 1 { "" } else { "s" }
    );
    if let Some(kb) = region.resident_kb {
        about = format!("{} in memory, {about}", format_kb(kb));
    }
    cells(
        vec![
            (resident, SIZE_COLS),
            (format_kb(region.size_kb), SIZE_COLS),
        ],
        region.name.clone(),
    )
    .tooltip(about)
}

/// An environment variable: its name, then its value.
fn env_row((name, value): &(String, String)) -> Element<'_, Details> {
    row([
        Element::from(text(name.as_str()).color(ThemeToken::Accent)),
        Element::from(text(value.as_str())).grow(1.0),
    ])
    .gap(Space::Cells(GAP_COLS))
    .padding(ROW_PADDING.0, ROW_PADDING.1)
}

/// A thread: its id, its state, and its name.
fn thread_row(thread: &ThreadInfo) -> Element<'_, Details> {
    cells(
        vec![
            (thread.tid.to_string(), NUMBER_COLS),
            (format!("{:?}", thread.state).to_lowercase(), SIZE_COLS),
        ],
        thread.name.clone(),
    )
}
