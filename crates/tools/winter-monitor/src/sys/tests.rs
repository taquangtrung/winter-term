//! The system monitor's tests.

use misty::testing::{TestWindow, drawn_text};
use winter_model::input::KeyCode;
use winter_model::system::{
    CoreLoad, CoreReading, CpuSample, CpuTicks, DiskReading, GpuReading, MemoryReading, Overview,
    TrafficReading,
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
        disk_io: Vec::new(),
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
        network: vec![traffic("eth0", busy * 1024, total * 1024)],
        overview: Overview {
            arch: "x86_64".to_string(),
            hostname: "box".to_string(),
            os: "Linux".to_string(),
            release: "6.8".to_string(),
            uptime_secs: 60,
        },
    }
}

fn traffic(name: &str, in_bytes: u64, out_bytes: u64) -> TrafficReading {
    TrafficReading {
        in_bytes,
        name: name.to_string(),
        out_bytes,
    }
}

fn loaded_editor() -> SysEditor {
    let mut editor = SysEditor::new();
    editor.on_job(JobReply::System(Ok(sample(250, 1000))));
    editor
}

/// A window `height` pixels tall, its scrolls jumping rather than gliding.
fn window(height: u32) -> TestWindow {
    let mut window = TestWindow::new(1000, height);
    window.set_reduced_motion(true);
    window
}

/// What `editor` draws over `window`, told the window's text rows and
/// columns first as the window tells a page.
fn draw(editor: &mut SysEditor, window: &TestWindow) -> String {
    let mut frame = window.frame();
    let bounds = frame.bounds();
    let cell = frame.cell_metrics();
    editor.content(
        (bounds.height / cell.height) as usize,
        (bounds.width / cell.width) as usize,
        false,
    );
    editor.draw_view(&mut frame, bounds, &misty::Theme::dark());
    drawn_text(&frame).join("\n")
}

/// How far down the body is scrolled, drawn again after a key so the
/// scroll it asked for is laid out.
fn offset(editor: &mut SysEditor, window: &TestWindow) -> f32 {
    draw(editor, window);
    editor.view.offset(BODY).expect("the body is drawn")
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
fn test_a_snapshot_fills_the_page_with_its_cards() {
    let mut editor = loaded_editor();
    let text = draw(&mut editor, &window(1600));
    for heading in ["CPU", "Memory", "Disks"] {
        assert!(text.contains(heading), "missing {heading:?} in {text:?}");
    }
    assert!(
        text.contains("box · Linux"),
        "the machine in one line: {text:?}"
    );
    assert!(
        text.contains("25.0%"),
        "the first reading is the share since boot"
    );
    assert!(!text.contains("GPU"), "no GPU card without a GPU");
}

#[test]
fn test_the_second_snapshot_shows_the_interval_not_the_whole_uptime() {
    let mut editor = loaded_editor();
    editor.on_job(JobReply::System(Ok(sample(260, 1100))));
    let text = draw(&mut editor, &window(1600));
    // Ten busy ticks in a hundred; since boot it would still read 23.6%.
    assert!(text.contains("10.0%"), "got {text:?}");
    assert_eq!(editor.view.state().history.cpu, [25.0, 10.0]);
}

#[test]
fn test_traffic_cards_measure_between_snapshots_and_show_only_what_is_counted() {
    let mut editor = SysEditor::new();
    let start = Instant::now();
    let machine = editor.view.state_mut();
    let mut first = sample(250, 1000);
    first.disk_io = vec![traffic("nvme0n1", 0, 4096)];
    machine.take(Ok(first), start);
    let window = window(2000);
    let text = draw(&mut editor, &window);
    assert!(
        text.contains("Network") && text.contains("Disk Activity"),
        "{text:?}"
    );
    assert!(
        text.contains("measuring…"),
        "no rate from one snapshot: {text:?}"
    );

    let mut second = sample(250 + 2048, 1000);
    second.disk_io = vec![traffic("nvme0n1", 1024 * 1024, 4096)];
    let later = start + std::time::Duration::from_secs(2);
    editor.view.state_mut().take(Ok(second), later);
    let text = draw(&mut editor, &window);
    // 2 MiB received over two seconds; the 1 MiB read, the same.
    assert!(text.contains("\u{2193}\n 1.0M/s"), "{text:?}");
    assert!(text.contains("\u{2191}\n 0K/s"), "{text:?}");
    assert!(text.contains("read 512K/s"), "{text:?}");

    let mut quiet = sample(250, 1000);
    quiet.network.clear();
    editor.view.state_mut().take(Ok(quiet), later);
    let text = draw(&mut editor, &window);
    assert!(
        !text.contains("Network"),
        "no card where nothing is counted: {text:?}"
    );
}

#[test]
fn test_a_card_keeps_its_shape_as_its_history_line_comes_in() {
    let mut editor = loaded_editor();
    let window = window(1600);
    draw(&mut editor, &window);
    editor.on_job(JobReply::System(Ok(sample(260, 1100))));
    // Drawn again with a history to trace, where a card that grew a line
    // would have its meters' state taken by the line's.
    let text = draw(&mut editor, &window);
    assert!(text.contains("10.0%"), "got {text:?}");
}

#[test]
fn test_the_history_keeps_the_last_two_minutes() {
    let mut editor = loaded_editor();
    for at in 0..(2 * HISTORY_LEN as u64) {
        editor.on_job(JobReply::System(Ok(sample(260 + at, 1100 + at))));
    }
    assert_eq!(editor.view.state().history.cpu.len(), HISTORY_LEN);
}

#[test]
fn test_a_failed_snapshot_is_shown_and_retried() {
    let mut editor = SysEditor::new();
    editor.on_tick(Instant::now());
    editor.on_job(JobReply::System(Err("df: not found".to_string())));
    assert!(draw(&mut editor, &window(400)).contains("df: not found"));
    assert!(editor.next_tick().is_some());
}

#[test]
fn test_cards_are_jumped_between_with_brackets() {
    let mut editor = loaded_editor();
    // A pane narrow enough to stack the cards and too short for them all,
    // so there is somewhere to go.
    let mut window = TestWindow::new(600, 150);
    window.set_reduced_motion(true);
    assert_eq!(offset(&mut editor, &window), 0.0);
    editor.on_key(&press(KeyCode::Char(']')));
    let first = offset(&mut editor, &window);
    assert!(first > 0.0, "down to the next card");
    editor.on_key(&press(KeyCode::Char(']')));
    let second = offset(&mut editor, &window);
    assert!(second > first, "{second} past {first}");
    editor.on_key(&press(KeyCode::Char('[')));
    assert_eq!(offset(&mut editor, &window), first);
}

#[test]
fn test_g_and_g_g_scroll_to_the_ends_and_j_stops_at_the_bottom() {
    let mut editor = loaded_editor();
    let window = window(300);
    offset(&mut editor, &window);
    editor.on_key(&Key {
        shift: true,
        ..press(KeyCode::Char('G'))
    });
    let bottom = offset(&mut editor, &window);
    assert!(bottom > 0.0);
    editor.on_key(&press(KeyCode::Char('j')));
    assert_eq!(offset(&mut editor, &window), bottom, "nowhere further");
    editor.on_key(&press(KeyCode::Char('g')));
    editor.on_key(&press(KeyCode::Char('g')));
    assert_eq!(offset(&mut editor, &window), 0.0);
}

#[test]
fn test_a_pane_taller_than_the_page_never_scrolls() {
    let mut editor = loaded_editor();
    let window = window(3000);
    offset(&mut editor, &window);
    editor.on_key(&press(KeyCode::Char('j')));
    assert_eq!(offset(&mut editor, &window), 0.0);
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
    assert!(draw(&mut editor, &window(400)).contains(PAUSED));
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
    assert_eq!(table::invalid_specs(&KEYS), Vec::<&str>::new());
}

#[test]
fn test_every_control_says_what_it_is() {
    let mut editor = loaded_editor();
    let window = window(1600);
    let mut frame = window.frame();
    let bounds = frame.bounds();
    let cell = frame.cell_metrics();
    editor.content(
        (bounds.height / cell.height) as usize,
        (bounds.width / cell.width) as usize,
        false,
    );
    editor.draw_view(&mut frame, bounds, &misty::Theme::dark());
    let unnamed = misty::testing::unnamed_controls(&frame);
    assert!(unnamed.is_empty(), "{unnamed:?}");
}

#[test]
fn test_long_disk_mounts_and_gpu_names_are_not_truncated() {
    let mut s = sample(250, 1000);
    s.disks.push(DiskReading {
        available_kb: 100,
        fs_type: Some("ext4".to_string()),
        mount: "/var/lib/docker/volumes".to_string(),
        total_kb: 200,
        used_kb: 100,
    });
    s.gpus.push(GpuReading {
        mem_total_kb: 24 * 1024 * 1024,
        mem_used_kb: 4 * 1024 * 1024,
        name: "NVIDIA GeForce RTX 4090".to_string(),
        temp_c: Some(45),
        util_percent: 12,
    });
    let mut editor = SysEditor::new();
    editor.on_job(JobReply::System(Ok(s)));
    let text = draw(&mut editor, &window(1600));
    assert!(
        text.contains("/var/lib/docker/volumes"),
        "long mount truncated in {text:?}"
    );
    assert!(
        text.contains("NVIDIA GeForce RTX 4090"),
        "long GPU name truncated in {text:?}"
    );
}
