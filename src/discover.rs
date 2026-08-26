use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;

use crate::document::{Document, is_markdown};

#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub include_hidden: bool,
    pub respect_gitignore: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            include_hidden: false,
            respect_gitignore: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DocumentIndex {
    pub root: PathBuf,
    pub documents: Vec<Document>,
    pub folders: Vec<PathBuf>,
}

impl DocumentIndex {
    pub fn scan(root: impl AsRef<Path>, options: &ScanOptions) -> Result<Self> {
        let requested = root.as_ref();
        if !requested.exists() {
            bail!("folder does not exist: {}", requested.display());
        }
        if !requested.is_dir() {
            bail!("not a folder: {}", requested.display());
        }
        let root = requested
            .canonicalize()
            .with_context(|| format!("cannot resolve {}", requested.display()))?;

        let mut builder = WalkBuilder::new(&root);
        builder
            .hidden(!options.include_hidden)
            .follow_links(false)
            .git_ignore(options.respect_gitignore)
            .git_global(options.respect_gitignore)
            .git_exclude(options.respect_gitignore)
            .require_git(false)
            .parents(options.respect_gitignore);

        let mut documents = Vec::new();
        let mut folders = BTreeSet::new();
        folders.insert(PathBuf::new());

        for entry in builder.build().flatten() {
            let Some(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_file() || !is_markdown(entry.path()) {
                continue;
            }
            let document = match Document::from_path(&root, entry.path()) {
                Ok(document) => document,
                Err(_) => continue,
            };
            if let Some(parent) = document.relative_path.parent() {
                let mut cursor = PathBuf::new();
                for component in parent.components() {
                    cursor.push(component);
                    folders.insert(cursor.clone());
                }
            }
            documents.push(document);
        }

        documents.sort_by_cached_key(|doc| doc.relative_path.to_string_lossy().to_lowercase());

        Ok(Self {
            root,
            documents,
            folders: folders.into_iter().collect(),
        })
    }

    pub fn preferred_document(&self) -> Option<&Document> {
        const README_NAMES: &[&str] = &["readme.md", "readme.markdown", "readme.mdown"];
        self.documents
            .iter()
            .find(|doc| {
                doc.relative_path
                    .parent()
                    .is_some_and(|parent| parent.as_os_str().is_empty())
                    && doc
                        .relative_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            README_NAMES
                                .iter()
                                .any(|item| name.eq_ignore_ascii_case(item))
                        })
            })
            .or_else(|| self.documents.first())
    }

    pub fn find_route(&self, route: &str) -> Option<&Document> {
        let decoded = percent_encoding::percent_decode_str(route)
            .decode_utf8_lossy()
            .replace('\\', "/");
        self.documents.iter().find(|doc| {
            doc.route() == route
                || doc.relative_path.to_string_lossy().replace('\\', "/") == decoded
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn recursively_discovers_markdown_and_folders() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("guide/deep")).unwrap();
        fs::write(temp.path().join("README.md"), "# Home").unwrap();
        fs::write(temp.path().join("guide/deep/page.MD"), "# Page").unwrap();
        fs::write(temp.path().join("guide/code.rs"), "fn main() {}").unwrap();

        let index = DocumentIndex::scan(temp.path(), &ScanOptions::default()).unwrap();
        assert_eq!(index.documents.len(), 2);
        assert!(index.folders.contains(&PathBuf::from("guide/deep")));
        assert_eq!(index.preferred_document().unwrap().title, "Home");
    }

    #[test]
    fn respects_gitignore_by_default() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join(".gitignore"), "ignored/\n").unwrap();
        fs::create_dir(temp.path().join("ignored")).unwrap();
        fs::write(temp.path().join("ignored/secret.md"), "# Secret").unwrap();
        fs::write(temp.path().join("visible.md"), "# Visible").unwrap();

        let index = DocumentIndex::scan(temp.path(), &ScanOptions::default()).unwrap();
        assert_eq!(index.documents.len(), 1);
        assert_eq!(index.documents[0].title, "Visible");
    }
}
