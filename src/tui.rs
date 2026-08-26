//! Interactive terminal document browser.

use std::{
    collections::BTreeSet,
    fs,
    io::{self, Stdout},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use crossterm::{
    cursor,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use notify::{Event as NotifyEvent, RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use crate::{
    discover::{DocumentIndex, ScanOptions},
    document::{Document, strip_frontmatter},
    render::Theme,
};

const WIDE_TERMINAL_MIN: u16 = 88;
const WATCH_DEBOUNCE: Duration = Duration::from_millis(220);
const EVENT_TICK: Duration = Duration::from_millis(50);

/// Runtime options for the terminal document browser.
#[derive(Clone, Copy, Debug)]
pub struct TuiOptions {
    pub include_hidden: bool,
    pub respect_gitignore: bool,
    pub theme: Theme,
    pub line_numbers: bool,
    pub mouse: bool,
}

impl Default for TuiOptions {
    fn default() -> Self {
        Self {
            include_hidden: false,
            respect_gitignore: true,
            theme: Theme::Dark,
            line_numbers: false,
            mouse: true,
        }
    }
}

/// Browse all Markdown documents below `root` in an interactive terminal UI.
pub fn run(root: &Path, options: TuiOptions) -> Result<()> {
    let mut app = App::new(root, options)?;
    let (watch_tx, watch_rx) = mpsc::channel();
    let _watcher = watch_tree(&app.index.root, watch_tx)?;

    let _restore = TerminalRestore::enter(options.mouse)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).context("cannot initialize terminal renderer")?;
    terminal.clear().context("cannot clear terminal")?;

    let result = run_loop(&mut terminal, &mut app, &watch_rx);
    // This is best effort; `TerminalRestore` performs the essential cleanup even
    // when drawing, input handling, or this call itself fails.
    let _ = terminal.show_cursor();
    result
}

/// Restores the user's terminal on every normal error path and during unwinding.
struct TerminalRestore {
    raw_mode: bool,
    alternate_screen: bool,
    mouse_capture: bool,
}

impl TerminalRestore {
    fn enter(mouse: bool) -> Result<Self> {
        let mut restore = Self {
            raw_mode: false,
            alternate_screen: false,
            mouse_capture: false,
        };

        enable_raw_mode().context("cannot enable terminal raw mode")?;
        restore.raw_mode = true;

        let mut stdout = io::stdout();
        // Mark these before executing so cleanup is still attempted if the
        // terminal accepts only part of a multi-command sequence.
        restore.alternate_screen = true;
        execute!(stdout, EnterAlternateScreen, cursor::Hide)
            .context("cannot enter alternate terminal screen")?;

        if mouse {
            restore.mouse_capture = true;
            execute!(stdout, EnableMouseCapture).context("cannot enable mouse capture")?;
        }

        Ok(restore)
    }
}

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        if self.raw_mode {
            let _ = disable_raw_mode();
        }
        let mut stdout = io::stdout();
        if self.mouse_capture {
            let _ = execute!(stdout, DisableMouseCapture);
        }
        if self.alternate_screen {
            let _ = execute!(stdout, LeaveAlternateScreen, cursor::Show);
        } else {
            let _ = execute!(stdout, cursor::Show);
        }
    }
}

fn watch_tree(
    root: &Path,
    tx: mpsc::Sender<notify::Result<NotifyEvent>>,
) -> Result<RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    })
    .context("cannot create filesystem watcher")?;
    watcher
        .watch(root, RecursiveMode::Recursive)
        .with_context(|| format!("cannot watch {}", root.display()))?;
    Ok(watcher)
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    watch_rx: &Receiver<notify::Result<NotifyEvent>>,
) -> Result<()> {
    let mut pending_rescan = None;

    loop {
        let mut regions = UiRegions::default();
        terminal
            .draw(|frame| regions = draw(frame, app))
            .context("cannot draw terminal UI")?;

        loop {
            drain_watch_events(app, watch_rx, &mut pending_rescan);
            if pending_rescan.is_some_and(|last| last.elapsed() >= WATCH_DEBOUNCE) {
                pending_rescan = None;
                match app.rescan() {
                    Ok(()) => {
                        app.status =
                            format!("updated: {} Markdown files", app.index.documents.len());
                    }
                    Err(error) => app.status = format!("watch refresh failed: {error:#}"),
                }
                break;
            }

            if !event::poll(EVENT_TICK).context("cannot poll terminal input")? {
                continue;
            }

            match event::read().context("cannot read terminal input")? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    if app.handle_key(key)? == LoopControl::Quit {
                        return Ok(());
                    }
                }
                Event::Mouse(mouse) if app.options.mouse => app.handle_mouse(mouse, regions),
                Event::Paste(text) if app.filter_editing => {
                    app.filter.push_str(&text.replace(['\n', '\r'], ""));
                    app.rebuild_rows(None);
                }
                _ => {}
            }
            break;
        }
    }
}

fn drain_watch_events(
    app: &mut App,
    watch_rx: &Receiver<notify::Result<NotifyEvent>>,
    pending_rescan: &mut Option<Instant>,
) {
    while let Ok(event) = watch_rx.try_recv() {
        match event {
            Ok(_) => *pending_rescan = Some(Instant::now()),
            Err(error) => app.status = format!("filesystem watch error: {error}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Focus {
    Tree,
    Preview,
}

impl Focus {
    const fn label(self) -> &'static str {
        match self {
            Self::Tree => "files",
            Self::Preview => "preview",
        }
    }

    const fn toggled(self) -> Self {
        match self {
            Self::Tree => Self::Preview,
            Self::Preview => Self::Tree,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NodeKey {
    Folder(PathBuf),
    Document(PathBuf),
}

#[derive(Clone, Debug)]
struct TreeRow {
    key: NodeKey,
    depth: usize,
    document_index: Option<usize>,
    label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreviewIdentity {
    path: PathBuf,
    modified: SystemTime,
    size: u64,
}

struct PreviewCache {
    identity: PreviewIdentity,
    source: Option<String>,
    error: Option<String>,
    render_width: usize,
    line_numbers: bool,
    lines: Vec<Line<'static>>,
}

struct App {
    options: TuiOptions,
    scan_options: ScanOptions,
    index: DocumentIndex,
    collapsed: BTreeSet<PathBuf>,
    rows: Vec<TreeRow>,
    selected: usize,
    focus: Focus,
    filter: String,
    filter_editing: bool,
    help: bool,
    preview_scroll: usize,
    preview_viewport_height: usize,
    tree_offset: usize,
    preview_cache: Option<PreviewCache>,
    status: String,
}

impl App {
    fn new(root: &Path, options: TuiOptions) -> Result<Self> {
        let scan_options = ScanOptions {
            include_hidden: options.include_hidden,
            respect_gitignore: options.respect_gitignore,
        };
        let index = DocumentIndex::scan(root, &scan_options)?;
        let preferred = index
            .preferred_document()
            .map(|document| NodeKey::Document(document.relative_path.clone()));
        let count = index.documents.len();
        let mut app = Self {
            options,
            scan_options,
            index,
            collapsed: BTreeSet::new(),
            rows: Vec::new(),
            selected: 0,
            focus: Focus::Tree,
            filter: String::new(),
            filter_editing: false,
            help: false,
            preview_scroll: 0,
            preview_viewport_height: 0,
            tree_offset: 0,
            preview_cache: None,
            status: format!("discovered {count} Markdown files"),
        };
        app.rebuild_rows(preferred.as_ref());
        Ok(app)
    }

    fn rescan(&mut self) -> Result<()> {
        let selected = self.selected_key().cloned();
        self.index = DocumentIndex::scan(&self.index.root, &self.scan_options)?;
        self.rebuild_rows(selected.as_ref());
        Ok(())
    }

    fn rebuild_rows(&mut self, preferred: Option<&NodeKey>) {
        let old_selected = preferred.cloned().or_else(|| self.selected_key().cloned());
        let query = self.filter.trim().to_lowercase();
        let mut rows = Vec::new();
        self.append_children(Path::new(""), 0, false, &query, &mut rows);
        self.rows = rows;

        if self.rows.is_empty() {
            self.selected = 0;
            self.tree_offset = 0;
            self.preview_scroll = 0;
            return;
        }

        self.selected = old_selected
            .as_ref()
            .and_then(|key| self.rows.iter().position(|row| &row.key == key))
            .unwrap_or_else(|| self.selected.min(self.rows.len() - 1));
        self.preview_scroll = 0;
    }

    fn append_children(
        &self,
        parent: &Path,
        depth: usize,
        ancestor_matches_filter: bool,
        query: &str,
        rows: &mut Vec<TreeRow>,
    ) {
        let mut folders: Vec<&PathBuf> = self
            .index
            .folders
            .iter()
            .filter(|folder| {
                !folder.as_os_str().is_empty()
                    && folder.parent().unwrap_or_else(|| Path::new("")) == parent
            })
            .collect();
        folders.sort_by_cached_key(|folder| folder.to_string_lossy().to_lowercase());

        for folder in folders {
            let own_match = path_matches(folder, query);
            let inherited_match = ancestor_matches_filter || own_match;
            if !query.is_empty() && !inherited_match && !self.folder_has_match(folder, query) {
                continue;
            }

            rows.push(TreeRow {
                key: NodeKey::Folder(folder.clone()),
                depth,
                document_index: None,
                label: file_name(folder),
            });

            // Filtering always reveals matches and their ancestors. The user's
            // collapsed state remains intact and is restored when filtering ends.
            if query.is_empty() && self.collapsed.contains(folder) {
                continue;
            }
            self.append_children(folder, depth + 1, inherited_match, query, rows);
        }

        let mut documents: Vec<(usize, &Document)> = self
            .index
            .documents
            .iter()
            .enumerate()
            .filter(|(_, document)| {
                document
                    .relative_path
                    .parent()
                    .unwrap_or_else(|| Path::new(""))
                    == parent
            })
            .collect();
        documents.sort_by_cached_key(|(_, document)| {
            document.relative_path.to_string_lossy().to_lowercase()
        });

        for (document_index, document) in documents {
            if !query.is_empty() && !ancestor_matches_filter && !document_matches(document, query) {
                continue;
            }
            rows.push(TreeRow {
                key: NodeKey::Document(document.relative_path.clone()),
                depth,
                document_index: Some(document_index),
                label: file_name(&document.relative_path),
            });
        }
    }

    fn folder_has_match(&self, folder: &Path, query: &str) -> bool {
        self.index.documents.iter().any(|document| {
            document.relative_path.starts_with(folder) && document_matches(document, query)
        }) || self
            .index
            .folders
            .iter()
            .any(|child| child.starts_with(folder) && path_matches(child, query))
    }

    fn selected_key(&self) -> Option<&NodeKey> {
        self.rows.get(self.selected).map(|row| &row.key)
    }

    fn selected_document(&self) -> Option<&Document> {
        self.rows
            .get(self.selected)
            .and_then(|row| row.document_index)
            .and_then(|index| self.index.documents.get(index))
    }

    fn selected_path(&self) -> Option<&Path> {
        match self.selected_key()? {
            NodeKey::Folder(path) | NodeKey::Document(path) => Some(path),
        }
    }

    fn visible_document_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.document_index.is_some())
            .count()
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.rows.len() - 1);
        self.preview_scroll = 0;
    }

    fn select_first(&mut self) {
        if !self.rows.is_empty() {
            self.selected = 0;
            self.preview_scroll = 0;
        }
    }

    fn select_last(&mut self) {
        if !self.rows.is_empty() {
            self.selected = self.rows.len() - 1;
            self.preview_scroll = 0;
        }
    }

    fn toggle_selected_folder(&mut self) -> bool {
        let Some(NodeKey::Folder(folder)) = self.selected_key().cloned() else {
            return false;
        };
        if !self.collapsed.remove(&folder) {
            self.collapsed.insert(folder.clone());
        }
        self.rebuild_rows(Some(&NodeKey::Folder(folder)));
        true
    }

    fn expand_selected_folder(&mut self) -> bool {
        let Some(NodeKey::Folder(folder)) = self.selected_key().cloned() else {
            return false;
        };
        if self.collapsed.remove(&folder) {
            self.rebuild_rows(Some(&NodeKey::Folder(folder)));
        }
        true
    }

    fn collapse_or_select_parent(&mut self) {
        let Some(key) = self.selected_key().cloned() else {
            return;
        };
        if let NodeKey::Folder(folder) = &key
            && !self.collapsed.contains(folder)
        {
            self.collapsed.insert(folder.clone());
            self.rebuild_rows(Some(&key));
            return;
        }

        let path = match &key {
            NodeKey::Folder(path) | NodeKey::Document(path) => path,
        };
        let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        else {
            return;
        };
        let parent_key = NodeKey::Folder(parent.to_path_buf());
        if let Some(index) = self.rows.iter().position(|row| row.key == parent_key) {
            self.selected = index;
            self.preview_scroll = 0;
        }
    }

    fn max_preview_scroll(&self) -> usize {
        self.preview_cache
            .as_ref()
            .map_or(0, |cache| cache.lines.len())
            .saturating_sub(self.preview_viewport_height)
    }

    fn scroll_preview(&mut self, delta: isize) {
        self.preview_scroll = self
            .preview_scroll
            .saturating_add_signed(delta)
            .min(self.max_preview_scroll());
    }

    fn preview_top(&mut self) {
        self.preview_scroll = 0;
    }

    fn preview_bottom(&mut self) {
        self.preview_scroll = self.max_preview_scroll();
    }

    fn page_size(&self) -> isize {
        self.preview_viewport_height.saturating_sub(1).max(1) as isize
    }

    fn ensure_preview(&mut self, width: usize) {
        let Some(document) = self.selected_document().cloned() else {
            self.preview_cache = None;
            self.preview_scroll = 0;
            return;
        };

        let identity = PreviewIdentity {
            path: document.absolute_path,
            modified: document.modified,
            size: document.size,
        };
        let must_load = self
            .preview_cache
            .as_ref()
            .is_none_or(|cache| cache.identity != identity);

        if must_load {
            let (source, error) = match fs::read_to_string(&identity.path) {
                Ok(markdown) => (Some(strip_frontmatter(&markdown).to_owned()), None),
                Err(error) => (
                    None,
                    Some(format!("Cannot read {}: {error}", identity.path.display())),
                ),
            };
            self.preview_cache = Some(PreviewCache {
                identity,
                source,
                error,
                render_width: 0,
                line_numbers: self.options.line_numbers,
                lines: Vec::new(),
            });
            self.preview_scroll = 0;
        }

        let cache = self
            .preview_cache
            .as_mut()
            .expect("preview cache initialized");
        let width = width.max(8);
        if cache.render_width == width
            && cache.line_numbers == self.options.line_numbers
            && !cache.lines.is_empty()
        {
            return;
        }

        cache.render_width = width;
        cache.line_numbers = self.options.line_numbers;
        cache.lines = if let Some(error) = &cache.error {
            vec![Line::from(Span::styled(
                error.clone(),
                Style::default().fg(Color::Red),
            ))]
        } else {
            let source = cache.source.as_deref().unwrap_or_default();
            let content_width = if self.options.line_numbers {
                width.saturating_sub(7).max(8)
            } else {
                width
            };
            let lines =
                crate::render::terminal::render_lines(source, content_width, self.options.theme);
            if self.options.line_numbers {
                add_line_numbers(lines)
            } else {
                lines
            }
        };
        self.preview_scroll = self.preview_scroll.min(self.max_preview_scroll());
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<LoopControl> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(LoopControl::Quit);
        }

        if self.filter_editing {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.filter_editing = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.rebuild_rows(None);
                }
                KeyCode::Char(character)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.filter.push(character);
                    self.rebuild_rows(None);
                }
                _ => {}
            }
            return Ok(LoopControl::Continue);
        }

        if self.help {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?' | 'q')) {
                self.help = false;
            }
            return Ok(LoopControl::Continue);
        }

        match key.code {
            KeyCode::Char('q') => return Ok(LoopControl::Quit),
            KeyCode::Esc => {
                if self.filter.is_empty() {
                    return Ok(LoopControl::Quit);
                }
                self.filter.clear();
                self.rebuild_rows(None);
                self.status = "filter cleared".to_string();
            }
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('/') => self.filter_editing = true,
            KeyCode::Tab | KeyCode::BackTab => self.focus = self.focus.toggled(),
            KeyCode::Enter => {
                if !self.toggle_selected_folder() && self.selected_document().is_some() {
                    self.focus = Focus::Preview;
                    self.preview_scroll = 0;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                Focus::Tree => self.move_selection(1),
                Focus::Preview => self.scroll_preview(1),
            },
            KeyCode::Up | KeyCode::Char('k') => match self.focus {
                Focus::Tree => self.move_selection(-1),
                Focus::Preview => self.scroll_preview(-1),
            },
            KeyCode::PageDown => self.scroll_preview(self.page_size()),
            KeyCode::PageUp => self.scroll_preview(-self.page_size()),
            KeyCode::Char('g') | KeyCode::Home => match self.focus {
                Focus::Tree => self.select_first(),
                Focus::Preview => self.preview_top(),
            },
            KeyCode::Char('G') | KeyCode::End => match self.focus {
                Focus::Tree => self.select_last(),
                Focus::Preview => self.preview_bottom(),
            },
            KeyCode::Right => {
                if !self.expand_selected_folder() && self.selected_document().is_some() {
                    self.focus = Focus::Preview;
                }
            }
            KeyCode::Left => self.collapse_or_select_parent(),
            KeyCode::Char('r') => {
                self.rescan()?;
                self.status = format!("refreshed: {} Markdown files", self.index.documents.len());
            }
            _ => {}
        }
        Ok(LoopControl::Continue)
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, regions: UiRegions) {
        let in_tree = regions
            .tree
            .is_some_and(|rect| rect_contains(rect, mouse.column, mouse.row));
        let in_preview = regions
            .preview
            .is_some_and(|rect| rect_contains(rect, mouse.column, mouse.row));

        match mouse.kind {
            MouseEventKind::ScrollDown if in_preview => {
                self.focus = Focus::Preview;
                self.scroll_preview(3);
            }
            MouseEventKind::ScrollUp if in_preview => {
                self.focus = Focus::Preview;
                self.scroll_preview(-3);
            }
            MouseEventKind::ScrollDown if in_tree => {
                self.focus = Focus::Tree;
                self.move_selection(3);
            }
            MouseEventKind::ScrollUp if in_tree => {
                self.focus = Focus::Tree;
                self.move_selection(-3);
            }
            MouseEventKind::Down(MouseButton::Left) if in_preview => self.focus = Focus::Preview,
            MouseEventKind::Down(MouseButton::Left) if in_tree => {
                self.focus = Focus::Tree;
                if let Some(tree) = regions.tree {
                    let content_top = tree.y.saturating_add(1);
                    let content_bottom = tree.y.saturating_add(tree.height.saturating_sub(1));
                    if mouse.row >= content_top && mouse.row < content_bottom {
                        let row = self.tree_offset + usize::from(mouse.row - content_top);
                        if row < self.rows.len() {
                            self.selected = row;
                            self.preview_scroll = 0;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoopControl {
    Continue,
    Quit,
}

#[derive(Clone, Copy, Debug, Default)]
struct UiRegions {
    tree: Option<Rect>,
    preview: Option<Rect>,
}

fn draw(frame: &mut Frame<'_>, app: &mut App) -> UiRegions {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);

    draw_header(frame, app, vertical[0]);

    let mut regions = UiRegions::default();
    if area.width >= WIDE_TERMINAL_MIN {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
            .split(vertical[1]);
        draw_tree(frame, app, columns[0]);
        draw_preview(frame, app, columns[1]);
        regions.tree = Some(columns[0]);
        regions.preview = Some(columns[1]);
    } else {
        match app.focus {
            Focus::Tree => {
                draw_tree(frame, app, vertical[1]);
                regions.tree = Some(vertical[1]);
            }
            Focus::Preview => {
                draw_preview(frame, app, vertical[1]);
                regions.preview = Some(vertical[1]);
            }
        }
    }

    draw_footer(frame, app, vertical[2]);
    if app.help {
        draw_help(frame, area);
    }
    regions
}

fn draw_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let root_name = app
        .index
        .root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("/");
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            " glow ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {root_name}  •  recursive Markdown browser"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(header, area);
}

fn draw_tree(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Tree;
    let title = if app.filter.is_empty() {
        format!(" Files ({}) ", app.index.documents.len())
    } else {
        format!(
            " Files ({}/{}) ",
            app.visible_document_count(),
            app.index.documents.len()
        )
    };
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let items: Vec<ListItem<'_>> = if app.rows.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            if app.filter.is_empty() {
                "  No Markdown files found"
            } else {
                "  No files match this filter"
            },
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        app.rows
            .iter()
            .map(|row| {
                let indent = "  ".repeat(row.depth);
                let (icon, style) = match &row.key {
                    NodeKey::Folder(path) => {
                        let icon = if app.collapsed.contains(path) && app.filter.is_empty() {
                            "▸ "
                        } else {
                            "▾ "
                        };
                        (
                            icon,
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        )
                    }
                    NodeKey::Document(_) => ("◆ ", Style::default().fg(Color::White)),
                };
                ListItem::new(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(icon, style),
                    Span::styled(row.label.clone(), style),
                ]))
            })
            .collect()
    };

    let mut state = ListState::default();
    if !app.rows.is_empty() {
        state.select(Some(app.selected));
        *state.offset_mut() = app.tree_offset;
    }
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        )
        .highlight_symbol("│ ")
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut state);
    app.tree_offset = state.offset();
}

fn draw_preview(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Preview;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let inner_width = usize::from(area.width.saturating_sub(2));
    app.preview_viewport_height = usize::from(area.height.saturating_sub(2));
    app.ensure_preview(inner_width);
    app.preview_scroll = app.preview_scroll.min(app.max_preview_scroll());

    let title = app.selected_document().map_or_else(
        || " Preview ".to_string(),
        |document| format!(" {} ", document.relative_path.display()),
    );
    let lines = app.preview_cache.as_ref().map_or_else(
        || {
            vec![Line::from(Span::styled(
                "Select a Markdown document to preview it.",
                Style::default().fg(Color::DarkGray),
            ))]
        },
        |cache| cache.lines.clone(),
    );
    let scroll = u16::try_from(app.preview_scroll).unwrap_or(u16::MAX);
    let preview = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        )
        .scroll((scroll, 0));
    frame.render_widget(preview, area);
}

fn draw_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let selected = app
        .selected_path()
        .map_or_else(|| "—".to_string(), |path| path.display().to_string());
    let count = if app.filter.is_empty() {
        format!("{} files", app.index.documents.len())
    } else {
        format!(
            "{}/{} files",
            app.visible_document_count(),
            app.index.documents.len()
        )
    };
    let first = Line::from(vec![
        Span::styled(
            format!(" {selected} "),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  {count}  •  focus: {}  •  {}",
                app.focus.label(),
                app.status
            ),
            Style::default().fg(Color::Gray),
        ),
    ]);
    let second = if app.filter_editing {
        Line::from(vec![
            Span::styled(" / ", Style::default().fg(Color::Black).bg(Color::Yellow)),
            Span::styled(
                format!("{}█", app.filter),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(
                "  Enter accept  Esc close",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        let filter = if app.filter.is_empty() {
            String::new()
        } else {
            format!("  •  filter: /{}", app.filter)
        };
        Line::from(Span::styled(
            format!(
                " ↑↓/jk move or scroll  Enter open/toggle  Tab pane  / filter  PgUp/PgDn  r refresh  ? help  q quit{filter}"
            ),
            Style::default().fg(Color::DarkGray),
        ))
    };
    frame.render_widget(Paragraph::new(Text::from(vec![first, second])), area);
}

fn draw_help(frame: &mut Frame<'_>, area: Rect) {
    let popup = centered_rect(66, 20, area);
    frame.render_widget(Clear, popup);
    let help = Text::from(vec![
        Line::from(Span::styled(
            "Navigation",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  ↑/↓ or j/k       move files / scroll preview"),
        Line::from("  Enter / →        expand folder or open preview"),
        Line::from("  ←                collapse folder / select parent"),
        Line::from("  Tab              switch files and preview"),
        Line::from("  g / G            first/top and last/bottom"),
        Line::from("  PgUp / PgDn      scroll preview by a page"),
        Line::from(""),
        Line::from(Span::styled(
            "Workspace",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  /                filter paths and titles"),
        Line::from("  r                rescan now (changes are auto-watched)"),
        Line::from("  q / Esc          quit"),
        Line::from("  ?                close this help"),
    ]);
    let paragraph = Paragraph::new(help).alignment(Alignment::Left).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Help "),
    );
    frame.render_widget(paragraph, popup);
}

fn add_line_numbers(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let digits = lines.len().max(1).to_string().len();
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let Line {
                style,
                alignment,
                spans: old_spans,
            } = line;
            let mut spans = Vec::with_capacity(old_spans.len() + 1);
            spans.push(Span::styled(
                format!("{:>digits$} │ ", index + 1),
                Style::default().fg(Color::DarkGray),
            ));
            spans.extend(old_spans);
            let line = Line::from(spans).style(style);
            match alignment {
                Some(alignment) => line.alignment(alignment),
                None => line,
            }
        })
        .collect()
}

fn document_matches(document: &Document, query: &str) -> bool {
    query.is_empty()
        || document
            .relative_path
            .to_string_lossy()
            .to_lowercase()
            .contains(query)
        || document.title.to_lowercase().contains(query)
}

fn path_matches(path: &Path, query: &str) -> bool {
    query.is_empty() || path.to_string_lossy().to_lowercase().contains(query)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| path.display().to_string(), ToOwned::to_owned)
}

const fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let width = area
        .width
        .saturating_mul(percent_x)
        .saturating_div(100)
        .max(32);
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crossterm::event::{KeyEvent, KeyModifiers};
    use tempfile::tempdir;

    use super::*;

    fn fixture() -> (tempfile::TempDir, App) {
        let temp = tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("guide/deep")).expect("mkdir");
        fs::write(temp.path().join("README.md"), "# Home").expect("write root");
        fs::write(temp.path().join("guide/intro.md"), "# Introduction").expect("write intro");
        fs::write(temp.path().join("guide/deep/api.md"), "# API Reference").expect("write api");
        let app = App::new(temp.path(), TuiOptions::default()).expect("app");
        (temp, app)
    }

    #[test]
    fn tree_is_recursive_and_folders_start_expanded() {
        let (_temp, app) = fixture();
        let keys: Vec<_> = app.rows.iter().map(|row| row.key.clone()).collect();
        assert!(keys.contains(&NodeKey::Folder(PathBuf::from("guide"))));
        assert!(keys.contains(&NodeKey::Folder(PathBuf::from("guide/deep"))));
        assert!(keys.contains(&NodeKey::Document(PathBuf::from("guide/deep/api.md"))));
        assert!(app.collapsed.is_empty());
    }

    #[test]
    fn collapsing_a_folder_hides_descendants_and_keeps_folder_selected() {
        let (_temp, mut app) = fixture();
        app.selected = app
            .rows
            .iter()
            .position(|row| row.key == NodeKey::Folder(PathBuf::from("guide")))
            .expect("guide row");

        assert!(app.toggle_selected_folder());
        assert_eq!(
            app.selected_key(),
            Some(&NodeKey::Folder(PathBuf::from("guide")))
        );
        assert!(
            !app.rows
                .iter()
                .any(|row| { row.key == NodeKey::Document(PathBuf::from("guide/deep/api.md")) })
        );
    }

    #[test]
    fn filter_matches_titles_and_retains_ancestors() {
        let (_temp, mut app) = fixture();
        app.filter = "api reference".to_string();
        app.rebuild_rows(None);

        let keys: Vec<_> = app.rows.iter().map(|row| row.key.clone()).collect();
        assert_eq!(
            keys,
            vec![
                NodeKey::Folder(PathBuf::from("guide")),
                NodeKey::Folder(PathBuf::from("guide/deep")),
                NodeKey::Document(PathBuf::from("guide/deep/api.md")),
            ]
        );
    }

    #[test]
    fn navigation_is_clamped_and_tab_changes_focus() {
        let (_temp, mut app) = fixture();
        app.select_first();
        app.move_selection(-20);
        assert_eq!(app.selected, 0);
        app.move_selection(200);
        assert_eq!(app.selected, app.rows.len() - 1);

        assert_eq!(app.focus, Focus::Tree);
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .expect("tab");
        assert_eq!(app.focus, Focus::Preview);
    }

    #[test]
    fn filter_input_consumes_q_instead_of_quitting() {
        let (_temp, mut app) = fixture();
        app.filter_editing = true;
        let result = app
            .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
            .expect("key");
        assert_eq!(result, LoopControl::Continue);
        assert_eq!(app.filter, "q");
    }
}
