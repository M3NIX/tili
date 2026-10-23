use std::fs;
use std::path::PathBuf;
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
    let output = Command::new("python3")
        .arg(script_path())
        .args(["select", "production"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let action: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(action["action"], "open_pane");
    assert_eq!(action["command"], "ssh");
    assert_eq!(action["args"][0], "production");
}

fn list_aliases(home: &PathBuf) -> Vec<String> {
    list_entries(home)
        .into_iter()
        .map(|entry| entry["id"].as_str().unwrap().to_owned())
        .collect()
}

fn list_entries(home: &PathBuf) -> Vec<Value> {
    let output = Command::new("python3")
        .arg(script_path())
        .arg("list")
        .env("HOME", home)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/ssh-hosts.py")
}

fn temporary_home(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("tili-{label}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}
