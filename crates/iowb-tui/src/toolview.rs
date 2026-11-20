//! Draws tool calls the way the Claude Code and Codex terminals do: a header with
//! the tool and what it was asked to do, the result under a tree connector, and file
//! changes as a coloured diff. The header's icon shows the state: a spinner while
//! the call runs, green when it is done and red when it failed.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

use crate::{
    activity::{self, Block, Class, DiffKind, DiffLine, Phase, Tool},
    icons,
    text::{truncate, wrap},
};

pub struct View {
    pub width: usize,
    /// Everything the call produced, instead of its first few lines.
    pub expanded: bool,
    /// The call may still be waiting for its result.
    pub running: bool,
    /// The spinner frame to show while it waits.
    pub spinner: &'static str,
}

const COMMAND_LINES: usize = 10;
const OUTPUT_LINES: usize = 3;
const DIFF_LINES: usize = 8;
const EXPANDED_LINES: usize = 200;

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

pub fn render(block: &Block, view: &View) -> Vec<Line<'static>> {
    let tool = block.tool();
    let class = tool.class();
    let failed = block.result.as_ref().is_some_and(Tool::failed);
    let waiting = block.result.is_none()
        && block
            .call
            .as_ref()
            .is_some_and(|call| call.phase == Some(Phase::Parameters));
    let running = view.running && waiting;
    let diff = block
        .call
        .as_ref()
        .and_then(Tool::diff)
        .or_else(|| block.result.as_ref().and_then(Tool::diff))
        .filter(|lines| !lines.is_empty());

    let (icon, color) = if running {
        (view.spinner, Color::Yellow)
    } else if failed {
        (icons::tool(class), Color::Red)
    } else if waiting {
        (icons::tool(class), Color::DarkGray)
    } else {
        (icons::tool(class), Color::Green)
    };

    // ---- header ----------------------------------------------------------
    let name = display_name(tool);
    let used = 2 + icon.width() + 1 + name.width();
    let mut tail: Vec<Span<'static>> = Vec::new();
    if let Some((added, removed)) = diff.as_deref().map(activity::diff_stats)
        && (added > 0 || removed > 0)
    {
        tail.push(Span::styled(
            format!("+{added}"),
            Style::new().fg(Color::Green),
        ));
        tail.push(Span::raw(" "));
        tail.push(Span::styled(
            format!("-{removed}"),
            Style::new().fg(Color::Red),
        ));
    }
    if failed {
        let code = block
            .result
            .as_ref()
            .and_then(Tool::exit_code)
            .map_or("failed".to_string(), |code| format!("exit {code}"));
        if !tail.is_empty() {
            tail.push(Span::raw(" "));
        }
        tail.push(Span::styled(code, Style::new().fg(Color::Red)));
    }
    let tail_width: usize = tail.iter().map(|span| span.content.width()).sum();
    let gap = if tail.is_empty() { 0 } else { 2 };
    let args = arguments(block, class, diff.as_deref());
    let room = view.width.saturating_sub(used + 2 + gap + tail_width);

    let mut header = vec![
        Span::raw("  "),
        Span::styled(icon, Style::new().fg(color)),
        Span::raw(" "),
        Span::styled(name, Style::new().add_modifier(Modifier::BOLD)),
    ];
    if !args.is_empty() && room >= 4 {
        header.push(Span::raw(format!("  {}", truncate(&args, room))));
    }
    if !tail.is_empty() {
        header.push(Span::raw("  "));
        header.extend(tail);
    }

    // ---- body ------------------------------------------------------------
    let mut rows = body_rows(block, class, diff.as_deref(), view.expanded);
    let limit = if view.expanded {
        EXPANDED_LINES
    } else if diff.is_some() {
        DIFF_LINES
    } else {
        OUTPUT_LINES
    };
    let hidden = rows.len().saturating_sub(limit);
    rows.truncate(limit);
    if hidden > 0 {
        let hint = if view.expanded {
            format!("… +{hidden} more lines")
        } else {
            format!("… +{hidden} lines (t to expand)")
        };
        rows.push((hint, dim()));
    }

    let mut lines = vec![Line::from(header)];
    if class == Class::Exec
        && let Some(command) = block.call.as_ref().and_then(Tool::command)
    {
        lines.extend(command_lines(&command, view));
    }
    let room = view.width.saturating_sub(4);
    for (index, (text, style)) in rows.iter().enumerate() {
        let prefix = if index == 0 { "  └ " } else { "    " };
        lines.push(Line::from(vec![
            Span::styled(prefix, dim()),
            Span::styled(truncate(&text.replace('\t', "    "), room), *style),
        ]));
    }
    if !rows.is_empty() {
        lines.push(Line::default());
    }
    lines
}

type Row = (String, Style);

/// The whole command under the title, wrapped to the width rather than cut off.
/// Only a very long script is cut, with a hint that `t` shows the rest.
fn command_lines(command: &str, view: &View) -> Vec<Line<'static>> {
    let wrapped = wrap(command, view.width.saturating_sub(6).max(8));
    let limit = if view.expanded {
        EXPANDED_LINES
    } else {
        COMMAND_LINES
    };
    let hidden = wrapped.len().saturating_sub(limit);
    let mut lines: Vec<Line<'static>> = wrapped
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(index, text)| {
            let prompt = if index == 0 { "$ " } else { "  " };
            Line::from(vec![
                Span::raw("    "),
                Span::styled(prompt, dim()),
                Span::raw(text),
            ])
        })
        .collect();
    if hidden > 0 {
        let hint = if view.expanded {
            format!("      … +{hidden} more lines")
        } else {
            format!("      … +{hidden} lines (t to expand)")
        };
        lines.push(Line::from(Span::styled(hint, dim())));
    }
    lines
}

fn body_rows(block: &Block, class: Class, diff: Option<&[DiffLine]>, expanded: bool) -> Vec<Row> {
    if let Some(diff) = diff {
        return diff_rows(diff);
    }
    let output = block.result.as_ref().map(Tool::visible_output);
    let mut rows: Vec<Row> = Vec::new();
    // A read only says how much it read, until the call is expanded.
    if class == Class::Read && !expanded {
        if let Some(output) = output.filter(|output| !output.is_empty()) {
            let count = output.len();
            let noun = if count == 1 { "line" } else { "lines" };
            rows.push((format!("Read {count} {noun}"), dim()));
        }
        return rows;
    }
    match output {
        Some(output) if output.is_empty() && class == Class::Exec => {
            rows.push(("(no output)".to_string(), dim()));
        }
        Some(output) => rows.extend(output.into_iter().map(|line| (line, dim()))),
        None => {}
    }
    rows
}

/// The changed lines, coloured. File headers are left out unless the diff touches
/// several files, since the header already names the one file.
fn diff_rows(diff: &[DiffLine]) -> Vec<Row> {
    let is_file = |line: &DiffLine| {
        line.kind == DiffKind::Header
            && (line.text.starts_with("+++ ") || line.text.starts_with("*** "))
    };
    let several_files = diff.iter().filter(|line| is_file(line)).count() > 1;
    diff.iter()
        .filter_map(|line| {
            let text = line.text.as_str();
            match line.kind {
                DiffKind::Added => Some((format!("+ {text}"), Style::new().fg(Color::Green))),
                DiffKind::Removed => Some((format!("- {text}"), Style::new().fg(Color::Red))),
                DiffKind::Hunk => Some((
                    text.to_string(),
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::DIM),
                )),
                DiffKind::Context => Some((format!("  {text}"), dim())),
                DiffKind::Header if several_files && is_file(line) => {
                    let path = text
                        .trim_start_matches("+++ ")
                        .trim_start_matches("b/")
                        .trim_start_matches("*** ");
                    Some((path.to_string(), Style::new().add_modifier(Modifier::BOLD)))
                }
                DiffKind::Header => None,
            }
        })
        .collect()
}

/// The tool's name as a person would say it.
fn display_name(tool: &Tool) -> String {
    match tool.name.to_ascii_lowercase().as_str() {
        "exec" | "command_execution" | "exec_command" | "shell_command" | "local_shell"
        | "shell" | "sh" | "command" => "Run".to_string(),
        "apply_patch" | "patch" => "Patch".to_string(),
        "file_change" => "Edit".to_string(),
        _ if tool.name.chars().all(|ch| ch.is_ascii_lowercase()) => {
            let mut chars = tool.name.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        }
        _ => tool.name.clone(),
    }
}

/// What the call was asked to do, in a few words: the command, the file, the pattern.
fn arguments(block: &Block, class: Class, diff: Option<&[DiffLine]>) -> String {
    let tool = block.tool();
    let call = block.call.as_ref();
    let input = call.and_then(Tool::input);
    let get = |keys: &[&str]| {
        let input = input.as_ref()?;
        keys.iter()
            .find_map(|key| input.get(*key)?.as_str().map(str::to_string))
    };
    match class {
        // A command is long, so it gets lines of its own under the title.
        Class::Exec => String::new(),
        Class::Read => get(&["file_path", "path", "notebook_path"]).unwrap_or_default(),
        Class::Edit => tool
            .path
            .clone()
            .or_else(|| get(&["file_path", "path", "notebook_path"]))
            .or_else(|| diff.and_then(activity::diff_target))
            .unwrap_or_default(),
        Class::Search => match (get(&["pattern", "query"]), get(&["path"])) {
            (Some(pattern), Some(path)) => format!("\"{pattern}\" in {path}"),
            (Some(pattern), None) => format!("\"{pattern}\""),
            (None, Some(path)) => path,
            (None, None) => String::new(),
        },
        Class::Web => get(&["url", "query"]).unwrap_or_default(),
        Class::Agent => get(&["description", "prompt"]).unwrap_or_default(),
        Class::Todo => String::new(),
        Class::Other => summarize(input.as_ref()),
    }
}

/// The most telling fields of a call's JSON input.
fn summarize(input: Option<&Value>) -> String {
    let Some(Value::Object(object)) = input else {
        return String::new();
    };
    const FIRST: [&str; 9] = [
        "command",
        "cmd",
        "file_path",
        "path",
        "pattern",
        "query",
        "url",
        "prompt",
        "description",
    ];
    let mut keys: Vec<&String> = object.keys().collect();
    keys.sort_by_key(|key| {
        FIRST
            .iter()
            .position(|first| first == key)
            .unwrap_or(FIRST.len())
    });
    keys.into_iter()
        .take(2)
        .map(|key| {
            let value = match &object[key] {
                Value::String(text) => text.lines().next().unwrap_or("").to_string(),
                other => other.to_string(),
            };
            format!("{key}: {value}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const BASH_CALL: &str = "exec / Parameters\n**Tool:** `Bash`\n- **ID:** `t1`\n\n### Command\n```sh\ncargo test -p web\n```";
    const BASH_RESULT: &str = "exec / Details\n**Tool:** `Bash`\n- **ID:** `t1`\n\n```text\nline one\nline two\nline three\nline four\nline five\n```";
    const EDIT_CALL: &str = "tool / Parameters\n**Tool:** `Edit`\n- **ID:** `t2`\n\n```json\n{\"file_path\": \"src/main.rs\", \"old_string\": \"let a = 1;\", \"new_string\": \"let a = 10;\\nlet b = 2;\"}\n```";
    const EDIT_RESULT: &str = "tool / Details\n**Tool:** `Edit`\n- **ID:** `t2`\n\n```text\nThe file src/main.rs has been updated.\n```";

    fn tool(content: &str, name: &str) -> Tool {
        Tool::from_message(content, &json!({"toolName": name}))
    }

    fn block(parts: &[(&str, &str)]) -> Block {
        activity::pair(
            parts
                .iter()
                .map(|(content, name)| tool(content, name))
                .collect(),
        )
        .into_iter()
        .next()
        .unwrap()
    }

    fn view(width: usize) -> View {
        View {
            width,
            expanded: false,
            running: false,
            spinner: "⠋",
        }
    }

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

    fn icon_color(lines: &[Line<'_>]) -> Option<Color> {
        lines[0].spans[1].style.fg
    }

    #[test]
    fn a_command_shows_what_ran_and_the_start_of_its_output() {
        let done = block(&[(BASH_CALL, "Bash"), (BASH_RESULT, "Bash")]);
        let lines = plain(&render(&done, &view(80)));
        assert_eq!(
            lines,
            [
                format!("  {} Bash", icons::tool(Class::Exec)),
                "    $ cargo test -p web".to_string(),
                "  └ line one".to_string(),
                "    line two".to_string(),
                "    line three".to_string(),
                "    … +2 lines (t to expand)".to_string(),
                String::new(),
            ]
        );
    }

    #[test]
    fn expanding_shows_all_the_output_without_a_hint() {
        let done = block(&[(BASH_CALL, "Bash"), (BASH_RESULT, "Bash")]);
        let mut expanded = view(80);
        expanded.expanded = true;
        let lines = plain(&render(&done, &expanded));
        assert!(lines.contains(&"    line five".to_string()));
        assert!(!lines.iter().any(|line| line.contains("t to expand")));
    }

    #[test]
    fn the_icon_is_green_when_done_red_when_failed_and_a_spinner_while_running() {
        let call_only = block(&[(BASH_CALL, "Bash")]);
        let done = block(&[(BASH_CALL, "Bash"), (BASH_RESULT, "Bash")]);
        let failed_result = BASH_RESULT.replace("`t1`", "`t1`\n- **Exit code:** `2`");
        let failed = block(&[(BASH_CALL, "Bash"), (&failed_result, "Bash")]);

        assert_eq!(icon_color(&render(&done, &view(80))), Some(Color::Green));
        assert_eq!(icon_color(&render(&failed, &view(80))), Some(Color::Red));
        assert!(plain(&render(&failed, &view(80)))[0].ends_with("exit 2"));

        assert_eq!(
            icon_color(&render(&call_only, &view(80))),
            Some(Color::DarkGray)
        );
        let mut running = view(80);
        running.running = true;
        let lines = render(&call_only, &running);
        assert_eq!(icon_color(&lines), Some(Color::Yellow));
        assert!(
            plain(&lines)[0].contains('⠋'),
            "the spinner replaces the icon"
        );
        assert_eq!(
            lines.len(),
            2,
            "only the title and the command while it runs"
        );
    }

    #[test]
    fn a_command_with_no_output_says_so() {
        let silent = "exec / Details\n**Tool:** `Bash`\n- **ID:** `t1`\n\n```text\n\n```";
        let lines = plain(&render(
            &block(&[(BASH_CALL, "Bash"), (silent, "Bash")]),
            &view(80),
        ));
        assert_eq!(lines[2], "  └ (no output)");
    }

    #[test]
    fn a_file_edit_is_a_coloured_diff_with_counts_in_the_header() {
        let edit = block(&[(EDIT_CALL, "Edit"), (EDIT_RESULT, "Edit")]);
        let rendered = render(&edit, &view(80));
        let lines = plain(&rendered);
        assert_eq!(
            lines[0],
            format!("  {} Edit  src/main.rs  +2 -1", icons::tool(Class::Edit))
        );
        assert_eq!(
            &lines[1..4],
            ["  └ - let a = 1;", "    + let a = 10;", "    + let b = 2;"]
        );
        assert!(!lines.iter().any(|line| line.contains("has been updated")));
        let style_of = |text: &str| {
            rendered
                .iter()
                .flat_map(|line| &line.spans)
                .find(|span| span.content == text)
                .unwrap()
                .style
                .fg
        };
        assert_eq!(style_of("- let a = 1;"), Some(Color::Red));
        assert_eq!(style_of("+ let a = 10;"), Some(Color::Green));
    }

    #[test]
    fn a_long_diff_is_cut_with_a_hint_and_expands_in_full() {
        let new: String = (1..=20).map(|n| format!("line {n}\\n")).collect();
        let call = format!(
            "tool / Parameters\n**Tool:** `Write`\n\n```json\n{{\"file_path\": \"big.txt\", \"content\": \"{new}\"}}\n```"
        );
        let write = block(&[(&call, "Write")]);
        let lines = plain(&render(&write, &view(80)));
        assert!(lines[0].ends_with("big.txt  +20 -0"), "{lines:?}");
        assert!(lines.contains(&"    … +12 lines (t to expand)".to_string()));
        let mut expanded = view(80);
        expanded.expanded = true;
        assert!(plain(&render(&write, &expanded)).contains(&"    + line 20".to_string()));
    }

    #[test]
    fn a_unified_diff_hides_its_file_headers_for_one_file_and_shows_them_for_several() {
        let one = "diff / Details\n```diff\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-old\n+new\n```";
        let lines = plain(&render(&block(&[(one, "diff")]), &view(80)));
        assert!(
            lines[0].ends_with("Edit  x.rs  +1 -1") || lines[0].contains("x.rs"),
            "{lines:?}"
        );
        assert!(!lines.iter().any(|line| line.contains("--- a/")));

        let two = "diff / Details\n```diff\n+++ b/a.rs\n@@\n+x\n+++ b/b.rs\n@@\n+y\n```";
        let lines = plain(&render(&block(&[(two, "diff")]), &view(80)));
        assert!(
            lines.iter().any(|line| line.ends_with("a.rs"))
                && lines.iter().any(|line| line.ends_with("b.rs"))
        );
    }

    #[test]
    fn a_read_shows_the_file_and_how_many_lines_not_its_contents() {
        let call = "tool / Parameters\n**Tool:** `Read`\n- **ID:** `t3`\n\n```json\n{\"file_path\": \"src/lib.rs\"}\n```";
        let result = "tool / Details\n**Tool:** `Read`\n- **ID:** `t3`\n\n```text\nfn a() {}\nfn b() {}\nfn c() {}\n```";
        let lines = plain(&render(
            &block(&[(call, "Read"), (result, "Read")]),
            &view(80),
        ));
        assert_eq!(
            lines[0],
            format!("  {} Read  src/lib.rs", icons::tool(Class::Read))
        );
        assert_eq!(lines[1], "  └ Read 3 lines");
        assert!(!lines.iter().any(|line| line.contains("fn a()")));
    }

    #[test]
    fn expanding_a_read_shows_the_file_it_read() {
        let call = "tool / Parameters\n**Tool:** `Read`\n- **ID:** `t3`\n\n```json\n{\"file_path\": \"src/lib.rs\"}\n```";
        let result = "tool / Details\n**Tool:** `Read`\n- **ID:** `t3`\n\n```text\nfn a() {}\nfn b() {}\n```";
        let read = block(&[(call, "Read"), (result, "Read")]);
        let mut expanded = view(80);
        expanded.expanded = true;
        let lines = plain(&render(&read, &expanded));
        assert_eq!(&lines[1..3], ["  └ fn a() {}", "    fn b() {}"]);
    }

    #[test]
    fn a_failed_command_shows_the_exit_code_once() {
        let result = "exec / Details\n**Tool:** `Bash`\n- **ID:** `t1`\n\n```text\nExit code 1\nerror: boom\n```";
        let failed = block(&[(BASH_CALL, "Bash"), (result, "Bash")]);
        let lines = plain(&render(&failed, &view(80)));
        assert!(lines[0].ends_with("exit 1"), "{lines:?}");
        assert_eq!(lines[2], "  └ error: boom");
        assert!(!lines.iter().any(|line| line.contains("Exit code")));
    }

    fn command_block(command: &str) -> Block {
        let call = format!(
            "exec / Parameters\n**Tool:** `Bash`\n- **ID:** `t1`\n\n### Command\n```sh\n{command}\n```"
        );
        block(&[(&call, "Bash")])
    }

    #[test]
    fn the_command_sits_under_the_title_and_is_not_cut_off_when_it_is_long() {
        let command = "cargo test -p iowb-tui --lib --release -- --nocapture --test-threads=1 markdown::tests::tables";
        let lines = plain(&render(&command_block(command), &view(40)));
        assert_eq!(lines[0], format!("  {} Bash", icons::tool(Class::Exec)));
        assert!(lines[1].starts_with("    $ "), "{lines:?}");
        assert!(
            lines[2..lines.len() - 1]
                .iter()
                .all(|line| line.starts_with("      "))
        );
        // Wrapped, not truncated: every word of the command is still on screen.
        let shown: Vec<&str> = lines
            .iter()
            .flat_map(|line| line.split_whitespace())
            .collect();
        for word in command.split_whitespace() {
            assert!(shown.contains(&word), "{word:?} missing from {lines:?}");
        }
        assert!(!lines.iter().any(|line| line.contains('…')));
    }

    #[test]
    fn a_multi_line_command_keeps_all_its_lines_and_indentation() {
        let command = "cat > a.txt <<EOF\n  indented\nEOF\nls a.txt";
        let lines = plain(&render(&command_block(command), &view(80)));
        assert_eq!(
            &lines[1..5],
            [
                "    $ cat > a.txt <<EOF",
                "        indented",
                "      EOF",
                "      ls a.txt"
            ]
        );
    }

    #[test]
    fn a_very_long_script_is_cut_with_a_hint_and_expands_in_full() {
        let command: String = (1..=14).map(|n| format!("echo step {n}\n")).collect();
        let script = command_block(command.trim_end());
        let lines = plain(&render(&script, &view(80)));
        assert!(lines.contains(&"      echo step 10".to_string()));
        assert!(!lines.iter().any(|line| line.ends_with("echo step 11")));
        assert!(
            lines.contains(&"      … +4 lines (t to expand)".to_string()),
            "{lines:?}"
        );

        let mut expanded = view(80);
        expanded.expanded = true;
        let lines = plain(&render(&script, &expanded));
        assert!(lines.contains(&"      echo step 14".to_string()));
        assert!(!lines.iter().any(|line| line.contains("t to expand")));
    }

    #[test]
    fn searches_and_other_tools_show_their_arguments() {
        let grep = "tool / Parameters\n**Tool:** `Grep`\n\n```json\n{\"pattern\": \"TODO\", \"path\": \"src\", \"glob\": \"*.rs\"}\n```";
        assert!(
            plain(&render(&block(&[(grep, "Grep")]), &view(80)))[0]
                .ends_with("Grep  \"TODO\" in src")
        );
        let other = "tool / Parameters\n**Tool:** `mcp__x__lookup`\n\n```json\n{\"id\": 7, \"name\": \"ana\"}\n```";
        assert!(
            plain(&render(&block(&[(other, "mcp__x__lookup")]), &view(80)))[0]
                .ends_with("id: 7, name: ana")
        );
    }

    #[test]
    fn a_file_change_section_shows_the_operation_and_the_file() {
        let change = Tool::from_message(
            "edit / src/main.rs\n- **Status:** `completed`",
            &Value::Null,
        );
        let block = Block {
            call: Some(change),
            result: None,
        };
        let lines = plain(&render(&block, &view(80)));
        assert_eq!(
            lines[0],
            format!("  {} Edit  src/main.rs", icons::tool(Class::Edit))
        );
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn internal_names_are_made_readable() {
        let name = |raw: &str| display_name(&tool("x", raw));
        assert_eq!(name("exec_command"), "Run");
        assert_eq!(name("apply_patch"), "Patch");
        assert_eq!(name("read"), "Read");
        assert_eq!(name("Bash"), "Bash");
        assert_eq!(name("mcp__x__lookup"), "mcp__x__lookup");
    }

    #[test]
    fn nothing_is_wider_than_the_view() {
        let long = "x".repeat(300);
        let call = format!("exec / Parameters\n**Tool:** `Bash`\n\n```sh\n{long}\n```");
        let result =
            format!("exec / Details\n**Tool:** `Bash`\n\n```text\n{long}\n日本語日本語\n```");
        let lines = render(&block(&[(&call, "Bash"), (&result, "Bash")]), &view(40));
        for line in plain(&lines) {
            assert!(line.width() <= 40, "{line:?}");
        }
    }
}
