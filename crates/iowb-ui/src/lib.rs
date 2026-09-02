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
        "landing" | "landing/" | "landing.html" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/landing.html"),
        }),
        "docs" | "docs/" | "docs.html" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/index.html"),
        }),
        "docs/quick-start" | "docs/quick-start/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/quick-start/index.html"),
        }),
        "docs/install-and-update" | "docs/install-and-update/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/install-and-update/index.html"),
        }),
        "docs/web-workspace" | "docs/web-workspace/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/web-workspace/index.html"),
        }),
        "docs/mobile" | "docs/mobile/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/mobile/index.html"),
        }),
        "docs/desktop-client" | "docs/desktop-client/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/desktop-client/index.html"),
        }),
        "docs/remote-access" | "docs/remote-access/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/remote-access/index.html"),
        }),
        "docs/agents-and-sessions" | "docs/agents-and-sessions/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/agents-and-sessions/index.html"),
        }),
        "docs/projects-files-git" | "docs/projects-files-git/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/projects-files-git/index.html"),
        }),
        "docs/database-workspace" | "docs/database-workspace/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/database-workspace/index.html"),
        }),
        "docs/terminal-and-mobile" | "docs/terminal-and-mobile/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/terminal-and-mobile/index.html"),
        }),
        "docs/boards-and-tools" | "docs/boards-and-tools/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/boards-and-tools/index.html"),
        }),
        "docs/security-and-boundaries" | "docs/security-and-boundaries/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/security-and-boundaries/index.html"),
        }),
        "docs/configuration" | "docs/configuration/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/configuration/index.html"),
        }),
        "docs/deployment-and-recovery" | "docs/deployment-and-recovery/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/deployment-and-recovery/index.html"),
        }),
        "docs/settings-and-integrations" | "docs/settings-and-integrations/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/settings-and-integrations/index.html"),
        }),
        "docs/troubleshooting" | "docs/troubleshooting/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/troubleshooting/index.html"),
        }),
        "docs/api" | "docs/api/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/api/index.html"),
        }),
        "docs/category/get-started" | "docs/category/get-started/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/category/get-started/index.html"),
        }),
        "docs/category/workbench" | "docs/category/workbench/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/category/workbench/index.html"),
        }),
        "docs/category/operations" | "docs/category/operations/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/category/operations/index.html"),
        }),
        "docs/category/reference" | "docs/category/reference/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/category/reference/index.html"),
        }),
        "docs/topic/web-workspace" | "docs/topic/web-workspace/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/topic/web-workspace/index.html"),
        }),
        "docs/topic/mobile-clients" | "docs/topic/mobile-clients/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/topic/mobile-clients/index.html"),
        }),
        "docs/topic/native-desktop-client" | "docs/topic/native-desktop-client/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/topic/native-desktop-client/index.html"),
        }),
        "docs/topic/agents-and-providers" | "docs/topic/agents-and-providers/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/topic/agents-and-providers/index.html"),
        }),
        "docs/topic/projects-and-git" | "docs/topic/projects-and-git/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/topic/projects-and-git/index.html"),
        }),
        "docs/topic/database-and-terminal" | "docs/topic/database-and-terminal/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/topic/database-and-terminal/index.html"),
        }),
        "docs/topic/remote-access-and-security" | "docs/topic/remote-access-and-security/" => {
            Some(UiAsset {
                content_type: "text/html; charset=utf-8",
                bytes: include_bytes!("../static/docs/topic/remote-access-and-security/index.html"),
            })
        }
        "docs/topic/operations-and-api" | "docs/topic/operations-and-api/" => Some(UiAsset {
            content_type: "text/html; charset=utf-8",
            bytes: include_bytes!("../static/docs/topic/operations-and-api/index.html"),
        }),
        "docs/search-index.json" => Some(UiAsset {
            content_type: "application/json; charset=utf-8",
            bytes: include_bytes!("../static/docs/search-index.json"),
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
        "sw.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/sw.js"),
        }),
        "styles.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles.css"),
        }),
        "styles/landing.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/landing.css"),
        }),
        "styles/docs.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/docs.css"),
        }),
        "app.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app.js"),
        }),
        "app/landing-theme.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/landing-theme.js"),
        }),
        "app/landing.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/landing.js"),
        }),
        "app/docs.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/docs.js"),
        }),
        "app/core.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/core.js"),
        }),
        "app/sidebar.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/sidebar.js"),
        }),
        "app/chat/prompt-history.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/chat/prompt-history.js"),
        }),
        "app/chat/drafts.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/chat/drafts.js"),
        }),
        "app/chat/history.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/chat/history.js"),
        }),
        "app/chat/recovery.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/chat/recovery.js"),
        }),
        "app/chat/stream.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/chat/stream.js"),
        }),
        "app/chat/settings.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/chat/settings.js"),
        }),
        "app/workspace/files.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/files.js"),
        }),
        "app/workspace/git.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/git.js"),
        }),
        "app/workspace/git/status.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/git/status.js"),
        }),
        "app/workspace/git/commit.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/git/commit.js"),
        }),
        "app/workspace/git/chat_composer.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/git/chat_composer.js"),
        }),
        "app/workspace/git/markdown.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/git/markdown.js"),
        }),
        "app/workspace/git/session_actions.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/git/session_actions.js"),
        }),
        "app/workspace/git/conflicts.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/git/conflicts.js"),
        }),
        "app/workspace/git/diff.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/git/diff.js"),
        }),
        "app/workspace/git/history.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/git/history.js"),
        }),
        "app/workspace/database.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/database.js"),
        }),
        "app/workspace/shell.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/shell.js"),
        }),
        "app/workspace/settings.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/settings.js"),
        }),
        "app/workspace/websocket.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/workspace/websocket.js"),
        }),
        "app/navigation.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/navigation.js"),
        }),
        "app/board.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/board.js"),
        }),
        "app/commands.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/commands.js"),
        }),
        "app/forms.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/forms.js"),
        }),
        "app/startup.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/app/startup.js"),
        }),
        "styles/base.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/base.css"),
        }),
        "styles/sidebar.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/sidebar.css"),
        }),
        "styles/layout.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/layout.css"),
        }),
        "styles/workspace.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/workspace.css"),
        }),
        "styles/chat.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/chat.css"),
        }),
        "styles/shell-and-git.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/shell-and-git.css"),
        }),
        "styles/output.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/output.css"),
        }),
        "styles/board.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/board.css"),
        }),
        "styles/responsive/large.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/responsive/large.css"),
        }),
        "styles/responsive/mobile.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/responsive/mobile.css"),
        }),
        "styles/responsive/touch-and-small.css" => Some(UiAsset {
            content_type: "text/css; charset=utf-8",
            bytes: include_bytes!("../static/styles/responsive/touch-and-small.css"),
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
        "vendor/codemirror/addon/mode/overlay.js" => Some(UiAsset {
            content_type: "application/javascript; charset=utf-8",
            bytes: include_bytes!("../static/vendor/codemirror/addon/mode/overlay.js"),
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

    fn app_source() -> String {
        [
            "app.js",
            "app/core.js",
            "app/sidebar.js",
            "app/chat/prompt-history.js",
            "app/chat/drafts.js",
            "app/chat/history.js",
            "app/chat/recovery.js",
            "app/chat/stream.js",
            "app/chat/settings.js",
            "app/workspace/files.js",
            "app/workspace/git.js",
            "app/workspace/git/status.js",
            "app/workspace/git/commit.js",
            "app/workspace/git/chat_composer.js",
            "app/workspace/git/markdown.js",
            "app/workspace/git/session_actions.js",
            "app/workspace/git/conflicts.js",
            "app/workspace/git/diff.js",
            "app/workspace/git/history.js",
            "app/workspace/database.js",
            "app/workspace/shell.js",
            "app/workspace/settings.js",
            "app/workspace/websocket.js",
            "app/navigation.js",
            "app/board.js",
            "app/commands.js",
            "app/forms.js",
            "app/startup.js",
        ]
        .into_iter()
        .fold(String::new(), |mut source, path| {
            source.push_str(asset_text(path));
            source
        })
    }

    fn styles_source() -> String {
        [
            "styles.css",
            "styles/base.css",
            "styles/sidebar.css",
            "styles/layout.css",
            "styles/workspace.css",
            "styles/chat.css",
            "styles/shell-and-git.css",
            "styles/output.css",
            "styles/board.css",
            "styles/responsive/large.css",
            "styles/responsive/mobile.css",
            "styles/responsive/touch-and-small.css",
        ]
        .into_iter()
        .fold(String::new(), |mut source, path| {
            source.push_str(asset_text(path));
            source
        })
    }

    #[test]
    fn chat_history_treats_agent_content_as_sanitized_markdown_and_uses_snapshot() {
        let source = app_source();
        assert!(source.contains("text.innerHTML = renderChatBubbleHtml(String(content));"));
        assert!(source.contains("text.textContent = String(message?.content || \"\")"));
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
    fn chat_context_recovery_blocks_normal_send_and_retries_with_a_fresh_request() {
        let source = app_source();
        let remember_recovery = source
            .split_once("function rememberChatRecovery(sessionId, recovery) {")
            .map(|(_, rest)| rest)
            .expect("chat recovery state helper")
            .split_once("\n}\n")
            .map(|(body, _)| body)
            .expect("chat recovery state helper end");
        let submit_handler = source
            .split_once("qs(\"#chat-form\").addEventListener(\"submit\", async (event) => {")
            .map(|(_, rest)| rest)
            .expect("chat submit handler");

        assert!(source.contains(
            "return Boolean(recovery && [\"required\", \"failed\", \"starting\"].includes(recovery.state));"
        ));
        assert!(source.contains("const requestId = chatRecoveryRequestId();"));
        assert!(source.contains("recovery.requestId = requestId;"));
        assert!(!source.contains("recovery.requestId ||= chatRecoveryRequestId();"));
        assert!(source.contains(
            "const attemptIsCurrent = () => state.chatRecoveryBySession[sid] === recovery"
        ));
        assert!(source.contains("if (responseState === \"failed\") {"));
        assert!(source.contains("if (responseState !== \"starting\") {"));
        assert!(source.contains("scheduleCompletedChatReconciliation(sid);"));
        let recovery_request = source
            .split_once("async function compactAndRetryChatContext(sessionId, button = null) {")
            .map(|(_, rest)| rest)
            .expect("compact and retry handler")
            .split_once("\n}\n\nfunction renderChatManualCompactionCard")
            .map(|(body, _)| body)
            .expect("compact and retry handler end");
        assert_eq!(
            recovery_request
                .matches("if (!attemptIsCurrent()) return;")
                .count(),
            2
        );
        let starting_branch = recovery_request
            .split_once("if (responseState !== \"starting\") {")
            .map(|(_, rest)| rest)
            .expect("non-starting response branch")
            .split_once("ensureChatProcessing({ provider: \"codex\", sessionId: sid });")
            .map(|(_, tail)| tail)
            .expect("starting processing branch");
        assert!(starting_branch.contains("updateProcessingLabel(\"Compacting context\");"));
        assert!(source.contains("Clean-context compaction failed"));
        assert!(source.contains("Your original context mapping is still intact."));
        assert!(remember_recovery.contains("clearChatProcessing();"));
        assert!(remember_recovery.contains(
            "rememberCurrentChatSession({ sessionId: sid, live: false, status: \"failed\" });"
        ));
        assert!(submit_handler.contains("if (chatRecoveryBlocksNormalSend(recovery)) {"));
        assert!(
            submit_handler.contains("Compact & retry this chat before sending another message.")
        );
        assert!(source.contains("function chatRecoveryMatchesResponse(recovery, payload = {})"));
        assert!(source.contains("return Boolean(expected && observed && expected === observed);"));
        assert!(source.contains("sessionRecovery?.state === \"starting\" && chatRecoveryMatchesResponse(sessionRecovery, payload)"));
        assert!(source.contains(
            "async function reconcileSelectedChatRecoverySnapshot(generation = state.wsGeneration)"
        ));
        assert!(source.contains("/snapshot?limit=1`"));
        assert!(
            source.contains("reconcileSelectedChatRecoverySnapshot(generation).catch((error) => {")
        );
        assert!(source.contains("function chatRecoveryHasOptimisticTimelineState(sessionId)"));
        assert!(source.contains("function handleChatRecoveryRequired(payload = {})"));
        assert!(source.contains("writeLocalChatPromptDraft(rejectedPrompt, sessionId);"));
        assert!(source.contains("scheduleChatPromptDraftSave();"));
        assert!(source.contains(
            "loadChatHistoryForSession(sessionId, { forceBottom: true }).catch((error) => {"
        ));
        assert!(source.contains("handleChatRecoveryRequired(payload);"));
    }

    #[test]
    fn edit_from_here_stages_locally_and_replaces_only_on_submit() {
        let source = app_source();

        assert!(source.contains("state.chatEditFromHere.staged = staged;"));
        assert!(source.contains("renderStagedChatEdit(staged);"));
        assert!(source.contains("const stagedEdit = stagedChatEdit();"));
        assert!(source.contains("replace: true,"));
        assert!(source.contains("draftContent,"));
        assert!(source.contains("if (replacement.sourceHidden === true)"));
        assert!(!source.contains("Create a new chat before this prompt?"));
    }

    #[test]
    fn active_chat_stream_is_cached_and_restored_across_session_switches() {
        let source = app_source();
        let snapshot_loader = source
            .split_once("async function loadChatHistoryForSession(sessionId, opts = {}) {")
            .map(|(_, rest)| rest)
            .expect("chat snapshot loader")
            .split_once("\n}\n\nfunction scheduleChatReconciliation")
            .map(|(body, _)| body)
            .expect("chat snapshot loader end");
        let session_picker = source
            .split_once("async function pickChatSession(sessionId, projectPath, options = {}) {")
            .map(|(_, rest)| rest)
            .expect("chat session picker")
            .split_once("\n}\n\n// Start a fresh chat for a project.")
            .map(|(body, _)| body)
            .expect("chat session picker end");

        assert!(source.contains("function preserveActiveChatStreamSnapshot"));
        assert!(source.contains("function restoreChatStreamSnapshot"));
        assert!(source.contains("streamingBuffer: (patch.live ?? chatSessionIsLive(sessionId))"));
        assert!(source.contains(
            "if (sessionId) state.chatOutputBuffersBySession[sessionId] = state.chatBuffer;"
        ));
        assert!(
            source.contains("opts.provider || state.currentSession?.provider || chatCliValue()")
        );
        assert!(source.contains("delete state.chatOutputBuffersBySession[sid];"));
        assert!(session_picker.contains("preserveActiveChatStreamSnapshot();"));
        assert!(
            snapshot_loader
                .contains("const preservedStream = preserveActiveChatStreamSnapshot(sessionId);")
        );
        assert!(snapshot_loader.contains("const replayMessages = split.messages;"));
        assert!(snapshot_loader.contains("restoreChatStreamSnapshot(sessionId, streamSnapshot);"));
    }

    #[test]
    fn web_chat_sidebar_regressions_stay_fixed() {
        let source = app_source();
        let project_sessions = source
            .split_once("function sidebarProjectSessions(project) {")
            .map(|(_, rest)| rest)
            .expect("sidebar project sessions helper")
            .split_once("\n}\n")
            .map(|(body, _)| body)
            .expect("sidebar project sessions helper end");

        assert!(source.contains("function clearChatPromptInput()"));
        assert!(source.contains("input.style.removeProperty(\"height\");"));
        assert!(source.matches("clearChatPromptInput();").count() >= 2);
        assert!(project_sessions.contains("(project.sessions || [])"));
        assert!(!project_sessions.contains("state.sessions"));
        assert!(!project_sessions.contains("slice(0, 12)"));
    }

    #[test]
    fn board_task_transcripts_are_read_only_and_use_complete_snapshots() {
        let source = app_source();
        let opener = source
            .split_once("async function openBoardTaskTranscript(taskId) {")
            .map(|(_, rest)| rest)
            .expect("board task chat opener")
            .split_once("\n}\n\nfunction renderBoardCard")
            .map(|(body, _)| body)
            .expect("board task chat opener end");
        let snapshot_merge = source
            .split_once("async function loadChatHistoryForSession(sessionId, opts = {}) {")
            .map(|(_, rest)| rest)
            .expect("chat snapshot loader")
            .split_once("\n}\n\nfunction scheduleChatReconciliation")
            .map(|(body, _)| body)
            .expect("chat snapshot loader end");

        assert!(source.contains("task?.providerSessionId"));
        assert!(source.contains("task?.provider_session_id"));
        assert!(source.contains("data-board-view-transcript"));
        assert!(source.contains("async function loadBoardSessionTranscript(sessionId)"));
        assert!(opener.contains("loadBoardSessionTranscript(sessionId)"));
        assert!(!source.contains("data-board-open-chat=\""));
        assert!(source.contains("return Boolean(boardTaskSessionId(task));"));
        assert!(
            snapshot_merge
                .contains("const boardSession = isBoardChatSession(snapshotSession, sessionId);")
        );
        assert!(snapshot_merge.contains("hideBoardChatSessionsFromLists();"));
        assert!(source.contains("state.sessions = (payload.sessions || []).filter((session) => !isBoardChatSession(session));"));
        assert!(source.contains(
            "state.boardChatSessionIds.has(sessionId) && !isActiveChatSessionEvent({ sessionId })"
        ));
        assert!(source.contains("if (state.boardChatSessionIds.has(sessionId)) return false;"));
        assert!(source.contains(
            "state.projects = payload.projects || [];\n      hideBoardChatSessionsFromLists();"
        ));
        assert!(source.contains(
            "state.projects = body.projects || [];\n  hideBoardChatSessionsFromLists();"
        ));
        assert!(source.contains("sessionIds: boardSessionId ? [boardSessionId] : []"));
        assert!(
            source
                .contains("setBoardChatWsSubscription(options.boardSession === true ? id : \"\");")
        );
        assert!(source.contains(
            "if (view !== \"chat\" && state.boardWsSessionId) setBoardChatWsSubscription(\"\");"
        ));
        assert!(source.contains("if (isSelectedBoardChatSession()) {\n      setBoardChatWsSubscription(state.chatSessionId);"));
        assert!(source.contains("setBoardChatWsSubscription(destination.id);"));
        assert!(
            source.contains("if (payload.sessionId && !isActiveChatSessionEvent(payload)) return;")
        );
    }

    #[test]
    fn board_mobile_config_exposes_malformed_tool_call_repair_policy() {
        let html = asset_text("index.html");
        let source = app_source();

        for id in ["board-tool-repair-enabled", "board-tool-repair-retries"] {
            assert!(html.contains(&format!(r#"id="{id}""#)), "missing {id}");
        }
        assert!(source.contains("const repairMalformedToolCalls ="));
        assert!(source.contains("const toolRepairRetries ="));
        assert!(source.contains("repairMalformedToolCalls,"));
        assert!(source.contains("malformedToolCallRepairRetries: toolRepairRetries"));
        assert!(source.contains("Tool repair:"));
    }

    #[test]
    fn openapi_documents_board_scope_and_direct_snapshot_access() {
        let source = asset_text("openapi.json");

        assert!(source.contains("\"boardSession\""));
        assert!(source.contains("\"boardId\""));
        assert!(source.contains("\"boardTaskId\""));
        assert!(source.contains("\"/api/sessions/{session_id}/snapshot\""));
        assert!(source.contains("excluded from ordinary project/session discovery"));
    }

    #[test]
    fn pinned_chat_sync_accepts_authoritative_empty_and_refreshes_live() {
        let source = app_source();

        assert!(source.contains("response?.initialized === true"));
        assert!(source.contains("response?.initialized == null && remotePinned.length > 0"));
        assert!(source.contains("state.pinnedChatSessionsDirty"));
        assert!(source.contains("if (state.pinnedChatSessionsDirty) {"));
        assert!(source.contains("loadGeneration === state.pinnedChatSessionsLoadGeneration"));
        assert!(source.contains("state.pinnedChatSessionsSaveChain"));
        assert!(source.contains("document.addEventListener(\"visibilitychange\""));
        assert!(source.contains("if (opening) {"));
    }

    #[test]
    fn mobile_chat_composer_cannot_expand_past_viewport() {
        let styles = styles_source();

        assert!(styles.contains("grid-template-columns: minmax(0, 1fr);"));
        assert!(styles.contains(".chat-composer {"));
        assert!(styles.contains("max-width: 100%;"));
        assert!(styles.contains(".chat-controls > label:first-child {"));
        assert!(styles.contains("grid-column: 1 / -1;"));
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
    fn chat_fast_controls_share_the_session_aware_setter() {
        let html = asset_text("index.html");
        let source = app_source();

        assert!(html.contains(r#"id="chat-fast-toggle""#));
        assert!(source.contains(r#"data-chat-fast-setting"#));
        assert!(source.contains("function setChatFastRequested(requested)"));
        assert!(source.contains("saveSessionOverrides(sid, { fast: next });"));
        assert_eq!(
            source
                .matches("setChatFastRequested(event.currentTarget.checked);")
                .count(),
            2
        );
        assert!(source.contains(r#"fast: chatFastValue(),"#));
    }

    #[test]
    fn web_chat_turn_cards_match_mobile_session_actions() {
        let source = app_source();
        let styles = styles_source();

        assert!(source.contains("function chatResponsePresentation(messages, options = {})"));
        assert!(source.contains("showResponseHeader: true"));
        assert!(source.contains("showResponseFooter: true"));
        assert!(source.contains("function renderChatResponseHeader(node, meta = {})"));
        assert!(source.contains("function attachUserChatActions(node, message, options = {})"));
        assert!(source.contains("Copy prompt"));
        assert!(source.contains("Copy response"));
        assert!(source.contains("withoutChatToolTelemetrySections(content)"));
        assert!(source.contains("replayChatMessages(replayMessages, {"));
        assert!(styles.contains(".chat-response-header"));
        assert!(styles.contains(".chat-line-assistant .chat-line-actions"));
    }

    #[test]
    fn sidebar_session_menu_copies_codex_resume_command() {
        let source = app_source();

        assert!(source.contains("function codexResumeCommand(session, projectPath = \"\")"));
        assert!(source.contains("Copy Codex Resume Command"));
        assert!(source.contains("cd ${shellSingleQuote(path)} && ${resume}"));
        assert!(source.contains("data-sidebar-session-card"));
        assert!(source.contains("openSessionContextMenuFromRow(row"));
    }

    #[test]
    fn io_gateway_settings_match_the_mobile_configuration_contract() {
        let html = asset_text("index.html");
        let source = app_source();

        for id in [
            "io-gateway-enabled",
            "io-gateway-url",
            "io-gateway-api-key",
            "io-gateway-otp-secret",
            "save-direct-ai",
        ] {
            assert!(html.contains(&format!(r#"id="{id}""#)), "missing {id}");
        }
        assert!(source.contains("/api/settings/direct-ai?revealSecrets=true"));
        assert!(source.contains("API key is required when IO Gateway is selected."));
        assert!(source.contains("Gateway URL must start with http:// or https://."));
        assert!(source.contains("Secret OTP must be a valid Base32 TOTP secret."));
        assert!(source.contains("mode: \"aiproxy\""));
        assert!(source.contains("chatRuntime: useIoGateway ? \"io_gateway\" : \"native_cli\""));
        assert!(source.contains("baseUrl: `${normalizedUrl}/claude`"));
        assert!(source.contains("gatewayUrl: normalizedUrl"));
    }

    #[test]
    fn chat_model_loader_preserves_empty_cli_default_value() {
        let source = app_source();
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
    fn file_editor_supports_markdown_preview_and_full_view() {
        let html = asset_text("index.html");
        let source = app_source();
        let styles = styles_source();

        for id in [
            "file-editor-mode-toggle",
            "file-edit-mode",
            "file-preview-mode",
            "editor-full-view",
            "file-editor-shell",
            "file-editor-preview",
        ] {
            assert!(html.contains(&format!(r#"id="{id}""#)), "missing {id}");
        }
        assert!(html.contains(r#"data-file-editor-mode="edit""#));
        assert!(html.contains(r#"data-file-editor-mode="preview""#));
        assert!(html.contains(r#"aria-label="Formatted Markdown preview""#));

        for helper in [
            "function isMarkdownFile(filePath = \"\")",
            "function renderFileMarkdownPreview()",
            "function syncFileEditorModeUi()",
            "function setFileEditorMode(mode)",
            "function setFileEditorFullView(enabled",
            "function toggleFileEditorFullView()",
        ] {
            assert!(source.contains(helper), "missing {helper}");
        }
        assert!(source.contains("typeof renderMarkdownSegment === \"function\""));
        assert!(source.contains("renderer(content)"));
        assert!(source.contains("state.fileEditorMode = \"edit\""));
        assert!(source.contains("state.fileEditorFullView = false"));
        assert!(source.contains("setFileEditorFullView(false);"));
        assert!(source.contains(
            "qs(\"#editor-full-view\")?.addEventListener(\"click\", toggleFileEditorFullView);"
        ));
        assert!(source.contains("if (state.fileEditorFullView)"));

        for selector in [
            ".file-editor-pane.file-editor-preview-mode",
            ".editor-shell.file-editor-preview-mode",
            ".markdown-file-preview",
            ".markdown-file-content",
            ".file-editor-pane.file-editor-full-view",
            "body.file-editor-full-view",
        ] {
            assert!(styles.contains(selector), "missing {selector}");
        }
    }

    #[test]
    fn documentation_assets_are_embedded_and_linked_from_the_landing_page() {
        let docs_home = asset_text("docs/");
        let search_index = asset_text("docs/search-index.json");
        let landing = asset_text("landing");
        let docs_css = asset_text("styles/docs.css");
        let docs_script = asset_text("app/docs.js");

        // Keep this list in step with `scripts/generate-docs.mjs`. Checking both
        // forms of each path catches a missing embedded route as well as a route
        // accidentally wired to the wrong generated article.
        let generated_routes = [
            ("docs/quick-start", "quick-start", "Quick start"),
            (
                "docs/install-and-update",
                "install-and-update",
                "Install and update",
            ),
            ("docs/web-workspace", "web-workspace", "Web workspace"),
            ("docs/mobile", "mobile", "Mobile apps"),
            (
                "docs/desktop-client",
                "desktop-client",
                "Native desktop client",
            ),
            ("docs/remote-access", "remote-access", "Remote access"),
            (
                "docs/agents-and-sessions",
                "agents-and-sessions",
                "Agents and sessions",
            ),
            (
                "docs/projects-files-git",
                "projects-files-git",
                "Projects, files, and Git",
            ),
            (
                "docs/database-workspace",
                "database-workspace",
                "Database workspace",
            ),
            (
                "docs/terminal-and-mobile",
                "terminal-and-mobile",
                "Terminal and mobile controls",
            ),
            (
                "docs/boards-and-tools",
                "boards-and-tools",
                "Boards and tools",
            ),
            (
                "docs/security-and-boundaries",
                "security-and-boundaries",
                "Security and boundaries",
            ),
            ("docs/configuration", "configuration", "Configuration"),
            (
                "docs/deployment-and-recovery",
                "deployment-and-recovery",
                "Deployment and recovery",
            ),
            (
                "docs/settings-and-integrations",
                "settings-and-integrations",
                "Settings and integrations",
            ),
            ("docs/troubleshooting", "troubleshooting", "Troubleshooting"),
            ("docs/api", "api", "API reference"),
            (
                "docs/category/get-started",
                "category-get-started",
                "Get started",
            ),
            (
                "docs/category/workbench",
                "category-workbench",
                "Workbench workflows",
            ),
            (
                "docs/category/operations",
                "category-operations",
                "Operate safely",
            ),
            ("docs/category/reference", "category-reference", "Reference"),
            (
                "docs/topic/web-workspace",
                "topic-web-workspace",
                "Web workspace",
            ),
            (
                "docs/topic/mobile-clients",
                "topic-mobile-clients",
                "Mobile clients",
            ),
            (
                "docs/topic/native-desktop-client",
                "topic-native-desktop-client",
                "Native desktop client",
            ),
            (
                "docs/topic/agents-and-providers",
                "topic-agents-and-providers",
                "Agents and providers",
            ),
            (
                "docs/topic/projects-and-git",
                "topic-projects-and-git",
                "Projects and Git",
            ),
            (
                "docs/topic/database-and-terminal",
                "topic-database-and-terminal",
                "Database and terminal",
            ),
            (
                "docs/topic/remote-access-and-security",
                "topic-remote-access-and-security",
                "Remote access and security",
            ),
            (
                "docs/topic/operations-and-api",
                "topic-operations-and-api",
                "Operations and API",
            ),
        ];

        assert_eq!(
            get_asset("docs").unwrap().content_type,
            "text/html; charset=utf-8"
        );
        assert_eq!(
            get_asset("docs.html").unwrap().content_type,
            "text/html; charset=utf-8"
        );
        for (route, page_slug, title) in generated_routes {
            let canonical_route = format!("{route}/");
            let canonical_asset = get_asset(&canonical_route)
                .unwrap_or_else(|| panic!("missing generated docs route: {canonical_route}"));
            let bare_asset =
                get_asset(route).unwrap_or_else(|| panic!("missing generated docs route: {route}"));

            assert_eq!(
                canonical_asset.content_type, "text/html; charset=utf-8",
                "wrong content type for {canonical_route}"
            );
            assert_eq!(
                bare_asset.content_type, canonical_asset.content_type,
                "route aliases differ for {route}"
            );
            assert_eq!(
                bare_asset.bytes, canonical_asset.bytes,
                "route aliases serve different pages for {route}"
            );

            let html = std::str::from_utf8(canonical_asset.bytes)
                .unwrap_or_else(|_| panic!("non-UTF-8 generated docs route: {canonical_route}"));
            assert!(
                html.contains(&format!(r#"data-page-slug="{page_slug}""#)),
                "generated docs route is wired to the wrong article: {canonical_route}"
            );
            assert!(
                html.contains(&format!("<h1>{title}</h1>")),
                "generated docs route is missing its expected title: {canonical_route}"
            );
        }

        assert_eq!(
            get_asset("docs/search-index.json").unwrap().content_type,
            "application/json; charset=utf-8"
        );
        assert!(landing.contains(r#"href="/docs/""#));
        assert!(docs_home.contains(r#"href="/styles/docs.css?v="#));
        assert!(docs_home.contains(r#"src="/app/docs.js?v="#));
        assert!(docs_home.contains(r#"href="/docs/web-workspace/""#));
        assert!(docs_home.contains(r#"href="/docs/install-and-update/""#));
        assert!(docs_home.contains(r#"href="/docs/mobile/""#));
        assert!(docs_home.contains(r#"href="/docs/desktop-client/""#));
        assert!(docs_home.contains(r#"href="/docs/topic/operations-and-api/""#));
        assert!(search_index.contains(r#""href": "/docs/mobile/""#));
        assert!(search_index.contains(r#""href": "/docs/install-and-update/""#));
        assert!(search_index.contains(r#""href": "/docs/topic/mobile-clients/""#));
        assert!(docs_css.contains(".docs-layout"));
        assert!(docs_css.contains(".docs-topic-links"));
        assert!(docs_script.contains("function installSearch()"));
        assert!(docs_script.contains("function installActivePageState()"));
        assert!(docs_script.contains(r#"fetch("/docs/search-index.json""#));
        assert!(!docs_script.contains("docsTopicByPath"));
    }

    #[test]
    fn web_shell_asset_versions_stay_in_sync() {
        let app = app_source();
        let html = asset_text("index.html");
        let service_worker = asset_text("sw.js");
        let styles = styles_source();
        let version = app_version(&app);

        assert_eq!(app_version(service_worker), version);
        assert!(html.contains(&format!(r#"/styles.css?v={version}"#)));
        assert!(html.contains(&format!(r#"/app.js?v={version}"#)));
        assert!(styles.contains(&format!("?v={version}")));
        assert!(service_worker.contains("const CACHE_NAME = `io-workbench-web-${APP_VERSION}`;"));
        assert!(service_worker.contains("`/styles.css?v=${APP_VERSION}`"));
        assert!(service_worker.contains("`/app.js?v=${APP_VERSION}`"));
    }
}
