use chrono::{DateTime, Utc};
use iowb_protocol::{ChatMessage, MessageRole};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    activity::{self, Part, Tool},
    app::{App, Chat, HeadersEditor, LoginForm, Rewind, Screen},
    icons, markdown,
    switcher::{Load, Row, Switcher},
    text::{truncate, wrap},
    toolview::{self, View},
};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const MAX_INPUT_LINES: usize = 6;
const MESSAGE_LINES: usize = 3;

pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen {
        Screen::Login => {
            draw_login(frame, app);
            if let Some(editor) = &app.login.editor {
                draw_headers_editor(frame, editor);
            }
        }
        Screen::Chat => draw_chat(frame, app),
    }
    if let Some(switcher) = &app.switcher {
        draw_switcher(frame, app, switcher);
    }
    if let Some(rewind) = &app.rewind {
        draw_rewind(frame, rewind);
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// The last `width` columns of `text`, so a long value shows what is being typed.
fn tail(text: &str, width: usize) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    while chars.iter().collect::<String>().width() > width && !chars.is_empty() {
        chars.remove(0);
    }
    chars.into_iter().collect()
}

pub fn relative_time(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    let seconds = (now - then).num_seconds().max(0);
    match seconds {
        0..=59 => "now".into(),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        86_400..=8_639_999 => format!("{}d", seconds / 86_400),
        _ => then.format("%Y-%m-%d").to_string(),
    }
}

// ---- login ------------------------------------------------------------------

fn draw_login(frame: &mut Frame, app: &App) {
    let area = centered(frame.area(), 64, 15);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" io-workbench ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let login = &app.login;
    let label_width = 10;
    let value_width = (inner.width as usize).saturating_sub(label_width + 1);
    // Only the names: header values are secrets and are shown in the editor alone.
    let header_names = if login.headers.is_empty() {
        "none".to_string()
    } else {
        login
            .headers
            .iter()
            .map(|header| header.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let fields = [
        ("Server", login.server.clone()),
        ("Username", login.username.clone()),
        ("Password", "•".repeat(login.password.chars().count())),
        ("Headers", truncate(&header_names, value_width)),
    ];

    let mut lines = vec![
        Line::from("Sign in to your io-workbench server").bold(),
        Line::default(),
    ];
    for (index, (label, value)) in fields.iter().enumerate() {
        let focused = index == login.focus && !login.busy;
        let label_style = if focused {
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        };
        let value_style = if focused {
            Style::new().add_modifier(Modifier::UNDERLINED)
        } else {
            Style::new()
        };
        lines.push(Line::from(vec![
            ratatui::text::Span::styled(format!("{label:<label_width$}"), label_style),
            ratatui::text::Span::styled(tail(value, value_width), value_style),
        ]));
    }
    lines.push(Line::default());
    // Messages such as a proxy redirect are long, so they wrap over up to three lines.
    let message = match (&login.error, &login.status) {
        (Some(error), _) => Some((error, Color::Red)),
        (None, Some(status)) => Some((status, Color::Yellow)),
        (None, None) => None,
    };
    let mut shown = 0;
    if let Some((text, color)) = message {
        let wrapped = wrap(text, inner.width as usize);
        let cut = wrapped.len() > MESSAGE_LINES;
        for (index, line) in wrapped.into_iter().take(MESSAGE_LINES).enumerate() {
            let line = if cut && index == MESSAGE_LINES - 1 {
                format!(
                    "{}…",
                    truncate(&line, (inner.width as usize).saturating_sub(1))
                )
            } else {
                line
            };
            lines.push(Line::from(line).fg(color));
            shown += 1;
        }
    }
    lines.extend((shown..MESSAGE_LINES).map(|_| Line::default()));
    lines.push(Line::default());
    let hint = if login.focus == LoginForm::HEADERS {
        "Tab next field · Enter edit headers · Esc quit"
    } else {
        "Tab next field · Enter sign in · Esc quit"
    };
    lines.push(Line::from(hint).dim());
    frame.render_widget(Paragraph::new(lines), inner);

    if !login.busy && login.focus < LoginForm::HEADERS {
        let shown = tail(&fields[login.focus].1, value_width);
        frame.set_cursor_position((
            inner.x + (label_width + shown.width()) as u16,
            inner.y + 2 + login.focus as u16,
        ));
    }
}

fn draw_headers_editor(frame: &mut Frame, editor: &HeadersEditor) {
    let area = centered(frame.area(), 76, 16);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Custom headers ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [intro, list, error, hints] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new(
            "Sent with every request to the server, for example the service token of an access proxy.",
        )
        .wrap(Wrap { trim: true })
        .dim(),
        intro,
    );

    let name_width = 28;
    let value_width = (list.width as usize).saturating_sub(2 + name_width + 2);
    let visible = (list.height as usize).max(1);
    let first = (editor.row + 1).saturating_sub(visible);
    let cell = |text: &str, placeholder: &str, width: usize, focused: bool| {
        let style = if focused {
            Style::new()
                .fg(Color::Cyan)
                .add_modifier(Modifier::UNDERLINED)
        } else {
            Style::new()
        };
        let (shown, style) = if text.is_empty() {
            (
                placeholder.to_string(),
                Style::new().add_modifier(Modifier::DIM),
            )
        } else {
            (tail(text, width), style)
        };
        let padding = " ".repeat(width.saturating_sub(shown.width()));
        ratatui::text::Span::styled(format!("{shown}{padding}"), style)
    };
    let lines: Vec<Line> = editor
        .rows
        .iter()
        .enumerate()
        .skip(first)
        .take(visible)
        .map(|(index, row)| {
            let current = index == editor.row;
            Line::from(vec![
                ratatui::text::Span::raw(if current { "▸ " } else { "  " }),
                cell(
                    &row.name,
                    "Header name",
                    name_width,
                    current && !editor.on_value,
                ),
                ratatui::text::Span::raw("  "),
                cell(
                    &row.value,
                    "Header value",
                    value_width,
                    current && editor.on_value,
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list);

    if let Some(message) = &editor.error {
        frame.render_widget(
            Paragraph::new(Line::from(truncate(message, error.width as usize)).red()),
            error,
        );
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("Tab name/value · ↑/↓ row · Ctrl+N add · Ctrl+D remove").dim(),
            Line::from("Ctrl+F Cloudflare Access · Enter save · Esc cancel").dim(),
        ]),
        hints,
    );

    let (text, offset) = match editor.rows.get(editor.row) {
        Some(row) if editor.on_value => (row.value.as_str(), 2 + name_width + 2),
        Some(row) => (row.name.as_str(), 2),
        None => ("", 2),
    };
    let width = if editor.on_value {
        value_width
    } else {
        name_width
    };
    let typed = tail(text, width).width();
    frame.set_cursor_position((
        list.x + (offset + typed) as u16,
        list.y + (editor.row - first) as u16,
    ));
}

// ---- chat -------------------------------------------------------------------

fn draw_chat(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chat = &app.chat;
    let input_width = (area.width as usize).saturating_sub(2).max(1);
    let input_lines = chat
        .input
        .layout(input_width)
        .rows
        .len()
        .clamp(1, MAX_INPUT_LINES) as u16;
    let [header, body, input, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(input_lines + 2),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, app, header);
    draw_messages(frame, app, body);
    draw_input(frame, app, input);
    draw_footer(frame, app, footer);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let chat = &app.chat;
    let text = match &chat.session {
        Some(session) => {
            let title = if session.title.is_empty() {
                &session.id
            } else {
                &session.title
            };
            // What the reader must not miss comes first; the rest is cut off on narrow terminals.
            let mut parts = vec![title.to_string()];
            parts.push(match chat.phase() {
                Some(phase) => format!("{} {phase}", SPINNER[app.tick % SPINNER.len()]),
                None => "idle".to_string(),
            });
            if chat.scroll > 0 {
                parts.push("scrolled, Ctrl+End for latest".into());
            }
            parts.push(session.provider.as_str().to_string());
            parts.extend(
                [&session.model, &session.effort, &session.mode]
                    .into_iter()
                    .flatten()
                    .filter(|value| !value.is_empty())
                    .cloned(),
            );
            parts.push(format!("{} msgs", chat.total_count));
            format!(" {}", parts.join("  ·  "))
        }
        None if chat.loading => " Opening chat…".to_string(),
        None => " No chat open — press Ctrl+K to choose one".to_string(),
    };
    let width = area.width as usize;
    let padded = format!("{:<width$}", truncate(&text, width));
    frame.render_widget(Paragraph::new(Line::from(padded).reversed()), area);
}

/// What a message you sent looks like: light text on a grey band the width of the
/// chat. The band takes the place of a "you" label, and the assistant's replies
/// need no label either, since they are the text that is not on a band.
fn user_style() -> Style {
    Style::new().fg(Color::Indexed(252)).bg(Color::Indexed(237))
}

fn user_block(content: &str, width: usize) -> Vec<Line<'static>> {
    wrap(content, width.saturating_sub(4).max(1))
        .into_iter()
        .map(|line| {
            let padding = width.saturating_sub(2 + line.width());
            Line::from(format!("  {line}{}", " ".repeat(padding))).style(user_style())
        })
        .collect()
}

fn push_message(lines: &mut Vec<Line<'static>>, message: &ChatMessage, width: usize) {
    match message.role {
        MessageRole::User => lines.extend(user_block(&message.content, width)),
        MessageRole::Assistant => lines.extend(markdown_lines(&message.content, width, false)),
        _ => {
            for line in wrap(&message.content, width.saturating_sub(2)) {
                lines.push(Line::from(format!("  {line}")).yellow());
            }
        }
    }
    lines.push(Line::default());
}

/// Tool calls the way the Claude Code and Codex terminals show them, each result
/// next to the call it answers. `running` means the last call may still be waiting.
fn tool_blocks(
    tools: Vec<Tool>,
    width: usize,
    chat: &Chat,
    tick: usize,
    running: bool,
) -> Vec<Line<'static>> {
    let blocks = activity::pair(tools);
    let last = blocks.len().saturating_sub(1);
    let spinner = SPINNER[tick % SPINNER.len()];
    blocks
        .iter()
        .enumerate()
        .flat_map(|(index, block)| {
            let view = View {
                width,
                expanded: chat.show_tools,
                running: running && index == last,
                spinner,
            };
            toolview::render(block, &view)
        })
        .collect()
}

/// One empty line before the next kind of content, unless there already is one.
fn separate(lines: &mut Vec<Line<'static>>) {
    let blank = |line: &Line<'static>| line.spans.iter().all(|span| span.content.trim().is_empty());
    if lines.last().is_some_and(|line| !blank(line)) {
        lines.push(Line::default());
    }
}

/// A reply that is still arriving: its text as Markdown and its tool sections as
/// tool blocks, in the order they came.
fn live_lines(text: &str, width: usize, chat: &Chat, tick: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut tools: Vec<Tool> = Vec::new();
    for part in activity::split_live(text) {
        match part {
            Part::Tool(tool) => tools.push(tool),
            Part::Prose(prose) => {
                if !tools.is_empty() {
                    separate(&mut lines);
                    lines.extend(tool_blocks(
                        std::mem::take(&mut tools),
                        width,
                        chat,
                        tick,
                        false,
                    ));
                }
                separate(&mut lines);
                lines.extend(markdown_lines(&prose, width, true));
            }
        }
    }
    if !tools.is_empty() {
        separate(&mut lines);
        lines.extend(tool_blocks(tools, width, chat, tick, chat.running));
    }
    lines
}

/// Replies are Markdown (with Mermaid diagrams), shown under the role label.
fn markdown_lines(text: &str, width: usize, streaming: bool) -> Vec<Line<'static>> {
    markdown::render(text, width.saturating_sub(2), streaming)
        .into_iter()
        .map(|mut line| {
            line.spans.insert(0, Span::raw("  "));
            line
        })
        .collect()
}

/// A prompt the user sent, and the line of the chat where it starts.
struct Prompt {
    line: usize,
    text: String,
}

/// The first line of a prompt, marked when there is more of it.
fn prompt_summary(content: &str) -> String {
    let mut lines = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or("").to_string();
    if lines.next().is_some() {
        format!("{first} …")
    } else {
        first
    }
}

struct ChatView {
    lines: Vec<Line<'static>>,
    prompts: Vec<Prompt>,
}

/// Servers start a streamed reply with the provider's name on a line of its own
/// (the stored message does not have it). The header already says who is talking.
fn without_provider_header(text: &str) -> &str {
    let is_header = |line: &str| {
        matches!(
            line.trim().to_lowercase().as_str(),
            "claude" | "codex" | "gemini" | "assistant" | "response"
        )
    };
    match text.split_once('\n') {
        Some((first, rest)) if is_header(first) => rest,
        None if is_header(text) => "",
        _ => text,
    }
}

fn chat_view(chat: &Chat, width: usize, tick: usize) -> ChatView {
    let mut lines = Vec::new();
    let mut prompts = Vec::new();
    if chat.older_count() > 0 {
        lines.push(
            Line::from(format!(
                "{} {} older messages — press o to load",
                icons::OLDER,
                chat.older_count()
            ))
            .dim(),
        );
        lines.push(Line::default());
    }
    let mut index = 0;
    while index < chat.messages.len() {
        let message = &chat.messages[index];
        if message.role == MessageRole::Tool {
            let run = chat.messages[index..]
                .iter()
                .take_while(|next| next.role == MessageRole::Tool)
                .count();
            let tools = chat.messages[index..index + run]
                .iter()
                .map(|tool| Tool::from_message(&tool.content, &tool.metadata))
                .collect();
            separate(&mut lines);
            lines.extend(tool_blocks(tools, width, chat, tick, false));
            index += run;
            continue;
        }
        if message.role == MessageRole::User {
            prompts.push(Prompt {
                line: lines.len(),
                text: prompt_summary(&message.content),
            });
        }
        push_message(&mut lines, message, width);
        index += 1;
    }
    let live = without_provider_header(&chat.live);
    if chat.running || !live.is_empty() {
        if live.is_empty() {
            let phase = chat.phase().unwrap_or("Processing");
            lines.push(Line::from(format!("  {} {phase}…", SPINNER[tick % SPINNER.len()])).dim());
        } else {
            lines.extend(live_lines(live, width, chat, tick));
        }
    } else if chat.messages.is_empty() && chat.session.is_some() && !chat.loading {
        lines.push(Line::from("No messages yet — write one below.").dim());
    }
    ChatView { lines, prompts }
}

fn draw_messages(frame: &mut Frame, app: &App, area: Rect) {
    let chat = &app.chat;
    let strip = area;
    let area = Rect::new(
        area.x + 1,
        area.y,
        area.width.saturating_sub(2),
        area.height,
    );
    let ChatView { lines, prompts } = chat_view(chat, area.width as usize, app.tick);
    let view = area.height as usize;
    let max_scroll = lines.len().saturating_sub(view);
    chat.max_scroll.set(max_scroll);
    chat.view_height.set(view);
    let end = lines.len() - chat.scroll.min(max_scroll);
    let start = end.saturating_sub(view);

    // Once a prompt has scrolled off the top, it stays pinned there so the text
    // below it is always read against the question it answers. It changes as the
    // view moves past each earlier prompt. When a prompt's own line is the top
    // line it is already on screen, so nothing is pinned.
    let pinned = if prompts.iter().any(|prompt| prompt.line == start) {
        None
    } else {
        prompts.iter().rev().find(|prompt| prompt.line < start)
    };
    match pinned {
        Some(prompt) if area.height >= 3 => {
            let width = strip.width as usize;
            let text = truncate(&format!(" {} {}", icons::PIN, prompt.text), width);
            let bar = Line::from(format!("{text:<width$}")).style(user_style());
            let bar_area = Rect::new(strip.x, area.y, strip.width, 1);
            frame.render_widget(Paragraph::new(bar), bar_area);
            // The pinned bar takes the place of the top line.
            let rest = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
            frame.render_widget(Paragraph::new(lines[start + 1..end].to_vec()), rest);
        }
        _ => frame.render_widget(Paragraph::new(lines[start..end].to_vec()), area),
    }
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let chat = &app.chat;
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" message — Enter send · Alt+Enter newline ")
        .border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    chat.input_width.set(inner.width as usize);
    let layout = chat.input.layout(inner.width.max(1) as usize);
    // When the text is taller than the box, show the rows around the cursor.
    let visible = (inner.height as usize).max(1);
    let first = (layout.cursor_row + 1).saturating_sub(visible);
    let body: Vec<Line> = if chat.input.is_empty() {
        vec![Line::from("Write a message…").dim()]
    } else {
        layout
            .rows
            .iter()
            .skip(first)
            .take(visible)
            .map(|(_, row)| Line::from(row.clone()))
            .collect()
    };
    frame.render_widget(Paragraph::new(body), inner);
    if app.switcher.is_none() && app.rewind.is_none() {
        let x = (layout.cursor_col as u16).min(inner.width.saturating_sub(1));
        frame.set_cursor_position((inner.x + x, inner.y + (layout.cursor_row - first) as u16));
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let (status, color) = if app.ws_connected {
        (format!("{} connected", icons::CONNECTED), Color::Green)
    } else if app.ws_detail.is_empty() {
        (format!("{} connecting…", icons::DISCONNECTED), Color::Red)
    } else {
        (
            format!(
                "{} reconnecting — {}",
                icons::DISCONNECTED,
                truncate(&app.ws_detail, 40)
            ),
            Color::Red,
        )
    };
    let room = (area.width as usize).saturating_sub(status.width() + 2);
    let left = match &app.notice {
        Some((text, _)) => Line::from(truncate(text, room)).yellow(),
        None => {
            let hints = if app.chat.running {
                "Esc stop · Ctrl+O tools · Ctrl+K chats · PgUp/PgDn scroll"
            } else {
                "Esc Esc rewind · Ctrl+O tools · Ctrl+K chats · PgUp/PgDn scroll"
            };
            Line::from(truncate(hints, room)).dim()
        }
    };
    frame.render_widget(Paragraph::new(left), area);
    frame.render_widget(
        Paragraph::new(Line::from(status).style(Style::new().fg(color)))
            .alignment(Alignment::Right),
        area,
    );
}

// ---- switcher ---------------------------------------------------------------

fn row_item(app: &App, switcher: &Switcher, row: &Row, width: usize) -> ListItem<'static> {
    let now = Utc::now();
    let line = match row {
        Row::Header(name) => {
            return ListItem::new(Line::from(name.to_string()).bold());
        }
        Row::Note(text) => {
            return ListItem::new(Line::from(format!("    {text}")).dim().italic());
        }
        Row::Pinned(index) => match &switcher.pinned {
            Load::Ready(pinned) => {
                let entry = &pinned[*index];
                let title = if entry.title.is_empty() {
                    &entry.session_id
                } else {
                    &entry.title
                };
                let context = [entry.project.as_str(), entry.provider.as_str()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                format!("  {} {title}  {context}", icons::PIN)
            }
            _ => String::new(),
        },
        Row::Project(index) => match switcher.nodes() {
            Some(nodes) => {
                let node = &nodes[*index];
                let folder = if node.expanded {
                    icons::FOLDER_OPEN
                } else {
                    icons::FOLDER
                };
                let count = if node.loaded {
                    format!(" ({})", node.total)
                } else {
                    String::new()
                };
                let busy = if node.loading { " …" } else { "" };
                format!("  {folder} {}{count}{busy}", node.project.name)
            }
            None => String::new(),
        },
        Row::Session(project, session) => match switcher.nodes() {
            Some(nodes) => {
                let session = &nodes[*project].sessions[*session];
                let marker = if app.chat.current_id.as_deref() == Some(session.id.as_str()) {
                    icons::CHAT_CURRENT
                } else {
                    icons::CHAT
                };
                let title = if session.title.is_empty() {
                    &session.id
                } else {
                    &session.title
                };
                format!(
                    "    {marker} {title}  {} · {} · {} msgs",
                    session.provider.as_str(),
                    relative_time(now, session.last_activity),
                    session.message_count
                )
            }
            None => String::new(),
        },
        Row::More(index) => match switcher.nodes() {
            Some(nodes) => {
                let node = &nodes[*index];
                format!(
                    "      {} load more ({} of {})",
                    icons::MORE,
                    node.sessions.len(),
                    node.total
                )
            }
            None => String::new(),
        },
    };
    ListItem::new(Line::from(truncate(&line, width)))
}

fn draw_switcher(frame: &mut Frame, app: &App, switcher: &Switcher) {
    let full = frame.area();
    let area = centered(
        full,
        (full.width * 4 / 5).max(40),
        (full.height * 4 / 5).max(12),
    );
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Switch chat ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [filter_area, list_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    let filter_line = if switcher.filter.is_empty() {
        Line::from("> type to filter").dim()
    } else {
        Line::from(format!("> {}", switcher.filter))
    };
    frame.render_widget(Paragraph::new(filter_line), filter_area);
    let x = filter_area.x + 2 + switcher.filter.width() as u16;
    frame.set_cursor_position((x.min(filter_area.right().saturating_sub(1)), filter_area.y));

    let rows = switcher.rows();
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| row_item(app, switcher, row, list_area.width as usize))
        .collect();
    let mut state = ListState::default();
    state.select(
        rows.get(switcher.cursor)
            .filter(|row| row.selectable())
            .map(|_| switcher.cursor),
    );
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        list_area,
        &mut state,
    );

    frame.render_widget(
        Paragraph::new(
            Line::from("↑↓ move · ←→ fold · Enter select · type to filter · Esc close").dim(),
        ),
        hint_area,
    );
}

/// The list a second Esc opens: earlier prompts, newest first.
fn draw_rewind(frame: &mut Frame, rewind: &Rewind) {
    let full = frame.area();
    let area = centered(
        full,
        (full.width * 4 / 5).max(40),
        (full.height * 3 / 5).max(10),
    );
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Rewind to an earlier prompt ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [note, list, hint] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new(
            "Starts a new chat from everything before the prompt you pick and puts that prompt in the box to edit. This chat stays as it is.",
        )
        .wrap(Wrap { trim: true })
        .dim(),
        note,
    );
    let items: Vec<ListItem> = rewind
        .prompts
        .iter()
        .map(|prompt| {
            let text = format!("  {}", prompt_summary(&prompt.text));
            ListItem::new(Line::from(truncate(&text, list.width as usize)))
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(rewind.cursor));
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        list,
        &mut state,
    );
    let hint_text = if rewind.busy {
        "Making the new chat…"
    } else {
        "↑/↓ move · Enter rewind to before it · Esc cancel"
    };
    frame.render_widget(Paragraph::new(Line::from(hint_text).dim()), hint);
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use iowb_protocol::{ProjectSessionsPage, ProjectSummary, Provider, SessionSummary};
    use ratatui::{Terminal, backend::TestBackend};
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::{
        activity::Class,
        api::PinnedSession,
        app::{Msg, Screen},
        config::Config,
        headers::Header,
        switcher::ProjectNode,
    };

    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app() -> App {
        let (tx, _rx) = unbounded_channel();
        App::new(
            tx,
            Config::default(),
            Some("http://127.0.0.1:8100".into()),
            false,
        )
    }

    fn message(id: &str, role: MessageRole, content: &str) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            role,
            content: content.into(),
            timestamp: Utc::now(),
            metadata: serde_json::Value::Null,
        }
    }

    fn open_chat() -> App {
        let mut app = app();
        app.screen = Screen::Chat;
        app.ws_connected = true;
        app.chat.current_id = Some("s1".into());
        app.chat.session = Some(SessionSummary {
            id: "s1".into(),
            title: "Fix login".into(),
            provider: Provider::Codex,
            model: Some("gpt-test".into()),
            ..Default::default()
        });
        app
    }

    #[test]
    fn relative_times_are_short() {
        let now = Utc::now();
        assert_eq!(relative_time(now, now - Duration::seconds(5)), "now");
        assert_eq!(relative_time(now, now - Duration::minutes(7)), "7m");
        assert_eq!(relative_time(now, now - Duration::hours(3)), "3h");
        assert_eq!(relative_time(now, now - Duration::days(2)), "2d");
    }

    #[test]
    fn the_login_form_masks_the_password() {
        let mut app = app();
        app.login.username = "ana".into();
        app.login.password = "hunter2".into();
        app.login.error = Some("Invalid credentials".into());
        let screen = render(&app, 80, 24);
        assert!(screen.contains("Sign in to your io-workbench server"));
        assert!(screen.contains("http://127.0.0.1:8100") && screen.contains("ana"));
        assert!(screen.contains("•••••••") && !screen.contains("hunter2"));
        assert!(screen.contains("Invalid credentials"));
    }

    #[test]
    fn the_provider_name_that_starts_a_stream_is_not_shown_twice() {
        assert_eq!(without_provider_header("claude\nStreaming "), "Streaming ");
        assert_eq!(without_provider_header("Codex\nhi"), "hi");
        assert_eq!(without_provider_header("claude"), "");
        assert_eq!(
            without_provider_header("Claude is great\nyes"),
            "Claude is great\nyes"
        );
        assert_eq!(without_provider_header("plain text"), "plain text");

        let mut app = open_chat();
        app.chat.running = true;
        app.chat.live = "codex\nStreaming".into();
        let screen = render(&app, 80, 14);
        assert!(screen.contains("Streaming"));
        // Only the top bar names the provider; a leaked header line would be a second.
        assert_eq!(screen.matches("codex").count(), 1);
    }

    #[test]
    fn the_login_form_lists_header_names_but_never_their_values() {
        let mut app = app();
        assert!(render(&app, 80, 24).contains("Headers   none"));

        app.login.headers = vec![
            Header {
                name: "CF-Access-Client-Id".into(),
                value: "id-value".into(),
            },
            Header {
                name: "CF-Access-Client-Secret".into(),
                value: "s3cret-value".into(),
            },
        ];
        let screen = render(&app, 80, 24);
        assert!(screen.contains("CF-Access-Client-Id, CF-Access-Client-Secret"));
        assert!(!screen.contains("id-value") && !screen.contains("s3cret-value"));

        app.login.focus = LoginForm::HEADERS;
        assert!(render(&app, 80, 24).contains("Enter edit headers"));
    }

    #[test]
    fn the_headers_editor_shows_rows_hints_and_errors() {
        let mut app = app();
        app.login.headers = vec![Header {
            name: "X-Team".into(),
            value: "blue".into(),
        }];
        app.login.focus = LoginForm::HEADERS;
        app.update(Msg::Key(ratatui::crossterm::event::KeyEvent::from(
            ratatui::crossterm::event::KeyCode::Enter,
        )));
        let editor = app.login.editor.as_mut().expect("the editor opens");
        editor.rows.push(Header::blank());
        editor.error = Some("Header 2 needs a name.".into());
        let screen = render(&app, 100, 30);
        assert!(screen.contains("Custom headers") && screen.contains("every request"));
        assert!(screen.contains("▸ X-Team") && screen.contains("blue"));
        assert!(screen.contains("Header name") && screen.contains("Header value"));
        assert!(screen.contains("Header 2 needs a name."));
        assert!(screen.contains("Ctrl+F Cloudflare Access") && screen.contains("Esc cancel"));
    }

    #[test]
    fn a_long_login_error_wraps_instead_of_being_cut_off() {
        let mut app = app();
        app.login.error = Some(
            "the server redirected to team.cloudflareaccess.com (HTTP 302); a sign-in page or proxy is in front of it. If it is Cloudflare Access, add its service token under Custom headers."
                .into(),
        );
        let screen = render(&app, 100, 24);
        assert!(screen.contains("team.cloudflareaccess.com"));
        assert!(screen.contains("under Custom headers."));
    }

    #[test]
    fn the_chat_shows_history_streaming_text_and_a_collapsed_tool_call() {
        let mut app = open_chat();
        app.chat.total_count = 5;
        app.chat.messages = vec![
            message("1", MessageRole::User, "please fix the login bug"),
            tool_message("2", "Bash", TOOL_CALL),
            message(
                "3",
                MessageRole::Assistant,
                "Done. The token was not saved.",
            ),
        ];
        app.chat.live = "Working on a second look".into();
        app.chat.running = true;
        let screen = render(&app, 100, 24);
        assert!(
            screen.contains("Fix login") && screen.contains("codex") && screen.contains("gpt-test")
        );
        assert!(screen.contains("please fix the login bug"));
        assert!(screen.contains("$ cargo test -p web"));
        assert!(
            !screen.contains("exec / Parameters"),
            "the raw heading is not shown"
        );
        assert!(screen.contains("Done. The token was not saved."));
        assert!(screen.contains("Working on a second look"));
        assert!(screen.contains("2 older messages — press o to load"));
        assert!(screen.contains(&format!("{} connected", icons::CONNECTED)));
    }

    #[test]
    fn scrolling_up_hides_the_newest_lines() {
        let mut app = open_chat();
        app.chat.total_count = 30;
        app.chat.messages = (0..30)
            .map(|i| {
                message(
                    &i.to_string(),
                    MessageRole::User,
                    &format!("line number {i}"),
                )
            })
            .collect();
        let bottom = render(&app, 60, 14);
        assert!(bottom.contains("line number 29") && !bottom.contains("line number 0\n"));
        app.chat.scroll = 1_000;
        let top = render(&app, 60, 14);
        assert!(top.contains("line number 0") && !top.contains("line number 29"));
        assert!(top.contains("scrolled"));
    }

    #[test]
    fn the_switcher_lists_pins_and_projects_with_paged_sessions() {
        let mut app = open_chat();
        let mut switcher = Switcher::new();
        switcher.pinned = Load::Ready(vec![PinnedSession {
            session_id: "p1".into(),
            title: "Pinned thing".into(),
            project: "web".into(),
            provider: "codex".into(),
        }]);
        let now = Utc::now();
        let mut node = ProjectNode::new(ProjectSummary {
            id: "p-web".into(),
            name: "web".into(),
            path: "/work/web".into(),
            repo_name: None,
            created_at: now,
            updated_at: now,
            sessions: Vec::new(),
        });
        node.expanded = true;
        node.absorb(
            0,
            ProjectSessionsPage {
                sessions: vec![SessionSummary {
                    id: "s1".into(),
                    title: "Fix login".into(),
                    provider: Provider::Codex,
                    message_count: 7,
                    last_activity: now - Duration::minutes(5),
                    ..Default::default()
                }],
                has_more: true,
                total_count: 120,
            },
        );
        switcher.projects = Load::Ready(vec![node]);
        switcher.clamp();
        app.switcher = Some(switcher);
        let screen = render(&app, 100, 30);
        assert!(screen.contains("Switch chat"));
        assert!(screen.contains(&format!("{} Pinned thing", icons::PIN)));
        assert!(!screen.contains('★'), "the pin icon replaced the star");
        assert!(
            !screen.contains('📌'),
            "the emoji draws an empty box without an emoji font"
        );
        assert!(screen.contains("web · codex"));
        assert!(screen.contains(&format!("{} web (120)", icons::FOLDER_OPEN)));
        assert!(
            screen.contains(&format!("{} Fix login", icons::CHAT_CURRENT))
                && screen.contains("5m · 7 msgs")
        );
        assert!(screen.contains("load more (1 of 120)"));
        assert!(screen.contains("Esc close"));
    }

    /// Tool sections exactly as the server writes them into chat text.
    const TOOL_CALL: &str = "exec / Parameters\n**Tool:** `Bash`\n- **ID:** `toolu_1`\n\n### Command\n```sh\ncargo test -p web\n```";
    const TOOL_RESULT: &str = "exec / Details\n**Tool:** `Bash`\n- **ID:** `toolu_1`\n- **Exit code:** `1`\n\n```text\nerror: test failed\nsecond line of output\nthird line of output\n```";

    fn tool_message(id: &str, name: &str, content: &str) -> ChatMessage {
        let mut message = message(id, MessageRole::Tool, content);
        message.metadata = serde_json::json!({"kind": "tool_output", "toolName": name});
        message
    }

    #[test]
    fn a_stored_command_and_its_failing_result_are_drawn_as_one_block() {
        let mut app = open_chat();
        app.chat.total_count = 3;
        app.chat.messages = vec![
            message("1", MessageRole::User, "run the tests"),
            tool_message("2", "Bash", TOOL_CALL),
            tool_message("3", "Bash", TOOL_RESULT),
        ];
        let screen = render(&app, 100, 24);
        let terminal = icons::tool(Class::Exec);
        assert!(
            screen.contains(&format!("{terminal} Bash  exit 1")),
            "{screen}"
        );
        assert!(screen.contains("$ cargo test -p web"), "{screen}");
        assert!(screen.contains("└ error: test failed"), "{screen}");
        assert!(
            screen.contains("second line of output") && screen.contains("third line of output")
        );
        assert!(
            !screen.contains("Parameters") && !screen.contains("Details"),
            "one block, not two"
        );
        assert!(!screen.contains("toolu_1"));
    }

    #[test]
    fn long_output_is_cut_after_three_lines_and_t_shows_all_of_it() {
        let result = "exec / Details\n**Tool:** `Bash`\n- **ID:** `toolu_1`\n\n```text\na1\na2\na3\na4\na5\na6\n```";
        let mut app = open_chat();
        app.chat.total_count = 2;
        app.chat.messages = vec![
            tool_message("1", "Bash", TOOL_CALL),
            tool_message("2", "Bash", result),
        ];
        let collapsed = render(&app, 100, 24);
        assert!(
            collapsed.contains("a3") && !collapsed.contains("a4"),
            "{collapsed}"
        );
        assert!(collapsed.contains("… +3 lines (t to expand)"));

        app.chat.show_tools = true;
        let expanded = render(&app, 100, 24);
        assert!(
            expanded.contains("a6") && !expanded.contains("t to expand"),
            "{expanded}"
        );
    }

    #[test]
    fn a_stored_file_edit_shows_its_diff_and_how_much_changed() {
        let edit = "tool / Parameters\n**Tool:** `Edit`\n- **ID:** `t9`\n\n```json\n{\"file_path\": \"src/main.rs\", \"old_string\": \"old line\", \"new_string\": \"new line\\nsecond\"}\n```";
        let mut app = open_chat();
        app.chat.total_count = 1;
        app.chat.messages = vec![tool_message("1", "Edit", edit)];
        let screen = render(&app, 100, 20);
        let pencil = icons::tool(Class::Edit);
        assert!(
            screen.contains(&format!("{pencil} Edit  src/main.rs  +2 -1")),
            "{screen}"
        );
        assert!(
            screen.contains("└ - old line")
                && screen.contains("+ new line")
                && screen.contains("+ second")
        );
    }

    #[test]
    fn a_streaming_reply_shows_tool_blocks_between_its_text() {
        let mut app = open_chat();
        app.chat.running = true;
        app.chat.live = format!(
            "claude\nLet me check the tests.\n\n{TOOL_CALL}\n\n{TOOL_RESULT}\n\nclaude\nThey fail on one assertion."
        );
        let screen = render(&app, 100, 24);
        let at = |needle: &str| {
            screen
                .find(needle)
                .unwrap_or_else(|| panic!("{needle:?} missing:\n{screen}"))
        };
        assert!(
            at("Let me check the tests.") < at("Bash")
                && at("Bash") < at("$ cargo test -p web")
                && at("$ cargo test -p web") < at("└ error: test failed")
                && at("└ error: test failed") < at("They fail on one assertion.")
        );
        assert!(!screen.contains("Tool: Bash") && !screen.contains("exec / Parameters"));
        assert!(
            !screen.contains(&format!("{} Bash", SPINNER[0])),
            "the call finished, so its icon does not spin"
        );
    }

    #[test]
    fn a_call_still_waiting_for_its_result_shows_a_spinner() {
        let mut app = open_chat();
        app.chat.running = true;
        app.chat.live = format!("claude\nWorking on it.\n\n{TOOL_CALL}");
        let screen = render(&app, 100, 20);
        assert!(
            screen.contains(&format!("{} Bash", SPINNER[0]))
                && screen.contains("$ cargo test -p web"),
            "{screen}"
        );
        app.chat.running = false;
        let stopped = render(&app, 100, 20);
        assert!(
            stopped.contains(&format!("{} Bash", icons::tool(Class::Exec))),
            "{stopped}"
        );
    }

    /// The colours of the cells of one screen row.
    fn row_backgrounds(app: &App, width: u16, height: u16, containing: &str) -> Vec<Color> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let text_of = |y: u16| -> String {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        };
        let y = (0..height)
            .find(|y| text_of(*y).contains(containing))
            .unwrap_or_else(|| panic!("no row contains {containing:?}"));
        (1..width - 1).map(|x| buffer[(x, y)].bg).collect()
    }

    #[test]
    fn what_you_send_sits_on_a_grey_band_and_nothing_carries_a_name_label() {
        let mut app = open_chat();
        app.chat.total_count = 2;
        app.chat.messages = vec![
            message("1", MessageRole::User, "please fix the login bug"),
            message("2", MessageRole::Assistant, "Done, the token is saved now."),
        ];
        let screen = render(&app, 80, 16);
        assert!(!screen.lines().any(|line| line.trim() == "you"), "{screen}");
        assert!(
            !screen.lines().any(|line| line.trim() == "codex"),
            "{screen}"
        );

        let band = row_backgrounds(&app, 80, 16, "please fix the login bug");
        assert!(
            band.iter().all(|bg| *bg == Color::Indexed(237)),
            "the band runs the width of the chat: {band:?}"
        );
        let reply = row_backgrounds(&app, 80, 16, "Done, the token");
        assert!(
            reply.iter().all(|bg| *bg == Color::Reset),
            "the reply has no band: {reply:?}"
        );
    }

    #[test]
    fn a_long_message_you_sent_wraps_inside_the_band() {
        let mut app = open_chat();
        app.chat.total_count = 1;
        app.chat.messages = vec![message("1", MessageRole::User, &"word ".repeat(30))];
        let band = row_backgrounds(&app, 40, 14, "word word");
        assert!(band.iter().all(|bg| *bg == Color::Indexed(237)));
        let screen = render(&app, 40, 14);
        assert!(screen.lines().all(|line| line.chars().count() <= 40));
        assert!(screen.matches("word").count() >= 30, "all of it is shown");
    }

    #[test]
    fn the_waiting_text_follows_the_phase_like_web_and_mobile() {
        let mut app = open_chat();
        app.chat.running = true;
        let sending = render(&app, 80, 14);
        assert!(
            sending.contains("Sending…") && sending.contains(" Sending "),
            "{sending}"
        );
        assert!(!sending.contains("working") && !sending.contains("running"));

        app.chat.started = true;
        let processing = render(&app, 80, 14);
        assert!(processing.contains("Processing…") && processing.contains(" Processing "));

        app.chat.stopping = true;
        let stopping = render(&app, 80, 14);
        assert!(stopping.contains("Stopping…") && stopping.contains(" Stopping "));
    }

    #[test]
    fn the_terminal_cursor_sits_where_the_text_cursor_is() {
        let mut app = open_chat();
        app.chat.input.set("hello world");
        for _ in 0..5 {
            app.chat.input.left();
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let cursor = terminal.get_cursor_position().unwrap();
        let screen = render(&app, 80, 20);
        let row = screen
            .lines()
            .position(|line| line.contains("hello world"))
            .unwrap() as u16;
        assert_eq!((cursor.x, cursor.y), (1 + 6, row), "after \"hello \"");

        app.chat.input.set("one\ntwo three");
        app.chat.input.word_left();
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let cursor = terminal.get_cursor_position().unwrap();
        let screen = render(&app, 80, 20);
        let row = screen
            .lines()
            .position(|line| line.contains("two three"))
            .unwrap() as u16;
        assert_eq!(
            (cursor.x, cursor.y),
            (1 + 4, row),
            "before \"three\" on the second row"
        );
    }

    #[test]
    fn a_tall_prompt_keeps_the_cursor_row_visible() {
        let mut app = open_chat();
        app.chat.input.set(
            (1..=12)
                .map(|n| format!("row {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let screen = render(&app, 60, 24);
        assert!(
            screen.contains("row 12") && !screen.contains("row 1\n"),
            "{screen}"
        );
        app.chat.input.up(60);
        app.chat.input.up(60);
        let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let cursor = terminal.get_cursor_position().unwrap();
        let screen = render(&app, 60, 24);
        let row = screen
            .lines()
            .position(|line| line.contains("row 10"))
            .unwrap() as u16;
        assert_eq!(cursor.y, row, "the cursor is on the row it edits");
    }

    #[test]
    fn the_footer_and_the_prompt_box_say_which_keys_do_what() {
        let mut app = open_chat();
        let idle = render(&app, 100, 14);
        assert!(idle.contains("Enter send · Alt+Enter newline"));
        assert!(idle.contains("Esc Esc rewind") && idle.contains("Ctrl+O tools"));
        assert!(idle.contains("Write a message…"));
        assert!(!idle.contains("press i"), "there are no modes to explain");

        app.chat.running = true;
        let running = render(&app, 100, 14);
        assert!(running.contains("Esc stop") && !running.contains("Esc Esc rewind"));
    }

    #[test]
    fn the_rewind_list_shows_the_prompts_newest_first_and_what_choosing_does() {
        let mut app = open_chat();
        app.chat.messages = vec![
            message("m1", MessageRole::User, "first question"),
            message(
                "m2",
                MessageRole::User,
                "second question\nwith a second line",
            ),
        ];
        app.update(Msg::Key(ratatui::crossterm::event::KeyEvent::from(
            ratatui::crossterm::event::KeyCode::Esc,
        )));
        app.update(Msg::Key(ratatui::crossterm::event::KeyEvent::from(
            ratatui::crossterm::event::KeyCode::Esc,
        )));
        let screen = render(&app, 100, 24);
        assert!(screen.contains("Rewind to an earlier prompt"), "{screen}");
        assert!(screen.contains("new chat") && screen.contains("stays as it is"));
        // The chat behind the popup has the same words, so look inside the popup only.
        let popup = &screen[screen.find("Rewind to an earlier prompt").unwrap()..];
        let second = popup.find("second question …").expect("newest first");
        let first = popup.find("first question").expect("older prompt");
        assert!(second < first, "{screen}");
        assert!(screen.contains("Enter rewind to before it · Esc cancel"));
    }

    #[test]
    fn the_switcher_filters_as_you_type() {
        let mut app = open_chat();
        let mut switcher = Switcher::new();
        switcher.pinned = Load::Ready(Vec::new());
        switcher.projects = Load::Ready(Vec::new());
        app.switcher = Some(switcher);
        assert!(render(&app, 100, 24).contains("> type to filter"));
        app.switcher.as_mut().unwrap().filter = "login".into();
        assert!(render(&app, 100, 24).contains("> login"));
    }

    #[test]
    fn replies_are_formatted_as_markdown_and_prompts_are_shown_as_typed() {
        let mut app = open_chat();
        app.chat.total_count = 2;
        app.chat.messages = vec![
            message("1", MessageRole::User, "show me **everything**"),
            message(
                "2",
                MessageRole::Assistant,
                "# Plan\n\nUse **bold** and `code`.\n\n- one\n- two\n\n```mermaid\ngraph LR; A[Build] --> B[Test]\n```",
            ),
        ];
        let screen = render(&app, 80, 30);
        assert!(screen.contains("Plan") && !screen.contains("# Plan"));
        assert!(screen.contains("Use bold and code.") && !screen.contains("**bold**"));
        assert!(screen.contains("• one") && screen.contains("• two"));
        assert!(
            screen.contains("mermaid") && screen.contains("│ Build │"),
            "{screen}"
        );
        assert!(
            !screen.contains("graph LR"),
            "the diagram replaces its source"
        );
        assert!(screen.contains("show me **everything**"));
    }

    #[test]
    fn a_reply_that_is_still_streaming_is_formatted_and_keeps_an_open_diagram_as_source() {
        let mut app = open_chat();
        app.chat.running = true;
        app.chat.live = "claude\n**Streaming** answer\n\n```mermaid\ngraph TD; A --> B".into();
        let screen = render(&app, 80, 20);
        assert!(screen.contains("Streaming answer") && !screen.contains("**"));
        assert!(screen.contains("╭─ mermaid") && screen.contains("graph TD; A --> B"));
        assert!(
            !screen.contains('┌'),
            "an unfinished diagram is not drawn: {screen}"
        );
    }

    #[test]
    fn a_prompt_is_summarised_by_its_first_line() {
        assert_eq!(prompt_summary("  hello  \n\n"), "hello");
        assert_eq!(prompt_summary("fix it\n\nand then more"), "fix it …");
        assert_eq!(prompt_summary(""), "");
    }

    /// Two prompts, each answered by a reply too long to fit on the screen.
    fn two_turns() -> App {
        let mut app = open_chat();
        let reply = |tag: &str| {
            (1..=14)
                .map(|n| format!("- {tag} item {n}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        app.chat.total_count = 4;
        app.chat.messages = vec![
            message("1", MessageRole::User, "first question"),
            message("2", MessageRole::Assistant, &reply("alpha")),
            message("3", MessageRole::User, "second question\nwith more detail"),
            message("4", MessageRole::Assistant, &reply("beta")),
        ];
        app
    }

    /// The screen with the first visible chat line at `start` (a 14-row terminal
    /// shows 9 chat lines).
    fn screen_from(app: &mut App, start: usize) -> String {
        let total = chat_view(&app.chat, 78, 0).lines.len();
        app.chat.scroll = total - (start + 9);
        render(app, 80, 14)
    }

    #[test]
    fn the_latest_prompt_is_pinned_while_its_long_reply_is_on_screen() {
        let app = two_turns();
        let screen = render(&app, 80, 14);
        assert!(screen.contains("\u{f08d} second question …"), "{screen}");
        assert!(!screen.contains("first question"));
        assert!(screen.contains("beta item 14"));
    }

    #[test]
    fn scrolling_into_an_earlier_reply_pins_the_prompt_that_reply_answers() {
        let mut app = two_turns();
        let second = chat_view(&app.chat, 78, 0).prompts[1].line;
        let screen = screen_from(&mut app, second - 4);
        assert!(screen.contains("\u{f08d} first question"), "{screen}");
        assert!(!screen.contains("\u{f08d} second question"));
        assert!(screen.contains("alpha item"));
    }

    #[test]
    fn nothing_is_pinned_when_a_prompt_is_the_top_line_or_the_top_of_the_chat() {
        let mut app = two_turns();
        let second = chat_view(&app.chat, 78, 0).prompts[1].line;
        let at_prompt = screen_from(&mut app, second);
        assert!(!at_prompt.contains('\u{f08d}'), "{at_prompt}");
        assert!(at_prompt.contains("second question"));

        let top = screen_from(&mut app, 0);
        assert!(!top.contains('\u{f08d}'), "{top}");
        assert!(top.contains("first question"));
    }

    #[test]
    fn the_pinned_prompt_is_cut_to_the_terminal_width() {
        let mut app = two_turns();
        app.chat.messages[2].content = "an extremely long question ".repeat(10);
        let screen = render(&app, 30, 14);
        let bar = screen
            .lines()
            .find(|line| line.contains('\u{f08d}'))
            .expect("the prompt is pinned");
        assert!(bar.chars().count() <= 30 && bar.ends_with('…'), "{bar:?}");
    }

    #[test]
    fn long_text_never_overflows_the_terminal_width() {
        let mut app = open_chat();
        app.chat.total_count = 1;
        app.chat.messages = vec![message("1", MessageRole::User, &"x".repeat(500))];
        app.update(Msg::Tick);
        let screen = render(&app, 40, 12);
        assert!(screen.lines().all(|line| line.chars().count() <= 40));
    }
}
