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
