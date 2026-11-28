//! The header every System page draws: where it is among the System pages,
//! `System > Port Monitor`, either part a press away from a list of them
//! all to go to another; and a summary of what it shows at the right, its
//! parts pressed for what their keys do.

use misty::view::{Page, Space, untitled_page};
use winter_model::editor::{
    EditorOutcome, Header, HeaderPart, HeaderPlace, HeaderSegment, PathPart, header_view,
};
use winter_model::view::ToolbarTone;

// ========================================================================
// Constants
// ========================================================================

/// What the System pages are called, each its place in the path.
pub const RESOURCE_MONITOR: &str = "Resource Monitor";
pub const PROCESS_MONITOR: &str = "Process Monitor";
pub const PORT_MONITOR: &str = "Port Monitor";
pub const STARTUP_PROGRAMS: &str = "Startup Programs";

/// What the path's first part is called: the group the pages are in.
const SYSTEM: &str = "System";

/// The System pages, in the order the path lists them: each one's name and
/// the command that shows it.
const SYSTEM_PAGES: [(&str, &str); 4] = [
    (RESOURCE_MONITOR, "system_monitor"),
    (PROCESS_MONITOR, "process_monitor"),
    (PORT_MONITOR, "ports_monitor"),
    (STARTUP_PROGRAMS, "startup_monitor"),
];

/// The first action a System page in the path's list is told back as,
/// past every part of a summary.
const PLACE_ACTION_BASE: u16 = 0x1000;

// ========================================================================
// Data Structures
// ========================================================================

/// One part of a monitor's summary, on its header: what it says and in what
/// tone, what a press on it runs, and its tooltip.
#[derive(Clone, Debug, PartialEq)]
pub struct SummaryPart<A> {
    /// What a press runs; `None` for a part with nothing to press.
    pub act: Option<A>,
    /// For one of a set of choices of how the list is laid out, whether it
    /// is the one chosen; `None` for a part that is no choice.
    pub chosen: Option<bool>,
    /// What it says.
    pub text: String,
    /// How it is drawn.
    pub tone: ToolbarTone,
    /// What it means, and the key that does what a press does.
    pub tooltip: Option<String>,
}

// ========================================================================
// SummaryPart
// ========================================================================

impl<A> SummaryPart<A> {
    /// A part saying `text` in `tone`, with nothing to press.
    pub fn label(text: impl Into<String>, tone: ToolbarTone) -> Self {
        Self {
            act: None,
            chosen: None,
            text: text.into(),
            tone,
            tooltip: None,
        }
    }

    /// A part saying `text` in `tone` that runs `act` on a press, its
    /// tooltip `about`.
    pub fn button(text: impl Into<String>, tone: ToolbarTone, act: A, about: String) -> Self {
        Self {
            act: Some(act),
            chosen: None,
            text: text.into(),
            tone,
            tooltip: Some(about),
        }
    }

    /// One of a set of choices of how the list is laid out, saying `text`
    /// and lit when `chosen`, a press on one not chosen running `act`, its
    /// tooltip `about`.
    pub fn choice(text: impl Into<String>, chosen: bool, act: A, about: String) -> Self {
        let tone = match chosen {
            true => ToolbarTone::Accent,
            false => ToolbarTone::Dim,
        };
        Self {
            act: (!chosen).then_some(act),
            chosen: Some(chosen),
            text: text.into(),
            tone,
            tooltip: Some(about),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// An untitled page for `monitor`, the System page called `shown`: its
/// header the path to it, a press on either part listing every System page
/// to go to with `go`, and the summary `summary` gives, a press on a part
/// of it running `run` with what the part runs.
pub fn monitor_page<'a, A: 'static, C: 'static>(
    monitor: &A,
    shown: &str,
    summary: fn(&A) -> Vec<SummaryPart<C>>,
    run: fn(&mut A, C),
    go: fn(&mut A, EditorOutcome),
) -> Page<'a, A> {
    let pressed = move |monitor: &mut A, action: u16| match place_command(action) {
        Some(command) => go(monitor, EditorOutcome::RunActionHere(command.to_string())),
        None => {
            if let Some(act) = summary_act(summary(monitor), action) {
                run(monitor, act);
            }
        }
    };
    let header = monitor_header(shown, &summary(monitor));
    untitled_page()
        .content_padding(Space::Cells(0.0))
        .header(header_view(&header, |_: &mut A, _| {}, pressed))
}

/// The header of the System page called `shown`: the path to it, either
/// part listing every System page, and `parts` as its summary, each part
/// with a press told back as its place in `parts`.
fn monitor_header<A>(shown: &str, parts: &[SummaryPart<A>]) -> Header {
    let places: Vec<HeaderPlace> = SYSTEM_PAGES
        .iter()
        .zip(PLACE_ACTION_BASE..)
        .map(|((name, _), action)| HeaderPlace {
            action,
            current: *name == shown,
            name: (*name).to_string(),
        })
        .collect();
    let path = vec![
        PathPart::listing(SYSTEM, places.clone()),
        PathPart::listing(shown, places),
    ];
    let summary = parts
        .iter()
        .zip(0u16..)
        .map(|(part, at)| {
            let action = part.act.as_ref().map(|_| at);
            let tooltip = part.tooltip.clone().unwrap_or_default();
            match part.chosen {
                Some(chosen) => HeaderPart::choice(part.text.clone(), chosen, action, tooltip),
                None => match action {
                    Some(action) => {
                        HeaderPart::button(part.text.clone(), part.tone, action, tooltip)
                    }
                    None => HeaderPart::label(part.text.clone(), part.tone),
                },
            }
        })
        .collect();
    Header::new(vec![
        HeaderSegment::Path(path),
        HeaderSegment::Summary(summary),
    ])
}

/// The command showing the System page a press told back as `action`, if it
/// is one.
fn place_command(action: u16) -> Option<&'static str> {
    let at = usize::from(action.checked_sub(PLACE_ACTION_BASE)?);
    SYSTEM_PAGES.get(at).map(|(_, command)| *command)
}

/// What the part of `parts` told back as `action` runs.
fn summary_act<A>(parts: Vec<SummaryPart<A>>, action: u16) -> Option<A> {
    parts.into_iter().nth(usize::from(action))?.act
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_path_lists_every_system_page_the_shown_one_marked() {
        let header = monitor_header::<char>(PORT_MONITOR, &[]);
        let path = header.path();
        let names: Vec<&str> = path.iter().map(|part| part.name.as_str()).collect();
        assert_eq!(names, [SYSTEM, PORT_MONITOR]);
        let current: Vec<&str> = path[0]
            .places
            .iter()
            .filter(|place| place.current)
            .map(|place| place.name.as_str())
            .collect();
        assert_eq!(current, [PORT_MONITOR]);
        let commands: Vec<Option<&str>> = path[0]
            .places
            .iter()
            .map(|place| place_command(place.action))
            .collect();
        assert_eq!(
            commands,
            SYSTEM_PAGES.map(|(_, command)| Some(command)),
            "each place runs its page's command"
        );
    }

    #[test]
    fn test_a_summary_part_is_pressed_by_the_action_it_is_drawn_with() {
        let parts = vec![
            SummaryPart::label("12 processes", ToolbarTone::Normal),
            SummaryPart::choice("Flat", true, 'f', "Listed flat".to_string()),
            SummaryPart::choice("Tree", false, 't', "List as a tree".to_string()),
        ];
        let header = monitor_header(PROCESS_MONITOR, &parts);
        let actions: Vec<Option<u16>> = header.summary().map(|part| part.action).collect();
        assert_eq!(
            actions,
            [None, None, Some(2)],
            "a label and the chosen choice run nothing"
        );
        assert_eq!(summary_act(parts.clone(), 2), Some('t'));
        assert_eq!(summary_act(parts.clone(), 1), None);
        assert_eq!(summary_act(parts, 9), None);
        assert_eq!(place_command(2), None, "a part is no page");
    }
}
