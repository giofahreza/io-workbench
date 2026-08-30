struct DirectAiConfig {
    mode: String,
    base_url: Option<String>,
    api_key_env: Option<String>,
    model: Option<String>,
}

impl DirectAiConfig {
    fn from_value(value: Option<Value>) -> Self {
        let value = value.unwrap_or(Value::Null);
        Self {
            mode: value
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("off")
                .to_string(),
            base_url: value
                .get("baseUrl")
                .or_else(|| value.get("base_url"))
                .and_then(Value::as_str)
                .map(str::to_string),
            api_key_env: value
                .get("apiKeyEnv")
                .or_else(|| value.get("api_key_env"))
                .and_then(Value::as_str)
                .map(str::to_string),
            model: value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }

    fn is_enabled(&self) -> bool {
        !matches!(self.mode.as_str(), "off" | "")
    }

    fn base_url(&self) -> Result<String> {
        if let Some(base_url) = self
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(base_url.trim_end_matches('/').to_string());
        }

        match self.mode.as_str() {
            "direct" | "anthropic" => Ok("https://api.anthropic.com".to_string()),
            "minimax" => Ok("https://api.minimax.io/anthropic".to_string()),
            "proxy" | "aiproxy" => Ok("http://141.144.197.96:8319/claude".to_string()),
            _ => Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Direct AI baseUrl is required",
            )),
        }
    }

    fn api_key(&self) -> Option<String> {
        let configured_key = self
            .api_key_env
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|key| std::env::var(key).ok());
        configured_key
            .or_else(|| match self.mode.as_str() {
                "direct" | "anthropic" => std::env::var("ANTHROPIC_API_KEY")
                    .or_else(|_| std::env::var("ANTHROPIC_AUTH_TOKEN"))
                    .ok(),
                "minimax" => std::env::var("MINIMAX_API_KEY")
                    .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
                    .ok(),
                _ => std::env::var("CODEX_GATEWAY_KEY").ok(),
            })
            .filter(|value| !value.trim().is_empty())
    }

    fn model(&self) -> String {
        self.model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("claude-haiku-4-5-20251001")
            .to_string()
    }
}

async fn generate_commit_message_with_ai(
    files: &[String],
    diff_context: &str,
    config: Option<Value>,
) -> Result<String> {
    let config = DirectAiConfig::from_value(config);
    if !config.is_enabled() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Direct AI is off",
        ));
    }

    let prompt = commit_message_prompt(files, diff_context);
    let raw = call_direct_ai(&config, &prompt, 512).await?;
    Ok(clean_commit_message(&raw))
}

fn commit_message_prompt(files: &[String], diff_context: &str) -> String {
    let files = files
        .iter()
        .map(|file| format!("- {file}"))
        .collect::<Vec<_>>()
        .join("\n");
    let diff_context = diff_context.chars().take(6000).collect::<String>();
    format!(
        "Generate a conventional commit message for these changes.\n\n\
RULES:\n\
- Format: type(scope): subject\n\
- Include body explaining what changed and why\n\
- Types: feat, fix, docs, style, refactor, perf, test, build, ci, chore\n\
- Subject under 50 chars, body wrapped at 72 chars\n\
- Focus on user-facing changes, not implementation details\n\
- Return ONLY the commit message (no markdown, explanations, or code blocks)\n\n\
FILES CHANGED:\n{files}\n\n\
DIFFS:\n{diff_context}\n\n\
Commit message:"
    )
}

async fn call_direct_ai(config: &DirectAiConfig, prompt: &str, max_tokens: u64) -> Result<String> {
    let api_key = config.api_key().ok_or_else(|| {
        ServerError::new(
            StatusCode::BAD_REQUEST,
            "Direct AI API key is not available in the server environment",
        )
    })?;
    let base_url = config.base_url()?;
    let model = config.model();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create Direct AI client",
                error.to_string(),
            )
        })?;

    let messages_body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let response = post_direct_ai_json(
        &client,
        &format!("{base_url}/v1/messages"),
        &api_key,
        &messages_body,
    )
    .await?;

    let value = if response.status().is_success() {
        response
            .json::<Value>()
            .await
            .map_err(direct_ai_json_error)?
    } else if matches!(response.status().as_u16(), 400 | 404 | 405)
        && matches!(config.mode.as_str(), "proxy" | "aiproxy")
    {
        let chat_body = serde_json::json!({
            "model": config.model(),
            "max_tokens": max_tokens,
            "messages": [{ "role": "user", "content": prompt }],
        });
        let chat_response = post_direct_ai_json(
            &client,
            &format!("{base_url}/v1/chat/completions"),
            &api_key,
            &chat_body,
        )
        .await?;
        if !chat_response.status().is_success() {
            return Err(direct_ai_http_error(chat_response).await);
        }
        chat_response
            .json::<Value>()
            .await
            .map_err(direct_ai_json_error)?
    } else {
        return Err(direct_ai_http_error(response).await);
    };

    Ok(extract_response_text(&value))
}

async fn post_direct_ai_json(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &Value,
) -> std::result::Result<reqwest::Response, ServerError> {
    client
        .post(url)
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .bearer_auth(api_key)
        .header("x-api-key", api_key)
        .json(body)
        .send()
        .await
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::BAD_GATEWAY,
                "Direct AI request failed",
                error.to_string(),
            )
        })
}

async fn direct_ai_http_error(response: reqwest::Response) -> ServerError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    ServerError::with_details(
        StatusCode::BAD_GATEWAY,
        format!("Direct AI HTTP {status}"),
        body.chars().take(300).collect::<String>(),
    )
}

fn direct_ai_json_error(error: reqwest::Error) -> ServerError {
    ServerError::with_details(
        StatusCode::BAD_GATEWAY,
        "Direct AI returned invalid JSON",
        error.to_string(),
    )
}

fn extract_response_text(value: &Value) -> String {
    collect_text(value.get("content"))
        .or_else(|| {
            value
                .get("choices")
                .and_then(Value::as_array)
                .map(|choices| {
                    choices
                        .iter()
                        .filter_map(|choice| {
                            collect_text(
                                choice
                                    .get("message")
                                    .and_then(|message| message.get("content")),
                            )
                            .or_else(|| {
                                collect_text(
                                    choice.get("delta").and_then(|delta| delta.get("content")),
                                )
                            })
                            .or_else(|| {
                                choice
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                        })
                        .collect::<Vec<_>>()
                        .join("")
                })
        })
        .or_else(|| {
            value
                .get("output_text")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn collect_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .or_else(|| item.get("text").and_then(Value::as_str).map(str::to_string))
                        .or_else(|| collect_text(item.get("content")))
                        .or_else(|| {
                            item.get("output_text")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                })
                .collect::<Vec<_>>()
                .join("");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn clean_commit_message(text: &str) -> String {
    let mut cleaned = text.trim().replace("```text", "").replace("```", "");
    while cleaned.starts_with('#') {
        cleaned = cleaned.trim_start_matches('#').trim_start().to_string();
    }
    cleaned = cleaned.trim_matches(['"', '\'']).to_string();
    while cleaned.contains("\n\n\n") {
        cleaned = cleaned.replace("\n\n\n", "\n\n");
    }
    if let Some(index) = conventional_commit_index(&cleaned) {
        cleaned = cleaned[index..].to_string();
    }
    cleaned.trim().to_string()
}

fn conventional_commit_index(text: &str) -> Option<usize> {
    const TYPES: &[&str] = &[
        "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore",
    ];
    TYPES
        .iter()
        .filter_map(|kind| {
            text.find(&format!("{kind}:"))
                .or_else(|| text.find(&format!("{kind}(")))
        })
        .min()
}

fn fallback_commit_message(files: &[String]) -> String {
    let kind = if files.len() == 1 { "file" } else { "files" };
    format!("chore: update {} {kind}", files.len())
}

fn join_output(output: &GitOutput) -> String {
    format!("{}\n{}", output.stdout, output.stderr)
        .trim()
        .to_string()
}

fn join_output_or(output: &GitOutput, fallback: &str) -> String {
    let joined = join_output(output);
    if joined.is_empty() {
        fallback.to_string()
    } else {
        joined
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn io_server_error(error: std::io::Error) -> ServerError {
    if error.kind() == std::io::ErrorKind::NotFound {
        ServerError::new(StatusCode::NOT_FOUND, "path not found")
    } else {
        ServerError::with_details(
            StatusCode::INTERNAL_SERVER_ERROR,
            "io error",
            error.to_string(),
        )
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn pathdiff(path: &Path, base: &Path) -> Option<PathBuf> {
    path.strip_prefix(base).ok().map(Path::to_path_buf)
}
