fn compat_value(
    state: &AppState,
    user_id: &str,
    namespace: &str,
    path: &str,
    default_value: Value,
) -> Result<Value> {
    let key = user_setting_key(
        user_id,
        &format!("compat:{namespace}:{}", compat_path_key(path)),
    );
    Ok(state.storage.get_setting(&key)?.unwrap_or(default_value))
}

async fn fetch_direct_ai_models(config: &Value) -> std::result::Result<Vec<Value>, ServerError> {
    let Some((base_url, api_key)) = direct_ai_endpoint_config(config) else {
        return Ok(Vec::new());
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create IO Gateway client",
                error.to_string(),
            )
        })?;

    // Try multiple catalog URLs (path-aware, then origin-based) so we can
    // match the URL shapes served by both OpenAI-compatible and Claude
    // /v1/models gateways. Mirrors web-ai-cli/server/utils/codex-models.js
    // `buildModelCatalogUrls`. The first URL that returns a non-empty model
    // list wins.
    let mut urls: Vec<String> = Vec::new();
    urls.push(format!("{base_url}/models"));
    urls.push(format!("{base_url}/v1/models"));
    if let Some(origin) = url_origin(&base_url) {
        urls.push(format!("{origin}/models"));
        urls.push(format!("{origin}/v1/models"));
    }

    for url in urls {
        let response = match client
            .get(&url)
            .bearer_auth(&api_key)
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => continue,
        };
        if !response.status().is_success() {
            continue;
        }
        let body = match response.json::<Value>().await {
            Ok(body) => body,
            Err(_) => continue,
        };
        let raw_models = body
            .get("data")
            .or_else(|| body.get("models"))
            .and_then(Value::as_array)
            .or_else(|| body.as_array())
            .cloned()
            .unwrap_or_default();
        if raw_models.is_empty() {
            continue;
        }
        let mapped: Vec<Value> = raw_models
            .into_iter()
            .filter_map(|model| {
                if model
                    .get("visibility")
                    .and_then(Value::as_str)
                    .is_some_and(|visibility| visibility.eq_ignore_ascii_case("hide"))
                {
                    return None;
                }
                let value = model
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| model.get("id").and_then(Value::as_str).map(str::to_string))
                    .or_else(|| {
                        model
                            .get("value")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .or_else(|| {
                        model
                            .get("slug")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .or_else(|| {
                        model
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })?;
                let label = model
                    .get("display_name")
                    .and_then(Value::as_str)
                    .or_else(|| model.get("label").and_then(Value::as_str))
                    .or_else(|| model.get("name").and_then(Value::as_str))
                    .unwrap_or(&value)
                    .to_string();
                Some(serde_json::json!({
                    "value": value,
                    "label": label,
                }))
            })
            .collect();
        if !mapped.is_empty() {
            return Ok(mapped);
        }
    }

    Ok(Vec::new())
}

/// Extract the origin (scheme + host + port) from a URL string. Returns
/// None for malformed URLs. Used to build origin-based fallbacks when
/// the configured base URL has a path prefix the gateway does not echo.
fn url_origin(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let scheme_end = trimmed.find("://")?;
    let after_scheme = &trimmed[scheme_end + 3..];
    let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());
    let origin = &trimmed[..scheme_end + 3 + path_start];
    if origin.is_empty() {
        None
    } else {
        Some(origin.trim_end_matches('/').to_string())
    }
}

#[cfg(test)]
mod url_origin_tests {
    use super::url_origin;

    #[test]
    fn strips_trailing_path() {
        assert_eq!(
            url_origin("http://141.144.197.96:8319/claude"),
            Some("http://141.144.197.96:8319".to_string())
        );
    }

    #[test]
    fn leaves_origin_only_intact() {
        assert_eq!(
            url_origin("https://api.anthropic.com/"),
            Some("https://api.anthropic.com".to_string())
        );
    }

    #[test]
    fn returns_none_for_garbage() {
        assert_eq!(url_origin("not a url"), None);
    }
}

fn direct_ai_endpoint_config(config: &Value) -> Option<(String, String)> {
    let mode = config.get("mode").and_then(Value::as_str).unwrap_or("off");
    if mode == "off" || mode.is_empty() {
        return None;
    }

    let base_url = config
        .get("baseUrl")
        .or_else(|| config.get("base_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .or_else(|| match mode {
            "direct" | "anthropic" => Some("https://api.anthropic.com".to_string()),
            "minimax" => Some("https://api.minimax.io/anthropic".to_string()),
            "proxy" | "aiproxy" => Some(DEFAULT_IO_GATEWAY_CLAUDE_BASE_URL.to_string()),
            _ => None,
        })?;

    let env_key = config
        .get("apiKeyEnv")
        .or_else(|| config.get("api_key_env"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let stored_gateway_key = config
        .get("gatewayApiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let api_key = if matches!(mode, "proxy" | "aiproxy") {
        stored_gateway_key
    } else {
        stored_gateway_key.or_else(|| {
            env_key
                .and_then(|key| env::var(key).ok())
                .or_else(|| match mode {
                    "direct" | "anthropic" => env::var("ANTHROPIC_API_KEY")
                        .or_else(|_| env::var("ANTHROPIC_AUTH_TOKEN"))
                        .ok(),
                    "minimax" => env::var("MINIMAX_API_KEY")
                        .or_else(|_| env::var("ANTHROPIC_API_KEY"))
                        .ok(),
                    _ => None,
                })
        })
    }
    .filter(|value| !value.trim().is_empty())?;

    Some((base_url, api_key))
}
