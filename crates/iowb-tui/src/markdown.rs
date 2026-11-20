//! Renders Markdown, and the Mermaid diagrams inside it, as styled terminal lines.

use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// A run of text with one style; a lone `"\n"` is a hard line break.
type Piece = (String, Style);

const MIN_WIDTH: usize = 8;
const MIN_CONTENT: usize = 4;
const MIN_COLUMN: usize = 4;

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// `streaming` is true while the text is still arriving: a Mermaid block whose
/// closing fence has not come yet stays as source instead of being drawn half-way.
pub fn render(source: &str, width: usize, streaming: bool) -> Vec<Line<'static>> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut renderer = Renderer::new(source, width.max(MIN_WIDTH), streaming);
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        renderer.event(event, range);
    }
    renderer.finish()
}

enum Indent {
    Quote,
    Item { marker: String, pending: bool },
}

struct Code {
    language: String,
    text: String,
}

#[derive(Default)]
struct Table {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Vec<Piece>>>,
    row: Vec<Vec<Piece>>,
    has_header: bool,
}

struct Renderer<'a> {
    source: &'a str,
    width: usize,
    streaming: bool,
    lines: Vec<Line<'static>>,
    last_blank: bool,
    inline: Vec<Piece>,
    styles: Vec<Style>,
    links: Vec<(String, usize)>,
    indent: Vec<Indent>,
    lists: Vec<Option<u64>>,
    code: Option<Code>,
    table: Option<Table>,
}

impl<'a> Renderer<'a> {
    fn new(source: &'a str, width: usize, streaming: bool) -> Self {
        Self {
            source,
            width,
            streaming,
            lines: Vec::new(),
            last_blank: true,
            inline: Vec::new(),
            styles: Vec::new(),
            links: Vec::new(),
            indent: Vec::new(),
            lists: Vec::new(),
            code: None,
            table: None,
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush();
        while self.last_blank && !self.lines.is_empty() {
            self.lines.pop();
            self.last_blank = self.lines.last().is_some_and(is_blank);
        }
        self.lines
    }

    // ---- output ------------------------------------------------------------

    fn prefix_width(&self) -> usize {
        self.indent
            .iter()
            .map(|entry| match entry {
                Indent::Quote => 2,
                Indent::Item { marker, .. } => marker.width(),
            })
            .sum()
    }

    fn content_width(&self) -> usize {
        self.width
            .saturating_sub(self.prefix_width())
            .max(MIN_CONTENT)
    }

    /// The quote bars and list markers in front of a line. A list marker is used
    /// once, on the first line of its item; later lines get matching blanks.
    fn prefix(&mut self, use_markers: bool) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for entry in &mut self.indent {
            match entry {
                Indent::Quote => spans.push(Span::styled("│ ", dim())),
                Indent::Item { marker, pending } => {
                    if *pending && use_markers {
                        spans.push(Span::raw(marker.clone()));
                        *pending = false;
                    } else {
                        spans.push(Span::raw(" ".repeat(marker.width())));
                    }
                }
            }
        }
        spans
    }

    fn emit(&mut self, content: Vec<Span<'static>>) {
        let mut spans = self.prefix(true);
        spans.extend(content);
        self.lines.push(Line::from(spans));
        self.last_blank = false;
    }

    /// One empty line between blocks, never two in a row and never first.
    fn blank(&mut self) {
        if self.last_blank {
            return;
        }
        let spans = self.prefix(false);
        self.lines.push(Line::from(spans));
        self.last_blank = true;
    }

    fn flush(&mut self) {
        let pieces = std::mem::take(&mut self.inline);
        if pieces.iter().all(|(text, _)| text.trim().is_empty()) {
            return;
        }
        let width = self.content_width();
        for spans in wrap_pieces(&pieces, width) {
            self.emit(spans);
        }
    }

    // ---- inline text ---------------------------------------------------------

    fn style(&self) -> Style {
        self.styles
            .iter()
            .fold(Style::default(), |all, style| all.patch(*style))
    }

    fn push_text(&mut self, text: &str) {
        let style = self.style();
        self.inline.push((text.to_string(), style));
    }

    fn push_dim(&mut self, text: &str) {
        let style = self.style().patch(dim());
        for (index, part) in text.trim_end_matches('\n').split('\n').enumerate() {
            if index > 0 {
                self.inline.push(("\n".into(), Style::default()));
            }
            self.inline.push((part.to_string(), style));
        }
    }

    // ---- events --------------------------------------------------------------

    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag, range),
            Event::Text(text) => match &mut self.code {
                Some(code) => code.text.push_str(&text),
                None => self.push_text(&text),
            },
            Event::Code(text) => {
                let style = self.style().patch(Style::new().fg(Color::Yellow));
                self.inline.push((text.to_string(), style));
            }
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.inline.push(("\n".into(), Style::default())),
            Event::Rule => {
                self.flush();
                let rule = "─".repeat(self.content_width());
                self.emit(vec![Span::styled(rule, dim())]);
                self.blank();
            }
            Event::TaskListMarker(done) => self.push_text(if done { "[x] " } else { "[ ] " }),
            Event::Html(text) | Event::InlineHtml(text) => self.push_dim(&text),
            Event::FootnoteReference(label) => self.push_text(&format!("[^{label}]")),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush();
                self.styles.push(heading_style(level));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.indent.push(Indent::Quote);
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(Code {
                    language,
                    text: String::new(),
                });
            }
            Tag::List(first) => {
                self.flush();
                self.lists.push(first);
            }
            Tag::Item => {
                self.flush();
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}. ");
                        *number += 1;
                        marker
                    }
                    _ => ["• ", "◦ ", "▪ "][depth.min(2)].to_string(),
                };
                self.indent.push(Indent::Item {
                    marker,
                    pending: true,
                });
            }
            Tag::Emphasis => self
                .styles
                .push(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.styles.push(Style::new().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self
                .styles
                .push(Style::new().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.styles.push(
                    Style::new()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::UNDERLINED),
                );
                self.links.push((dest_url.to_string(), self.inline.len()));
            }
            Tag::Image { .. } => self.push_text("[image: "),
            Tag::Table(aligns) => {
                self.flush();
                self.table = Some(Table {
                    aligns,
                    ..Table::default()
                });
            }
            Tag::HtmlBlock => self.flush(),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd, range: Range<usize>) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                self.blank();
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.styles.pop();
                self.blank();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.indent.pop();
                self.blank();
            }
            TagEnd::CodeBlock => self.finish_code(range),
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => {
                self.flush();
                self.indent.pop();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                if let Some((url, from)) = self.links.pop() {
                    let shown: String = self.inline[from.min(self.inline.len())..]
                        .iter()
                        .map(|(text, _)| text.as_str())
                        .collect();
                    let same =
                        shown.trim() == url || url.strip_prefix("mailto:") == Some(shown.trim());
                    if !url.is_empty() && !same {
                        self.inline.push((format!(" ({url})"), dim()));
                    }
                }
            }
            TagEnd::Image => self.push_text("]"),
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.render_table(table);
                    self.blank();
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                    table.has_header = true;
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                if let Some(table) = &mut self.table {
                    table.row.push(cell);
                }
            }
            TagEnd::HtmlBlock => self.flush(),
            _ => {}
        }
    }

    // ---- code and diagrams ---------------------------------------------------

    /// A block still being streamed has no closing fence yet; drawing a diagram
    /// from half of its source would only flicker.
    fn is_unfinished(&self, range: &Range<usize>) -> bool {
        let block = &self.source[range.clone()];
        let closed = block.lines().count() >= 2
            && block.lines().last().is_some_and(|line| {
                let line = line.trim();
                line.starts_with("```") || line.starts_with("~~~")
            });
        let last_block = self.source[range.end..].trim().is_empty();
        self.streaming && last_block && !closed
    }

    fn finish_code(&mut self, range: Range<usize>) {
        let Some(code) = self.code.take() else {
            return;
        };
        let text = code.text.trim_end_matches('\n').replace('\t', "    ");
        if code.language.eq_ignore_ascii_case("mermaid") && !self.is_unfinished(&range) {
            match diagram(&text, self.content_width()) {
                Ok(drawn) => {
                    let label = Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC);
                    self.emit(vec![Span::styled("mermaid", label)]);
                    for line in drawn.lines() {
                        self.emit(vec![Span::raw(line.trim_end().to_string())]);
                    }
                    self.blank();
                    return;
                }
                Err(reason) => {
                    let note = format!("mermaid diagram not drawn: {reason}");
                    for line in crate::text::wrap(&note, self.content_width()) {
                        self.emit(vec![Span::styled(line, dim())]);
                    }
                }
            }
        }
        let header = code.language;
        let bar = Span::styled("│ ", dim());
        let width = self.content_width().saturating_sub(2).max(1);
        let top = if header.is_empty() {
            "╭─".to_string()
        } else {
            format!("╭─ {header}")
        };
        self.emit(vec![Span::styled(top, dim())]);
        for line in text.split('\n') {
            for chunk in hard_wrap(line, width) {
                self.emit(vec![
                    bar.clone(),
                    Span::styled(chunk, Style::new().fg(Color::Yellow)),
                ]);
            }
        }
        self.emit(vec![Span::styled("╰─", dim())]);
        self.blank();
    }

    // ---- tables --------------------------------------------------------------

    fn render_table(&mut self, table: Table) {
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let plain_width = |cell: &Vec<Piece>| {
            cell.iter()
                .map(|(text, _)| text.replace('\n', " "))
                .collect::<String>()
                .width()
        };
        let mut widths = vec![1usize; columns];
        for row in &table.rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(plain_width(cell));
            }
        }
        // Narrow columns of a wide table: take space from the widest one first.
        let available = self.content_width();
        let mut total = widths.iter().sum::<usize>() + 3 * columns + 1;
        while total > available {
            let Some((widest, size)) = widths.iter().copied().enumerate().max_by_key(|(_, w)| *w)
            else {
                break;
            };
            if size <= MIN_COLUMN {
                break;
            }
            widths[widest] -= 1;
            total -= 1;
        }

        let border = |left: &str, middle: &str, right: &str| {
            let cells: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            Span::styled(format!("{left}{}{right}", cells.join(middle)), dim())
        };
        self.emit(vec![border("┌", "┬", "┐")]);
        let row_count = table.rows.len();
        for (row_index, row) in table.rows.iter().enumerate() {
            let header = table.has_header && row_index == 0;
            let wrapped: Vec<Vec<Vec<Span<'static>>>> = (0..columns)
                .map(|column| {
                    let pieces: Vec<Piece> = row
                        .get(column)
                        .map(|cell| {
                            cell.iter()
                                .map(|(text, style)| {
                                    let style = if header {
                                        style.patch(Style::new().add_modifier(Modifier::BOLD))
                                    } else {
                                        *style
                                    };
                                    (text.clone(), style)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut lines = wrap_pieces(&pieces, widths[column]);
                    if lines.is_empty() {
                        lines.push(Vec::new());
                    }
                    lines
                })
                .collect();
            let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
            for line_index in 0..height {
                let mut spans = vec![Span::styled("│", dim())];
                for (column, cell) in wrapped.iter().enumerate() {
                    let content = cell.get(line_index).cloned().unwrap_or_default();
                    let used: usize = content.iter().map(|span| span.content.width()).sum();
                    let spare = widths[column].saturating_sub(used);
                    let (left, right) = match table.aligns.get(column) {
                        Some(Alignment::Right) => (spare, 0),
                        Some(Alignment::Center) => (spare / 2, spare - spare / 2),
                        _ => (0, spare),
                    };
                    spans.push(Span::raw(" ".repeat(1 + left)));
                    spans.extend(content);
                    spans.push(Span::raw(" ".repeat(1 + right)));
                    spans.push(Span::styled("│", dim()));
                }
                self.emit(spans);
            }
            if header && row_index + 1 < row_count {
                self.emit(vec![border("├", "┼", "┤")]);
            }
        }
        self.emit(vec![border("└", "┴", "┘")]);
    }
}

fn is_blank(line: &Line<'_>) -> bool {
    line.spans
        .iter()
        .all(|span| span.content.chars().all(|ch| ch == ' ' || ch == '│'))
}

fn heading_style(level: HeadingLevel) -> Style {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    match level {
        HeadingLevel::H1 => bold.fg(Color::Magenta).add_modifier(Modifier::UNDERLINED),
        HeadingLevel::H2 => bold.fg(Color::Magenta),
        HeadingLevel::H3 => bold.fg(Color::Cyan),
        _ => bold,
    }
}

// ---- diagrams ----------------------------------------------------------------

/// Layer and node spacing for diagrams that do not fit at the default one.
const TIGHT_GAPS: Option<(usize, usize)> = Some((2, 1));

fn draw(source: &str, gaps: Option<(usize, usize)>) -> Result<String, mermaid_text::Error> {
    mermaid_text::render_with_options(
        source,
        &mermaid_text::RenderOptions {
            gaps_override: gaps,
            ..Default::default()
        },
    )
}

/// Draws a Mermaid diagram that fits `width` columns. The renderer's own width
/// limit is only a hint, so wider diagrams are retried with tighter spacing, then
/// (for a left-to-right flowchart) top-down, which is taller but narrower; if
/// nothing fits, the reason is returned.
fn diagram(source: &str, width: usize) -> Result<String, String> {
    let natural = draw(source, None).map_err(|error| error.to_string())?;
    let fits = |text: &str| text.lines().all(|line| line.width() <= width);
    if fits(&natural) {
        return Ok(natural);
    }
    if let Ok(drawn) = draw(source, TIGHT_GAPS)
        && fits(&drawn)
    {
        return Ok(drawn);
    }
    if let Some(vertical) = top_down(source)
        && let Ok(drawn) = draw(&vertical, TIGHT_GAPS)
        && fits(&drawn)
    {
        return Ok(drawn);
    }
    if let Ok(drawn) = mermaid_text::render_with_width(source, Some(width))
        && fits(&drawn)
    {
        return Ok(drawn);
    }
    let needed = natural
        .lines()
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0);
    Err(format!("it needs {needed} columns and {width} fit"))
}

/// The same flowchart declared top-down, when it was declared left-to-right.
fn top_down(source: &str) -> Option<String> {
    let mut out = String::with_capacity(source.len());
    let mut found = false;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if !found && !trimmed.is_empty() && !trimmed.starts_with("%%") {
            found = true;
            let (keyword, rest) = trimmed.split_once(char::is_whitespace)?;
            if keyword != "graph" && keyword != "flowchart" {
                return None;
            }
            let rest = rest.trim_start();
            let end = rest
                .find(|ch: char| !ch.is_ascii_alphabetic())
                .unwrap_or(rest.len());
            if !matches!(&rest[..end], "LR" | "RL") {
                return None;
            }
            out.push_str(&format!("{keyword} TD{}\n", &rest[end..]));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    found.then_some(out)
}

// ---- wrapping ------------------------------------------------------------------

/// Cuts `line` into pieces of at most `width` columns, ignoring word boundaries.
fn hard_wrap(line: &str, width: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    for ch in line.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width && used > 0 {
            chunks.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(ch);
        used += w;
    }
    chunks.push(current);
    chunks
}

struct Wrapper {
    width: usize,
    lines: Vec<Vec<(char, Style)>>,
    line: Vec<(char, Style)>,
    used: usize,
    word: Vec<(char, Style)>,
    word_width: usize,
    space: Option<Style>,
}

impl Wrapper {
    fn push(&mut self, ch: char, style: Style) {
        match ch {
            '\n' => {
                self.place();
                self.break_line();
            }
            ' ' | '\t' => {
                self.place();
                // Spaces at the start of a line are dropped.
                if self.used > 0 {
                    self.space = Some(style);
                }
            }
            _ => {
                self.word.push((ch, style));
                self.word_width += ch.width().unwrap_or(0);
            }
        }
    }

    fn break_line(&mut self) {
        self.lines.push(std::mem::take(&mut self.line));
        self.used = 0;
    }

    fn place(&mut self) {
        if self.word.is_empty() {
            return;
        }
        let word = std::mem::take(&mut self.word);
        let word_width = std::mem::take(&mut self.word_width);
        let space = self.space.take();
        if self.used > 0 {
            if self.used + usize::from(space.is_some()) + word_width <= self.width {
                if let Some(style) = space {
                    self.line.push((' ', style));
                    self.used += 1;
                }
                self.line.extend(word);
                self.used += word_width;
                return;
            }
            self.break_line();
        }
        if word_width <= self.width {
            self.line.extend(word);
            self.used += word_width;
            return;
        }
        // A word wider than the whole line is cut where it runs out.
        for (ch, style) in word {
            let w = ch.width().unwrap_or(0);
            if self.used + w > self.width && self.used > 0 {
                self.break_line();
            }
            self.line.push((ch, style));
            self.used += w;
        }
    }
}

/// Word-wraps styled text to `width` columns, keeping every character's style.
fn wrap_pieces(pieces: &[Piece], width: usize) -> Vec<Vec<Span<'static>>> {
    let mut wrapper = Wrapper {
        width: width.max(1),
        lines: Vec::new(),
        line: Vec::new(),
        used: 0,
        word: Vec::new(),
        word_width: 0,
        space: None,
    };
    for (text, style) in pieces {
        for ch in text.chars() {
            wrapper.push(ch, *style);
        }
    }
    wrapper.place();
    let last = std::mem::take(&mut wrapper.line);
    wrapper.lines.push(last);
    wrapper.lines.into_iter().map(into_spans).collect()
}

fn into_spans(cells: Vec<(char, Style)>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut style = Style::default();
    for (ch, next) in cells {
        if !text.is_empty() && next != style {
            spans.push(Span::styled(std::mem::take(&mut text), style));
        }
        style = next;
        text.push(ch);
    }
    if !text.is_empty() {
        spans.push(Span::styled(text, style));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    fn text(source: &str, width: usize) -> Vec<String> {
        plain(&render(source, width, false))
    }

    /// The style of the span that holds `word`.
    fn style_of(lines: &[Line<'_>], word: &str) -> Style {
        lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains(word))
            .unwrap_or_else(|| panic!("no span contains {word:?}"))
            .style
    }

    fn width_of(line: &str) -> usize {
        line.width()
    }

    #[test]
    fn paragraphs_wrap_with_one_blank_line_between_and_none_at_the_end() {
        assert_eq!(
            text("hello brave new world\n\nsecond", 11),
            ["hello brave", "new world", "", "second"]
        );
        assert!(text("", 20).is_empty());
    }

    #[test]
    fn inline_styles_survive_wrapping() {
        let lines = render(
            "plain **bold words that wrap** and `code` and ~~gone~~ *soft*",
            14,
            false,
        );
        assert!(
            style_of(&lines, "bold")
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            style_of(&lines, "wrap")
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(style_of(&lines, "code").fg == Some(Color::Yellow));
        assert!(
            style_of(&lines, "gone")
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
        assert!(
            style_of(&lines, "soft")
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert!(
            plain(&lines)
                .iter()
                .all(|line| !line.contains('*') && !line.contains('`'))
        );
    }

    #[test]
    fn headings_are_styled_without_their_hash_marks() {
        let lines = render("# Title\n\n## Part\n\ntext", 30, false);
        assert_eq!(plain(&lines), ["Title", "", "Part", "", "text"]);
        assert!(
            style_of(&lines, "Title")
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        assert!(
            style_of(&lines, "Part")
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn lists_nest_and_continuation_lines_line_up_under_the_text() {
        let source =
            "- first item that is long enough to wrap\n  - nested\n- second\n\n1. one\n2. two";
        assert_eq!(
            text(source, 20),
            [
                "• first item that is",
                "  long enough to",
                "  wrap",
                "  ◦ nested",
                "• second",
                "",
                "1. one",
                "2. two",
            ]
        );
    }

    #[test]
    fn task_lists_show_their_state() {
        assert_eq!(
            text("- [x] done\n- [ ] todo", 20),
            ["• [x] done", "• [ ] todo"]
        );
    }

    #[test]
    fn block_quotes_get_a_bar_on_every_line() {
        assert_eq!(
            text("> quoted text that wraps\n>\n> second", 12),
            ["│ quoted", "│ text that", "│ wraps", "│ ", "│ second"]
        );
    }

    #[test]
    fn links_show_their_address_unless_it_is_the_text() {
        assert_eq!(
            text("[docs](https://example.com/a)", 60),
            ["docs (https://example.com/a)"]
        );
        assert_eq!(
            text("<https://example.com/a>", 60),
            ["https://example.com/a"]
        );
    }

    #[test]
    fn code_blocks_are_boxed_labelled_and_not_parsed() {
        let lines = text("```rust\nlet **x** = 1;\n```", 40);
        assert_eq!(lines, ["╭─ rust", "│ let **x** = 1;", "╰─"]);
        assert_eq!(
            text("```\nabcdefghij\n```", 8),
            ["╭─", "│ abcdef", "│ ghij", "╰─"]
        );
    }

    #[test]
    fn rules_span_the_width() {
        assert_eq!(text("a\n\n---\n\nb", 10), ["a", "", "──────────", "", "b"]);
    }

    #[test]
    fn tables_are_drawn_as_a_grid_with_alignment() {
        let source = "| name | n |\n|:-----|--:|\n| a | 10 |\n| bb | 2 |";
        assert_eq!(
            text(source, 40),
            [
                "┌──────┬────┐",
                "│ name │  n │",
                "├──────┼────┤",
                "│ a    │ 10 │",
                "│ bb   │  2 │",
                "└──────┴────┘",
            ]
        );
        assert!(
            style_of(&render(source, 40, false), "name")
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn a_table_wider_than_the_view_wraps_its_cells() {
        let source = "| key | description |\n|---|---|\n| a | this description is far too long for one line |";
        let lines = text(source, 24);
        assert!(lines.iter().all(|line| width_of(line) <= 24), "{lines:#?}");
        assert!(lines.iter().filter(|line| line.contains("│ a")).count() == 1);
        assert!(lines.iter().any(|line| line.contains("description")));
        assert!(lines.iter().any(|line| line.contains("one line")));
    }

    #[test]
    fn wide_characters_wrap_by_display_width() {
        assert_eq!(text("日本語日本語", 8), ["日本語日", "本語"]);
    }

    #[test]
    fn a_flowchart_is_drawn_as_a_diagram() {
        let lines = text("```mermaid\ngraph LR; A[Build] --> B[Test]\n```", 60);
        assert_eq!(lines[0], "mermaid");
        let drawn = lines[1..].join("\n");
        assert!(drawn.contains("Build") && drawn.contains("Test"));
        assert!(drawn.contains('┌') && drawn.contains('▸'), "{drawn}");
        assert!(
            !drawn.contains("graph LR"),
            "the source is replaced, not repeated"
        );
    }

    #[test]
    fn a_sequence_diagram_is_drawn_too() {
        let source = "```mermaid\nsequenceDiagram\n  U->>S: hello\n  S-->>U: hi\n```";
        let drawn = text(source, 60).join("\n");
        assert!(drawn.contains("hello") && drawn.contains("hi"));
        assert!(drawn.contains('┌'));
    }

    #[test]
    fn a_wide_left_to_right_flowchart_is_redrawn_top_down_to_fit() {
        let source = "```mermaid\nflowchart LR\n  A[Parse request] --> B[Validate input] --> C[Load session] --> D[Call provider] --> E[Stream tokens]\n```";
        let lines = text(source, 50);
        assert!(lines.iter().all(|line| width_of(line) <= 50), "{lines:#?}");
        let drawn = lines.join("\n");
        assert!(
            drawn.contains("Validate input") && drawn.contains("Stream tokens"),
            "{drawn}"
        );
        assert!(!drawn.contains("not drawn"));
    }

    #[test]
    fn a_flowchart_that_is_a_little_too_wide_is_tightened_and_stays_left_to_right() {
        let source = "```mermaid\nflowchart LR\n  A[Parse request] --> B[Validate input] --> C[Load session] --> D[Call provider] --> E[Stream tokens]\n```";
        // 105 columns at the default spacing, 93 when tightened.
        let lines = text(source, 96);
        assert!(lines.iter().all(|line| width_of(line) <= 96), "{lines:#?}");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Parse request") && line.contains("Stream tokens")),
            "all five steps share a row: {lines:#?}"
        );
    }

    #[test]
    fn a_diagram_that_cannot_be_drawn_is_shown_as_source_with_the_reason() {
        let lines = text("```mermaid\nnot a diagram\n```", 60);
        assert!(
            lines[0].starts_with("mermaid diagram not drawn:"),
            "{lines:#?}"
        );
        assert_eq!(lines[1], "╭─ mermaid");
        assert!(lines.iter().any(|line| line == "│ not a diagram"));

        let narrow = text(
            "```mermaid\ngraph LR; A[A very long label indeed] --> B[Another very long label]\n```",
            12,
        );
        assert!(
            narrow.join(" ").contains("not drawn: it needs"),
            "{narrow:#?}"
        );
        assert!(
            narrow.iter().all(|line| width_of(line) <= 12),
            "{narrow:#?}"
        );
    }

    #[test]
    fn a_diagram_still_arriving_stays_as_source_until_its_fence_closes() {
        let open = "intro\n\n```mermaid\ngraph TD; A --> B";
        let streaming = plain(&render(open, 40, true));
        assert!(streaming.iter().any(|line| line.starts_with("╭─ mermaid")));
        assert!(streaming.iter().all(|line| !line.contains("not drawn")));
        assert!(streaming.iter().any(|line| line == "│ graph TD; A --> B"));

        let closed = format!("{open}\n```");
        let drawn = plain(&render(&closed, 40, true)).join("\n");
        assert!(
            drawn.contains('┌'),
            "a closed block is drawn while the rest streams: {drawn}"
        );

        // Finished messages never wait for a fence.
        let finished = plain(&render(open, 40, false)).join("\n");
        assert!(finished.contains('┌'));
    }

    #[test]
    fn partial_and_odd_input_never_panics_and_always_fits_the_width() {
        let document = "# Title\n\nSome **bold** text with `code` and a [link](http://x.y).\n\n- a\n  - b\n\n> quote\n\n| h1 | h2 |\n|:--|--:|\n| 日本 | 2 |\n\n```mermaid\ngraph TD\n  A[Start] --> B{Ok?}\n  B -->|yes| C[Done]\n```\n\n```mermaid\nsequenceDiagram\n  A->>B: hi\n```\n\n---\n\n1. one\n2. two\n";
        for width in [8, 20, 40, 80] {
            let mut cut = 0;
            while cut <= document.len() {
                if document.is_char_boundary(cut) {
                    for streaming in [false, true] {
                        let lines = render(&document[..cut], width, streaming);
                        // At the minimum width a table cannot shrink enough to fit.
                        if width >= 20 {
                            for line in plain(&lines) {
                                assert!(width_of(&line) <= width, "width {width}: {line:?}");
                            }
                        }
                    }
                }
                cut += 1;
            }
        }
    }
}
