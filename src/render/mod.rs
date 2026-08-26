//! Markdown renderers shared by the CLI, TUI, and web server.

pub mod html;
pub mod terminal;

use serde::{Deserialize, Serialize};

fn is_mermaid_language(info: &str) -> bool {
    let token = info
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(['{', '}'])
        .trim_start_matches('.')
        .to_ascii_lowercase();
    let language = token.strip_prefix("language-").unwrap_or(&token);
    matches!(language, "mermaid" | "mmd" | "mermaidjs" | "mermaid-js")
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Auto,
    Dark,
    Light,
}

impl std::str::FromStr for Theme {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "dark" => Ok(Self::Dark),
            "light" => Ok(Self::Light),
            other => Err(format!("unknown theme '{other}'; use auto, dark, or light")),
        }
    }
}

impl Theme {
    #[must_use]
    pub fn resolved(self) -> Self {
        match self {
            Self::Auto => {
                let colorfgbg = std::env::var("COLORFGBG").unwrap_or_default();
                let background = colorfgbg
                    .rsplit(';')
                    .next()
                    .and_then(|value| value.parse::<u8>().ok());
                if background.is_some_and(|color| (0..=6).contains(&color) || color == 8) {
                    Self::Dark
                } else if background.is_some() {
                    Self::Light
                } else {
                    Self::Dark
                }
            }
            concrete => concrete,
        }
    }
}
