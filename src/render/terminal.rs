use pulldown_cmark::{CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{Theme, is_mermaid_language};
use crate::document::strip_frontmatter;

#[derive(Clone, Copy, Debug)]
pub struct TerminalRenderOptions {
    pub width: usize,
    pub theme: Theme,
    pub line_numbers: bool,
    pub preserve_newlines: bool,
}

impl Default for TerminalRenderOptions {
    fn default() -> Self {
        Self {
            width: 80,
            theme: Theme::Auto,
            line_numbers: false,
            preserve_newlines: false,
        }
    }
}

#[must_use]
pub fn render_lines(input: &str, width: usize, theme: Theme) -> Vec<Line<'static>> {
    render_lines_with_options(
        input,
        TerminalRenderOptions {
            width,
            theme,
            ..TerminalRenderOptions::default()
        },
    )
}

#[must_use]
pub fn render_lines_with_options(
    input: &str,
    options: TerminalRenderOptions,
) -> Vec<Line<'static>> {
    let parser = Parser::new_ext(strip_frontmatter(input), markdown_options());
    let mut renderer = TerminalBuilder::new(options);
    for event in parser {
        renderer.event(event);
    }
    renderer.finish()
}

#[must_use]
pub fn render_ansi(input: &str, options: TerminalRenderOptions, color: bool) -> String {
    let lines = render_lines_with_options(input, options);
    let mut output = String::new();
    for line in lines {
        for span in line.spans {
            if color {
                output.push_str(&ansi_style(span.style));
            }
            output.push_str(&span.content);
            if color {
                output.push_str("\x1b[0m");
            }
        }
        output.push('\n');
    }
    output
}

fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_MATH
}

struct TerminalBuilder {
    options: TerminalRenderOptions,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<ListState>,
    quote_depth: usize,
    heading: Option<HeadingLevel>,
    code: Option<CodeCapture>,
    link: Option<String>,
    in_table: bool,
    table_cell: usize,
    table_head: bool,
}

struct ListState {
    next: Option<u64>,
}

struct CodeCapture {
    language: String,
    body: String,
}

impl TerminalBuilder {
    fn new(options: TerminalRenderOptions) -> Self {
        Self {
            options,
            lines: Vec::new(),
            current: Vec::new(),
            styles: vec![Style::default()],
            lists: Vec::new(),
            quote_depth: 0,
            heading: None,
            code: None,
            link: None,
            in_table: false,
            table_cell: 0,
            table_head: false,
        }
    }

    fn event(&mut self, event: Event<'_>) {
        if let Some(code) = &mut self.code {
            match event {
                Event::End(TagEnd::CodeBlock) => {
                    let code = self.code.take().expect("code capture exists");
                    self.render_code_block(&code.language, &code.body);
                }
                Event::Text(text) | Event::Code(text) => code.body.push_str(&text),
                Event::SoftBreak | Event::HardBreak => code.body.push('\n'),
                _ => {}
            }
            return;
        }

        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                let style = self.palette().inline_code;
                self.push_span(format!(" {code} "), style);
            }
            Event::SoftBreak => {
                if self.options.preserve_newlines {
                    self.flush_line();
                } else {
                    self.text(" ");
                }
            }
            Event::HardBreak => self.flush_line(),
            Event::Rule => {
                self.flush_nonempty();
                let width = self.options.width.clamp(8, 120);
                self.lines.push(Line::from(Span::styled(
                    "─".repeat(width),
                    self.palette().muted,
                )));
                self.blank_line();
            }
            Event::TaskListMarker(checked) => {
                self.push_span(if checked { "☑ " } else { "☐ " }, self.palette().accent);
            }
            Event::FootnoteReference(name) => {
                self.push_span(format!("[^{name}]"), self.palette().link);
            }
            Event::Html(html) | Event::InlineHtml(html) => self.text(&strip_html_hint(&html)),
            Event::InlineMath(math) => {
                // Terminals cannot typeset TeX. Keep the complete source and
                // its delimiters, while styling it distinctly from inline code.
                self.push_span(format!("${math}$"), self.palette().math_inline);
            }
            Event::DisplayMath(math) => self.render_display_math(&math),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.flush_nonempty();
                self.heading = Some(level);
                let palette = self.palette();
                self.styles.push(match level {
                    HeadingLevel::H1 => palette.heading1,
                    HeadingLevel::H2 => palette.heading2,
                    _ => palette.heading3,
                });
                let marker = match level {
                    HeadingLevel::H1 => "▌ ",
                    HeadingLevel::H2 => "◆ ",
                    _ => "› ",
                };
                self.push_span(marker, self.current_style());
            }
            Tag::BlockQuote(_) => {
                self.flush_nonempty();
                self.quote_depth += 1;
                self.styles.push(self.palette().quote);
            }
            Tag::CodeBlock(kind) => {
                self.flush_nonempty();
                let language = match kind {
                    CodeBlockKind::Indented => String::new(),
                    CodeBlockKind::Fenced(language) => language.into_string(),
                };
                self.code = Some(CodeCapture {
                    language,
                    body: String::new(),
                });
            }
            Tag::List(start) => {
                self.flush_nonempty();
                self.lists.push(ListState { next: start });
            }
            Tag::Item => {
                self.flush_nonempty();
                let indent = "  ".repeat(self.lists.len().saturating_sub(1));
                self.push_span(indent, Style::default());
                let marker = self
                    .lists
                    .last_mut()
                    .and_then(|list| {
                        list.next.map(|value| {
                            list.next = Some(value + 1);
                            format!("{value}. ")
                        })
                    })
                    .unwrap_or_else(|| "• ".to_owned());
                self.push_span(marker, self.palette().bullet);
            }
            Tag::Emphasis => self.styles.push(self.current_style().italic()),
            Tag::Strong => self.styles.push(self.current_style().bold()),
            Tag::Strikethrough => self
                .styles
                .push(self.current_style().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.link = Some(dest_url.to_string());
                self.styles.push(self.palette().link);
            }
            Tag::Image { dest_url, .. } => {
                self.link = Some(dest_url.to_string());
                self.styles.push(self.palette().image);
                self.push_span("🖼 ", self.palette().image);
            }
            Tag::Table(_) => {
                self.flush_nonempty();
                self.in_table = true;
            }
            Tag::TableHead => self.table_head = true,
            Tag::TableRow => self.table_cell = 0,
            Tag::TableCell => {
                if self.table_cell > 0 {
                    self.push_span(" │ ", self.palette().muted);
                }
                self.table_cell += 1;
            }
            Tag::FootnoteDefinition(name) => {
                self.flush_nonempty();
                self.push_span(format!("[^{name}] "), self.palette().link);
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_nonempty();
                if self.lists.is_empty() && !self.in_table {
                    self.blank_line();
                }
            }
            TagEnd::Heading(_) => {
                self.flush_nonempty();
                self.blank_line();
                self.heading = None;
                self.pop_style();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_nonempty();
                self.blank_line();
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.pop_style();
            }
            TagEnd::List(_) => {
                self.flush_nonempty();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank_line();
                }
            }
            TagEnd::Item => self.flush_nonempty(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_style(),
            TagEnd::Link => {
                self.pop_style();
                if let Some(target) = self.link.take()
                    && (target.starts_with("http://") || target.starts_with("https://"))
                {
                    self.push_span(" ↗", self.palette().muted);
                }
            }
            TagEnd::Image => {
                self.pop_style();
                if let Some(target) = self.link.take() {
                    self.push_span(format!(" ({target})"), self.palette().muted);
                }
            }
            TagEnd::Table => {
                self.flush_nonempty();
                self.in_table = false;
                self.blank_line();
            }
            TagEnd::TableHead => {
                self.flush_nonempty();
                self.table_head = false;
                self.lines.push(Line::from(Span::styled(
                    "─".repeat(self.options.width.clamp(8, 120)),
                    self.palette().muted,
                )));
            }
            TagEnd::TableRow => self.flush_nonempty(),
            TagEnd::FootnoteDefinition => {
                self.flush_nonempty();
                self.blank_line();
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        self.push_span(text.to_owned(), self.current_style());
    }

    fn push_span(&mut self, content: impl Into<CowStr<'static>>, style: Style) {
        let text = content.into().into_string();
        if self.current.is_empty() && self.quote_depth > 0 {
            let prefix = format!("{} ", "│".repeat(self.quote_depth));
            self.current
                .push(Span::styled(prefix, self.palette().quote_bar));
        }
        self.current.push(Span::styled(text, style));
    }

    fn current_style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn flush_nonempty(&mut self) {
        if !self.current.is_empty() {
            self.flush_line();
        }
    }

    fn flush_line(&mut self) {
        self.lines
            .push(Line::from(std::mem::take(&mut self.current)));
    }

    fn blank_line(&mut self) {
        if self.lines.last().is_some_and(|line| !line.spans.is_empty()) {
            self.lines.push(Line::default());
        }
    }

    fn render_code_block(&mut self, language: &str, body: &str) {
        if is_mermaid_language(language) {
            self.render_mermaid_block(body);
            return;
        }

        let palette = self.palette();
        let label = if language.trim().is_empty() {
            " code ".to_owned()
        } else {
            format!(" {} ", language.trim())
        };
        self.lines.push(Line::from(vec![
            Span::styled("╭─", palette.code_border),
            Span::styled(label, palette.code_label),
        ]));

        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let syntax = syntax_set
            .find_syntax_by_token(language.trim())
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
        let theme_name = if self.options.theme.resolved() == Theme::Light {
            "InspiredGitHub"
        } else {
            "base16-ocean.dark"
        };
        let syntax_theme = theme_set
            .themes
            .get(theme_name)
            .or_else(|| theme_set.themes.values().next());
        let digits = body.lines().count().max(1).to_string().len();

        if let Some(syntax_theme) = syntax_theme {
            let mut highlighter = HighlightLines::new(syntax, syntax_theme);
            for (index, source_line) in LinesWithEndings::from(body).enumerate() {
                let mut spans = vec![Span::styled("│ ", palette.code_border)];
                if self.options.line_numbers {
                    spans.push(Span::styled(
                        format!("{:>digits$} │ ", index + 1),
                        palette.code_number,
                    ));
                }
                match highlighter.highlight_line(source_line.trim_end_matches('\n'), &syntax_set) {
                    Ok(regions) => {
                        for (style, content) in regions {
                            spans.push(Span::styled(content.to_owned(), syntect_style(style)));
                        }
                    }
                    Err(_) => spans.push(Span::styled(source_line.to_owned(), palette.code)),
                }
                self.lines.push(Line::from(spans));
            }
        } else {
            for (index, source_line) in body.lines().enumerate() {
                let mut spans = vec![Span::styled("│ ", palette.code_border)];
                if self.options.line_numbers {
                    spans.push(Span::styled(
                        format!("{:>digits$} │ ", index + 1),
                        palette.code_number,
                    ));
                }
                spans.push(Span::styled(source_line.to_owned(), palette.code));
                self.lines.push(Line::from(spans));
            }
        }
        self.lines
            .push(Line::from(Span::styled("╰─", palette.code_border)));
        self.blank_line();
    }

    fn render_display_math(&mut self, source: &str) {
        self.flush_nonempty();
        let palette = self.palette();
        // Pulldown-cmark keeps the delimiter-adjacent newlines from the
        // conventional `$$\n…\n$$` form. They are syntax padding rather than
        // formula content, so omit one on each edge for a tighter source card.
        let source = source
            .strip_prefix("\r\n")
            .or_else(|| source.strip_prefix('\n'))
            .unwrap_or(source);
        let source = source
            .strip_suffix("\r\n")
            .or_else(|| source.strip_suffix('\n'))
            .unwrap_or(source);
        self.lines.push(Line::from(vec![
            Span::styled("╭─", palette.math_border),
            Span::styled(" LaTeX · source ", palette.math_label),
        ]));
        self.lines.push(Line::from(vec![
            Span::styled("│ ", palette.math_border),
            Span::styled("$$", palette.math_delimiter),
        ]));
        // `split` retains any intentional empty lines inside the formula.
        for source_line in source.split('\n') {
            self.lines.push(Line::from(vec![
                Span::styled("│ ", palette.math_border),
                Span::styled(source_line.to_owned(), palette.math_display),
            ]));
        }
        self.lines.push(Line::from(vec![
            Span::styled("│ ", palette.math_border),
            Span::styled("$$", palette.math_delimiter),
        ]));
        self.lines
            .push(Line::from(Span::styled("╰─", palette.math_border)));
        self.blank_line();
    }

    fn render_mermaid_block(&mut self, body: &str) {
        let palette = self.palette();
        self.lines.push(Line::from(vec![
            Span::styled("╭─", palette.diagram_border),
            Span::styled(" Mermaid diagram · source ", palette.diagram_label),
        ]));

        let digits = body.lines().count().max(1).to_string().len();
        for (index, source_line) in LinesWithEndings::from(body).enumerate() {
            let source_line = source_line.trim_end_matches(['\r', '\n']);
            let mut spans = vec![Span::styled("│ ", palette.diagram_border)];
            if self.options.line_numbers {
                spans.push(Span::styled(
                    format!("{:>digits$} │ ", index + 1),
                    palette.code_number,
                ));
            }
            spans.extend(highlight_mermaid_line(source_line, palette));
            self.lines.push(Line::from(spans));
        }

        self.lines
            .push(Line::from(Span::styled("╰─", palette.diagram_border)));
        self.blank_line();
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        if let Some(code) = self.code.take() {
            self.render_code_block(&code.language, &code.body);
        }
        self.flush_nonempty();
        while self.lines.last().is_some_and(|line| line.spans.is_empty()) {
            self.lines.pop();
        }
        wrap_lines(self.lines, self.options.width.max(8))
    }

    fn palette(&self) -> Palette {
        Palette::new(self.options.theme.resolved())
    }
}

#[derive(Clone, Copy)]
struct Palette {
    accent: Style,
    heading1: Style,
    heading2: Style,
    heading3: Style,
    muted: Style,
    bullet: Style,
    quote: Style,
    quote_bar: Style,
    inline_code: Style,
    code: Style,
    code_border: Style,
    code_label: Style,
    code_number: Style,
    link: Style,
    image: Style,
    math_inline: Style,
    math_display: Style,
    math_delimiter: Style,
    math_border: Style,
    math_label: Style,
    diagram_source: Style,
    diagram_keyword: Style,
    diagram_edge: Style,
    diagram_comment: Style,
    diagram_border: Style,
    diagram_label: Style,
}

impl Palette {
    fn new(theme: Theme) -> Self {
        let (text, muted, code_bg) = if theme == Theme::Light {
            (
                Color::Rgb(42, 39, 45),
                Color::Rgb(112, 105, 116),
                Color::Rgb(239, 237, 240),
            )
        } else {
            (
                Color::Rgb(232, 228, 235),
                Color::Rgb(135, 127, 141),
                Color::Rgb(37, 34, 40),
            )
        };
        let green = Color::Rgb(4, 181, 117);
        let fuchsia = Color::Rgb(238, 111, 248);
        let yellow = Color::Rgb(236, 253, 101);
        Self {
            accent: Style::default().fg(green).bold(),
            heading1: Style::default().fg(fuchsia).bold(),
            heading2: Style::default().fg(green).bold(),
            heading3: Style::default().fg(yellow).bold(),
            muted: Style::default().fg(muted),
            bullet: Style::default().fg(fuchsia).bold(),
            quote: Style::default().fg(text).italic(),
            quote_bar: Style::default().fg(green).bold(),
            inline_code: Style::default().fg(yellow).bg(code_bg),
            code: Style::default().fg(text).bg(code_bg),
            code_border: Style::default().fg(Color::Rgb(87, 81, 92)),
            code_label: Style::default().fg(green).bold(),
            code_number: Style::default().fg(muted).bg(code_bg),
            link: Style::default().fg(Color::Rgb(89, 174, 255)).underlined(),
            image: Style::default().fg(fuchsia).italic(),
            math_inline: Style::default().fg(fuchsia).bg(code_bg),
            math_display: Style::default().fg(text).bg(code_bg),
            math_delimiter: Style::default().fg(fuchsia).bg(code_bg).bold(),
            math_border: Style::default().fg(Color::Rgb(87, 81, 92)),
            math_label: Style::default().fg(fuchsia).bold(),
            diagram_source: Style::default().fg(text).bg(code_bg),
            diagram_keyword: Style::default().fg(green).bg(code_bg).bold(),
            diagram_edge: Style::default().fg(fuchsia).bg(code_bg).bold(),
            diagram_comment: Style::default().fg(muted).bg(code_bg).italic(),
            diagram_border: Style::default().fg(Color::Rgb(87, 81, 92)),
            diagram_label: Style::default().fg(green).bold(),
        }
    }
}

fn highlight_mermaid_line(source: &str, palette: Palette) -> Vec<Span<'static>> {
    let trimmed = source.trim_start();
    if trimmed.starts_with("%%") {
        return vec![Span::styled(source.to_owned(), palette.diagram_comment)];
    }

    let leading_bytes = source.len() - trimmed.len();
    let keyword_bytes = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let keyword = &trimmed[..keyword_bytes];
    let mut spans = Vec::new();
    let remaining = if is_mermaid_keyword(keyword) {
        if leading_bytes > 0 {
            spans.push(Span::styled(
                source[..leading_bytes].to_owned(),
                palette.diagram_source,
            ));
        }
        spans.push(Span::styled(keyword.to_owned(), palette.diagram_keyword));
        &source[leading_bytes + keyword_bytes..]
    } else {
        source
    };
    spans.extend(highlight_mermaid_edges(remaining, palette));
    spans
}

fn is_mermaid_keyword(token: &str) -> bool {
    matches!(
        token,
        "flowchart"
            | "graph"
            | "sequenceDiagram"
            | "classDiagram"
            | "stateDiagram"
            | "stateDiagram-v2"
            | "erDiagram"
            | "journey"
            | "gantt"
            | "pie"
            | "mindmap"
            | "timeline"
            | "gitGraph"
            | "quadrantChart"
            | "xychart-beta"
            | "xychart"
            | "sankey-beta"
            | "sankey"
            | "block-beta"
            | "block"
            | "packet-beta"
            | "packet"
            | "architecture-beta"
            | "architecture"
            | "radar-beta"
            | "radar"
            | "kanban"
            | "treemap-beta"
            | "treemap"
            | "requirementDiagram"
            | "C4Context"
            | "C4Container"
            | "C4Component"
            | "C4Dynamic"
            | "C4Deployment"
            | "zenuml"
            | "subgraph"
            | "end"
            | "participant"
            | "actor"
            | "note"
            | "loop"
            | "alt"
            | "else"
            | "opt"
            | "par"
            | "and"
            | "rect"
            | "critical"
            | "break"
            | "activate"
            | "deactivate"
            | "title"
            | "section"
            | "classDef"
            | "style"
            | "linkStyle"
            | "click"
    )
}

fn highlight_mermaid_edges(source: &str, palette: Palette) -> Vec<Span<'static>> {
    const OPERATORS: &[&str] = &[
        "<-->", "-.->", "-->>", "<<--", "==>", "-->", "---", "->>", "--", "->",
    ];

    let mut spans = Vec::new();
    let mut rest = source;
    while !rest.is_empty() {
        let next = OPERATORS
            .iter()
            .filter_map(|operator| rest.find(operator).map(|index| (index, *operator)))
            .min_by_key(|(index, operator)| (*index, usize::MAX - operator.len()));
        let Some((index, operator)) = next else {
            spans.push(Span::styled(rest.to_owned(), palette.diagram_source));
            break;
        };
        if index > 0 {
            spans.push(Span::styled(
                rest[..index].to_owned(),
                palette.diagram_source,
            ));
        }
        spans.push(Span::styled(operator.to_owned(), palette.diagram_edge));
        rest = &rest[index + operator.len()..];
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), palette.diagram_source));
    }
    spans
}

fn syntect_style(style: syntect::highlighting::Style) -> Style {
    let mut output = Style::default()
        .fg(Color::Rgb(
            style.foreground.r,
            style.foreground.g,
            style.foreground.b,
        ))
        .bg(Color::Rgb(
            style.background.r,
            style.background.g,
            style.background.b,
        ));
    if style.font_style.contains(FontStyle::BOLD) {
        output = output.bold();
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        output = output.italic();
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        output = output.underlined();
    }
    output
}

fn wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let mut output = Vec::new();
    for line in lines {
        let total_width: usize = line
            .spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum();
        if total_width <= width || line.spans.is_empty() {
            output.push(line);
            continue;
        }

        let mut current = Vec::new();
        let mut current_width = 0;
        for span in line.spans {
            let mut chunk = String::new();
            for grapheme in span.content.graphemes(true) {
                let grapheme_width = UnicodeWidthStr::width(grapheme);
                if current_width + grapheme_width > width && current_width > 0 {
                    if !chunk.is_empty() {
                        current.push(Span::styled(std::mem::take(&mut chunk), span.style));
                    }
                    output.push(Line::from(std::mem::take(&mut current)));
                    current_width = 0;
                }
                chunk.push_str(grapheme);
                current_width += grapheme_width;
            }
            if !chunk.is_empty() {
                current.push(Span::styled(chunk, span.style));
            }
        }
        output.push(Line::from(current));
    }
    output
}

fn ansi_style(style: Style) -> String {
    let mut codes = Vec::new();
    if style.add_modifier.contains(Modifier::BOLD) {
        codes.push("1".to_owned());
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        codes.push("3".to_owned());
    }
    if style.add_modifier.contains(Modifier::UNDERLINED) {
        codes.push("4".to_owned());
    }
    if style.add_modifier.contains(Modifier::CROSSED_OUT) {
        codes.push("9".to_owned());
    }
    if let Some(color) = style.fg {
        push_color_code(&mut codes, color, false);
    }
    if let Some(color) = style.bg {
        push_color_code(&mut codes, color, true);
    }
    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}

fn push_color_code(codes: &mut Vec<String>, color: Color, background: bool) {
    let prefix = if background { 48 } else { 38 };
    match color {
        Color::Rgb(r, g, b) => codes.push(format!("{prefix};2;{r};{g};{b}")),
        Color::Indexed(index) => codes.push(format!("{prefix};5;{index}")),
        _ => {}
    }
}

fn strip_html_hint(input: &str) -> String {
    let mut output = String::new();
    let mut inside_tag = false;
    for character in input.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => output.push(character),
            _ => {}
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_headings_lists_and_code() {
        let rendered = render_ansi(
            "# Hello\n\n- one\n- two\n\n```rust\nfn main() {}\n```",
            TerminalRenderOptions {
                width: 80,
                theme: Theme::Dark,
                line_numbers: true,
                preserve_newlines: false,
            },
            false,
        );
        assert!(rendered.contains("▌ Hello"));
        assert!(rendered.contains("• one"));
        assert!(rendered.contains("fn main() {}"));
        assert!(rendered.contains("1 │"));
    }

    #[test]
    fn wraps_cjk_by_display_width() {
        let lines = render_lines("你好世界你好世界", 8, Theme::Dark);
        assert!(lines.len() >= 2);
        assert!(lines.iter().all(|line| line.width() <= 8));
    }

    #[test]
    fn preserves_and_styles_inline_and_display_latex_source() {
        let markdown = "Energy is $E = mc^2$.\n\n$$\n\\frac{1}{n} \\sum_{i=1}^{n} x_i\n$$";
        let lines = render_lines(markdown, 100, Theme::Dark);
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Energy is $E = mc^2$."));
        assert!(rendered.contains("╭─ LaTeX · source"));
        assert!(rendered.contains("│ $$"));
        assert!(rendered.contains(r"\frac{1}{n} \sum_{i=1}^{n} x_i"));
        assert_eq!(rendered.matches("│ $$").count(), 2);

        let inline = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains("$E = mc^2$"))
            .expect("inline math span");
        assert_eq!(inline.style, Palette::new(Theme::Dark).math_inline);
    }

    #[test]
    fn renders_mermaid_as_highlighted_source_without_losing_diagram_text() {
        let markdown = "```mermaid\n%% a faithful terminal fallback\nflowchart TD\n  A[Start] --> B{Done?}\n```";
        let lines = render_lines_with_options(
            markdown,
            TerminalRenderOptions {
                width: 120,
                theme: Theme::Dark,
                line_numbers: true,
                preserve_newlines: false,
            },
        );
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Mermaid diagram · source"));
        assert!(rendered.contains("1 │ %% a faithful terminal fallback"));
        assert!(rendered.contains("2 │ flowchart TD"));
        assert!(rendered.contains("3 │   A[Start] --> B{Done?}"));

        let palette = Palette::new(Theme::Dark);
        assert!(
            lines.iter().flat_map(|line| &line.spans).any(|span| {
                span.content == "flowchart" && span.style == palette.diagram_keyword
            })
        );
        assert!(
            lines
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| { span.content == "-->" && span.style == palette.diagram_edge })
        );
        assert!(lines.iter().flat_map(|line| &line.spans).any(|span| {
            span.content.starts_with("%%") && span.style == palette.diagram_comment
        }));
    }

    #[test]
    fn recognizes_common_mermaid_fence_aliases() {
        for language in [
            "mermaid",
            "Mermaid",
            "mmd",
            "mermaidjs",
            "mermaid-js",
            "{.mermaid}",
            "language-mermaid",
        ] {
            assert!(is_mermaid_language(language), "{language}");
        }
        assert!(!is_mermaid_language("javascript"));
    }
}
