//! The system monitor's tests.

use crate::model::input::KeyCode;
use crate::model::paint::row_text;
use crate::model::system::{
    CoreLoad, CoreReading, CpuSample, CpuTicks, DiskReading, MemoryReading, Overview,
};

use super::*;

fn press(code: KeyCode) -> Key {
    Key::plain(code)
}

fn sample(busy: u64, total: u64) -> SystemSample {
    SystemSample {
        cpu: CpuSample {
            cores: vec![CoreReading {
                load: CoreLoad::Ticks(CpuTicks { busy, total }),
                mhz: 0,
            }],
            load: None,
            model: "Test CPU".to_string(),
            total: CoreLoad::Ticks(CpuTicks { busy, total }),
        },
        disks: vec![DiskReading {
            available_kb: 1,
            fs_type: None,
            mount: "/".to_string(),
            total_kb: 2,
            used_kb: 1,
        }],
        gpus: Vec::new(),
        memory: MemoryReading {
            available_kb: 1,
            swap_total_kb: 0,
            swap_used_kb: 0,
            total_kb: 2,
        },
        overview: Overview {
            arch: "x86_64".to_string(),
            hostname: "box".to_string(),
            os: "Linux".to_string(),
            release: "6.8".to_string(),
            uptime_secs: 60,
        },
    }
}

fn loaded_editor() -> SysEditor {
    let mut editor = SysEditor::new();
    editor.on_job(JobReply::System(Ok(sample(250, 1000))));
    editor
}

fn lines(content: &Frame) -> Vec<String> {
    content.rows.iter().map(row_text).collect()
}

#[test]
fn test_the_first_tick_asks_for_a_snapshot() {
    let mut editor = SysEditor::new();
    assert_eq!(
        editor.on_tick(Instant::now()),
        EditorOutcome::Job(JobRequest::System)
    );
}

#[test]
fn test_a_snapshot_fills_the_editor_with_its_sections() {
    let mut editor = loaded_editor();
    let text = lines(&editor.content(40, 120, false)).join("\n");
    for heading in [
        "System Overview",
        "CPU Usage",
        "Memory & Swap",
        "Storage / Disks",
    ] {
        assert!(text.contains(heading), "missing {heading:?} in {text:?}");
    }
    assert!(
        text.contains("25.0%"),
        "the first reading is the share since boot"
    );
}

#[test]
fn test_the_second_snapshot_shows_the_interval_not_the_whole_uptime() {
    let mut editor = loaded_editor();
    editor.on_job(JobReply::System(Ok(sample(260, 1100))));
    let text = lines(&editor.content(40, 120, false)).join("\n");
    // Ten busy ticks in a hundred; since boot it would still read 23.6%.
    assert!(text.contains("10.0%"), "got {text:?}");
}

#[test]
fn test_a_failed_snapshot_is_shown_and_retried() {
    let mut editor = SysEditor::new();
    editor.on_tick(Instant::now());
    editor.on_job(JobReply::System(Err("df: not found".to_string())));
    let content = editor.content(10, 80, false);
    assert!(lines(&content).join("\n").contains("df: not found"));
    assert!(editor.next_tick().is_some());
}

#[test]
fn test_sections_are_jumped_between_with_n_and_back_with_capital_n() {
    let mut editor = loaded_editor();
    // A pane too short for the whole editor, so there is somewhere to go.
    editor.content(6, 120, false);
    assert!(editor.section_starts.len() >= 3);
    editor.on_key(&press(KeyCode::Char('n')));
    assert_eq!(
        editor.view.scroll(),
        editor.section_starts[1].min(editor.body_len - editor.view.rows())
    );
    let after_first = editor.view.scroll();
    editor.on_key(&press(KeyCode::Char('n')));
    assert!(editor.view.scroll() > after_first);
    editor.on_key(&press(KeyCode::Char('N')));
    assert_eq!(editor.view.scroll(), after_first);
}

#[test]
fn test_scrolling_stops_at_the_last_row_that_still_fills_the_pane() {
    let mut editor = loaded_editor();
    editor.content(6, 120, false);
    editor.on_key(&press(KeyCode::Char('G')));
    assert_eq!(editor.view.scroll(), editor.body_len - editor.view.rows());
    editor.on_key(&press(KeyCode::Char('j')));
    assert_eq!(editor.view.scroll(), editor.body_len - editor.view.rows());
    editor.on_key(&press(KeyCode::Char('g')));
    editor.on_key(&press(KeyCode::Char('g')));
    assert_eq!(editor.view.scroll(), 0);
}

#[test]
fn test_a_pane_taller_than_the_editor_never_scrolls() {
    let mut editor = loaded_editor();
    editor.content(80, 120, false);
    editor.on_key(&press(KeyCode::Char('j')));
    assert_eq!(editor.view.scroll(), 0);
}

#[test]
fn test_refresh_asks_at_once_and_pause_stops_the_asking() {
    let mut editor = loaded_editor();
    assert_eq!(
        editor.on_key(&press(KeyCode::Char('r'))),
        EditorOutcome::Job(JobRequest::System)
    );
    editor.on_key(&press(KeyCode::Char('p')));
    assert_eq!(editor.on_tick(Instant::now()), EditorOutcome::Consumed);
    assert!(row_text(&editor.title_row()).contains("paused"));
}

#[test]
fn test_no_plain_key_closes_the_editor() {
    // `q` and `Esc` sit too close to the keys a reader reaches for to be
    // ways out; the window's close chord is.
    let mut editor = loaded_editor();
    assert_ne!(
        editor.on_key(&press(KeyCode::Char('q'))),
        EditorOutcome::Close
    );
    assert_ne!(editor.on_key(&press(KeyCode::Escape)), EditorOutcome::Close);
}

#[test]
fn test_every_key_of_the_monitor_is_spelled_right() {
    assert_eq!(table::invalid_specs(KEYS), Vec::<&str>::new());
}
