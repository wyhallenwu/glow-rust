//! Read-only local documentation website.

use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, anyhow, bail};
use axum::{
    Json, Router,
    body::Body,
    extract::{OriginalUri, Request, State},
    http::{HeaderName, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use ignore::WalkBuilder;
use percent_encoding::percent_decode_str;
use serde::Serialize;
use tokio::{
    net::TcpListener,
    sync::watch,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};

use crate::{
    discover::{DocumentIndex, ScanOptions},
    render::html::render_html,
    tunnel::{QuickTunnel, TunnelOptions},
};

const WEB_CSS: &str = include_str!("../assets/web.css");
const WEB_JS: &str = include_str!("../assets/web.js");
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(900);
const MIN_POLL_INTERVAL: Duration = Duration::from_millis(250);

const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; base-uri 'none'; connect-src 'self'; font-src 'self'; form-action 'none'; frame-ancestors 'none'; img-src 'self' data:; object-src 'self'; script-src 'self'; style-src 'self'";
const ASSET_CONTENT_SECURITY_POLICY: &str =
    "sandbox; default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'";

/// Local website settings.
#[derive(Clone, Debug)]
pub struct ServeOptions {
    /// Directory used as the documentation root.
    pub root: PathBuf,
    /// Interface to bind. The default is IPv4 loopback.
    pub bind_ip: IpAddr,
    /// Port to bind. Zero asks the OS for an available port.
    pub port: u16,
    /// Include hidden paths while building the read-only allowlists.
    pub include_hidden: bool,
    /// Apply `.gitignore`, global gitignore, and git exclude rules.
    pub respect_gitignore: bool,
    /// Interval between filesystem index snapshots.
    pub poll_interval: Duration,
}

impl ServeOptions {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            bind_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            include_hidden: false,
            respect_gitignore: true,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    #[must_use]
    pub const fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    #[must_use]
    pub const fn with_bind_ip(mut self, bind_ip: IpAddr) -> Self {
        self.bind_ip = bind_ip;
        self
    }

    #[must_use]
    pub const fn with_hidden(mut self, include_hidden: bool) -> Self {
        self.include_hidden = include_hidden;
        self
    }
}

impl From<PathBuf> for ServeOptions {
    fn from(root: PathBuf) -> Self {
        Self::new(root)
    }
}

/// Settings for a local server plus Cloudflare Quick Tunnel.
#[derive(Clone, Debug)]
pub struct ShareOptions {
    pub server: ServeOptions,
    pub tunnel: TunnelOptions,
    /// Open the validated public URL in the system browser after startup.
    pub open_browser: bool,
}

impl ShareOptions {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            server: ServeOptions::new(root),
            tunnel: TunnelOptions::default(),
            open_browser: false,
        }
    }
}

/// Handle to a spawned local documentation server.
pub struct RunningServer {
    local_addr: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
    server_task: Option<JoinHandle<Result<()>>>,
    poll_task: Option<JoinHandle<()>>,
}

impl RunningServer {
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    #[must_use]
    pub fn local_url(&self) -> String {
        format!("http://{}", self.local_addr)
    }

    /// Ask the listener and polling loop to stop without waiting for them.
    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Wait until the HTTP listener stops.
    ///
    /// This method is cancellation-safe and is useful in `tokio::select!`.
    pub async fn stopped(&mut self) -> Result<()> {
        let joined = match self.server_task.as_mut() {
            Some(task) => task.await,
            None => return Ok(()),
        };
        self.server_task = None;
        match joined {
            Ok(result) => result,
            Err(error) => Err(anyhow!("documentation server task failed: {error}")),
        }
    }

    /// Gracefully stop the listener and wait for its task to finish.
    pub async fn shutdown(mut self) -> Result<()> {
        self.request_shutdown();
        let server_result = self.stopped().await;
        self.stop_polling().await;
        server_result
    }

    /// Wait for the listener to finish without first sending a stop request.
    pub async fn wait(mut self) -> Result<()> {
        let server_result = self.stopped().await;
        self.request_shutdown();
        self.stop_polling().await;
        server_result
    }

    async fn stop_polling(&mut self) {
        if let Some(task) = self.poll_task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

#[derive(Clone)]
struct AppState {
    root: Arc<PathBuf>,
    scan_options: ScanOptions,
    snapshot: Arc<RwLock<SiteSnapshot>>,
}

struct SiteSnapshot {
    index: DocumentIndex,
    documents: BTreeMap<String, WebDocument>,
    assets: BTreeMap<String, WebAsset>,
    fingerprint: u64,
    revision: u64,
}

#[derive(Clone)]
struct WebDocument {
    path: PathBuf,
    relative: PathBuf,
    route: String,
    title: String,
    modified: SystemTime,
    size: u64,
}

#[derive(Clone)]
struct WebAsset {
    path: PathBuf,
    relative: PathBuf,
    content_type: &'static str,
    modified: SystemTime,
    size: u64,
}

impl SiteSnapshot {
    fn load(root: &Path, scan_options: &ScanOptions) -> Result<Self> {
        let index = DocumentIndex::scan(root, scan_options)?;
        let root = index.root.as_path();
        let mut documents = BTreeMap::new();

        for document in &index.documents {
            let Some(key) = relative_key(&document.relative_path) else {
                continue;
            };
            let Some(path) = canonical_regular_file(root, &document.absolute_path) else {
                continue;
            };
            documents.insert(
                key,
                WebDocument {
                    path,
                    relative: document.relative_path.clone(),
                    route: document.route(),
                    title: document.title.clone(),
                    modified: document.modified,
                    size: document.size,
                },
            );
        }

        let assets = scan_assets(root, scan_options);
        let fingerprint = snapshot_fingerprint(&documents, &assets);
        Ok(Self {
            index,
            documents,
            assets,
            fingerprint,
            revision: 1,
        })
    }

    fn preferred_route(&self) -> Option<&str> {
        self.index
            .preferred_document()
            .and_then(|document| relative_key(&document.relative_path))
            .and_then(|key| self.documents.get(&key))
            .map(|document| document.route.as_str())
            .or_else(|| {
                self.documents
                    .values()
                    .next()
                    .map(|document| document.route.as_str())
            })
    }
}

/// Bind and spawn a read-only documentation server.
pub async fn spawn(options: ServeOptions) -> Result<RunningServer> {
    let scan_options = ScanOptions {
        include_hidden: options.include_hidden,
        respect_gitignore: options.respect_gitignore,
    };
    let requested_root = options.root.clone();
    let initial = tokio::task::spawn_blocking({
        let scan_options = scan_options.clone();
        move || SiteSnapshot::load(&requested_root, &scan_options)
    })
    .await
    .context("initial document scan task failed")??;
    let root = Arc::new(initial.index.root.clone());
    let state = AppState {
        root,
        scan_options,
        snapshot: Arc::new(RwLock::new(initial)),
    };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/doc/{*path}", get(document_handler))
        .route("/asset/{*path}", get(asset_handler))
        .route("/api/status", get(status_handler))
        .route("/static/web.css", get(css_handler))
        .route("/static/web.js", get(js_handler))
        .fallback(not_found_handler)
        .layer(middleware::from_fn(security_headers))
        .with_state(state.clone());

    let requested_addr = SocketAddr::new(options.bind_ip, options.port);
    let listener = TcpListener::bind(requested_addr)
        .await
        .with_context(|| format!("could not bind documentation server to {requested_addr}"))?;
    let local_addr = listener
        .local_addr()
        .context("could not determine the documentation server address")?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server_shutdown = shutdown_rx.clone();
    let server_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(wait_for_shutdown(server_shutdown))
            .await
            .context("documentation server exited unexpectedly")
    });
    let poll_interval = options.poll_interval.max(MIN_POLL_INTERVAL);
    let poll_task = tokio::spawn(poll_snapshots(state, poll_interval, shutdown_rx));

    Ok(RunningServer {
        local_addr,
        shutdown_tx,
        server_task: Some(server_task),
        poll_task: Some(poll_task),
    })
}

/// Serve until Ctrl-C is received or the listener fails.
pub async fn serve(options: ServeOptions) -> Result<()> {
    let mut server = spawn(options).await?;
    println!("Local documentation: {}", server.local_url());
    println!("Press Ctrl-C to stop.");

    let outcome = tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal.context("could not listen for Ctrl-C")
        }
        stopped = server.stopped() => {
            stopped.and_then(|()| Err(anyhow!("documentation server stopped unexpectedly")))
        }
    };
    let shutdown = server.shutdown().await;
    outcome.and(shutdown)
}

/// Serve on loopback and expose the site through a Cloudflare Quick Tunnel.
pub async fn share(mut options: ShareOptions) -> Result<()> {
    // Quick Tunnel always targets this exact origin. This also prevents a share
    // command from accidentally exposing a second LAN-facing listener.
    options.server.bind_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mut server = spawn(options.server).await?;
    let port = server.local_addr().port();
    let mut tunnel = match QuickTunnel::start_with_options(port, options.tunnel).await {
        Ok(tunnel) => tunnel,
        Err(error) => {
            let _ = server.shutdown().await;
            return Err(error);
        }
    };

    println!("Local documentation: {}", server.local_url());
    println!("Public documentation: {}", tunnel.public_url());
    if options.open_browser
        && let Err(error) = open_browser(tunnel.public_url()).await
    {
        let _ = server.shutdown().await;
        let _ = tunnel.shutdown().await;
        return Err(error);
    }
    println!("Press Ctrl-C to stop sharing.");

    enum StopReason {
        Signal(Result<(), std::io::Error>),
        Server(Result<()>),
        Tunnel(Result<std::process::ExitStatus>),
    }

    let reason = tokio::select! {
        signal = tokio::signal::ctrl_c() => StopReason::Signal(signal),
        stopped = server.stopped() => StopReason::Server(stopped),
        status = tunnel.wait() => StopReason::Tunnel(status),
    };

    let server_shutdown = server.shutdown().await;
    let tunnel_shutdown = tunnel.shutdown().await;
    server_shutdown?;
    tunnel_shutdown?;

    match reason {
        StopReason::Signal(result) => result.context("could not listen for Ctrl-C"),
        StopReason::Server(result) => {
            result?;
            bail!("documentation server stopped unexpectedly while sharing")
        }
        StopReason::Tunnel(result) => {
            let status = result?;
            bail!("cloudflared stopped unexpectedly ({status})")
        }
    }
}

async fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = tokio::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = tokio::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    #[cfg(not(any(unix, target_os = "windows")))]
    bail!("opening a browser is not supported on this platform; visit {url}");

    #[cfg(any(unix, target_os = "windows"))]
    {
        let status = command
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .with_context(|| format!("could not open the system browser; visit {url}"))?;
        if !status.success() {
            bail!("the system browser command failed ({status}); visit {url}");
        }
        Ok(())
    }
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}

async fn poll_snapshots(
    state: AppState,
    poll_interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut ticks = interval(poll_interval);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // Tokio intervals tick immediately; the initial snapshot is already fresh.
    ticks.tick().await;

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = ticks.tick() => {
                let root = Arc::clone(&state.root);
                let scan_options = state.scan_options.clone();
                let loaded = tokio::task::spawn_blocking(move || {
                    SiteSnapshot::load(root.as_path(), &scan_options)
                }).await;

                let Ok(Ok(mut next)) = loaded else {
                    // A half-written file or transient permission failure should
                    // not take down an otherwise healthy reader.
                    continue;
                };
                let Ok(mut current) = state.snapshot.write() else {
                    continue;
                };
                if next.fingerprint != current.fingerprint {
                    next.revision = current.revision.saturating_add(1);
                    *current = next;
                }
            }
        }
    }
}

async fn index_handler(State(state): State<AppState>) -> Response {
    let route = match state.snapshot.read() {
        Ok(snapshot) => snapshot.preferred_route().map(str::to_owned),
        Err(_) => return internal_error_page(),
    };
    match route {
        Some(route) => Redirect::temporary(&format!("/doc/{route}")).into_response(),
        None => {
            let (revision, root_name) = match state.snapshot.read() {
                Ok(snapshot) => (snapshot.revision, display_root_name(&snapshot.index.root)),
                Err(_) => return internal_error_page(),
            };
            html_response(StatusCode::OK, render_empty_page(&root_name, revision))
        }
    }
}

async fn document_handler(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let Some(key) = route_key(uri.path(), "/doc/") else {
        return not_found_page();
    };
    let (document, navigation, revision, root_name) = match state.snapshot.read() {
        Ok(snapshot) => {
            let Some(document) = snapshot.documents.get(&key).cloned() else {
                return not_found_page();
            };
            (
                document,
                snapshot.documents.values().cloned().collect::<Vec<_>>(),
                snapshot.revision,
                display_root_name(&snapshot.index.root),
            )
        }
        Err(_) => return internal_error_page(),
    };

    let bytes = match read_allowlisted_file(state.root.as_path(), &document.path).await {
        Ok(bytes) => bytes,
        Err(_) => return not_found_page(),
    };
    let markdown = match String::from_utf8(bytes) {
        Ok(markdown) => markdown,
        Err(_) => {
            return error_page(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Invalid Markdown encoding",
                "This document is not valid UTF-8.",
            );
        }
    };
    let rendered = render_html(&markdown, &document.relative);
    html_response(
        StatusCode::OK,
        render_document_page(&root_name, &document, &navigation, revision, &rendered),
    )
}

async fn asset_handler(State(state): State<AppState>, OriginalUri(uri): OriginalUri) -> Response {
    let Some(key) = route_key(uri.path(), "/asset/") else {
        return not_found_page();
    };
    let asset = match state.snapshot.read() {
        Ok(snapshot) => match snapshot.assets.get(&key) {
            Some(asset) => asset.clone(),
            None => return not_found_page(),
        },
        Err(_) => return internal_error_page(),
    };
    let bytes = match read_allowlisted_file(state.root.as_path(), &asset.path).await {
        Ok(bytes) => bytes,
        Err(_) => return not_found_page(),
    };
    binary_response(StatusCode::OK, asset.content_type, bytes, true)
}

#[derive(Serialize)]
struct StatusPayload {
    revision: u64,
    documents: usize,
}

async fn status_handler(State(state): State<AppState>) -> Response {
    let payload = match state.snapshot.read() {
        Ok(snapshot) => StatusPayload {
            revision: snapshot.revision,
            documents: snapshot.documents.len(),
        },
        Err(_) => return internal_error_page(),
    };
    Json(payload).into_response()
}

async fn css_handler() -> Response {
    text_response(StatusCode::OK, "text/css; charset=utf-8", WEB_CSS)
}

async fn js_handler() -> Response {
    text_response(StatusCode::OK, "text/javascript; charset=utf-8", WEB_JS)
}

async fn not_found_handler() -> Response {
    not_found_page()
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    insert_header_if_missing(
        headers,
        header::CONTENT_SECURITY_POLICY,
        CONTENT_SECURITY_POLICY,
    );
    insert_header_if_missing(headers, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    insert_header_if_missing(headers, header::REFERRER_POLICY, "no-referrer");
    insert_header_if_missing(headers, header::X_FRAME_OPTIONS, "DENY");
    insert_header_if_missing(headers, header::CACHE_CONTROL, "no-store");
    insert_header_if_missing(
        headers,
        HeaderName::from_static("permissions-policy"),
        "camera=(), geolocation=(), microphone=(), payment=(), usb=()",
    );
    insert_header_if_missing(
        headers,
        HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    );
    insert_header_if_missing(
        headers,
        HeaderName::from_static("cross-origin-resource-policy"),
        "same-origin",
    );
    response
}

fn insert_header_if_missing(
    headers: &mut axum::http::HeaderMap,
    name: HeaderName,
    value: &'static str,
) {
    if !headers.contains_key(&name) {
        headers.insert(name, HeaderValue::from_static(value));
    }
}

fn scan_assets(root: &Path, options: &ScanOptions) -> BTreeMap<String, WebAsset> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(!options.include_hidden)
        .follow_links(false)
        .git_ignore(options.respect_gitignore)
        .git_global(options.respect_gitignore)
        .git_exclude(options.respect_gitignore)
        .parents(options.respect_gitignore);

    let mut assets = BTreeMap::new();
    for entry in builder.build().filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Some(content_type) = allowed_asset_content_type(entry.path()) else {
            continue;
        };
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let Some(key) = relative_key(relative) else {
            continue;
        };
        let Some(path) = canonical_regular_file(root, entry.path()) else {
            continue;
        };
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        assets.insert(
            key,
            WebAsset {
                path,
                relative: relative.to_path_buf(),
                content_type,
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                size: metadata.len(),
            },
        );
    }
    assets
}

fn allowed_asset_content_type(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "avif" => Some("image/avif"),
        "bmp" => Some("image/bmp"),
        "gif" => Some("image/gif"),
        "ico" => Some("image/x-icon"),
        "jpeg" | "jpg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "svg" => Some("image/svg+xml"),
        "webp" => Some("image/webp"),
        "pdf" => Some("application/pdf"),
        _ => None,
    }
}

fn canonical_regular_file(root: &Path, requested: &Path) -> Option<PathBuf> {
    let metadata = std::fs::symlink_metadata(requested).ok()?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let canonical = requested.canonicalize().ok()?;
    canonical.starts_with(root).then_some(canonical)
}

async fn read_allowlisted_file(root: &Path, requested: &Path) -> Result<Vec<u8>> {
    let metadata = tokio::fs::symlink_metadata(requested)
        .await
        .context("allowlisted file disappeared")?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("allowlisted path is no longer a regular file");
    }
    let canonical = tokio::fs::canonicalize(requested)
        .await
        .context("could not resolve allowlisted file")?;
    if !canonical.starts_with(root) {
        bail!("allowlisted file escaped the documentation root");
    }
    tokio::fs::read(canonical)
        .await
        .context("could not read allowlisted file")
}

fn relative_key(path: &Path) -> Option<String> {
    let mut segments = Vec::new();
    for component in path.components() {
        let std::path::Component::Normal(segment) = component else {
            return None;
        };
        let segment = segment.to_str()?;
        if segment.is_empty() || segment.contains(['\\', '\0']) {
            return None;
        }
        segments.push(segment);
    }
    (!segments.is_empty()).then(|| segments.join("/"))
}

fn route_key(uri_path: &str, prefix: &str) -> Option<String> {
    let encoded = uri_path.strip_prefix(prefix)?;
    let decoded = percent_decode_str(encoded).decode_utf8().ok()?;
    if decoded.is_empty() || decoded.starts_with('/') || decoded.contains(['\\', '\0']) {
        return None;
    }
    let segments = decoded.split('/').collect::<Vec<_>>();
    if segments
        .iter()
        .any(|segment| segment.is_empty() || matches!(*segment, "." | ".."))
    {
        return None;
    }
    Some(segments.join("/"))
}

fn snapshot_fingerprint(
    documents: &BTreeMap<String, WebDocument>,
    assets: &BTreeMap<String, WebAsset>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (key, document) in documents {
        "document".hash(&mut hasher);
        key.hash(&mut hasher);
        document.size.hash(&mut hasher);
        system_time_key(document.modified).hash(&mut hasher);
    }
    for (key, asset) in assets {
        "asset".hash(&mut hasher);
        key.hash(&mut hasher);
        asset.relative.hash(&mut hasher);
        asset.size.hash(&mut hasher);
        system_time_key(asset.modified).hash(&mut hasher);
    }
    hasher.finish()
}

fn system_time_key(time: SystemTime) -> Duration {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
}

fn render_document_page(
    root_name: &str,
    current: &WebDocument,
    navigation: &[WebDocument],
    revision: u64,
    document_html: &str,
) -> String {
    let title = escape_text(&current.title);
    let root_name_attribute = escape_attribute(root_name);
    let root_name = escape_text(root_name);
    let relative = escape_text(&current.relative.to_string_lossy());
    let navigation = render_navigation(navigation, &current.relative);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="color-scheme" content="light dark">
  <title>{title} · {root_name}</title>
  <link rel="stylesheet" href="/static/web.css">
  <script src="/static/web.js" defer></script>
</head>
<body data-revision="{revision}" data-nav-open="false">
  <header class="topbar">
    <button class="menu-button" type="button" data-menu-toggle aria-label="Open document navigation" aria-controls="doc-navigation" aria-expanded="false">☰</button>
    <a class="brand" href="/"><span class="brand-mark">G</span> Glow</a>
    <span></span>
  </header>
  <div class="layout">
    <aside class="nav-panel" id="doc-navigation">
      <a class="brand" href="/"><span class="brand-mark">G</span> Glow Docs</a>
      <span class="root-label" title="{root_name_attribute}">{root_name}</span>
      <div class="search-wrap"><input class="doc-search" type="search" data-doc-search placeholder="Search documents…" aria-label="Search documents"></div>
      <div class="nav-heading">Documents</div>
      <nav class="doc-nav" aria-label="Documents">{navigation}</nav>
    </aside>
    <main class="content-shell">
      <div class="document-meta">{relative}</div>
      <article class="document" data-document>{document_html}</article>
    </main>
    <aside class="outline-panel">
      <div class="outline-heading">On this page</div>
      <nav class="outline" data-outline aria-label="On this page"></nav>
    </aside>
  </div>
  <button class="theme-button" type="button" data-theme-toggle aria-label="Toggle color theme">☾</button>
</body>
</html>"#
    )
}

fn render_navigation(documents: &[WebDocument], current: &Path) -> String {
    let mut output = String::new();
    for document in documents {
        let depth = document
            .relative
            .components()
            .count()
            .saturating_sub(1)
            .min(8);
        let title = escape_text(&document.title);
        let path = document.relative.to_string_lossy();
        let search = escape_attribute(&format!("{} {}", document.title, path).to_lowercase());
        let current_attribute = if document.relative == current {
            " aria-current=\"page\""
        } else {
            ""
        };
        output.push_str(&format!(
            "<a class=\"doc-link depth-{depth}\" href=\"/doc/{}\" data-search=\"{search}\" title=\"{}\"{current_attribute}>{title}</a>",
            document.route,
            escape_attribute(&path),
        ));
    }
    output
}

fn render_empty_page(root_name: &str, revision: u64) -> String {
    let root_name = escape_text(root_name);
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>No Markdown · {root_name}</title><link rel="stylesheet" href="/static/web.css"><script src="/static/web.js" defer></script></head>
<body data-revision="{revision}"><main class="empty-state"><span class="brand-mark">G</span><h1>No Markdown yet</h1><p>Glow is watching <strong>{root_name}</strong>. Add a Markdown file anywhere below this folder and this page will refresh.</p></main><button class="theme-button" type="button" data-theme-toggle aria-label="Toggle color theme">☾</button></body></html>"#
    )
}

fn display_root_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Documentation")
        .to_owned()
}

fn escape_text(value: &str) -> String {
    html_escape::encode_text(value).into_owned()
}

fn escape_attribute(value: &str) -> String {
    html_escape::encode_double_quoted_attribute(value).into_owned()
}

fn html_response(status: StatusCode, body: String) -> Response {
    text_response(status, "text/html; charset=utf-8", body)
}

fn text_response(
    status: StatusCode,
    content_type: &'static str,
    body: impl Into<Body>,
) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(body.into())
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

fn binary_response(
    status: StatusCode,
    content_type: &'static str,
    body: Vec<u8>,
    isolated: bool,
) -> Response {
    let mut response = text_response(status, content_type, body);
    if isolated {
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(ASSET_CONTENT_SECURITY_POLICY),
        );
    }
    response
}

fn not_found_page() -> Response {
    error_page(
        StatusCode::NOT_FOUND,
        "Not found",
        "That file is not in this documentation index.",
    )
}

fn internal_error_page() -> Response {
    error_page(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Index unavailable",
        "The documentation index is temporarily unavailable.",
    )
}

fn error_page(status: StatusCode, title: &str, detail: &str) -> Response {
    html_response(
        status,
        format!(
            r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>{}</title><link rel="stylesheet" href="/static/web.css"></head><body><main class="empty-state"><span class="brand-mark">G</span><h1>{}</h1><p>{}</p><p><a href="/">Back to documentation</a></p></main></body></html>"#,
            escape_text(title),
            escape_text(title),
            escape_text(detail),
        ),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn route_decoding_rejects_traversal_and_cross_platform_separators() {
        assert_eq!(
            route_key("/doc/guide%2Fintro.md", "/doc/"),
            Some("guide/intro.md".to_owned())
        );
        assert_eq!(route_key("/doc/%2e%2e/secret.md", "/doc/"), None);
        assert_eq!(route_key("/doc/guide\\secret.md", "/doc/"), None);
        assert_eq!(route_key("/doc//secret.md", "/doc/"), None);
    }

    #[test]
    fn asset_extensions_are_an_explicit_allowlist() {
        assert_eq!(
            allowed_asset_content_type(Path::new("diagram.SVG")),
            Some("image/svg+xml")
        );
        assert_eq!(
            allowed_asset_content_type(Path::new("manual.pdf")),
            Some("application/pdf")
        );
        assert_eq!(allowed_asset_content_type(Path::new("secrets.env")), None);
        assert_eq!(allowed_asset_content_type(Path::new("page.html")), None);
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_excludes_asset_symlinks_that_escape_the_root() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("root tempdir");
        let outside = tempdir().expect("outside tempdir");
        fs::write(root.path().join("README.md"), "# Home").expect("write markdown");
        fs::write(outside.path().join("secret.png"), b"not really an image").expect("write image");
        symlink(
            outside.path().join("secret.png"),
            root.path().join("leak.png"),
        )
        .expect("create symlink");

        let snapshot = SiteSnapshot::load(root.path(), &ScanOptions::default()).expect("scan");
        assert!(!snapshot.assets.contains_key("leak.png"));
    }

    #[tokio::test]
    async fn server_exposes_only_indexed_markdown_and_allowlisted_assets() {
        let root = tempdir().expect("tempdir");
        fs::write(
            root.path().join("README.md"),
            "# Welcome\n\nEuler: $e^{i\\pi}+1=0$.\n\n```mermaid\nflowchart LR\nA-->B\n```",
        )
        .expect("write markdown");
        fs::write(root.path().join("logo.png"), b"image bytes").expect("write image");
        fs::write(root.path().join("secret.txt"), b"do not serve").expect("write secret");

        let server = spawn(ServeOptions::new(root.path())).await.expect("spawn");
        let client = reqwest::Client::new();
        let document = client
            .get(format!("{}/doc/README.md", server.local_url()))
            .send()
            .await
            .expect("document request");
        assert_eq!(document.status(), StatusCode::OK);
        assert_eq!(
            document.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        let policy = document.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .expect("CSP header");
        assert!(policy.contains("script-src 'self'"));
        assert!(!policy.contains("unsafe-eval"));
        assert!(!policy.contains("https:"));
        let document_html = document.text().await.expect("html");
        assert!(document_html.contains("Welcome"));
        assert!(document_html.contains("<math display=\"inline\""));
        assert!(document_html.contains("data:image/svg+xml;base64,"));

        let image = client
            .get(format!("{}/asset/logo.png", server.local_url()))
            .send()
            .await
            .expect("image request");
        assert_eq!(image.status(), StatusCode::OK);
        assert_eq!(image.headers()[header::CONTENT_TYPE], "image/png");

        let secret = client
            .get(format!("{}/asset/secret.txt", server.local_url()))
            .send()
            .await
            .expect("secret request");
        assert_eq!(secret.status(), StatusCode::NOT_FOUND);

        server.shutdown().await.expect("shutdown");
    }
}
