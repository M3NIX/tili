use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::tiling::Orientation;

const BUNDLED_SSH_SCRIPT: &str = include_str!("../scripts/ssh-hosts.py");

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub appearance: AppearanceConfig,
    pub launcher: LauncherConfig,
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_dir = config_directory();
        install_bundled_ssh_script(&config_dir)?;
        let path = config_dir.join("config.toml");
        if !path.exists() {
            return Ok(Self::default_for(&config_dir));
        }
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut config: Self = toml::from_str(&contents)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        config.expand_paths(&config_dir);
        config.validate()?;
        Ok(config)
    }

    pub fn default_for(config_dir: &Path) -> Self {
        Self {
            general: GeneralConfig::default(),
            appearance: AppearanceConfig::default(),
            launcher: LauncherConfig::default_for(config_dir),
        }
    }

    fn expand_paths(&mut self, config_dir: &Path) {
        if self.launcher.modes.is_empty() {
            self.launcher.modes = LauncherConfig::default_for(config_dir).modes;
        }
        for mode in &mut self.launcher.modes {
            mode.command = expand_tilde(&mode.command);
        }
    }

    fn validate(&self) -> Result<()> {
        if self.general.resize_step == 0 {
            anyhow::bail!("general.resize_step must be greater than zero");
        }
        validate_color("appearance.focused_border", &self.appearance.focused_border)?;
        validate_color(
            "appearance.unfocused_border",
            &self.appearance.unfocused_border,
        )?;
        if !(20..=100).contains(&self.launcher.width_percent) {
            anyhow::bail!("launcher.width_percent must be between 20 and 100");
        }
        if self.launcher.max_results == 0 || self.launcher.max_results > 100 {
            anyhow::bail!("launcher.max_results must be between 1 and 100");
        }
        if self.launcher.timeout_ms < 100 || self.launcher.timeout_ms > 60_000 {
            anyhow::bail!("launcher.timeout_ms must be between 100 and 60000");
        }
        if self.launcher.modes.is_empty() {
            anyhow::bail!("at least one launcher mode is required");
        }
        for mode in &self.launcher.modes {
            if mode.name.trim().is_empty() || mode.command.as_os_str().is_empty() {
                anyhow::bail!("launcher mode names and commands must not be empty");
            }
        }
        Ok(())
    }
}

fn validate_color(name: &str, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let valid =
        bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit);
    if !valid {
        anyhow::bail!("{name} must use #RRGGBB format");
    }
    Ok(())
}

impl Default for Config {
    fn default() -> Self {
        Self::default_for(&config_directory())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub shell: String,
    pub default_split: SplitConfig,
    pub resize_step: u16,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            default_split: SplitConfig::Horizontal,
            resize_step: 10,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitConfig {
    #[default]
    Horizontal,
    Vertical,
}

impl From<SplitConfig> for Orientation {
    fn from(value: SplitConfig) -> Self {
        match value {
            SplitConfig::Horizontal => Self::Horizontal,
            SplitConfig::Vertical => Self::Vertical,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    pub focused_border: String,
    pub unfocused_border: String,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            focused_border: "#3971ED".to_owned(),
            unfocused_border: "#282a2e".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LauncherConfig {
    pub default_mode: String,
    pub width_percent: u16,
    pub max_results: usize,
    pub timeout_ms: u64,
    pub modes: Vec<LauncherModeConfig>,
}

impl LauncherConfig {
    fn default_for(config_dir: &Path) -> Self {
        Self {
            default_mode: "ssh".to_owned(),
            width_percent: 60,
            max_results: 10,
            timeout_ms: 3_000,
            modes: vec![LauncherModeConfig {
                name: "ssh".to_owned(),
                command: config_dir.join("scripts/ssh-hosts.py"),
            }],
        }
    }
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self::default_for(&config_directory())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct LauncherModeConfig {
    pub name: String,
    pub command: PathBuf,
}

pub fn config_directory() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join("tili");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/tili")
}

fn expand_tilde(path: &Path) -> PathBuf {
    let Some(path_text) = path.to_str() else {
        return path.to_owned();
    };
    if path_text == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_owned());
    }
    if let Some(rest) = path_text.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    path.to_owned()
}

fn install_bundled_ssh_script(config_dir: &Path) -> Result<()> {
    let scripts_dir = config_dir.join("scripts");
    let script_path = scripts_dir.join("ssh-hosts.py");
    if script_path.exists() {
        return Ok(());
    }
    fs::create_dir_all(&scripts_dir)
        .with_context(|| format!("failed to create {}", scripts_dir.display()))?;
    fs::write(&script_path, BUNDLED_SSH_SCRIPT)
        .with_context(|| format!("failed to install {}", script_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to make {} executable", script_path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_theme_colors_and_launcher_limits() {
        let mut config = Config::default_for(Path::new("/tmp/tili-config-test"));
        assert!(config.validate().is_ok());
        config.appearance.focused_border = "blue".to_owned();
        assert!(config.validate().is_err());
        config.appearance.focused_border = "éééx".to_owned();
        assert!(config.validate().is_err());

        config.appearance.focused_border = "#3971ED".to_owned();
        config.launcher.max_results = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn expands_home_relative_paths() {
        let expanded = expand_tilde(Path::new("~/scripts/menu"));
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(expanded, PathBuf::from(home).join("scripts/menu"));
        }
    }

    #[test]
    fn default_modes_preserve_explicit_launcher_settings() {
        let mut config: Config = toml::from_str(
            "[launcher]\nwidth_percent = 75\nmax_results = 4\ntimeout_ms = 500\nmodes = []",
        )
        .unwrap();
        config.expand_paths(Path::new("/tmp/tili-config-test"));
        assert_eq!(config.launcher.width_percent, 75);
        assert_eq!(config.launcher.max_results, 4);
        assert_eq!(config.launcher.timeout_ms, 500);
        assert_eq!(config.launcher.modes.len(), 1);
        assert!(config.validate().is_ok());
    }
}
