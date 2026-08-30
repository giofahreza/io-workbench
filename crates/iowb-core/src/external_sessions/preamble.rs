use std::{
    collections::hash_map::DefaultHasher,
    collections::{HashMap, HashSet},
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use iowb_protocol::{ChatMessage, MessageRole, Provider, SessionSummary, SessionTitleSource};
use iowb_storage::{Storage, StoredExternalHistorySource, StoredExternalSessionRecord};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const MAX_EXTERNAL_TOOL_CONTENT_BYTES: usize = 128 * 1024;
const EXTERNAL_TOOL_CONTENT_TAIL_BYTES: usize = 32 * 1024;
pub(crate) const EXTERNAL_HISTORY_PARSER_VERSION: u32 = 1;
pub(crate) const EXTERNAL_MESSAGE_PARSER_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub(crate) struct ExternalSessionRecord {
    pub summary: SessionSummary,
    pub file_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalFileFingerprint {
    pub file_identity: Option<String>,
    pub file_size: u64,
    pub modified_nanos: Option<i64>,
}

pub(crate) fn external_file_fingerprint(path: &Path) -> Option<ExternalFileFingerprint> {
    let metadata = fs::metadata(path).ok()?;
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_nanos()).ok());
    #[cfg(unix)]
    let file_identity = Some(format!("{}:{}", metadata.dev(), metadata.ino()));
    #[cfg(not(unix))]
    let file_identity = None;
    Some(ExternalFileFingerprint {
        file_identity,
        file_size: metadata.len(),
        modified_nanos,
    })
}

#[derive(Default)]
struct SessionBuilder {
    id: String,
    project_path: String,
    title: Option<String>,
    first_user: Option<String>,
    message_count: usize,
    last_activity: Option<DateTime<Utc>>,
    model: Option<String>,
}

struct CodexSessionMeta {
    id: String,
    forked_from_id: Option<String>,
    project_path: String,
    model: Option<String>,
    subagent: bool,
}

struct CountedCodexMessage {
    role: MessageRole,
    fingerprint: u64,
    trimmed_len: usize,
    native_final: bool,
    io_workbench_live_transcript: bool,
}
