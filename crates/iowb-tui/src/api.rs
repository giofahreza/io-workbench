use std::{fmt, time::Duration};

use iowb_protocol::{
    ApiErrorBody, AuthStatusResponse, AuthTokenResponse, ForkSessionResponse, LoginRequest,
    MessagesResponse, ProjectListResponse, ProjectSessionsPage, ProjectSummary,
    SessionSnapshotResponse, SessionSummary,
};
use reqwest::{Client, Method, Url, header::LOCATION, redirect::Policy};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::headers::{Header, header_map};

pub const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8787";
pub const SESSIONS_PAGE_SIZE: usize = 50;
pub const MESSAGES_PAGE_SIZE: usize = 30;

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: Option<u16>,
    pub message: String,
}

impl ApiError {
    pub fn unauthorized(&self) -> bool {
        self.status == Some(401)
    }

    pub fn local(message: impl Into<String>) -> Self {
        Self {
            status: None,
            message: message.into(),
        }
    }

    fn network(error: reqwest::Error) -> Self {
        let message = if error.is_connect() {
            "cannot connect to the server".to_string()
        } else if error.is_timeout() {
            "the server did not answer in time".to_string()
        } else {
            error.to_string()
        };
        Self::local(message)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

/// A session someone pinned in the web or mobile client.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PinnedSession {
    pub session_id: String,
    pub title: String,
    pub project: String,
    pub provider: String,
}

fn text_field(object: &Value, names: &[&str]) -> String {
    names
        .iter()
        .find_map(|name| object.get(*name).and_then(Value::as_str))
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

/// Pins come back as objects, but older clients stored `project::session::provider`
/// strings, so both shapes are accepted. Entries without a session id are dropped.
pub fn parse_pinned(value: &Value) -> Vec<PinnedSession> {
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let pinned = match entry {
                Value::String(text) => match text.split("::").collect::<Vec<_>>().as_slice() {
                    [session_id] => PinnedSession {
                        session_id: session_id.trim().to_string(),
                        ..Default::default()
                    },
                    [project_path, session_id, rest @ ..] => PinnedSession {
                        session_id: session_id.trim().to_string(),
                        title: String::new(),
                        project: project_path
                            .rsplit(['/', '\\'])
                            .next()
                            .unwrap_or_default()
                            .to_string(),
                        provider: rest.first().unwrap_or(&"").trim().to_string(),
                    },
                    [] => return None,
                },
                Value::Object(_) => PinnedSession {
                    session_id: text_field(entry, &["sessionId"]),
                    title: text_field(entry, &["title", "sessionName"]),
                    project: text_field(entry, &["projectDisplayName", "projectName"]),
                    provider: text_field(entry, &["provider"]),
                },
                _ => return None,
            };
            (!pinned.session_id.is_empty()).then_some(pinned)
        })
        .collect()
}

/// Servers that predate paging ignore `limit`/`offset` and answer with every
/// session as one plain array, so a follow-up page has nothing new to add.
fn parse_sessions_page(
    value: Value,
    offset: usize,
) -> Result<ProjectSessionsPage, serde_json::Error> {
    if !value.is_array() {
        return serde_json::from_value(value);
    }
    let all: Vec<SessionSummary> = serde_json::from_value(value)?;
    let total_count = all.len();
    Ok(ProjectSessionsPage {
        sessions: if offset == 0 { all } else { Vec::new() },
        has_more: false,
        total_count,
    })
}

/// Accepts `host:port`, `http://host`, trailing slashes and whitespace.
pub fn normalize_base_url(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return DEFAULT_SERVER_URL.to_string();
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    with_scheme.trim_end_matches('/').to_string()
}

pub fn ws_url(base: &str) -> String {
    let base = normalize_base_url(base);
    let socket = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base
    };
    format!("{socket}/ws")
}

/// Turns a failed response into a sentence a person can act on. Redirects and web
/// pages usually mean a sign-in proxy (for example Cloudflare Access) sits in front
/// of the server, which shows up as an unreadable response if not detected here.
fn explain_failure(status: u16, location: Option<&str>, body: &str) -> String {
    if (300..400).contains(&status) {
        // Only the host: the full address is long and the fix does not depend on it.
        let target = location
            .and_then(|location| Url::parse(location).ok())
            .and_then(|url| url.host_str().map(str::to_string))
            .or_else(|| location.map(str::to_string))
            .unwrap_or_else(|| "another address".to_string());
        return format!(
            "the server redirected to {target} (HTTP {status}); a sign-in page or proxy is in front of it. If it is Cloudflare Access, add its service token under Custom headers."
        );
    }
    if let Ok(error) = serde_json::from_str::<ApiErrorBody>(body) {
        return match error.details {
            Some(details) if !details.is_empty() => format!("{}: {details}", error.error),
            _ => error.error,
        };
    }
    let trimmed = body.trim();
    if trimmed.starts_with('<') {
        return format!("HTTP {status}: the server returned a web page instead of API data");
    }
    if trimmed.is_empty() {
        return format!("HTTP {status}");
    }
    format!("HTTP {status}: {}", crate::text::truncate(trimmed, 200))
}

#[derive(Clone)]
pub struct Api {
    http: Client,
    base: String,
    token: Option<String>,
    headers: Vec<Header>,
}

impl Api {
    /// `headers` go out with every request; the bearer token still wins over a
    /// custom `Authorization` header, as in the mobile app.
    pub fn new(base: &str, token: Option<String>, headers: Vec<Header>) -> Self {
        let http = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .default_headers(header_map(&headers))
            .build()
            .expect("HTTP client configuration is valid");
        Self {
            http,
            base: normalize_base_url(base),
            token,
            headers,
        }
    }

    pub fn headers(&self) -> &[Header] {
        &self.headers
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn url(&self, segments: &[&str], query: &[(&str, String)]) -> Result<Url, ApiError> {
        let mut url = Url::parse(&self.base)
            .map_err(|error| ApiError::local(format!("invalid server address: {error}")))?;
        url.path_segments_mut()
            .map_err(|_| ApiError::local("invalid server address"))?
            .pop_if_empty()
            .extend(segments);
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(key, value)| (*key, value.as_str())));
        }
        Ok(url)
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        body: Option<Value>,
    ) -> Result<T, ApiError> {
        let mut request = self.http.request(method, url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(ApiError::network)?;
        let status = response.status();
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response.text().await.map_err(ApiError::network)?;
        if !status.is_success() {
            return Err(ApiError {
                status: Some(status.as_u16()),
                message: explain_failure(status.as_u16(), location.as_deref(), &body),
            });
        }
        serde_json::from_str(&body).map_err(|error| ApiError {
            status: Some(status.as_u16()),
            message: format!("unexpected response from the server: {error}"),
        })
    }

    async fn get<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<T, ApiError> {
        self.send(Method::GET, self.url(segments, query)?, None)
            .await
    }

    pub async fn auth_status(&self) -> Result<AuthStatusResponse, ApiError> {
        self.get(&["api", "auth", "status"], &[]).await
    }

    pub async fn chat_commands(
        &self,
        context: &iowb_protocol::ChatCommandContext,
    ) -> Result<iowb_protocol::ChatCommandCatalog, ApiError> {
        let mut query = vec![
            ("provider", context.provider.as_str().to_string()),
            ("projectPath", context.project_path.clone()),
        ];
        if let Some(id) = &context.session_id {
            query.push(("sessionId", id.clone()));
        }
        if let Some(model) = &context.model {
            query.push(("model", model.clone()));
        }
        self.get(&["api", "chat", "commands"], &query).await
    }

    pub async fn prepare_chat_command(
        &self,
        context: iowb_protocol::ChatCommandContext,
        prompt: String,
    ) -> Result<iowb_protocol::PreparedChatCommand, ApiError> {
        let body = serde_json::to_value(iowb_protocol::PrepareChatCommandRequest {
            context,
            prompt,
            command_id: None,
        })
        .map_err(|error| ApiError::local(error.to_string()))?;
        self.send(
            Method::POST,
            self.url(&["api", "chat", "commands", "prepare"], &[])?,
            Some(body),
        )
        .await
    }

    pub async fn compact_chat(
        &self,
        session_id: &str,
        request_id: &str,
    ) -> Result<Value, ApiError> {
        self.send(
            Method::POST,
            self.url(&["api", "sessions", session_id, "compact"], &[])?,
            Some(serde_json::json!({"requestId": request_id})),
        )
        .await
    }

    pub async fn chat_models(
        &self,
        provider: iowb_protocol::Provider,
    ) -> Result<Vec<String>, ApiError> {
        let result: Value = self
            .get(
                &["api", "chat", "models"],
                &[("provider", provider.as_str().to_string())],
            )
            .await?;
        Ok(result
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| {
                model
                    .get("value")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect())
    }

    pub async fn login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<AuthTokenResponse, ApiError> {
        let body = serde_json::to_value(LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
        })
        .expect("login request serializes");
        self.send(
            Method::POST,
            self.url(&["api", "auth", "login"], &[])?,
            Some(body),
        )
        .await
    }

    /// Projects without their sessions; sessions are paged in per project.
    pub async fn projects(&self) -> Result<Vec<ProjectSummary>, ApiError> {
        let list: ProjectListResponse = self
            .get(&["api", "projects"], &[("includeSessions", "false".into())])
            .await?;
        Ok(list.projects)
    }

    pub async fn pinned_sessions(&self) -> Result<Vec<PinnedSession>, ApiError> {
        let value: Value = self
            .get(&["api", "settings", "sidebar-active-sessions"], &[])
            .await?;
        Ok(parse_pinned(
            value.get("pinnedSessions").unwrap_or(&Value::Null),
        ))
    }

    pub async fn project_sessions(
        &self,
        project_id: &str,
        offset: usize,
    ) -> Result<ProjectSessionsPage, ApiError> {
        let value: Value = self
            .get(
                &["api", "projects", project_id, "sessions"],
                &[
                    ("limit", SESSIONS_PAGE_SIZE.to_string()),
                    ("offset", offset.to_string()),
                ],
            )
            .await?;
        parse_sessions_page(value, offset).map_err(|error| ApiError {
            status: None,
            message: format!("unexpected response from the server: {error}"),
        })
    }

    /// The session itself plus its newest messages, in one request.
    /// Starts a new chat from everything before one of this chat's prompts. This
    /// chat stays as it is (`replace: false`).
    pub async fn fork_session(
        &self,
        session_id: &str,
        before_message_id: &str,
        request_id: &str,
    ) -> Result<ForkSessionResponse, ApiError> {
        let body = serde_json::json!({
            "beforeMessageId": before_message_id,
            "requestId": request_id,
            "replace": false,
        });
        self.send(
            Method::POST,
            self.url(&["api", "sessions", session_id, "fork"], &[])?,
            Some(body),
        )
        .await
    }

    pub async fn snapshot(&self, session_id: &str) -> Result<SessionSnapshotResponse, ApiError> {
        self.get(
            &["api", "sessions", session_id, "snapshot"],
            &[("limit", MESSAGES_PAGE_SIZE.to_string())],
        )
        .await
    }

    pub async fn messages(
        &self,
        session_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<MessagesResponse, ApiError> {
        self.get(
            &["api", "sessions", session_id, "messages"],
            &[("limit", limit.to_string()), ("offset", offset.to_string())],
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn normalizes_server_addresses() {
        assert_eq!(normalize_base_url(""), DEFAULT_SERVER_URL);
        assert_eq!(
            normalize_base_url(" 127.0.0.1:8100/ "),
            "http://127.0.0.1:8100"
        );
        assert_eq!(
            normalize_base_url("https://dev.example.com//"),
            "https://dev.example.com"
        );
    }

    #[test]
    fn derives_the_websocket_address() {
        assert_eq!(ws_url("http://127.0.0.1:8100"), "ws://127.0.0.1:8100/ws");
        assert_eq!(
            ws_url("https://dev.example.com/"),
            "wss://dev.example.com/ws"
        );
        assert_eq!(
            ws_url("dev.example.com:9000"),
            "ws://dev.example.com:9000/ws"
        );
    }

    #[test]
    fn encodes_path_segments_and_queries() {
        let api = Api::new("http://127.0.0.1:8100", None, Vec::new());
        let url = api
            .url(
                &["api", "projects", "a b/c", "sessions"],
                &[("limit", "50".into()), ("offset", "100".into())],
            )
            .unwrap();
        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:8100/api/projects/a%20b%2Fc/sessions?limit=50&offset=100"
        );
    }

    #[tokio::test]
    async fn custom_headers_are_sent_and_the_token_wins_over_a_custom_authorization() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = socket.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..read]);
            }
            let body = "{}";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&request).to_lowercase()
        });

        let headers = vec![
            Header {
                name: "CF-Access-Client-Id".into(),
                value: "the-id".into(),
            },
            Header {
                name: "Authorization".into(),
                value: "Basic proxy".into(),
            },
        ];
        let api = Api::new(&format!("http://{address}"), Some("tok".into()), headers);
        let _ = api.auth_status().await;

        let request = server.await.unwrap();
        assert!(request.contains("cf-access-client-id: the-id"));
        assert!(request.contains("authorization: bearer tok"));
        assert!(!request.contains("basic proxy"));
    }

    #[test]
    fn explains_redirects_pages_and_api_errors() {
        let redirect = explain_failure(
            302,
            Some("https://team.cloudflareaccess.com/cdn-cgi/access/login/app?kid=abc"),
            "",
        );
        assert!(redirect.contains("team.cloudflareaccess.com") && redirect.contains("sign-in"));
        assert!(redirect.contains("Custom headers"));
        assert!(!redirect.contains("kid=abc"), "only the host is shown");
        assert!(explain_failure(200, None, "<html></html>").contains("web page"));
        assert_eq!(
            explain_failure(401, None, r#"{"error":"Invalid credentials"}"#),
            "Invalid credentials"
        );
        assert_eq!(explain_failure(500, None, ""), "HTTP 500");
    }

    #[test]
    fn reads_paged_and_legacy_session_lists() {
        let paged = parse_sessions_page(
            json!({"sessions": [{"id": "a", "title": "A"}], "hasMore": true, "totalCount": 9}),
            0,
        )
        .unwrap();
        assert_eq!(
            (paged.sessions.len(), paged.has_more, paged.total_count),
            (1, true, 9)
        );

        let legacy = json!([{"id": "a"}, {"id": "b"}]);
        let first = parse_sessions_page(legacy.clone(), 0).unwrap();
        assert_eq!(
            (first.sessions.len(), first.has_more, first.total_count),
            (2, false, 2)
        );
        let later = parse_sessions_page(legacy, 50).unwrap();
        assert!(later.sessions.is_empty() && !later.has_more);
    }

    #[test]
    fn reads_pinned_objects_and_legacy_strings() {
        let pinned = parse_pinned(&json!([
            {"sessionId": "s1", "title": "Fix login", "projectDisplayName": "io-wb", "provider": "codex"},
            {"sessionId": "s2", "sessionName": "Named", "projectName": "api"},
            "/home/me/dev/web::s3::claude",
            "bare-id",
            {"title": "no id"},
            42
        ]));
        assert_eq!(
            pinned,
            [
                PinnedSession {
                    session_id: "s1".into(),
                    title: "Fix login".into(),
                    project: "io-wb".into(),
                    provider: "codex".into()
                },
                PinnedSession {
                    session_id: "s2".into(),
                    title: "Named".into(),
                    project: "api".into(),
                    provider: String::new()
                },
                PinnedSession {
                    session_id: "s3".into(),
                    title: String::new(),
                    project: "web".into(),
                    provider: "claude".into()
                },
                PinnedSession {
                    session_id: "bare-id".into(),
                    ..Default::default()
                },
            ]
        );
    }
}
