use std::{
    panic::catch_unwind,
    path::{Component, Path, PathBuf},
};

use ammonia::Builder;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use mermaid_svg::{Theme as MermaidTheme, render_with as render_mermaid_svg};
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd, html};
use pulldown_latex::{
    Parser as LatexParser, Storage,
    config::{DisplayMode, RenderConfig},
    mathml::push_mathml,
};

use super::is_mermaid_language;
use crate::document::{is_markdown, strip_frontmatter};

const MAX_MERMAID_SOURCE_BYTES: usize = 256 * 1024;

struct RenderSlot {
    marker: String,
    html: String,
}

#[must_use]
pub fn render_html(markdown: &str, current: &Path) -> String {
    let parser = Parser::new_ext(strip_frontmatter(markdown), markdown_options());
    let (events, slots) = rewrite_document_events(parser, current);
    let mut rendered = String::new();
    html::push_html(&mut rendered, events.into_iter());

    let mut sanitizer = Builder::default();
    sanitizer
        .add_tags(["input"])
        .add_generic_attributes(["class", "id"])
        .add_tag_attributes("input", ["type", "checked", "disabled"])
        .add_tag_attributes("code", ["class"])
        .url_relative(ammonia::UrlRelative::PassThrough);
    let mut sanitized = sanitizer.clean(&rendered).to_string();
    for slot in slots {
        sanitized = sanitized.replacen(&slot.marker, &slot.html, 1);
    }
    sanitized
}

fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_MATH
}

fn rewrite_document_events<'a>(
    parser: Parser<'a>,
    current: &Path,
) -> (Vec<Event<'a>>, Vec<RenderSlot>) {
    let mut parser = parser.into_iter();
    let mut events = Vec::new();
    let mut slots = Vec::new();

    while let Some(event) = parser.next() {
        match event {
            Event::InlineMath(source) => {
                events.push(slot_event(
                    &mut slots,
                    render_math(&source, DisplayMode::Inline),
                    false,
                ));
            }
            Event::DisplayMath(source) => {
                events.push(slot_event(
                    &mut slots,
                    render_math(&source, DisplayMode::Block),
                    false,
                ));
            }
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                if is_mermaid_language(&info) =>
            {
                let source = collect_code_block(&mut parser);
                events.push(slot_event(&mut slots, render_mermaid(&source), true));
            }
            event => events.push(rewrite_event(event, current)),
        }
    }

    (events, slots)
}

fn slot_event<'a>(slots: &mut Vec<RenderSlot>, replacement: String, block: bool) -> Event<'a> {
    let index = slots.len();
    let marker = if block {
        format!(r#"<div id="glow-render-slot-{index}"></div>"#)
    } else {
        format!(r#"<span id="glow-render-slot-{index}"></span>"#)
    };
    slots.push(RenderSlot {
        marker: marker.clone(),
        html: replacement,
    });
    Event::Html(CowStr::Boxed(marker.into_boxed_str()))
}

fn collect_code_block<'a>(parser: &mut impl Iterator<Item = Event<'a>>) -> String {
    let mut source = String::new();
    for event in parser.by_ref() {
        match event {
            Event::End(TagEnd::CodeBlock) => break,
            Event::Text(text)
            | Event::Code(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text) => source.push_str(&text),
            Event::SoftBreak | Event::HardBreak => source.push('\n'),
            _ => {}
        }
    }
    source
}

fn render_math(source: &str, display_mode: DisplayMode) -> String {
    let rendered = catch_unwind(|| {
        let storage = Storage::new();
        let parser = LatexParser::new(source, &storage);
        let mut mathml = String::new();
        let config = RenderConfig {
            display_mode,
            // The upstream renderer currently writes annotations without
            // escaping. The visible MathML is sufficient and is sanitized below.
            annotation: None,
            ..RenderConfig::default()
        };
        push_mathml(&mut mathml, parser, config).ok()?;
        let sanitized = sanitize_mathml(&mathml);
        sanitized.contains("<math").then_some(sanitized)
    });
    let Ok(Some(mathml)) = rendered else {
        return math_fallback(source, display_mode, "The formula could not be rendered.");
    };

    let class = match display_mode {
        DisplayMode::Inline => "math-inline",
        DisplayMode::Block => "math-display",
    };
    format!(r#"<span class="{class}">{mathml}</span>"#)
}

fn sanitize_mathml(mathml: &str) -> String {
    let mut sanitizer = Builder::empty();
    sanitizer
        .add_tags([
            "annotation",
            "maction",
            "math",
            "menclose",
            "merror",
            "mfrac",
            "mi",
            "mlabeledtr",
            "mmultiscripts",
            "mn",
            "mo",
            "mover",
            "mpadded",
            "mphantom",
            "mprescripts",
            "mroot",
            "mrow",
            "ms",
            "mspace",
            "msqrt",
            "mstyle",
            "msub",
            "msubsup",
            "msup",
            "mtable",
            "mtd",
            "mtext",
            "mtr",
            "munder",
            "munderover",
            "none",
            "semantics",
        ])
        .add_generic_attributes([
            "accent",
            "accentunder",
            "class",
            "close",
            "columnalign",
            "columnlines",
            "columnspacing",
            "columnspan",
            "columnwidth",
            "depth",
            "dir",
            "display",
            "displaystyle",
            "encoding",
            "fence",
            "form",
            "frame",
            "framespacing",
            "height",
            "largeop",
            "linethickness",
            "lspace",
            "mathbackground",
            "mathcolor",
            "mathsize",
            "mathvariant",
            "maxsize",
            "minsize",
            "movablelimits",
            "notation",
            "numalign",
            "open",
            "overflow",
            "rowalign",
            "rowlines",
            "rowspacing",
            "rowspan",
            "rspace",
            "scriptlevel",
            "scriptminsize",
            "scriptsizemultiplier",
            "selection",
            "separator",
            "separators",
            "stretchy",
            "symmetric",
            "width",
            "xmlns",
        ]);
    sanitizer.clean(mathml).to_string()
}

fn math_fallback(source: &str, display_mode: DisplayMode, message: &str) -> String {
    let source = html_escape::encode_text(source);
    let delimiter = match display_mode {
        DisplayMode::Inline => "$",
        DisplayMode::Block => "$$",
    };
    format!(
        r#"<span class="math-error" title="{}"><code>{delimiter}{source}{delimiter}</code></span>"#,
        html_escape::encode_double_quoted_attribute(message),
    )
}

fn render_mermaid(source: &str) -> String {
    if source.trim().is_empty() {
        return mermaid_error(source, "The Mermaid block is empty.");
    }
    if source.len() > MAX_MERMAID_SOURCE_BYTES {
        return mermaid_error(
            source,
            "This Mermaid block is larger than the 256 KiB rendering limit.",
        );
    }

    let light = render_mermaid_theme(source, false);
    let dark = render_mermaid_theme(source, true);
    match (light, dark) {
        (Ok(light), Ok(dark)) => mermaid_figure(source, &light, &dark),
        (Ok(svg), Err(_)) | (Err(_), Ok(svg)) => mermaid_figure(source, &svg, &svg),
        (Err(error), Err(_)) => mermaid_error(source, &short_error(&error.to_string())),
    }
}

fn render_mermaid_theme(source: &str, dark: bool) -> Result<String, String> {
    catch_unwind(|| {
        let theme = if dark {
            MermaidTheme::dark()
        } else {
            MermaidTheme::default_theme()
        };
        render_mermaid_svg(source, &theme)
    })
    .map_err(|_| "The Mermaid renderer stopped while parsing this diagram.".to_owned())?
    .map_err(|error| error.to_string())
}

fn mermaid_figure(source: &str, light_svg: &str, dark_svg: &str) -> String {
    let light = BASE64.encode(light_svg.as_bytes());
    let dark = BASE64.encode(dark_svg.as_bytes());
    let label = diagram_label(source);
    format!(
        r#"<figure class="diagram"><div class="diagram-viewport" role="img" aria-label="{}"><img class="diagram-image diagram-image-light" src="data:image/svg+xml;base64,{light}" alt="" loading="lazy" decoding="async"><img class="diagram-image diagram-image-dark" src="data:image/svg+xml;base64,{dark}" alt="" loading="lazy" decoding="async"></div>{}</figure>"#,
        html_escape::encode_double_quoted_attribute(&label),
        mermaid_source(source, false),
    )
}

fn mermaid_error(source: &str, message: &str) -> String {
    format!(
        r#"<figure class="diagram diagram-error"><figcaption><strong>Mermaid could not render this diagram.</strong><span>{}</span></figcaption>{}</figure>"#,
        html_escape::encode_text(message),
        mermaid_source(source, true),
    )
}

fn mermaid_source(source: &str, open: bool) -> String {
    let open = if open { " open" } else { "" };
    format!(
        r#"<details class="diagram-source"{open}><summary>Mermaid source</summary><pre><code class="language-mermaid">{}</code></pre></details>"#,
        html_escape::encode_text(source),
    )
}

fn diagram_label(source: &str) -> String {
    let header = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("%%"))
        .unwrap_or("diagram");
    format!("Mermaid diagram: {}", truncate_chars(header, 100))
}

fn short_error(error: &str) -> String {
    truncate_chars(error.lines().next().unwrap_or(error), 240)
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let mut output = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        output.push('…');
    }
    output
}

fn rewrite_event<'a>(mut event: Event<'a>, current: &Path) -> Event<'a> {
    match &mut event {
        Event::Start(Tag::Link { dest_url, .. }) => {
            *dest_url =
                CowStr::Boxed(rewrite_destination(dest_url, current, false).into_boxed_str());
        }
        Event::Start(Tag::Image { dest_url, .. }) => {
            *dest_url =
                CowStr::Boxed(rewrite_destination(dest_url, current, true).into_boxed_str());
        }
        Event::Html(raw) | Event::InlineHtml(raw) => {
            return Event::Text(CowStr::Boxed(raw.to_string().into_boxed_str()));
        }
        _ => {}
    }
    event
}

fn rewrite_destination(destination: &str, current: &Path, image: bool) -> String {
    let trimmed = destination.trim();
    if trimmed.starts_with('#')
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("mailto:")
        || trimmed.starts_with("tel:")
    {
        return trimmed.to_owned();
    }
    if trimmed.starts_with("//") || trimmed.contains('\0') {
        return "#".to_owned();
    }

    let (path_part, suffix) = split_suffix(trimmed);
    let decoded = percent_encoding::percent_decode_str(path_part)
        .decode_utf8_lossy()
        .into_owned();
    let base = current.parent().unwrap_or_else(|| Path::new(""));
    let Some(normalized) = normalize_relative(&base.join(decoded)) else {
        return "#".to_owned();
    };
    let encoded = encode_path(&normalized);
    if !image && is_markdown(&normalized) {
        format!("/doc/{encoded}{suffix}")
    } else {
        format!("/asset/{encoded}{suffix}")
    }
}

fn normalize_relative(path: &Path) -> Option<PathBuf> {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => output.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !output.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(output)
}

fn encode_path(path: &Path) -> String {
    path.components()
        .map(|component| {
            let text = component.as_os_str().to_string_lossy();
            url::form_urlencoded::byte_serialize(text.as_bytes())
                .collect::<String>()
                .replace('+', "%20")
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn split_suffix(destination: &str) -> (&str, &str) {
    destination
        .find(['?', '#'])
        .map_or((destination, ""), |index| destination.split_at(index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_relative_document_and_image_links() {
        let html = render_html(
            "[Next](../next.md#part) ![Logo](images/logo.png)",
            Path::new("guide/start.md"),
        );
        assert!(html.contains("href=\"/doc/next.md#part\""));
        assert!(html.contains("src=\"/asset/guide/images/logo.png\""));
    }

    #[test]
    fn blocks_root_escape_and_raw_html() {
        let html = render_html(
            "[secret](../../.env) <script>alert(1)</script>",
            Path::new("guide/start.md"),
        );
        assert!(html.contains("href=\"#\""));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn renders_inline_and_display_latex_as_sanitized_mathml() {
        let html = render_html(
            r#"Euler wrote $e^{i\pi} + 1 = 0$.

$$
\frac{-b \pm \sqrt{b^2 - 4ac}}{2a}
$$

$$
\begin{bmatrix} a & b \\ c & d \end{bmatrix}
$$"#,
            Path::new("paper.md"),
        );

        assert!(html.contains("class=\"math-inline\""));
        assert!(html.contains("class=\"math-display\""));
        assert!(html.contains("<math display=\"inline\""));
        assert!(html.contains("<math display=\"block\""));
        assert!(html.contains("<mfrac>"));
        assert!(html.contains("<mtable"), "{html}");
        assert!(!html.contains("glow-render-slot"));
    }

    #[test]
    fn math_never_reintroduces_untrusted_html() {
        let html = render_html(
            r#"$\text{<script>alert(1)</script>}$

$\definitelyUnknown{<img src=x onerror=alert(2)>}$"#,
            Path::new("unsafe.md"),
        );

        assert!(html.contains("<math"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("<img"));
        assert!(!html.contains("onerror"));
    }

    #[test]
    fn renders_mermaid_fences_to_isolated_theme_aware_svg_images() {
        let html = render_html(
            r#"```mermaid
flowchart LR
    Draft --> Review --> Publish
```"#,
            Path::new("architecture.md"),
        );

        assert!(html.contains("class=\"diagram\""));
        assert!(html.contains("diagram-image-light"));
        assert!(html.contains("diagram-image-dark"));
        assert_eq!(html.matches("data:image/svg+xml;base64,").count(), 2);
        assert!(html.contains("Mermaid source"));
        assert!(html.contains("Draft --&gt; Review --&gt; Publish"));
        assert!(!html.contains("glow-render-slot"));
    }

    #[test]
    fn supports_representative_mermaid_diagrams_and_charts() {
        let samples = [
            "sequenceDiagram\nAlice->>Bob: Hello",
            "pie showData\n\"Read\" : 70\n\"Write\" : 30",
            "xychart-beta\nx-axis [1, 2, 3]\ny-axis 0 --> 10\nline [2, 8, 5]",
            "gantt\ndateFormat YYYY-MM-DD\nsection Build\nShip : 2026-08-01, 5d",
            "erDiagram\nUSER ||--o{ NOTE : writes",
            "sankey-beta\nSource,Build,8\nBuild,Release,5",
            "radar-beta\naxis A, B, C\ncurve Team{8, 6, 9}\nmax 10",
        ];

        for source in samples {
            let html = render_mermaid(source);
            assert!(
                html.contains("data:image/svg+xml;base64,"),
                "failed to render {source}: {html}"
            );
            assert!(!html.contains("diagram-error"));
        }
    }

    #[test]
    fn invalid_mermaid_keeps_safe_copyable_source() {
        let html = render_html(
            r#"```mmd
not-a-diagram
<script>alert(1)</script>
```"#,
            Path::new("broken.md"),
        );

        assert!(html.contains("diagram-error"));
        assert!(html.contains("not-a-diagram"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("data:image/svg+xml;base64,"));
    }

    #[test]
    fn leaves_non_mermaid_code_fences_as_code() {
        let html = render_html("```rust\nfn main() {}\n```", Path::new("code.md"));
        assert!(html.contains("<pre><code class=\"language-rust\">"));
        assert!(!html.contains("class=\"diagram\""));
    }
}
