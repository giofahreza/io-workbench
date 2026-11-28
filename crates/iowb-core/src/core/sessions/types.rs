#[derive(Clone)]
pub struct SessionManager {
    storage: Storage,
    sessions: Arc<RwLock<HashMap<String, SessionSummary>>>,
    max_sessions: usize,
    external_home: Arc<PathBuf>,
    external_cache: Arc<RwLock<ExternalSessionCache>>,
    external_sync: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Default)]
struct ExternalSessionCache {
    loaded_at: Option<Instant>,
    records: Vec<ExternalSessionRecord>,
    message_bytes: usize,
    messages: HashMap<String, CachedExternalMessages>,
    history_windows: HashMap<String, CachedHistoryWindow>,
}

#[derive(Clone)]
struct CachedHistoryWindow {
    version: Option<MessageHistoryVersion>,
    fingerprint: Option<ExternalFileFingerprint>,
    last_access: Instant,
    estimated_bytes: usize,
    value: HistoryWindowValue,
}

#[derive(Clone)]
enum HistoryWindowValue {
    Descriptors(Arc<Vec<ActiveContextMessageDescriptor>>),
    Messages(Arc<Vec<ChatMessage>>),
}

#[derive(Clone)]
struct CachedExternalMessages {
    modified_at: Option<SystemTime>,
    estimated_bytes: usize,
    last_access: Instant,
    total_count: usize,
    complete: bool,
    messages: Arc<Vec<ChatMessage>>,
}

fn external_session_cache_key(record: &ExternalSessionRecord) -> String {
    format!(
        "{}:{}:{}",
        record.summary.provider.as_str(),
        record.summary.id.as_str(),
        record.file_path.display()
    )
}
