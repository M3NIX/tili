use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

use ssh2_config::{ParseRule, SshConfig};

use crate::launcher::{LauncherAction, LauncherEntry};

pub fn discover(config: &Path) -> Result<Vec<LauncherEntry>, String> {
    let file = match File::open(config) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read SSH configuration: {error}")),
    };
    let home = config
        .parent()
        .and_then(Path::parent)
        .unwrap_or(Path::new("."));
    let config = SshConfig::default()
        .home_dir(home)
        .parse(
            &mut BufReader::new(file),
            ParseRule::ALLOW_UNKNOWN_FIELDS | ParseRule::ALLOW_UNSUPPORTED_FIELDS,
        )
        .map_err(|error| format!("cannot parse SSH configuration: {error}"))?;
    let mut aliases = BTreeMap::new();
    for host in config.get_hosts() {
        for clause in &host.pattern {
            if !clause.negated && is_concrete(&clause.pattern) {
                aliases
                    .entry(clause.pattern.to_lowercase())
                    .or_insert_with(|| clause.pattern.clone());
            }
        }
    }
    if aliases.len() > 10_000 {
        return Err("too many SSH host aliases".to_owned());
    }
    Ok(aliases
        .into_values()
        .map(|alias| {
            let hostname = config
                .intersecting_hosts(&alias)
                .filter(|host| {
                    !host
                        .params
                        .ignored_fields
                        .keys()
                        .any(|key| key.eq_ignore_ascii_case("match"))
                })
                .find_map(|host| host.params.host_name.as_deref());
            LauncherEntry {
                label: hostname
                    .map_or_else(|| alias.clone(), |hostname| format!("{alias} ({hostname})")),
                id: alias,
                description: None,
            }
        })
        .collect())
}

pub fn select(alias: &str) -> Result<LauncherAction, String> {
    if !is_concrete(alias) {
        return Err("invalid SSH host alias".to_owned());
    }
    Ok(LauncherAction::OpenPane {
        command: "ssh".to_owned(),
        args: vec![alias.to_owned()],
    })
}

pub fn run(arguments: &[String], output: &mut impl Write) -> anyhow::Result<()> {
    match arguments {
        [operation] if operation == "list" => {
            let home =
                std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
            for entry in
                discover(&PathBuf::from(home).join(".ssh/config")).map_err(anyhow::Error::msg)?
            {
                serde_json::to_writer(&mut *output, &entry)?;
                writeln!(output)?;
            }
        }
        [operation, alias] if operation == "select" => {
            let action = select(alias).map_err(anyhow::Error::msg)?;
            serde_json::to_writer(&mut *output, &action)?;
            writeln!(output)?;
        }
        _ => anyhow::bail!("usage: tili --ssh-picker list | select <alias>"),
    }
    Ok(())
}

fn is_concrete(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 4096
        && !alias.starts_with(['!', '-'])
        && !alias.contains(['*', '?', '['])
        && !alias.chars().any(char::is_control)
}
