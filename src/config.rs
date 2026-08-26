use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::render::Theme;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    #[serde(rename = "style")]
    pub theme: Theme,
    pub width: usize,
    pub pager: bool,
    pub tui: bool,
    #[serde(rename = "all")]
    pub include_hidden: bool,
    #[serde(rename = "showLineNumbers", alias = "line_numbers")]
    pub line_numbers: bool,
    #[serde(rename = "preserveNewLines", alias = "preserve_newlines")]
    pub preserve_newlines: bool,
    pub mouse: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: Theme::Auto,
            width: 0,
            pager: false,
            tui: false,
            include_hidden: false,
            line_numbers: false,
            preserve_newlines: false,
            mouse: false,
        }
    }
}

impl Config {
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        let path = explicit.map(Path::to_path_buf).or_else(find_config_file);
        let mut config = if let Some(path) = path {
            let contents = fs::read_to_string(&path)
                .with_context(|| format!("cannot read config {}", path.display()))?;
            serde_yaml::from_str(&contents)
                .with_context(|| format!("cannot parse config {}", path.display()))?
        } else {
            Self::default()
        };
        config.apply_environment()?;
        Ok(config)
    }

    pub fn save(&self, path: &Path, overwrite: bool) -> Result<()> {
        if path.exists() && !overwrite {
            bail!(
                "config already exists at {}; pass --force to replace it",
                path.display()
            );
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        let mut output = String::from(
            "# Glow configuration. CLI flags and GLOW_* variables override these values.\n",
        );
        output.push_str(&serde_yaml::to_string(self).context("cannot serialize config")?);
        fs::write(path, output).with_context(|| format!("cannot write {}", path.display()))?;
        Ok(())
    }

    fn apply_environment(&mut self) -> Result<()> {
        if let Ok(value) = env::var("GLOW_STYLE") {
            self.theme = value.parse().map_err(anyhow::Error::msg)?;
        }
        if let Some(value) = env_value("GLOW_WIDTH") {
            self.width = value.parse().context("GLOW_WIDTH must be a number")?;
        }
        apply_bool_env("GLOW_PAGER", &mut self.pager)?;
        apply_bool_env("GLOW_TUI", &mut self.tui)?;
        apply_bool_env("GLOW_ALL", &mut self.include_hidden)?;
        apply_bool_env("GLOW_SHOWLINENUMBERS", &mut self.line_numbers)?;
        apply_bool_env("GLOW_PRESERVENEWLINES", &mut self.preserve_newlines)?;
        apply_bool_env("GLOW_MOUSE", &mut self.mouse)?;
        Ok(())
    }
}

pub fn default_config_path() -> PathBuf {
    if let Some(directory) = env_value("GLOW_CONFIG_HOME") {
        return PathBuf::from(directory).join("glow.yml");
    }
    if let Some(directory) = env_value("XDG_CONFIG_HOME") {
        return PathBuf::from(directory).join("glow/glow.yml");
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("glow/glow.yml")
}

pub fn find_config_file() -> Option<PathBuf> {
    let preferred = default_config_path();
    if preferred.is_file() {
        return Some(preferred);
    }
    let alternate = preferred.with_extension("yaml");
    alternate.is_file().then_some(alternate)
}

fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn apply_bool_env(name: &str, target: &mut bool) -> Result<()> {
    let Some(value) = env_value(name) else {
        return Ok(());
    };
    *target = match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => bail!("{name} must be true or false"),
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_legacy_camel_case_keys() {
        let config: Config = serde_yaml::from_str(
            "style: light\nwidth: 92\nshowLineNumbers: true\npreserveNewLines: true\n",
        )
        .unwrap();
        assert_eq!(config.theme, Theme::Light);
        assert_eq!(config.width, 92);
        assert!(config.line_numbers);
        assert!(config.preserve_newlines);
    }
}
