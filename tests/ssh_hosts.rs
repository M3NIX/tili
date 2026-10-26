use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[test]
fn discovers_includes_and_filters_non_concrete_hosts() {
    let home = temporary_home("ssh-discovery");
    let ssh = home.join(".ssh");
    fs::create_dir_all(ssh.join("conf.d")).unwrap();
    fs::write(
        ssh.join("config"),
        r#"
# ignored comment
Host production staging *.internal !blocked
Include "conf.d/*.conf"
include conf.d/loop.conf
"#,
    )
    .unwrap();
    fs::write(
        ssh.join("conf.d/hosts.conf"),
        "HOST Development production\n",
    )
    .unwrap();
    fs::write(
        ssh.join("conf.d/loop.conf"),
        "Include ../config\nHost backup? literal-host\n",
    )
    .unwrap();

    let aliases = list_aliases(&home);
    assert_eq!(
        aliases,
        ["Development", "literal-host", "production", "staging"]
    );
    let _ = fs::remove_dir_all(home);
}

#[test]
fn missing_ssh_configuration_produces_an_empty_list() {
    let home = temporary_home("ssh-missing");
    assert!(list_aliases(&home).is_empty());
    let _ = fs::remove_dir_all(home);
}

#[test]
fn native_picker_protocol_runs_without_python_or_external_commands() {
    let home = temporary_home("native-ssh-picker");
    fs::create_dir_all(home.join(".ssh")).unwrap();
    fs::write(
        home.join(".ssh/config"),
        "Host test1\n HostName test1.localhost.myydomain\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tili"))
        .args(["--ssh-picker", "list"])
        .env("HOME", &home)
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let entries = tili::launcher::parse_entries(&output.stdout).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].label, "test1 (test1.localhost.myydomain)");
    let output = Command::new(env!("CARGO_BIN_EXE_tili"))
        .args(["--ssh-picker", "select", &entries[0].id])
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        tili::launcher::parse_action(&output.stdout).unwrap(),
        tili::ssh_hosts::select("test1").unwrap()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_tili"))
        .args(["--ssh-picker", "select", "-oProxyCommand=bad"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn recursive_includes_fail_without_recursing_forever() {
    let home = temporary_home("ssh-loop");
    fs::create_dir_all(home.join(".ssh")).unwrap();
    fs::write(home.join(".ssh/config"), "Include loop.conf\n").unwrap();
    fs::write(home.join(".ssh/loop.conf"), "Include config\n").unwrap();
    let error = tili::ssh_hosts::discover(&home.join(".ssh/config")).unwrap_err();
    assert!(error.contains("cycle"));
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn parser_preserves_quotes_and_never_executes_match_commands() {
    let home = temporary_home("ssh-quotes");
    fs::create_dir_all(home.join(".ssh")).unwrap();
    fs::write(
        home.join(".ssh/config"),
        format!(
            r#"
Host "quoted-host"
 HostName "host#name.example"
Host conditional
 Match exec "touch {}/must-not-exist"
 HostName ignored.example
"#,
            home.display()
        ),
    )
    .unwrap();
    let entries = list_entries(&home);
    assert_eq!(entries[0]["label"], "conditional");
    assert_eq!(entries[1]["label"], "quoted-host (host#name.example)");
    assert!(!home.join("must-not-exist").exists());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn labels_show_hostnames_without_changing_alias_ids() {
    let home = temporary_home("ssh-labels");
    let ssh = home.join(".ssh");
    fs::create_dir_all(&ssh).unwrap();
    fs::write(
        ssh.join("config"),
        r#"
Host test1 test2
    HostName "test1.localhost.myydomain" # shared destination
Host test1
    HostName ignored.example
Host plain
Host included
    Include hostname.conf
Host repeated
    Include hostname.conf
Host guarded
Match exec "true"
    HostName conditional.example
Host *.internal !excluded
    HostName wildcard.example
HOST mixed
    hOsTnAmE = mixed.example
Host compact
    HostName=compact.example
Host spaced
    HostName= spaced.example
"#,
    )
    .unwrap();
    fs::write(ssh.join("hostname.conf"), "HostName included.example\n").unwrap();
    let entries = list_entries(&home);
    let labels: Vec<_> = entries
        .iter()
        .map(|entry| entry["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        [
            "compact (compact.example)",
            "guarded",
            "included (included.example)",
            "mixed (mixed.example)",
            "plain",
            "repeated (included.example)",
            "spaced (spaced.example)",
            "test1 (test1.localhost.myydomain)",
            "test2 (test1.localhost.myydomain)",
        ]
    );
    assert_eq!(entries[7]["id"], "test1");
    assert!(
        entries
            .iter()
            .all(|entry| entry.get("description").is_none())
    );
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn select_returns_an_ssh_open_pane_action() {
    assert_eq!(
        tili::ssh_hosts::select("production").unwrap(),
        tili::launcher::LauncherAction::OpenPane {
            command: "ssh".to_owned(),
            args: vec!["production".to_owned()],
        }
    );
    for alias in ["", "-oProxyCommand=bad", "!excluded", "*", "host\nname"] {
        assert!(tili::ssh_hosts::select(alias).is_err());
    }
}

fn list_aliases(home: &Path) -> Vec<String> {
    list_entries(home)
        .into_iter()
        .map(|entry| entry["id"].as_str().unwrap().to_owned())
        .collect()
}

fn list_entries(home: &Path) -> Vec<Value> {
    tili::ssh_hosts::discover(&home.join(".ssh/config"))
        .unwrap()
        .into_iter()
        .map(|entry| serde_json::json!({"id": entry.id, "label": entry.label}))
        .collect()
}

fn temporary_home(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("tili-{label}-{}-{nonce}", std::process::id()));
    fs::create_dir(&path).unwrap();
    path
}
