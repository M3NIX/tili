use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::PtySize;
use tastty_core::{Parser, Position, TerminalSize};
use tili::pane::Pane;
use tili::pty::{CommandSpec, PtyEvent, PtySession};
use tili::tiling::PaneId;

fn selected_text(pane: &mut Pane, start: Position, end: Position) -> Option<String> {
    pane.selection_clear();
    assert!(pane.selection_start(start));
    assert!(pane.selection_extend(end));
    pane.selected_text()
}

#[test]
fn bounded_output_preserves_bytes_from_multiple_busy_ptys() {
    let (event_tx, event_rx) = mpsc::sync_channel(2);
    let mut sessions = Vec::new();
    for index in 0..4 {
        sessions.push(
            PtySession::spawn(
                PaneId(index),
                &CommandSpec::new(
                    "python3",
                    [
                        "-c",
                        "import sys; sys.stdout.write('x' * 524288); sys.stdout.flush()",
                    ],
                ),
                None,
                PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                },
                event_tx.clone(),
            )
            .unwrap(),
        );
    }
    drop(event_tx);
    std::thread::sleep(Duration::from_millis(50));
    let mut counts = [0; 4];
    let mut exited = [false; 4];
    let deadline = Instant::now() + Duration::from_secs(10);
    while exited.contains(&false) {
        match event_rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            PtyEvent::Output(pane, bytes) => {
                assert!(bytes.iter().all(|byte| *byte == b'x'));
                counts[pane.0 as usize] += bytes.len();
            }
            PtyEvent::Eof(pane) => exited[pane.0 as usize] = true,
            PtyEvent::ReadError(_, error) => panic!("PTY failed: {error}"),
        }
    }
    assert_eq!(counts, [524288; 4]);
    for session in &mut sessions {
        assert!(session.try_wait().unwrap().unwrap().success());
    }
}

#[test]
fn saturated_pty_shuts_down_when_receiver_is_dropped() {
    let (event_tx, event_rx) = mpsc::sync_channel(1);
    let session = PtySession::spawn(
        PaneId(1),
        &CommandSpec::new("/usr/bin/yes", ["busy"]),
        None,
        PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        },
        event_tx,
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(30));
    drop(event_rx);
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        drop(session);
        done_tx.send(()).unwrap();
    });
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
}

#[test]
fn shell_pty_accepts_input_parses_output_resizes_and_exits() {
    let pane = PaneId(42);
    let (event_tx, event_rx) = mpsc::sync_channel(128);
    let mut session = PtySession::spawn(
        pane,
        &CommandSpec::new("/bin/sh", std::iter::empty::<String>()),
        None,
        PtySize {
            rows: 8,
            cols: 40,
            pixel_width: 0,
            pixel_height: 0,
        },
        event_tx,
    )
    .unwrap();

    session.resize(73, 19).unwrap();
    let size = session.size().unwrap();
    assert_eq!(size.cols, 73);
    assert_eq!(size.rows, 19);

    session
        .write_all(b"printf 'TILI_PTY_OK\\n'\nexit\n")
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut parser = Parser::new(TerminalSize { rows: 8, cols: 40 }, 100);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for PTY EOF");
        match event_rx.recv_timeout(remaining).unwrap() {
            PtyEvent::Output(event_pane, bytes) => {
                assert_eq!(event_pane, pane);
                parser.process(&bytes);
            }
            PtyEvent::Eof(event_pane) => {
                assert_eq!(event_pane, pane);
                break;
            }
            PtyEvent::ReadError(_, error) => panic!("PTY read failed: {error}"),
        }
    }

    assert!(parser.screen().contents().contains("TILI_PTY_OK"));
    let status = session
        .try_wait()
        .unwrap()
        .expect("shell should have exited");
    assert!(status.success());
}

#[test]
fn pane_reflows_existing_output_across_width_changes() {
    let (event_tx, _event_rx) = mpsc::sync_channel(128);
    let mut pane = Pane::spawn(
        PaneId(7),
        &CommandSpec::new("/bin/cat", std::iter::empty::<String>()),
        None,
        24,
        6,
        event_tx,
    )
    .unwrap();
    pane.process_output(b"abcdefghijklmnopqrstuv").unwrap();

    pane.resize(8, 6).unwrap();
    assert_eq!(pane.screen().size(), TerminalSize { rows: 6, cols: 8 });
    assert!(pane.screen().contents().contains("abcdefghijklmnopqrstuv"));

    pane.resize(24, 6).unwrap();
    assert_eq!(pane.screen().size(), TerminalSize { rows: 6, cols: 24 });
    assert!(pane.screen().contents().contains("abcdefghijklmnopqrstuv"));
}

#[test]
fn pane_selection_uses_terminal_emulator_spacing_and_rejects_empty_ranges() {
    let (event_tx, _event_rx) = mpsc::sync_channel(128);
    let mut pane = Pane::spawn(
        PaneId(12),
        &CommandSpec::new("/bin/cat", std::iter::empty::<String>()),
        None,
        30,
        6,
        event_tx,
    )
    .unwrap();
    pane.process_output(b"\x1b[6Gfirst second").unwrap();

    assert_eq!(
        selected_text(
            &mut pane,
            tastty_core::Position::new(0, 0),
            tastty_core::Position::new(0, 25),
        )
        .as_deref(),
        Some("     first second")
    );
    assert_eq!(
        selected_text(
            &mut pane,
            tastty_core::Position::new(3, 0),
            tastty_core::Position::new(3, 25),
        ),
        None
    );
}

#[test]
fn pane_selection_preserves_hard_line_breaks() {
    let (event_tx, _event_rx) = mpsc::sync_channel(128);
    let mut pane = Pane::spawn(
        PaneId(13),
        &CommandSpec::new("/bin/cat", std::iter::empty::<String>()),
        None,
        30,
        6,
        event_tx,
    )
    .unwrap();
    pane.process_output(b"first\r\nsecond\r\nthird").unwrap();

    assert_eq!(
        selected_text(
            &mut pane,
            tastty_core::Position::new(0, 0),
            tastty_core::Position::new(2, 4),
        )
        .as_deref(),
        Some("first\nsecond\nthird")
    );
    assert_eq!(
        selected_text(
            &mut pane,
            tastty_core::Position::new(2, 4),
            tastty_core::Position::new(0, 0),
        )
        .as_deref(),
        Some("first\nsecond\nthird")
    );
}

#[test]
fn pane_selection_preserves_unicode_and_punctuation() {
    let (event_tx, _event_rx) = mpsc::sync_channel(128);
    let mut pane = Pane::spawn(
        PaneId(14),
        &CommandSpec::new("/bin/cat", std::iter::empty::<String>()),
        None,
        30,
        6,
        event_tx,
    )
    .unwrap();
    pane.process_output("α中🙂\r\ncafé <>&[]{}".as_bytes())
        .unwrap();

    let selected = selected_text(
        &mut pane,
        tastty_core::Position::new(0, 0),
        tastty_core::Position::new(1, 11),
    )
    .unwrap();
    assert_eq!(
        selected.lines().map(str::trim_end).collect::<Vec<_>>(),
        ["α中🙂", "café <>&[]{}"]
    );
}

#[test]
fn pane_selection_preserves_a_complete_wide_character() {
    let (event_tx, _event_rx) = mpsc::sync_channel(128);
    let mut pane = Pane::spawn(
        PaneId(15),
        &CommandSpec::new("/bin/cat", std::iter::empty::<String>()),
        None,
        30,
        6,
        event_tx,
    )
    .unwrap();
    pane.process_output("中".as_bytes()).unwrap();

    assert_eq!(
        selected_text(
            &mut pane,
            tastty_core::Position::new(0, 0),
            tastty_core::Position::new(0, 1),
        )
        .as_deref(),
        Some("中")
    );
}

#[test]
fn live_pane_observes_growth_after_being_shrunk() {
    let (event_tx, event_rx) = mpsc::sync_channel(128);
    let mut pane = Pane::spawn(
        PaneId(11),
        &CommandSpec::new("/bin/sh", std::iter::empty::<String>()),
        None,
        40,
        8,
        event_tx,
    )
    .unwrap();

    pane.resize(8, 6).unwrap();
    pane.write_input(b"printf 'SMALL:'; stty size\n").unwrap();
    wait_for_pane_text(&mut pane, &event_rx, "SMALL:6 8");

    pane.resize(40, 10).unwrap();
    pane.write_input(b"printf 'LARGE:'; stty size; printf 'EXPANDED_12345678901234567890\\n'\n")
        .unwrap();
    wait_for_pane_text(&mut pane, &event_rx, "LARGE:10 40");
    wait_for_pane_text(&mut pane, &event_rx, "EXPANDED_12345678901234567890");
    assert_eq!(pane.screen().size(), TerminalSize { rows: 10, cols: 40 });
}

fn wait_for_pane_text(pane: &mut Pane, events: &mpsc::Receiver<PtyEvent>, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if pane.screen().contents().contains(expected) {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for {expected:?}; screen was {:?}",
            pane.screen().contents()
        );
        match events.recv_timeout(remaining).unwrap() {
            PtyEvent::Output(event_pane, bytes) => {
                assert_eq!(event_pane, pane.id());
                pane.process_output(&bytes).unwrap();
            }
            PtyEvent::Eof(event_pane) => panic!("pane {event_pane:?} exited unexpectedly"),
            PtyEvent::ReadError(event_pane, error) => {
                panic!("pane {event_pane:?} read failed: {error}")
            }
        }
    }
}

#[test]
fn pane_scrollback_moves_independently_of_the_child_process() {
    let (event_tx, _event_rx) = mpsc::sync_channel(128);
    let mut pane = Pane::spawn(
        PaneId(8),
        &CommandSpec::new("/bin/cat", std::iter::empty::<String>()),
        None,
        20,
        4,
        event_tx,
    )
    .unwrap();
    for line in 0..12 {
        pane.process_output(format!("line {line}\r\n").as_bytes())
            .unwrap();
    }

    assert!(pane.screen().scrollback_available() > 0);
    assert!(pane.scroll_up(3));
    assert_eq!(pane.screen().scrollback(), 3);
    assert!(pane.scroll_down(3));
    assert_eq!(pane.screen().scrollback(), 0);
}

#[test]
fn pane_detects_and_encodes_guest_mouse_reporting() {
    let (event_tx, _event_rx) = mpsc::sync_channel(128);
    let mut pane = Pane::spawn(
        PaneId(10),
        &CommandSpec::new("/bin/cat", std::iter::empty::<String>()),
        None,
        20,
        4,
        event_tx,
    )
    .unwrap();
    pane.process_output(b"\x1b[?1000h\x1b[?1006h").unwrap();

    assert!(pane.reports_mouse());
    assert!(
        pane.write_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left,),
            column: 3,
            row: 2,
            modifiers: crossterm::event::KeyModifiers::NONE,
        })
        .unwrap()
    );
}

#[test]
fn multiple_pty_sessions_keep_output_and_processes_independent() {
    let (event_tx, event_rx) = mpsc::sync_channel(128);
    let mut first = PtySession::spawn(
        PaneId(1),
        &CommandSpec::new("/bin/sh", std::iter::empty::<String>()),
        None,
        PtySize::default(),
        event_tx.clone(),
    )
    .unwrap();
    let mut second = PtySession::spawn(
        PaneId(2),
        &CommandSpec::new("/bin/sh", std::iter::empty::<String>()),
        None,
        PtySize::default(),
        event_tx,
    )
    .unwrap();

    first.write_all(b"printf 'ONLY_FIRST\\n'\nexit\n").unwrap();
    second
        .write_all(b"printf 'ONLY_SECOND\\n'\nexit\n")
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut first_output = Vec::new();
    let mut second_output = Vec::new();
    let mut eof_count = 0;
    while eof_count < 2 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for both PTYs");
        match event_rx.recv_timeout(remaining).unwrap() {
            PtyEvent::Output(PaneId(1), bytes) => first_output.extend(bytes),
            PtyEvent::Output(PaneId(2), bytes) => second_output.extend(bytes),
            PtyEvent::Eof(_) => eof_count += 1,
            PtyEvent::ReadError(_, error) => panic!("PTY read failed: {error}"),
            PtyEvent::Output(pane, _) => panic!("unexpected pane {pane}"),
        }
    }

    let first_output = String::from_utf8_lossy(&first_output);
    let second_output = String::from_utf8_lossy(&second_output);
    assert!(first_output.contains("ONLY_FIRST"));
    assert!(!first_output.contains("ONLY_SECOND"));
    assert!(second_output.contains("ONLY_SECOND"));
    assert!(!second_output.contains("ONLY_FIRST"));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut first_status = None;
    while first_status.is_none() && Instant::now() < deadline {
        first_status = first.try_wait().unwrap();
        if first_status.is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    assert!(first_status.unwrap().success());
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut second_status = None;
    while second_status.is_none() && Instant::now() < deadline {
        second_status = second.try_wait().unwrap();
        if second_status.is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    assert!(second_status.unwrap().success());
}

#[cfg(target_os = "linux")]
#[test]
fn focused_process_working_directory_is_discovered_from_proc() {
    let (event_tx, _event_rx) = mpsc::sync_channel(128);
    let mut session = PtySession::spawn(
        PaneId(9),
        &CommandSpec::new("/bin/sh", std::iter::empty::<String>()),
        Some(std::path::Path::new("/tmp")),
        PtySize::default(),
        event_tx,
    )
    .unwrap();

    assert_eq!(
        session.current_working_directory().unwrap(),
        std::path::PathBuf::from("/tmp")
    );
    session.write_all(b"exit\n").unwrap();
}
