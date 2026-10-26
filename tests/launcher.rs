use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tili::config::{LauncherConfig, LauncherModeConfig};
use tili::launcher::{
    Launcher, LauncherAction, LauncherEntry, LauncherEvent, execute_async, parse_action,
    parse_entries,
};
use tili::pty::CommandSpec;

const TEST_TIMEOUT: Duration = Duration::from_secs(15);

fn config(modes: Vec<(&str, PathBuf)>) -> LauncherConfig {
    LauncherConfig {
        default_mode: modes[0].0.to_owned(),
        width_percent: 60,
        max_results: 10,
        timeout_ms: 10_000,
        modes: modes
            .into_iter()
            .map(|(name, command)| LauncherModeConfig {
                name: name.to_owned(),
                command: Some(command),
                builtin: None,
            })
            .collect(),
    }
}

fn entry(id: &str, label: &str) -> LauncherEntry {
    LauncherEntry {
        id: id.to_owned(),
        label: label.to_owned(),
        description: None,
    }
}

fn write_executable_script(path: &std::path::Path, contents: &str) {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, contents).unwrap();
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(temporary, path).unwrap();
}

#[test]
fn script_descendants_cannot_bypass_timeout_or_cancellation() {
    for (cancel, keep_parent) in [(false, false), (true, false), (false, true), (true, true)] {
        let directory = temporary_directory("launcher-descendant");
        let script = directory.join("menu.py");
        let pid_file = directory.join("pid");
        write_executable_script(
            &script,
            &format!(
                r#"#!/usr/bin/env python3
import os, pathlib, subprocess, time
child = subprocess.Popen(["sleep", "30"])
pid_file = pathlib.Path({:?})
temporary = pid_file.with_suffix('.tmp')
temporary.write_text(str(child.pid))
temporary.replace(pid_file)
if {}: time.sleep(30)
os._exit(0)
"#,
                pid_file,
                if keep_parent { "True" } else { "False" }
            ),
        );
        let mut configuration = config(vec![("test", script)]);
        configuration.timeout_ms = if cancel { 10_000 } else { 3_000 };
        let (events, receiver) = mpsc::sync_channel(128);
        let mut launcher = Launcher::new(&configuration);
        launcher.open(&events);
        let deadline = std::time::Instant::now() + TEST_TIMEOUT;
        let pid = loop {
            if let Some(pid) = fs::read_to_string(&pid_file)
                .ok()
                .and_then(|pid| pid.parse::<u32>().ok())
            {
                break pid;
            }
            if let Ok(event) = receiver.try_recv() {
                panic!("request completed before publishing its child PID: {event:?}");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "script did not publish its child PID"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        if cancel {
            launcher.close();
        }
        let event = receiver.recv_timeout(TEST_TIMEOUT).unwrap();
        match event {
            LauncherEvent::Listed { result, .. } => {
                assert!(result.unwrap_err().contains(if cancel {
                    "cancelled"
                } else {
                    "timed out"
                }))
            }
            event => panic!("unexpected event: {event:?}"),
        }
        let deadline = std::time::Instant::now() + TEST_TIMEOUT;
        loop {
            let status = fs::read_to_string(format!("/proc/{pid}/status"));
            match status {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => panic!("could not inspect descendant {pid}: {error}"),
                Ok(status)
                    if status
                        .lines()
                        .any(|line| line.starts_with("State:") && line.contains('Z')) =>
                {
                    break;
                }
                Ok(_) => {}
            }
            assert!(
                std::time::Instant::now() < deadline,
                "script descendant still running"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn parses_json_lines_and_rejects_duplicate_ids() {
    let output = br#"{"id":"prod","label":"Production","description":"SSH host"}
{"id":"stage","label":"Staging"}
"#;
    let entries = parse_entries(output).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].id, "prod");

    let duplicate = br#"{"id":"prod","label":"One"}
{"id":"prod","label":"Two"}
"#;
    assert!(parse_entries(duplicate).is_err());
}

#[test]
fn script_output_limits_and_failure_status_are_reported() {
    for (body, expected) in [
        (
            "import sys; sys.stdout.write('x' * 2097152)",
            "output limit",
        ),
        (
            "import sys; sys.stderr.write('x' * 2097152)",
            "output limit",
        ),
        (
            "import sys; sys.stderr.write('example failure'); sys.exit(7)",
            "example failure",
        ),
    ] {
        let directory = temporary_directory("launcher-errors");
        let script = directory.join("menu.py");
        write_executable_script(&script, &format!("#!/usr/bin/env python3\n{body}\n"));
        let mut configuration = config(vec![("test", script)]);
        configuration.timeout_ms = 5000;
        let mut launcher = Launcher::new(&configuration);
        let (events, receiver) = mpsc::sync_channel(128);
        launcher.open(&events);
        match receiver.recv_timeout(TEST_TIMEOUT).unwrap() {
            LauncherEvent::Listed { result, .. } => assert!(result.unwrap_err().contains(expected)),
            event => panic!("unexpected event: {event:?}"),
        }
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn validates_script_actions() {
    assert_eq!(
        parse_action(br#"{"action":"open_pane","command":"ssh","args":["prod"]}"#).unwrap(),
        LauncherAction::OpenPane {
            command: "ssh".to_owned(),
            args: vec!["prod".to_owned()],
        }
    );
    assert!(parse_action(br#"{"action":"unknown"}"#).is_err());
    assert!(parse_action(br#"{"action":"execute","command":""}"#).is_err());
}

#[test]
fn fuzzy_filtering_and_mode_switching_work() {
    let configuration = config(vec![
        ("ssh", PathBuf::from("/bin/true")),
        ("docker", PathBuf::from("/bin/true")),
    ]);
    let (events, receiver) = mpsc::sync_channel(128);
    let mut launcher = Launcher::new(&configuration);
    launcher.open(&events);
    let request_id = match receiver.recv_timeout(TEST_TIMEOUT).unwrap() {
        LauncherEvent::Listed { request_id, .. } => request_id,
        event => panic!("unexpected launcher event: {event:?}"),
    };
    launcher.handle_event(LauncherEvent::Listed {
        request_id,
        result: Ok(vec![
            entry("production", "production"),
            entry("staging", "staging"),
            entry("backup", "production-backup"),
        ]),
    });
    for character in "prod".chars() {
        launcher.push_query(character);
    }
    let labels: Vec<_> = launcher
        .visible_entries()
        .into_iter()
        .map(|(_, item)| item.label.as_str())
        .collect();
    assert_eq!(labels, ["production", "production-backup"]);

    launcher.next_mode(&events);
    assert_eq!(launcher.mode_name(), "docker");
    launcher.next_mode(&events);
    assert_eq!(launcher.mode_name(), "ssh");
    launcher.previous_mode(&events);
    assert_eq!(launcher.mode_name(), "docker");
    launcher.close();
    // Wait for all three superseded requests to finish process-group cleanup.
    for _ in 0..3 {
        let event = receiver.recv_timeout(TEST_TIMEOUT).unwrap();
        assert!(launcher.handle_event(event).is_none());
    }
}

#[test]
fn fuzzy_search_ignores_query_case_and_matches_hostname_labels() {
    let configuration = config(vec![("ssh", PathBuf::from("/bin/true"))]);
    for query in ["TST1", "tSt1", "LCLMY", "lclmy"] {
        let (events, receiver) = mpsc::sync_channel(128);
        let mut launcher = Launcher::new(&configuration);
        launcher.open(&events);
        let LauncherEvent::Listed { request_id, .. } = receiver.recv_timeout(TEST_TIMEOUT).unwrap()
        else {
            panic!("expected list result");
        };
        launcher.handle_event(LauncherEvent::Listed {
            request_id,
            result: Ok(vec![
                entry("test1", "test1 (test1.localhost.myydomain)"),
                entry("other", "Other (other.example)"),
            ]),
        });
        for character in query.chars() {
            launcher.push_query(character);
        }
        let visible = launcher.visible_entries();
        assert_eq!(visible.len(), 1, "query: {query}");
        assert_eq!(visible[0].1.id, "test1");
        let positions = launcher.match_indices(visible[0].1);
        let matched: String = positions
            .iter()
            .map(|index| visible[0].1.label.chars().nth(*index as usize).unwrap())
            .collect();
        assert_eq!(matched.to_lowercase(), query.to_lowercase());
        for _ in query.chars() {
            launcher.pop_query();
        }
        for (_, entry) in launcher.visible_entries() {
            assert!(launcher.match_indices(entry).is_empty());
        }
    }
}

#[test]
fn executes_list_and_select_protocol() {
    let directory = temporary_directory("launcher-protocol");
    let script = directory.join("menu.py");
    write_executable_script(
        &script,
        r#"#!/usr/bin/env python3
import json, sys
if sys.argv[1:] == ["list"]:
    print(json.dumps({"id": "one", "label": "One"}))
elif sys.argv[1:] == ["select", "one"]:
    print(json.dumps({"action": "close"}))
else:
    raise SystemExit(2)
"#,
    );
    let configuration = config(vec![("test", script)]);
    let (events, receiver) = mpsc::sync_channel(128);
    let mut launcher = Launcher::new(&configuration);
    launcher.open(&events);
    let listed = receiver.recv_timeout(TEST_TIMEOUT).unwrap();
    assert!(
        matches!(&listed, LauncherEvent::Listed { result: Ok(_), .. }),
        "list request failed: {listed:?}"
    );
    assert!(launcher.handle_event(listed).is_none());
    assert_eq!(launcher.visible_entries().len(), 1);

    launcher.select(&events);
    let selected = receiver.recv_timeout(TEST_TIMEOUT).unwrap();
    assert!(
        matches!(&selected, LauncherEvent::Selected { result: Ok(_), .. }),
        "select request failed: {selected:?}"
    );
    assert_eq!(launcher.handle_event(selected), Some(LauncherAction::Close));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn navigation_keeps_selection_visible_in_small_viewports() {
    let configuration = config(vec![("test", PathBuf::from("/bin/true"))]);
    let (events, receiver) = mpsc::sync_channel(128);
    let mut launcher = Launcher::new(&configuration);
    launcher.open(&events);
    let request_id = match receiver.recv_timeout(TEST_TIMEOUT).unwrap() {
        LauncherEvent::Listed { request_id, .. } => request_id,
        event => panic!("unexpected launcher event: {event:?}"),
    };
    launcher.handle_event(LauncherEvent::Listed {
        request_id,
        result: Ok((0..15)
            .map(|index| entry(&format!("{index:02}"), &format!("{index:02}")))
            .collect()),
    });
    for index in 0..30 {
        for limit in [1, 3, 10] {
            let visible = launcher.visible_entries_with_limit(limit);
            assert!(visible.len() <= limit);
            let selected: Vec<_> = visible.iter().filter(|(selected, _)| *selected).collect();
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].1.id, format!("{:02}", index % 15));
        }
        launcher.select_next();
    }
    launcher.select_previous();
    assert_eq!(launcher.visible_entries_with_limit(1)[0].1.id, "14");
    assert!(launcher.visible_entries_with_limit(0).is_empty());
}

#[test]
fn closing_launcher_discards_a_completed_selection() {
    let configuration = config(vec![("test", PathBuf::from("/bin/true"))]);
    let (events, receiver) = mpsc::sync_channel(128);
    let mut launcher = Launcher::new(&configuration);
    launcher.open(&events);
    let request_id = match receiver.recv_timeout(TEST_TIMEOUT).unwrap() {
        LauncherEvent::Listed { request_id, .. } => request_id,
        event => panic!("unexpected launcher event: {event:?}"),
    };
    launcher.handle_event(LauncherEvent::Listed {
        request_id,
        result: Ok(vec![entry("one", "One")]),
    });
    launcher.select(&events);
    let request_id = match receiver.recv_timeout(TEST_TIMEOUT).unwrap() {
        LauncherEvent::Selected { request_id, .. } => request_id,
        event => panic!("unexpected launcher event: {event:?}"),
    };
    launcher.close();
    assert_eq!(
        launcher.handle_event(LauncherEvent::Selected {
            request_id,
            result: Ok(LauncherAction::OpenPane {
                command: "ssh".to_owned(),
                args: vec!["example".to_owned()]
            }),
        }),
        None
    );
}

#[cfg(target_os = "linux")]
#[test]
fn background_commands_cannot_access_terminal_streams_and_report_failure() {
    let (events, receiver) = mpsc::sync_channel(128);
    execute_async(
        CommandSpec::new(
            "python3",
            [
                "-c",
                "import os; assert all(os.path.realpath('/proc/self/fd/' + str(fd)) == '/dev/null' for fd in range(3))",
            ],
        ),
        events.clone(),
    );
    assert!(matches!(
        receiver.recv_timeout(TEST_TIMEOUT).unwrap(),
        LauncherEvent::Executed(Ok(()))
    ));
    execute_async(
        CommandSpec::new("/bin/false", std::iter::empty::<String>()),
        events,
    );
    assert!(matches!(
        receiver.recv_timeout(TEST_TIMEOUT).unwrap(),
        LauncherEvent::Executed(Err(_))
    ));
}

fn temporary_directory(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("tili-{label}-{}-{nonce}", std::process::id()));
    fs::create_dir(&path).unwrap();
    path
}
