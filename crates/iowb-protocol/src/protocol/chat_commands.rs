#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCommandContext {
    pub provider: Provider,
    pub project_path: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub runtime: ChatRuntime,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatCommandKind {
    AppAction,
    NativeOperation,
    Skill,
    LegacyPrompt,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatCommandAction {
    Prompt,
    Help,
    Model,
    New,
    Status,
    Compact,
    Mcp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChatCommandDescriptor {
    pub id: String,
    pub name: String,
    pub description: String,
    pub argument_hint: String,
    pub kind: ChatCommandKind,
    pub source: String,
    pub action: ChatCommandAction,
    pub available: bool,
    pub reason: Option<String>,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatCommandCatalog {
    pub commands: Vec<ChatCommandDescriptor>,
    pub revision: String,
    pub warnings: Vec<String>,
    /// True when the provider itself reported this command list, so a name
    /// missing from it is unknown rather than merely undiscovered.
    #[serde(default)]
    pub authoritative: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareChatCommandRequest {
    #[serde(flatten)]
    pub context: ChatCommandContext,
    pub prompt: String,
    #[serde(default)]
    pub command_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedChatCommand {
    pub action: ChatCommandAction,
    pub prompt: String,
    pub command: Option<ChatCommandDescriptor>,
    /// Text for the client to show when the action is answered by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub fn leading_chat_command(prompt: &str) -> Option<(&str, &str)> {
    let trimmed = prompt.trim_start();
    let token = trimmed.split_whitespace().next()?;
    let name = token.strip_prefix('/')?;
    if name.is_empty()
        || name.len() > 160
        || !name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':')
        })
    {
        return None;
    }
    Some((name, trimmed[token.len()..].trim_start()))
}

#[cfg(test)]
mod chat_command_tests {
    use super::*;

    #[test]
    fn parses_only_leading_command_tokens_and_preserves_arguments() {
        assert_eq!(
            leading_chat_command("  /plugin:check 'a b' --fix"),
            Some(("plugin:check", "'a b' --fix"))
        );
        for literal in [
            "/",
            "/tmp/project",
            "/file.md",
            "https://host/path",
            "explain /compact",
            "\\/compact",
            "//comment",
            "/../secret",
        ] {
            assert_eq!(leading_chat_command(literal), None, "{literal}");
        }
    }

    #[test]
    fn command_enums_keep_their_wire_names() {
        let prepared: PreparedChatCommand = serde_json::from_str(
            r#"{"action":"mcp","prompt":"/mcp","command":null,"message":"ok"}"#,
        )
        .unwrap();
        assert_eq!(prepared.action, ChatCommandAction::Mcp);
        assert_eq!(prepared.message.as_deref(), Some("ok"));
        assert_eq!(
            serde_json::to_value(ChatCommandKind::LegacyPrompt).unwrap(),
            "legacy_prompt"
        );
        let catalog: ChatCommandCatalog =
            serde_json::from_str(r#"{"commands":[],"revision":"r","warnings":[]}"#).unwrap();
        assert!(!catalog.authoritative);
    }
}
