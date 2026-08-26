//! Cloudflare Quick Tunnel lifecycle management.
//!
//! The tunnel is deliberately started as a child process rather than through a
//! shell. Besides avoiding shell interpolation, this makes the exact command
//! line and the process lifetime easy to audit.

use std::{
    ffi::{OsStr, OsString},
    io,
    process::ExitStatus,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
    sync::mpsc,
    task::JoinHandle,
    time::{Instant, timeout_at},
};
use url::Url;

const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_DIAGNOSTIC_LINES: usize = 12;

/// Options used while starting a Cloudflare Quick Tunnel.
#[derive(Clone, Debug)]
pub struct TunnelOptions {
    /// Path or executable name for `cloudflared`.
    pub executable: OsString,
    /// Maximum amount of time to wait for the public URL.
    pub startup_timeout: Duration,
}

impl Default for TunnelOptions {
    fn default() -> Self {
        Self {
            executable: OsString::from("cloudflared"),
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
        }
    }
}

/// A running Cloudflare Quick Tunnel.
///
/// Dropping this value requests termination of `cloudflared`. Call
/// [`QuickTunnel::shutdown`] when it is important to wait for the process to
/// exit before continuing.
pub struct QuickTunnel {
    public_url: String,
    child: Child,
    output_tasks: Vec<JoinHandle<()>>,
}

impl QuickTunnel {
    /// Start a tunnel that forwards to `http://127.0.0.1:<port>`.
    pub async fn start(port: u16) -> Result<Self> {
        Self::start_with_options(port, TunnelOptions::default()).await
    }

    /// Start a tunnel with custom startup options.
    pub async fn start_with_options(port: u16, options: TunnelOptions) -> Result<Self> {
        if port == 0 {
            bail!("cannot start a tunnel for port 0; bind the local server first");
        }

        let local_url = format!("http://127.0.0.1:{port}");
        let mut command = Command::new(&options.executable);
        command
            .args([
                OsStr::new("tunnel"),
                OsStr::new("--no-autoupdate"),
                OsStr::new("--url"),
                OsStr::new(&local_url),
            ])
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|error| spawn_error(&options.executable, &local_url, &error))?;

        let stdout = child
            .stdout
            .take()
            .context("cloudflared started without a readable stdout pipe")?;
        let stderr = child
            .stderr
            .take()
            .context("cloudflared started without a readable stderr pipe")?;

        let (line_tx, mut line_rx) = mpsc::channel(64);
        let mut output_tasks = vec![
            spawn_output_reader(stdout, line_tx.clone()),
            spawn_output_reader(stderr, line_tx),
        ];
        let deadline = Instant::now() + options.startup_timeout;
        let mut diagnostics = Vec::new();

        let public_url = loop {
            tokio::select! {
                status = child.wait() => {
                    let status = status.context("failed while waiting for cloudflared")?;
                    for task in &mut output_tasks {
                        let _ = task.await;
                    }
                    while let Ok(line) = line_rx.try_recv() {
                        remember_diagnostic(&mut diagnostics, line);
                    }
                    abort_tasks(&output_tasks);
                    bail!(startup_exit_message(status, &diagnostics));
                }
                maybe_line = timeout_at(deadline, line_rx.recv()) => {
                    match maybe_line {
                        Err(_) => {
                            abort_tasks(&output_tasks);
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                            let detail = diagnostic_suffix(&diagnostics);
                            bail!(
                                "cloudflared did not publish a trycloudflare.com URL within {} seconds{detail}",
                                options.startup_timeout.as_secs_f32()
                            );
                        }
                        Ok(None) => {
                            // Both pipes closed. The next wait normally resolves immediately,
                            // but handling it here also gives a useful error for unusual builds.
                            let status = child
                                .wait()
                                .await
                                .context("cloudflared closed its output and could not be reaped")?;
                            abort_tasks(&output_tasks);
                            bail!(startup_exit_message(status, &diagnostics));
                        }
                        Ok(Some(line)) => {
                            if let Some(url) = extract_trycloudflare_url(&line) {
                                break url;
                            }
                            remember_diagnostic(&mut diagnostics, line);
                        }
                    }
                }
            }
        };

        Ok(Self {
            public_url,
            child,
            output_tasks,
        })
    }

    /// The validated HTTPS URL allocated by Cloudflare.
    #[must_use]
    pub fn public_url(&self) -> &str {
        &self.public_url
    }

    /// Wait for an unexpected tunnel exit.
    pub async fn wait(&mut self) -> Result<ExitStatus> {
        self.child
            .wait()
            .await
            .context("failed while waiting for cloudflared")
    }

    /// Stop cloudflared and wait until it has exited.
    pub async fn shutdown(&mut self) -> Result<()> {
        abort_tasks(&self.output_tasks);
        match self
            .child
            .try_wait()
            .context("failed to inspect cloudflared")?
        {
            Some(_) => Ok(()),
            None => {
                self.child
                    .kill()
                    .await
                    .context("failed to stop cloudflared")?;
                self.child
                    .wait()
                    .await
                    .context("failed to reap cloudflared")?;
                Ok(())
            }
        }
    }
}

impl Drop for QuickTunnel {
    fn drop(&mut self) {
        abort_tasks(&self.output_tasks);
        // `kill_on_drop(true)` is the backstop. `start_kill` makes termination
        // immediate even on Tokio versions whose child-drop handling is deferred.
        let _ = self.child.start_kill();
    }
}

fn spawn_error(executable: &OsStr, local_url: &str, error: &io::Error) -> anyhow::Error {
    if error.kind() == io::ErrorKind::NotFound {
        anyhow!(
            "cloudflared was not found (tried {:?}); install it and ensure it is on PATH, then retry sharing {local_url}",
            executable
        )
    } else {
        anyhow!("failed to start cloudflared ({executable:?}) for {local_url}: {error}")
    }
}

fn spawn_output_reader<R>(reader: R, sender: mpsc::Sender<String>) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // Never let noisy logging block cloudflared. During startup the
            // consumer is fast; after startup a full queue can safely drop logs.
            let _ = sender.try_send(line);
        }
    })
}

fn abort_tasks(tasks: &[JoinHandle<()>]) {
    for task in tasks {
        task.abort();
    }
}

fn remember_diagnostic(lines: &mut Vec<String>, line: String) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    if lines.len() == MAX_DIAGNOSTIC_LINES {
        lines.remove(0);
    }
    lines.push(trimmed.chars().take(240).collect());
}

fn diagnostic_suffix(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        format!("; recent output: {}", lines.join(" | "))
    }
}

fn startup_exit_message(status: ExitStatus, diagnostics: &[String]) -> String {
    let detail = diagnostic_suffix(diagnostics);
    format!("cloudflared exited before publishing a public URL ({status}){detail}")
}

/// Extract and validate the Cloudflare Quick Tunnel URL from one log line.
///
/// Only a bare `https://<single-dns-label>.trycloudflare.com` origin is
/// accepted. User info, ports, paths, query strings, fragments, nested hosts,
/// underscores, and lookalike suffixes are rejected.
#[must_use]
pub fn extract_trycloudflare_url(line: &str) -> Option<String> {
    line.split_ascii_whitespace().find_map(|token| {
        let candidate = token.trim_matches(|character: char| {
            matches!(
                character,
                '|' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';' | '\'' | '"'
            )
        });
        validate_trycloudflare_url(candidate)
    })
}

fn validate_trycloudflare_url(candidate: &str) -> Option<String> {
    let parsed = Url::parse(candidate).ok()?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return None;
    }

    let host = parsed.host_str()?;
    let label = host.strip_suffix(".trycloudflare.com")?;
    if label.is_empty()
        || label.contains('.')
        || label.len() > 63
        || label.starts_with('-')
        || label.ends_with('-')
        || !label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return None;
    }

    let origin = format!("https://{label}.trycloudflare.com");
    // `url::Url` intentionally normalizes an explicit `:443` away. Comparing
    // the source token with the normalized origin keeps the accepted grammar
    // narrow and makes explicit ports (including the default one) invalid.
    if candidate != origin && candidate != format!("{origin}/") {
        return None;
    }
    Some(origin)
}

#[cfg(test)]
mod tests {
    use super::extract_trycloudflare_url;

    #[test]
    fn extracts_a_quick_tunnel_origin_from_decorated_output() {
        let line = "INF | https://daring-labyrinth-example.trycloudflare.com |";
        assert_eq!(
            extract_trycloudflare_url(line).as_deref(),
            Some("https://daring-labyrinth-example.trycloudflare.com")
        );
    }

    #[test]
    fn rejects_lookalikes_and_urls_with_extra_authority_or_location_data() {
        for line in [
            "https://demo.trycloudflare.com.evil.test",
            "https://trycloudflare.com",
            "https://nested.demo.trycloudflare.com",
            "https://user@demo.trycloudflare.com",
            "https://demo.trycloudflare.com:443",
            "https://demo.trycloudflare.com/admin",
            "https://demo.trycloudflare.com?next=evil",
            "http://demo.trycloudflare.com",
            "https://-demo.trycloudflare.com",
            "https://demo_.trycloudflare.com",
        ] {
            assert_eq!(extract_trycloudflare_url(line), None, "accepted {line}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn starts_with_the_exact_quick_tunnel_arguments() {
        use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

        use tempfile::tempdir;

        use super::{QuickTunnel, TunnelOptions};

        let directory = tempdir().expect("tempdir");
        let executable = directory.path().join("fake-cloudflared");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$0.args\"\nprintf '%s\\n' 'https://exact-command.trycloudflare.com' >&2\nexec tail -f /dev/null\n",
        )
        .expect("write fake cloudflared");
        let mut permissions = fs::metadata(&executable)
            .expect("script metadata")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).expect("make script executable");

        let mut tunnel = QuickTunnel::start_with_options(
            43123,
            TunnelOptions {
                executable: executable.clone().into_os_string(),
                startup_timeout: Duration::from_secs(3),
            },
        )
        .await
        .expect("start fake tunnel");
        assert_eq!(
            tunnel.public_url(),
            "https://exact-command.trycloudflare.com"
        );
        assert_eq!(
            fs::read_to_string(format!("{}.args", executable.display())).expect("read arguments"),
            "tunnel\n--no-autoupdate\n--url\nhttp://127.0.0.1:43123\n"
        );
        tunnel.shutdown().await.expect("stop fake tunnel");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reports_early_exit_with_recent_output() {
        use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

        use tempfile::tempdir;

        use super::{QuickTunnel, TunnelOptions};

        let directory = tempdir().expect("tempdir");
        let executable = directory.path().join("failing-cloudflared");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' 'credentials rejected' >&2\nexit 7\n",
        )
        .expect("write fake cloudflared");
        let mut permissions = fs::metadata(&executable)
            .expect("script metadata")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).expect("make script executable");

        let error = QuickTunnel::start_with_options(
            43124,
            TunnelOptions {
                executable: executable.into_os_string(),
                startup_timeout: Duration::from_secs(3),
            },
        )
        .await
        .err()
        .expect("startup should fail");
        let message = error.to_string();
        assert!(message.contains("exited before publishing"), "{message}");
        assert!(message.contains("credentials rejected"), "{message}");
    }
}
