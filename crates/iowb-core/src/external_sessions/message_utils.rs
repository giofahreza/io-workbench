pub(crate) fn visible_user_text(text: &str) -> String {
    let mut candidate = text.trim();
    while let Some(tag) = [
        "system-reminder",
        "recommended_plugins",
        "environment_context",
    ]
    .into_iter()
    .find(|tag| candidate.starts_with(&format!("<{tag}>")))
    {
        let close = format!("</{tag}>");
        let Some(index) = candidate.find(&close) else {
            return String::new();
        };
        candidate = candidate[index + close.len()..].trim();
    }
    if is_visible_user_text(candidate) {
        candidate.to_string()
    } else {
        String::new()
    }
}

fn is_visible_user_text(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && ![
            "<command-name>",
            "<command-message>",
            "<command-args>",
            "<local-command-caveat>",
            "<local-command-stdout>",
            "<system-reminder>",
            "<recommended_plugins>",
            "<environment_context>",
            "# AGENTS.md instructions",
            "Caveat:",
            "This session is being continued from a previous",
            "[Request interrupted",
            "<turn_aborted>",
        ]
        .into_iter()
        .any(|prefix| text.starts_with(prefix))
}

fn parse_role(role: &str) -> Option<MessageRole> {
    match role {
        "user" => Some(MessageRole::User),
        "assistant" | "gemini" => Some(MessageRole::Assistant),
        _ => None,
    }
}

fn summarize(text: &str) -> String {
    const MAX_CHARS: usize = 72;
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= MAX_CHARS {
        normalized
    } else {
        format!(
            "{}...",
            normalized.chars().take(MAX_CHARS).collect::<String>()
        )
    }
}

fn value_timestamp(value: Option<&Value>) -> Option<DateTime<Utc>> {
    match value {
        Some(Value::String(raw)) => DateTime::parse_from_rfc3339(raw)
            .map(|timestamp| timestamp.with_timezone(&Utc))
            .ok(),
        Some(Value::Number(raw)) => raw.as_i64().and_then(|timestamp| {
            if timestamp > 10_000_000_000 {
                DateTime::from_timestamp_millis(timestamp)
            } else {
                DateTime::from_timestamp(timestamp, 0)
            }
        }),
        _ => None,
    }
}

fn latest(
    current: Option<DateTime<Utc>>,
    candidate: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match (current, candidate) {
        (Some(current), Some(candidate)) => Some(current.max(candidate)),
        (current, candidate) => current.or(candidate),
    }
}

fn modified_time(path: &Path) -> Option<DateTime<Utc>> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(DateTime::<Utc>::from)
}

fn for_each_json_line(path: &Path, mut visit: impl FnMut(&Value)) {
    let Ok(file) = File::open(path) else {
        return;
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            visit(&value);
        }
    }
}

/// Parse complete JSONL records beginning at a previously persisted boundary.
/// The returned offset always points immediately after a newline, so a partial
/// final write is retried safely on the next synchronization.
fn for_each_json_line_from(path: &Path, offset: u64, mut visit: impl FnMut(&Value)) -> u64 {
    let Ok(mut file) = File::open(path) else {
        return offset;
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return offset;
    }
    let mut reader = BufReader::new(file);
    let mut committed_offset = offset;
    loop {
        let mut bytes = Vec::new();
        let Ok(read) = reader.read_until(b'\n', &mut bytes) else {
            break;
        };
        if read == 0 {
            break;
        }
        if !bytes.ends_with(b"\n") {
            break;
        }
        committed_offset = committed_offset.saturating_add(read as u64);
        bytes.pop();
        if bytes.ends_with(b"\r") {
            bytes.pop();
        }
        if let Ok(line) = std::str::from_utf8(&bytes)
            && let Ok(value) = serde_json::from_str::<Value>(line)
        {
            visit(&value);
        }
    }
    committed_offset
}

fn files_below(root: &Path, max_depth: usize, extension: &str) -> Vec<PathBuf> {
    fn visit(dir: &Path, depth: usize, max_depth: usize, extension: &str, out: &mut Vec<PathBuf>) {
        if depth > max_depth {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, depth + 1, max_depth, extension, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some(extension) {
                out.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, 0, max_depth, extension, &mut files);
    files
}

fn newest_matching_file(root: &Path, prefix: &str, extension: &str) -> Option<PathBuf> {
    fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix))
                && path.extension().and_then(|ext| ext.to_str()) == Some(extension)
        })
        .max_by_key(|path| {
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        })
}

fn normalize_path(path: &str) -> String {
    let normalized = path.trim().trim_end_matches(['/', '\\']).replace('\\', "/");
    if cfg!(windows) {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}

fn extract_uuid(value: &str) -> Option<&str> {
    value
        .split('-')
        .collect::<Vec<_>>()
        .windows(5)
        .find_map(|parts| {
            let candidate = parts.join("-");
            let offset = value.find(&candidate)?;
            Uuid::parse_str(&candidate)
                .ok()
                .map(|_| &value[offset..offset + candidate.len()])
        })
}
