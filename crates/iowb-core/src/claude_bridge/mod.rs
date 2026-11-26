//! One Claude session owns the conversation; the terminal mod and Workbench
//! communicate through a loopback helper. A resumed print process never shares
//! a transcript with a live terminal process.
mod broker;
mod client;

pub use broker::serve;
pub use client::{Client, private_owner};

use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const PLUGIN_MANIFEST: &str = include_str!("plugin/.claude-plugin/plugin.json");
pub const PLUGIN_HOOKS: &str = include_str!("plugin/hooks/hooks.json");
pub const PLUGIN_CODE: &str = include_str!("plugin/hooks/bridge.js");

/// Keep running terminals on their loaded plugin version when a newer
/// Workbench launcher starts another terminal from the same configuration.
pub fn prepare_plugin(config_dir: &Path) -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for source in [PLUGIN_MANIFEST, PLUGIN_HOOKS, PLUGIN_CODE] {
        digest.update(source.as_bytes());
    }
    let root = config_dir
        .join("claude-bridge-plugin")
        .join(hex::encode(digest.finalize()));
    for (relative, source) in [
        (".claude-plugin/plugin.json", PLUGIN_MANIFEST),
        ("hooks/hooks.json", PLUGIN_HOOKS),
        ("hooks/bridge.js", PLUGIN_CODE),
    ] {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("plugin file parent"))?;
        if std::fs::read_to_string(&path).ok().as_deref() != Some(source) {
            atomic_bytes(&path, source.as_bytes())?;
        }
    }
    Ok(root)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub protocol: u32,
    pub session_id: String,
    pub cwd: PathBuf,
    pub url: String,
    pub token: String,
    pub instance: String,
    pub pid: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Submission {
    pub id: String,
    pub prompt: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub user_message_id: Option<String>,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub activity: Vec<String>,
    #[serde(default)]
    pub usage: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub cancel_requested: bool,
}

impl Submission {
    pub fn terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "completed" | "aborted" | "failed" | "rejected" | "uncertain"
        )
    }
}

pub fn directory() -> PathBuf {
    std::env::var_os("IO_WORKBENCH_CLAUDE_BRIDGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".claude"))
                .join("io-workbench/bridges")
        })
}

fn validate_id(id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id)
        .map_err(|_| CoreError::InvalidInput("Invalid native Claude session UUID".into()))?;
    Ok(())
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    atomic_bytes(path, &serde_json::to_vec(value)?)
}

fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn request_file(root: &Path, id: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    root.join(format!(
        "request-{}.json",
        hex::encode(Sha256::digest(id.as_bytes()))
    ))
}
