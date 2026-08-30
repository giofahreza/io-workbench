#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitGenerateMessageResponse {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingEntry {
    pub key: String,
    pub value: Value,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatusResponse {
    pub product: String,
    pub version: String,
    pub server_id: String,
    pub config_dir: String,
    pub database_path: String,
    pub workspace_root: String,
    pub auth_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStatusResponse {
    pub enabled: bool,
    pub authenticated: bool,
    #[serde(rename = "needsSetup")]
    pub needs_setup: bool,
    #[serde(rename = "isAuthenticated")]
    pub is_authenticated: bool,
    #[serde(rename = "authMode")]
    pub auth_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<UserProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: String,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokenResponse {
    pub success: bool,
    pub token: String,
    pub user: UserProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessStartRequest {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub pty: bool,
    #[serde(default = "default_terminal_cols")]
    pub cols: u16,
    #[serde(default = "default_terminal_rows")]
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessStartResponse {
    pub id: String,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInputRequest {
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessResizeRequest {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub pty: bool,
}
