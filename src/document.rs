use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};

pub const MARKDOWN_EXTENSIONS: &[&str] = &["md", "mdown", "mkdn", "mkd", "markdown"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Document {
    pub absolute_path: PathBuf,
    pub relative_path: PathBuf,
    pub title: String,
    pub modified: SystemTime,
    pub size: u64,
}

impl Document {
    pub fn from_path(root: &Path, path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path)
            .with_context(|| format!("cannot read metadata for {}", path.display()))?;
        let relative_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();
        let contents = fs::read_to_string(path).unwrap_or_default();
        let title = title_from_markdown(&contents).unwrap_or_else(|| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("Untitled")
                .replace(['-', '_'], " ")
        });

        Ok(Self {
            absolute_path: path.to_path_buf(),
            relative_path,
            title,
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            size: metadata.len(),
        })
    }

    pub fn route(&self) -> String {
        self.relative_path
            .components()
            .filter_map(|part| part.as_os_str().to_str())
            .map(percent_encode_segment)
            .collect::<Vec<_>>()
            .join("/")
    }
}

pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            MARKDOWN_EXTENSIONS
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(ext))
        })
}

pub fn strip_frontmatter(input: &str) -> &str {
    let normalized = input.strip_prefix('\u{feff}').unwrap_or(input);
    let Some(first_newline) = normalized.find('\n') else {
        return normalized;
    };
    if normalized[..first_newline].trim_end_matches('\r') != "---" {
        return normalized;
    }

    let mut cursor = first_newline + 1;
    for line in normalized[cursor..].split_inclusive('\n') {
        cursor += line.len();
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return &normalized[cursor..];
        }
    }
    normalized
}

fn title_from_markdown(input: &str) -> Option<String> {
    strip_frontmatter(input).lines().find_map(|line| {
        let trimmed = line.trim();
        let heading = trimmed.strip_prefix('#')?.trim_start_matches('#').trim();
        (!heading.is_empty()).then(|| heading.trim_end_matches('#').trim().to_owned())
    })
}

fn percent_encode_segment(segment: &str) -> String {
    url::form_urlencoded::byte_serialize(segment.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_frontmatter_only_at_the_start() {
        assert_eq!(strip_frontmatter("---\ntitle: X\n---\n# Hello"), "# Hello");
        assert_eq!(
            strip_frontmatter("intro\n---\nvalue\n---"),
            "intro\n---\nvalue\n---"
        );
    }

    #[test]
    fn recognizes_markdown_case_insensitively() {
        assert!(is_markdown(Path::new("README.MD")));
        assert!(is_markdown(Path::new("guide.markdown")));
        assert!(!is_markdown(Path::new("main.rs")));
    }
}
