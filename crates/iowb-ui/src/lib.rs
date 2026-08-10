pub struct UiAsset {
    pub content_type: &'static str,
    pub bytes: &'static [u8],
}

pub fn get_asset(path: &str) -> Option<UiAsset> {
    let normalized = path.trim_start_matches('/');
    match normalized {
        "" | "index.html" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/index.html"),
        }),
        "api-docs.html" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/api-docs.html"),
        }),
        "clear-cache.html" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/clear-cache.html"),
        }),
        "manifest.webmanifest" => Some(UiAsset {
            content_type: "application/manifest+json; charset=utf-8",
            bytes: include_bytes!("../static/manifest.webmanifest"),
        }),
        "openapi.json" => Some(UiAsset {
            content_type: "application/json; charset=utf-8",
            bytes: include_bytes!("../static/openapi.json"),
        }),
        "icon.svg" => Some(UiAsset {
            content_type: "image/svg+xml; charset=utf-8",
            bytes: include_bytes!("../static/icon.svg"),
        }),
        "icons/codex.svg" => Some(UiAsset {
            content_type: "image/svg+xml; charset=utf-8",
            bytes: include_bytes!("../static/icons/codex.svg"),
        }),
        "icons/codex-white.svg" => Some(UiAsset {
            content_type: "image/svg+xml; charset=utf-8",
            bytes: include_bytes!("../static/icons/codex-white.svg"),
        }),
        "icons/claude-ai-icon.svg" => Some(UiAsset {
            content_type: "image/svg+xml; charset=utf-8",
            bytes: include_bytes!("../static/icons/claude-ai-icon.svg"),
        }),
        "icons/claude-white.svg" => Some(UiAsset {
            content_type: "image/svg+xml; charset=utf-8",
            bytes: include_bytes!("../static/icons/claude-white.svg"),
        }),
        "icons/gemini-ai-icon.svg" => Some(UiAsset {
            content_type: "image/svg+xml; charset=utf-8",
            bytes: include_bytes!("../static/icons/gemini-ai-icon.svg"),
        }),
        "icons/cursor.svg" => Some(UiAsset {
            content_type: "image/svg+xml; charset=utf-8",
            bytes: include_bytes!("../static/icons/cursor.svg"),
        }),
        "icons/cursor-white.svg" => Some(UiAsset {
            content_type: "image/svg+xml; charset=utf-8",
            bytes: include_bytes!("../static/icons/cursor-white.svg"),
        }),
        "sw.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/sw.js"),
        }),
        "styles.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles.css"),
        }),
        "app.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app.js"),
        }),
        "vendor/codemirror/codemirror.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/codemirror.css"),
        }),
        "vendor/codemirror/codemirror.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/codemirror.js"),
        }),
        "vendor/codemirror/LICENSE" => Some(UiAsset {
            content_type: "text/plain; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/LICENSE"),
        }),
        "vendor/codemirror/addon/matchbrackets.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/addon/matchbrackets.js"),
        }),
        "vendor/codemirror/addon/closebrackets.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/addon/closebrackets.js"),
        }),
        "vendor/codemirror/addon/simple.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/addon/simple.js"),
        }),
        "vendor/codemirror/mode/css.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/mode/css.js"),
        }),
        "vendor/codemirror/mode/gfm.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/mode/gfm.js"),
        }),
        "vendor/codemirror/mode/htmlmixed.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/mode/htmlmixed.js"),
        }),
        "vendor/codemirror/mode/javascript.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/mode/javascript.js"),
        }),
        "vendor/codemirror/mode/markdown.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/mode/markdown.js"),
        }),
        "vendor/codemirror/mode/python.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/mode/python.js"),
        }),
        "vendor/codemirror/mode/rust.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/mode/rust.js"),
        }),
        "vendor/codemirror/mode/shell.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/mode/shell.js"),
        }),
        "vendor/codemirror/mode/sql.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/mode/sql.js"),
        }),
        "vendor/codemirror/mode/toml.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/mode/toml.js"),
        }),
        "vendor/codemirror/mode/xml.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/mode/xml.js"),
        }),
        "vendor/codemirror/mode/yaml.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/mode/yaml.js"),
        }),
        "vendor/xterm/xterm.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/vendor/xterm/xterm.css"),
        }),
        "vendor/xterm/xterm.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/xterm/xterm.js"),
        }),
        "vendor/xterm/LICENSE" => Some(UiAsset {
            content_type: "text/plain; charset=utf-8",
            bytes: include_bytes!("../static/vendor/xterm/LICENSE"),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_history_treats_agent_content_as_sanitized_markdown_and_uses_snapshot() {
        let app = get_asset("app.js").expect("app.js");
        let source = std::str::from_utf8(app.bytes).expect("utf-8 JavaScript");
        assert!(source.contains("text.innerHTML = renderChatBubbleHtml(String(content));"));
        assert!(source.contains("text.textContent = String(prompt)"));
        assert!(!source.contains("text.innerHTML = content"));
        assert!(!source.contains("text.innerHTML = prompt"));
        assert!(source.contains("/snapshot?${query}"));
        assert!(source.contains("maybeLoadOlderChatMessages"));
        assert!(source.contains("chat-history-loading"));
        assert!(source.contains("before_timestamp"));
        assert!(source.contains("acceptsOrderedChatResponseEvent"));
        assert!(source.contains("replayToolLine"));
    }
}
