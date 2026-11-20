use std::{fs, io::Write, path::PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::headers::Header;

/// What the TUI remembers between runs. The token and custom header values are
/// stored as plain text in a file only the current user can read, because system
/// keyrings are often unavailable over SSH, which is where a terminal client gets used.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    /// Sent to `server_url` only, like the token.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<Header>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session_id: Option<String>,
}

pub fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("io-workbench").join("tui.json"))
}

/// A missing or unreadable file simply means a first run.
pub fn load() -> Config {
    path()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(config: &Config) -> anyhow::Result<()> {
    let path = path().context("no configuration directory is available")?;
    save_to(&path, config)
}

fn save_to(path: &std::path::Path, config: &Config) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(serde_json::to_string_pretty(config)?.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_keeps_the_file_private() {
        let dir = std::env::temp_dir().join(format!("iowb-tui-config-{}", std::process::id()));
        let path = dir.join("nested").join("tui.json");
        let config = Config {
            server_url: Some("http://127.0.0.1:8100".into()),
            headers: vec![Header {
                name: "CF-Access-Client-Id".into(),
                value: "id".into(),
            }],
            token: Some("secret".into()),
            last_session_id: None,
        };
        save_to(&path, &config).unwrap();
        let loaded: Config = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded, config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unknown_or_missing_fields_fall_back_to_defaults() {
        let loaded: Config = serde_json::from_str(r#"{"token":"t","extra":1}"#).unwrap();
        assert_eq!(loaded.token.as_deref(), Some("t"));
        assert_eq!(loaded.server_url, None);
        assert!(loaded.headers.is_empty());
    }
}
