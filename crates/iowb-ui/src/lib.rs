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

    fn asset_text(path: &str) -> &'static str {
        let asset = get_asset(path).unwrap_or_else(|| panic!("missing UI asset: {path}"));
        std::str::from_utf8(asset.bytes).unwrap_or_else(|_| panic!("non-UTF-8 UI asset: {path}"))
    }

    fn select_option_values<'a>(html: &'a str, select_id: &str) -> Vec<&'a str> {
        let marker = format!(r#"<select id="{select_id}""#);
        let select_start = html
            .find(&marker)
            .unwrap_or_else(|| panic!("missing select: {select_id}"));
        let select = &html[select_start..];
        let select_end = select
            .find("</select>")
            .unwrap_or_else(|| panic!("unterminated select: {select_id}"));
        let mut options = &select[..select_end];
        let option_marker = r#"<option value=""#;
        let mut values = Vec::new();

        while let Some(option_start) = options.find(option_marker) {
            let value = &options[option_start + option_marker.len()..];
            let value_end = value
                .find('"')
                .unwrap_or_else(|| panic!("unterminated option value in select: {select_id}"));
            values.push(&value[..value_end]);
            options = &value[value_end + 1..];
        }

        values
    }

    fn app_version(source: &str) -> &str {
        let marker = r#"const APP_VERSION = ""#;
        let version = source
            .split_once(marker)
            .map(|(_, rest)| rest)
            .expect("APP_VERSION declaration");
        version
            .split_once('"')
            .map(|(value, _)| value)
            .expect("APP_VERSION value")
    }

    #[test]
    fn chat_history_treats_agent_content_as_sanitized_markdown_and_uses_snapshot() {
        let source = asset_text("app.js");
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

    #[test]
    fn chat_override_options_match_mobile_order() {
        let html = asset_text("index.html");

        assert_eq!(
            select_option_values(html, "chat-mode"),
            vec!["default", "plan", "accept-edits", "bypass"]
        );
        assert_eq!(
            select_option_values(html, "chat-effort"),
            vec!["low", "medium", "high", "xhigh", "max", "ultra"]
        );
    }

    #[test]
    fn chat_model_loader_preserves_empty_cli_default_value() {
        let source = asset_text("app.js");
        let object_branch_start = source
            .find(r#"if (typeof entry === "object")"#)
            .expect("chat model object normalization branch");
        let object_branch = &source[object_branch_start..];
        let object_branch_end = object_branch
            .find("\n        }\n        return null;")
            .expect("chat model object normalization branch end");
        let object_branch = &object_branch[..object_branch_end];

        assert!(object_branch.contains("const value = entry.value ?? entry.id ?? entry.name;"));
        assert!(object_branch.contains("if (value === null || value === undefined) return null;"));
        assert!(object_branch.contains("value: normalizedValue"));
        assert!(!object_branch.contains("if (!value)"));
    }

    #[test]
    fn web_shell_asset_versions_stay_in_sync() {
        let app = asset_text("app.js");
        let html = asset_text("index.html");
        let service_worker = asset_text("sw.js");
        let version = app_version(app);

        assert_eq!(app_version(service_worker), version);
        assert!(html.contains(&format!(r#"/styles.css?v={version}"#)));
        assert!(html.contains(&format!(r#"/app.js?v={version}"#)));
        assert!(service_worker.contains("const CACHE_NAME = `io-workbench-web-${APP_VERSION}`;"));
        assert!(service_worker.contains("`/styles.css?v=${APP_VERSION}`"));
        assert!(service_worker.contains("`/app.js?v=${APP_VERSION}`"));
    }
}
