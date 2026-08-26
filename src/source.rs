use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::{Client, Url, header};
use serde::Deserialize;

const MAX_REMOTE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct LoadedSource {
    pub content: String,
    pub name: String,
    pub local_path: Option<PathBuf>,
    pub base_url: Option<Url>,
}

pub async fn load(source: &str) -> Result<LoadedSource> {
    if source == "-" {
        let mut content = String::new();
        std::io::stdin()
            .read_to_string(&mut content)
            .context("cannot read stdin")?;
        return Ok(LoadedSource {
            content,
            name: "stdin.md".to_owned(),
            local_path: None,
            base_url: None,
        });
    }

    let path = Path::new(source);
    if path.is_file() {
        let content = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("cannot read {}", path.display()))?;
        return Ok(LoadedSource {
            content,
            name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("document.md")
                .to_owned(),
            local_path: Some(path.canonicalize().unwrap_or_else(|_| path.to_path_buf())),
            base_url: None,
        });
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("glow/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("cannot build HTTP client")?;

    if let Some(repository) = RepositoryShortcut::parse(source)? {
        return repository.fetch(&client).await;
    }

    let url = Url::parse(source).with_context(|| format!("source not found: {source}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("{} is not a supported protocol", url.scheme());
    }
    fetch_url(&client, url).await
}

async fn fetch_url(client: &Client, url: Url) -> Result<LoadedSource> {
    let response = client
        .get(url.clone())
        .send()
        .await
        .with_context(|| format!("cannot fetch {url}"))?
        .error_for_status()
        .with_context(|| format!("server returned an error for {url}"))?;
    let bytes = read_limited_response(response).await?;
    let content = String::from_utf8(bytes).context("remote document is not UTF-8")?;
    let name = url
        .path_segments()
        .and_then(Iterator::last)
        .filter(|name| !name.is_empty())
        .unwrap_or("README.md")
        .to_owned();
    let mut base_url = url.clone();
    if let Ok(mut segments) = base_url.path_segments_mut() {
        segments.pop();
        segments.push("");
    }
    Ok(LoadedSource {
        content,
        name,
        local_path: None,
        base_url: Some(base_url),
    })
}

enum RepositoryShortcut {
    GitHub { owner: String, repo: String },
    GitLab { host: String, project: String },
}

impl RepositoryShortcut {
    fn parse(input: &str) -> Result<Option<Self>> {
        let candidate = if input.starts_with("github.com/")
            || input.starts_with("gitlab.com/")
            || input.starts_with("github://")
            || input.starts_with("gitlab://")
        {
            let normalized = input
                .replacen("github://", "https://github.com/", 1)
                .replacen("gitlab://", "https://gitlab.com/", 1);
            if normalized.starts_with("http") {
                normalized
            } else {
                format!("https://{normalized}")
            }
        } else {
            input.to_owned()
        };
        let Ok(url) = Url::parse(&candidate) else {
            return Ok(None);
        };
        let host = url.host_str().unwrap_or_default();
        let parts = url
            .path_segments()
            .map(|segments| {
                segments
                    .filter(|segment| !segment.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if host.eq_ignore_ascii_case("github.com") && parts.len() == 2 {
            return Ok(Some(Self::GitHub {
                owner: parts[0].to_owned(),
                repo: parts[1].trim_end_matches(".git").to_owned(),
            }));
        }
        if host.contains("gitlab") && parts.len() >= 2 && !url.path().contains("/-/") {
            return Ok(Some(Self::GitLab {
                host: host.to_owned(),
                project: parts.join("/"),
            }));
        }
        Ok(None)
    }

    async fn fetch(self, client: &Client) -> Result<LoadedSource> {
        match self {
            Self::GitHub { owner, repo } => {
                let url = Url::parse(&format!(
                    "https://api.github.com/repos/{owner}/{repo}/readme"
                ))?;
                let response = client
                    .get(url)
                    .header(header::ACCEPT, "application/vnd.github.raw+json")
                    .send()
                    .await
                    .context("cannot query GitHub README API")?
                    .error_for_status()
                    .context("GitHub could not find the repository README")?;
                let bytes = read_limited_response(response).await?;
                Ok(LoadedSource {
                    content: String::from_utf8(bytes).context("GitHub README is not UTF-8")?,
                    name: format!("{owner}/{repo}/README.md"),
                    local_path: None,
                    base_url: Some(Url::parse(&format!(
                        "https://raw.githubusercontent.com/{owner}/{repo}/HEAD/"
                    ))?),
                })
            }
            Self::GitLab { host, project } => {
                #[derive(Deserialize)]
                struct Project {
                    readme_url: Option<String>,
                }
                let encoded: String =
                    url::form_urlencoded::byte_serialize(project.as_bytes()).collect();
                let api = Url::parse(&format!("https://{host}/api/v4/projects/{encoded}"))?;
                let metadata: Project = client
                    .get(api)
                    .send()
                    .await
                    .context("cannot query GitLab project API")?
                    .error_for_status()
                    .context("GitLab could not find the project")?
                    .json()
                    .await
                    .context("cannot parse GitLab response")?;
                let readme = metadata
                    .readme_url
                    .context("GitLab project does not have a README")?
                    .replace("/-/blob/", "/-/raw/");
                fetch_url(client, Url::parse(&readme)?).await
            }
        }
    }
}

async fn read_limited_response(mut response: reqwest::Response) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_REMOTE_BYTES as u64)
    {
        bail!("remote document exceeds the 10 MiB safety limit");
    }
    let mut output = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0)
            .min(MAX_REMOTE_BYTES),
    );
    while let Some(chunk) = response
        .chunk()
        .await
        .context("cannot read HTTP response")?
    {
        if output.len().saturating_add(chunk.len()) > MAX_REMOTE_BYTES {
            bail!("remote document exceeds the 10 MiB safety limit");
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_repository_shortcuts() {
        assert!(matches!(
            RepositoryShortcut::parse("github.com/charmbracelet/glow").unwrap(),
            Some(RepositoryShortcut::GitHub { .. })
        ));
        assert!(matches!(
            RepositoryShortcut::parse("gitlab://group/project").unwrap(),
            Some(RepositoryShortcut::GitLab { .. })
        ));
        assert!(
            RepositoryShortcut::parse("https://example.com/readme.md")
                .unwrap()
                .is_none()
        );
    }
}
