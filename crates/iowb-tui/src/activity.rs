//! Tool use, command execution and file changes. The server writes them into chat
//! text as sections headed `exec / Parameters`, `tool / Details`, `edit / path` and
//! so on, either inside the streamed reply or as stored `Tool` messages.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Parameters,
    Details,
}

/// What kind of thing a tool does, which decides how its call is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Exec,
    Read,
    Edit,
    Search,
    Web,
    Agent,
    Todo,
    Other,
}

/// One tool call, one result, or one file change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    pub phase: Option<Phase>,
    pub id: Option<String>,
    /// The file of a file-change section such as `edit / src/main.rs`.
    pub path: Option<String>,
    /// Everything under the heading.
    pub body: String,
}

/// A call and, once it came back, its result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub call: Option<Tool>,
    pub result: Option<Tool>,
}

impl Block {
    /// The call, or the result when it arrived without one.
    pub fn tool(&self) -> &Tool {
        self.call
            .as_ref()
            .or(self.result.as_ref())
            .expect("a block holds a call or a result")
    }
}

/// Part of a streamed reply: ordinary text, or a tool section.
#[derive(Debug, PartialEq, Eq)]
pub enum Part {
    Prose(String),
    Tool(Tool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Added,
    Removed,
    Hunk,
    Header,
    Context,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    /// The line without its `+` / `-` sign.
    pub text: String,
}

/// The section kinds the server writes, the same ones the web client recognises.
const KINDS: [&str; 12] = [
    "exec",
    "tool",
    "diff",
    "file_change",
    "bash",
    "shell",
    "command_execution",
    "function_call",
    "function_call_output",
    "custom_tool_call",
    "custom_tool_call_output",
    "apply_patch",
];

enum Header {
    Section { kind: String, phase: Phase },
    File { operation: String, path: String },
}

/// `exec / Parameters`, `tool / Details`, or a file change such as `edit / a.rs`.
fn header(line: &str) -> Option<Header> {
    let line = line.trim().trim_start_matches('#').trim();
    if let Some((kind, phase)) = line.rsplit_once('/') {
        let phase = match phase.trim().to_ascii_lowercase().as_str() {
            "parameters" => Some(Phase::Parameters),
            "details" => Some(Phase::Details),
            _ => None,
        };
        if let Some(phase) = phase {
            let kind = kind.trim().to_ascii_lowercase();
            return KINDS
                .contains(&kind.as_str())
                .then_some(Header::Section { kind, phase });
        }
    }
    let (operation, path) = line.split_once('/')?;
    let operation = operation.trim().to_ascii_lowercase();
    let path = path.trim();
    (["edit", "create", "delete", "move"].contains(&operation.as_str()) && !path.is_empty()).then(
        || Header::File {
            operation,
            path: path.to_string(),
        },
    )
}

/// The label line that starts the assistant's own text again after a tool section.
fn is_boundary(line: &str) -> bool {
    matches!(
        line.trim().to_lowercase().as_str(),
        "claude" | "codex" | "gemini" | "assistant" | "response"
    )
}

/// Lines that belong to a section's own framing rather than to prose after it.
fn is_framing(line: &str) -> bool {
    let line = line.trim();
    line.is_empty()
        || line.starts_with("**Tool:**")
        || line.starts_with("- **")
        || line.starts_with("###")
}

/// The opening backtick count when `line` starts a code fence.
fn fence_ticks(line: &str) -> Option<usize> {
    let ticks = line
        .trim_start()
        .chars()
        .take_while(|ch| *ch == '`')
        .count();
    (ticks >= 3).then_some(ticks)
}

fn meta_value(body: &str, label: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let value = line
            .trim()
            .strip_prefix(label)?
            .trim()
            .trim_matches('`')
            .trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn tool_name(body: &str) -> Option<String> {
    meta_value(body, "**Tool:**")
}

fn tool_id(body: &str) -> Option<String> {
    meta_value(body, "- **ID:**")
}

/// Every fenced code block of `body`: its info string and its text.
fn fences(body: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut open: Option<(String, usize, Vec<&str>)> = None;
    for line in body.lines() {
        match (&mut open, fence_ticks(line)) {
            (None, Some(ticks)) => {
                let info = line.trim_start().trim_start_matches('`').trim().to_string();
                open = Some((info, ticks, Vec::new()));
            }
            (Some((_, ticks, _)), Some(closing)) if closing >= *ticks => {
                if let Some((info, _, text)) = open.take() {
                    found.push((info, text.join("\n")));
                }
            }
            (Some((_, _, text)), _) => text.push(line),
            (None, None) => {}
        }
    }
    // A block still arriving has no closing fence yet.
    if let Some((info, _, text)) = open {
        found.push((info, text.join("\n")));
    }
    found
}

impl Tool {
    /// A stored `Tool` message: the name comes from its metadata when the server
    /// recorded one.
    pub fn from_message(content: &str, metadata: &Value) -> Self {
        let first = content.lines().next().unwrap_or("");
        let rest = content.split_once('\n').map_or("", |(_, rest)| rest);
        let (kind, mut phase, path, body) = match header(first) {
            Some(Header::Section { kind, phase }) => {
                (Some(kind), Some(phase), None, rest.to_string())
            }
            Some(Header::File { operation, path }) => {
                (Some(operation), None, Some(path), rest.to_string())
            }
            None => (None, None, None, content.to_string()),
        };
        if phase.is_none() && path.is_none() {
            phase = match metadata.get("kind").and_then(Value::as_str) {
                Some("tool_call") => Some(Phase::Parameters),
                Some("tool_result") => Some(Phase::Details),
                _ => None,
            };
        }
        let name = metadata
            .get("toolName")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .map(str::to_string)
            .or_else(|| tool_name(&body))
            .or(kind)
            .unwrap_or_else(|| "tool".to_string());
        let id = tool_id(&body);
        Self {
            name,
            phase,
            id,
            path,
            body,
        }
    }

    pub fn class(&self) -> Class {
        if self.path.is_some() {
            return Class::Edit;
        }
        match self.name.to_ascii_lowercase().as_str() {
            "bash" | "bashoutput" | "killshell" | "shell" | "sh" | "exec" | "command"
            | "shell_command" | "exec_command" | "command_execution" | "local_shell" => Class::Exec,
            "read" | "view" | "cat" | "notebookread" => Class::Read,
            "edit"
            | "multiedit"
            | "write"
            | "notebookedit"
            | "apply_patch"
            | "str_replace_editor"
            | "str_replace_based_edit_tool"
            | "update"
            | "create"
            | "patch"
            | "file_change"
            | "diff" => Class::Edit,
            "grep" | "glob" | "ls" | "search" | "find" | "rg" => Class::Search,
            "webfetch" | "websearch" | "web_search" | "fetch" | "browser" => Class::Web,
            "task" | "agent" => Class::Agent,
            "todowrite" | "update_plan" => Class::Todo,
            _ => Class::Other,
        }
    }

    /// The call's arguments, from its JSON block.
    pub fn input(&self) -> Option<Value> {
        fences(&self.body)
            .into_iter()
            .find(|(info, text)| {
                info.eq_ignore_ascii_case("json") || text.trim_start().starts_with('{')
            })
            .and_then(|(_, text)| serde_json::from_str(&text).ok())
    }

    /// The shell command of a call.
    pub fn command(&self) -> Option<String> {
        let fenced = fences(&self.body)
            .into_iter()
            .find(|(info, _)| matches!(info.to_ascii_lowercase().as_str(), "sh" | "bash" | "shell"))
            .map(|(_, text)| text);
        fenced
            .or_else(|| {
                let input = self.input()?;
                ["command", "cmd", "script"]
                    .into_iter()
                    .find_map(|key| input.get(key)?.as_str().map(str::to_string))
            })
            .map(|command| command.trim().to_string())
            .filter(|command| !command.is_empty())
    }

    /// The lines of output a result carries.
    pub fn output(&self) -> Vec<String> {
        let blocks = fences(&self.body);
        let block = blocks.iter().find(|(info, _)| {
            !matches!(
                info.to_ascii_lowercase().as_str(),
                "json" | "sh" | "bash" | "shell" | "diff"
            )
        });
        let lines: Vec<String> = match block {
            Some((_, text)) => text.lines().map(str::to_string).collect(),
            None if blocks.is_empty() => self
                .body
                .lines()
                .filter(|line| !is_framing(line))
                .map(str::to_string)
                .collect(),
            None => Vec::new(),
        };
        let start = lines
            .iter()
            .position(|line| !line.trim().is_empty())
            .unwrap_or(0);
        let end = lines
            .iter()
            .rposition(|line| !line.trim().is_empty())
            .map_or(0, |last| last + 1);
        lines[start.min(end)..end].to_vec()
    }

    /// The output without Claude's own `Exit code 2` line, which a header already shows.
    pub fn visible_output(&self) -> Vec<String> {
        let mut lines = self.output();
        let is_exit_line = |line: &String| {
            line.trim()
                .strip_prefix("Exit code ")
                .is_some_and(|code| code.trim().parse::<i32>().is_ok())
        };
        if lines.first().is_some_and(is_exit_line) {
            lines.remove(0);
        }
        lines
    }

    pub fn exit_code(&self) -> Option<i32> {
        if let Some(code) = meta_value(&self.body, "- **Exit code:**") {
            return code.parse().ok();
        }
        // Claude reports a failed command as `Exit code 2` followed by its output.
        let first = self.output().into_iter().next()?;
        first.trim().strip_prefix("Exit code ")?.trim().parse().ok()
    }

    pub fn failed(&self) -> bool {
        let status =
            meta_value(&self.body, "- **Status:**").map(|status| status.to_ascii_lowercase());
        self.exit_code().is_some_and(|code| code != 0)
            || status
                .is_some_and(|status| matches!(status.as_str(), "failed" | "error" | "declined"))
    }

    /// The change a file-editing call makes: a diff the server sent, or one built
    /// from the old and new text of the call.
    pub fn diff(&self) -> Option<Vec<DiffLine>> {
        if let Some((_, text)) = fences(&self.body)
            .into_iter()
            .find(|(info, _)| info.eq_ignore_ascii_case("diff"))
        {
            return Some(parse_diff(&text));
        }
        let input = self.input()?;
        let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
        match self.name.to_ascii_lowercase().as_str() {
            "edit" | "str_replace_editor" | "update" => {
                Some(replacement(&text("old_string")?, &text("new_string")?))
            }
            "multiedit" => {
                let edits = input.get("edits")?.as_array()?;
                let mut lines = Vec::new();
                for edit in edits {
                    let old = edit.get("old_string")?.as_str()?;
                    let new = edit.get("new_string")?.as_str()?;
                    lines.extend(replacement(old, new));
                }
                Some(lines)
            }
            "write" | "create" => Some(
                text("content")?
                    .lines()
                    .map(|line| DiffLine {
                        kind: DiffKind::Added,
                        text: line.to_string(),
                    })
                    .collect(),
            ),
            "notebookedit" => Some(replacement("", &text("new_source")?)),
            _ => None,
        }
    }
}

fn replacement(old: &str, new: &str) -> Vec<DiffLine> {
    let line = |kind, text: &str| DiffLine {
        kind,
        text: text.to_string(),
    };
    let removed = old.lines().map(|text| line(DiffKind::Removed, text));
    let added = new.lines().map(|text| line(DiffKind::Added, text));
    removed.chain(added).collect()
}

/// A unified diff, or the patch format Codex writes (`*** Update File: path`).
fn parse_diff(text: &str) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    for line in text.lines() {
        let (kind, text) =
            if line.starts_with("*** Begin Patch") || line.starts_with("*** End Patch") {
                continue;
            } else if line.starts_with("+++ ")
                || line.starts_with("--- ")
                || line.starts_with("diff --git")
                || line.starts_with("index ")
                || line.starts_with("*** ")
            {
                (DiffKind::Header, line)
            } else if line.starts_with("@@") {
                (DiffKind::Hunk, line)
            } else if let Some(added) = line.strip_prefix('+') {
                (DiffKind::Added, added)
            } else if let Some(removed) = line.strip_prefix('-') {
                (DiffKind::Removed, removed)
            } else {
                (DiffKind::Context, line.strip_prefix(' ').unwrap_or(line))
            };
        lines.push(DiffLine {
            kind,
            text: text.to_string(),
        });
    }
    lines
}

/// How many lines a diff adds and removes.
pub fn diff_stats(lines: &[DiffLine]) -> (usize, usize) {
    let count = |kind| lines.iter().filter(|line| line.kind == kind).count();
    (count(DiffKind::Added), count(DiffKind::Removed))
}

/// The file a diff is about: its first file header.
pub fn diff_target(lines: &[DiffLine]) -> Option<String> {
    lines
        .iter()
        .filter(|line| line.kind == DiffKind::Header)
        .find_map(|line| {
            let text = line.text.as_str();
            let path = text
                .strip_prefix("+++ ")
                .map(|path| path.trim_start_matches("b/"))
                .or_else(|| text.strip_prefix("*** Update File:"))
                .or_else(|| text.strip_prefix("*** Add File:"))
                .or_else(|| text.strip_prefix("*** Delete File:"))?
                .trim();
            (!path.is_empty() && path != "/dev/null").then(|| path.to_string())
        })
}

/// Puts each result next to the call it answers.
pub fn pair(tools: Vec<Tool>) -> Vec<Block> {
    fn answers(call: &Tool, result: &Tool) -> bool {
        match (&call.id, &result.id) {
            (Some(call_id), Some(result_id)) => call_id == result_id,
            _ => call.name.eq_ignore_ascii_case(&result.name),
        }
    }

    let mut blocks: Vec<Block> = Vec::new();
    for tool in tools {
        let answered = tool.phase == Some(Phase::Details)
            && blocks.last().is_some_and(|block| {
                block.result.is_none()
                    && block.call.as_ref().is_some_and(|call| {
                        call.phase == Some(Phase::Parameters) && answers(call, &tool)
                    })
            });
        if answered {
            if let Some(block) = blocks.last_mut() {
                block.result = Some(tool);
            }
        } else if tool.phase == Some(Phase::Details) {
            blocks.push(Block {
                call: None,
                result: Some(tool),
            });
        } else {
            blocks.push(Block {
                call: Some(tool),
                result: None,
            });
        }
    }
    blocks
}

/// Splits a streamed reply into its text and its tool sections. A section runs from
/// its heading to the assistant's next label line, the next heading, or, once it has
/// shown a code block, the first line of ordinary text after it.
pub fn split_live(text: &str) -> Vec<Part> {
    struct Open {
        kind: String,
        phase: Option<Phase>,
        path: Option<String>,
        lines: Vec<String>,
        closed_fence: bool,
    }

    fn close(open: Open, parts: &mut Vec<Part>, last_name: &mut String) {
        let body = open.lines.join("\n").trim().to_string();
        let name = tool_name(&body)
            .or_else(|| {
                (open.phase == Some(Phase::Details) && !last_name.is_empty())
                    .then(|| last_name.clone())
            })
            .unwrap_or(open.kind);
        if open.path.is_none() {
            *last_name = name.clone();
        }
        parts.push(Part::Tool(Tool {
            id: tool_id(&body),
            name,
            phase: open.phase,
            path: open.path,
            body,
        }));
    }

    fn flush_prose(prose: &mut Vec<&str>, parts: &mut Vec<Part>) {
        let text = prose.join("\n").trim().to_string();
        prose.clear();
        if !text.is_empty() {
            parts.push(Part::Prose(text));
        }
    }

    let mut parts = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut open: Option<Open> = None;
    let mut last_name = String::new();
    let mut fence: Option<usize> = None;

    for line in text.lines() {
        if fence.is_none() {
            if let Some(found) = header(line) {
                if let Some(section) = open.take() {
                    close(section, &mut parts, &mut last_name);
                }
                flush_prose(&mut prose, &mut parts);
                open = Some(match found {
                    Header::Section { kind, phase } => Open {
                        kind,
                        phase: Some(phase),
                        path: None,
                        lines: Vec::new(),
                        closed_fence: false,
                    },
                    Header::File { operation, path } => Open {
                        kind: operation,
                        phase: None,
                        path: Some(path),
                        lines: Vec::new(),
                        closed_fence: false,
                    },
                });
                continue;
            }
            // The server names a patch before listing its files; the files say the rest.
            if line.trim() == "apply_patch" {
                continue;
            }
            if open.is_some() && is_boundary(line) {
                if let Some(section) = open.take() {
                    close(section, &mut parts, &mut last_name);
                }
                continue;
            }
            if open.as_ref().is_some_and(|section| section.closed_fence)
                && !is_framing(line)
                && let Some(section) = open.take()
            {
                close(section, &mut parts, &mut last_name);
            }
        }
        match &mut open {
            Some(section) => section.lines.push(line.to_string()),
            None => prose.push(line),
        }
        if let Some(ticks) = fence_ticks(line) {
            match fence {
                None => fence = Some(ticks),
                Some(opening) if ticks >= opening => {
                    fence = None;
                    if let Some(section) = &mut open {
                        section.closed_fence = true;
                    }
                }
                Some(_) => {}
            }
        }
    }
    if let Some(section) = open.take() {
        close(section, &mut parts, &mut last_name);
    }
    flush_prose(&mut prose, &mut parts);
    parts
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const BASH_CALL: &str = "exec / Parameters\n**Tool:** `Bash`\n- **ID:** `toolu_1`\n\n### Command\n```sh\nls -la src\n```";
    const BASH_RESULT: &str = "exec / Details\n**Tool:** `Bash`\n- **ID:** `toolu_1`\n\n```text\nmain.rs\nlib.rs\ntotal 2\n```";
    const READ_CALL: &str = "tool / Parameters\n**Tool:** `Read`\n- **ID:** `toolu_2`\n\n### Input\n```json\n{\n  \"file_path\": \"src/main.rs\"\n}\n```";
    const EDIT_CALL: &str = "tool / Parameters\n**Tool:** `Edit`\n- **ID:** `toolu_3`\n\n### Input\n```json\n{\"file_path\": \"src/main.rs\", \"old_string\": \"let a = 1;\\nlet b = 2;\", \"new_string\": \"let a = 10;\"}\n```";

    fn stored(content: &str, name: &str) -> Tool {
        Tool::from_message(content, &json!({"toolName": name}))
    }

    #[test]
    fn a_stored_command_call_has_its_class_command_and_id() {
        let tool = Tool::from_message(BASH_CALL, &json!({"toolName": "Bash", "kind": "tool_call"}));
        assert_eq!(
            (tool.name.as_str(), tool.phase, tool.id.as_deref()),
            ("Bash", Some(Phase::Parameters), Some("toolu_1"))
        );
        assert_eq!(tool.class(), Class::Exec);
        assert_eq!(tool.command().as_deref(), Some("ls -la src"));
    }

    #[test]
    fn a_result_has_its_output_trimmed_and_no_framing() {
        let tool = stored(BASH_RESULT, "Bash");
        assert_eq!(tool.output(), ["main.rs", "lib.rs", "total 2"]);
        assert_eq!(tool.exit_code(), None);
        assert!(!tool.failed());
    }

    #[test]
    fn json_input_and_commands_are_read_from_their_blocks() {
        let read = stored(READ_CALL, "Read");
        assert_eq!(read.class(), Class::Read);
        assert_eq!(read.input().unwrap()["file_path"], "src/main.rs");
        let command = "tool / Parameters\n**Tool:** `Bash`\n\n### Input\n```json\n{\"command\": \"cargo test\"}\n```";
        assert_eq!(
            stored(command, "Bash").command().as_deref(),
            Some("cargo test")
        );
    }

    #[test]
    fn failures_come_from_the_exit_code_or_the_status() {
        let metadata = "exec / Details\n- **Status:** `failed`\n- **Exit code:** `2`\n\n```text\nerror: boom\n```";
        let tool = stored(metadata, "Bash");
        assert_eq!(tool.exit_code(), Some(2));
        assert!(tool.failed());

        let claude = "exec / Details\n**Tool:** `Bash`\n\n```text\nExit code 1\nboom\n```";
        assert!(stored(claude, "Bash").failed());
        let fine = "exec / Details\n- **Exit code:** `0`\n\n```text\nok\n```";
        assert!(!stored(fine, "Bash").failed());
    }

    #[test]
    fn claudes_exit_code_line_is_left_out_of_the_visible_output() {
        let claude = "exec / Details\n**Tool:** `Bash`\n\n```text\nExit code 1\nboom\n```";
        let tool = stored(claude, "Bash");
        assert_eq!(tool.output(), ["Exit code 1", "boom"]);
        assert_eq!(tool.visible_output(), ["boom"]);
        let ordinary = stored(BASH_RESULT, "Bash");
        assert_eq!(ordinary.visible_output(), ordinary.output());
    }

    #[test]
    fn output_that_is_not_fenced_is_still_read() {
        let content = "tool / Details\n**Tool:** `WebFetch`\n\nfirst line of output\nsecond line";
        assert_eq!(
            stored(content, "WebFetch").output(),
            ["first line of output", "second line"]
        );
    }

    #[test]
    fn a_message_without_a_heading_falls_back_to_its_metadata() {
        let tool = Tool::from_message(
            "ran cargo test\nlots of output",
            &json!({"toolName": "Shell", "kind": "tool_result"}),
        );
        assert_eq!(
            (tool.name.as_str(), tool.phase),
            ("Shell", Some(Phase::Details))
        );
        assert_eq!(tool.output(), ["ran cargo test", "lots of output"]);
        assert_eq!(Tool::from_message("x", &Value::Null).name, "tool");
        let kind_only = Tool::from_message("exec / Details\n```text\nok\n```", &Value::Null);
        assert_eq!(kind_only.name, "exec");
    }

    #[test]
    fn tools_are_classified_by_name() {
        let class = |name: &str| stored("x", name).class();
        assert_eq!(class("Bash"), Class::Exec);
        assert_eq!(class("exec_command"), Class::Exec);
        assert_eq!(class("Read"), Class::Read);
        assert_eq!(class("MultiEdit"), Class::Edit);
        assert_eq!(class("Grep"), Class::Search);
        assert_eq!(class("WebFetch"), Class::Web);
        assert_eq!(class("Task"), Class::Agent);
        assert_eq!(class("mcp__server__thing"), Class::Other);
    }

    #[test]
    fn an_edit_call_becomes_a_diff_of_its_old_and_new_text() {
        let diff = stored(EDIT_CALL, "Edit").diff().unwrap();
        let shown: Vec<(DiffKind, &str)> = diff
            .iter()
            .map(|line| (line.kind, line.text.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                (DiffKind::Removed, "let a = 1;"),
                (DiffKind::Removed, "let b = 2;"),
                (DiffKind::Added, "let a = 10;"),
            ]
        );
        assert_eq!(diff_stats(&diff), (1, 2));
    }

    #[test]
    fn writes_and_multi_edits_are_diffs_too() {
        let write = "tool / Parameters\n**Tool:** `Write`\n\n```json\n{\"file_path\": \"a.txt\", \"content\": \"one\\ntwo\"}\n```";
        assert_eq!(diff_stats(&stored(write, "Write").diff().unwrap()), (2, 0));
        let multi = "tool / Parameters\n**Tool:** `MultiEdit`\n\n```json\n{\"file_path\": \"a\", \"edits\": [{\"old_string\": \"x\", \"new_string\": \"y\"}, {\"old_string\": \"p\", \"new_string\": \"q\\nr\"}]}\n```";
        assert_eq!(
            diff_stats(&stored(multi, "MultiEdit").diff().unwrap()),
            (3, 2)
        );
        assert!(stored(BASH_CALL, "Bash").diff().is_none());
    }

    #[test]
    fn a_unified_diff_and_a_codex_patch_are_parsed() {
        let unified = "diff / Details\n```diff\ndiff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,2 @@\n fn a() {}\n-old\n+new\n```";
        let diff = stored(unified, "diff").diff().unwrap();
        assert_eq!(diff_stats(&diff), (1, 1));
        assert_eq!(diff_target(&diff).as_deref(), Some("x.rs"));
        assert!(diff.iter().any(|line| line.kind == DiffKind::Hunk));
        assert!(
            diff.iter()
                .any(|line| line.kind == DiffKind::Context && line.text == "fn a() {}")
        );

        let patch = "apply_patch\nedit / src/lib.rs\n\n```diff\n*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-a\n+b\n*** End Patch\n```";
        let diff = stored(patch, "apply_patch").diff().unwrap();
        assert_eq!(diff_stats(&diff), (1, 1));
        assert_eq!(diff_target(&diff).as_deref(), Some("src/lib.rs"));
        assert!(!diff.iter().any(|line| line.text.contains("Begin Patch")));
    }

    #[test]
    fn a_file_change_section_names_its_file() {
        let tool = Tool::from_message(
            "edit / src/main.rs\n- **Status:** `completed`",
            &Value::Null,
        );
        assert_eq!(tool.path.as_deref(), Some("src/main.rs"));
        assert_eq!((tool.class(), tool.name.as_str()), (Class::Edit, "edit"));
        assert!(!tool.failed());
    }

    #[test]
    fn a_result_is_paired_with_the_call_it_answers() {
        let blocks = pair(vec![
            stored(BASH_CALL, "Bash"),
            stored(BASH_RESULT, "Bash"),
            stored(READ_CALL, "Read"),
            stored(EDIT_CALL, "Edit"),
        ]);
        assert_eq!(blocks.len(), 3);
        assert!(blocks[0].result.is_some());
        assert!(blocks[1].result.is_none() && blocks[2].result.is_none());
    }

    #[test]
    fn a_result_with_another_id_is_not_paired_and_stands_alone() {
        let other = BASH_RESULT.replace("toolu_1", "toolu_9");
        let blocks = pair(vec![stored(BASH_CALL, "Bash"), stored(&other, "Bash")]);
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].result.is_none());
        assert!(blocks[1].call.is_none() && blocks[1].result.is_some());
        assert_eq!(blocks[1].tool().name, "Bash");
    }

    #[test]
    fn a_live_reply_splits_into_text_and_tool_sections() {
        let live = format!(
            "Let me look at the project first.\n\n{BASH_CALL}\n\n{BASH_RESULT}\n\n{READ_CALL}\n\nclaude\nThe project has two source files."
        );
        let parts = split_live(&live);
        let shown: Vec<String> = parts
            .iter()
            .map(|part| match part {
                Part::Prose(text) => format!("prose: {text}"),
                Part::Tool(tool) => format!("{} {:?}", tool.name, tool.phase),
            })
            .collect();
        assert_eq!(
            shown,
            [
                "prose: Let me look at the project first.",
                "Bash Some(Parameters)",
                "Bash Some(Details)",
                "Read Some(Parameters)",
                "prose: The project has two source files.",
            ]
        );
    }

    #[test]
    fn a_result_without_a_name_takes_the_name_of_the_call_before_it() {
        let live = "tool / Parameters\n**Tool:** `Read`\n\n### Input\n```json\n{\"file_path\": \"a.rs\"}\n```\n\ntool / Details\n```text\ncontents\n```";
        let names: Vec<String> = split_live(live)
            .into_iter()
            .filter_map(|part| match part {
                Part::Tool(tool) => Some(tool.name),
                Part::Prose(_) => None,
            })
            .collect();
        assert_eq!(names, ["Read", "Read"]);
    }

    #[test]
    fn live_file_changes_are_sections_of_their_own() {
        let live = "I will patch two files.\n\napply_patch\n\nedit / src/a.rs\n\ncreate / src/b.rs\n- **Status:** `completed`\n\nclaude\nDone.";
        let parts = split_live(live);
        let shown: Vec<String> = parts
            .iter()
            .map(|part| match part {
                Part::Prose(text) => format!("prose: {text}"),
                Part::Tool(tool) => {
                    format!("{} {}", tool.name, tool.path.as_deref().unwrap_or(""))
                }
            })
            .collect();
        assert_eq!(
            shown,
            [
                "prose: I will patch two files.",
                "edit src/a.rs",
                "create src/b.rs",
                "prose: Done."
            ]
        );
    }

    #[test]
    fn text_after_a_finished_code_block_is_not_swallowed_by_the_section() {
        let live = "exec / Details\n```text\nok\n```\n\nAll done, the build passes.";
        let parts = split_live(live);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1], Part::Prose("All done, the build passes.".into()));
    }

    #[test]
    fn headings_inside_a_code_block_do_not_start_a_section() {
        let live = "Here is how the log looks:\n\n```text\nexec / Parameters\ntool / Details\n```";
        assert_eq!(split_live(live), [Part::Prose(live.into())]);
    }

    #[test]
    fn a_section_that_is_still_arriving_is_kept_as_it_is() {
        let parts = split_live("exec / Parameters\n**Tool:** `Bash`\n\n### Command\n```sh\nls -");
        let [Part::Tool(tool)] = &parts[..] else {
            panic!("{parts:?}");
        };
        assert_eq!(tool.command().as_deref(), Some("ls -"));
    }

    #[test]
    fn plain_text_and_odd_input_stay_plain() {
        assert_eq!(split_live(""), []);
        assert_eq!(split_live("just text"), [Part::Prose("just text".into())]);
        assert!(header("Input / Parameters").is_none());
        assert!(header("read / the manual").is_none());
        assert!(matches!(
            header("### exec / details"),
            Some(Header::Section {
                phase: Phase::Details,
                ..
            })
        ));
    }
}
