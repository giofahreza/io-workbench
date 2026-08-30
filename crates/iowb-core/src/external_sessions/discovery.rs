#[cfg(test)]
pub(crate) fn discover_external_sessions(home: &Path) -> Vec<ExternalSessionRecord> {
    let mut records = Vec::new();
    discover_claude(home, &mut records);
    discover_codex(home, &mut records);
    discover_gemini(home, &mut records);

    let mut unique = HashMap::<(Provider, String), ExternalSessionRecord>::new();
    for record in records {
        let key = (record.summary.provider, record.summary.id.clone());
        match unique.get(&key) {
            Some(existing) if existing.summary.last_activity >= record.summary.last_activity => {}
            _ => {
                unique.insert(key, record);
            }
        }
    }
    let mut records = unique.into_values().collect::<Vec<_>>();
    records.sort_by_key(|record| std::cmp::Reverse(record.summary.last_activity));
    records
}

/// Discover provider histories through a durable metadata index. Unchanged
/// sources are served directly from SQLite; append-only Claude JSONL files are
/// resumed from their last complete byte offset.
pub(crate) fn sync_external_sessions(
    home: &Path,
    storage: &Storage,
) -> iowb_storage::Result<Vec<ExternalSessionRecord>> {
    let mut records = Vec::new();
    sync_claude_sources(home, storage, &mut records)?;
    sync_codex_sources(home, storage, &mut records)?;
    sync_gemini_sources(home, storage, &mut records)?;
    Ok(unique_external_records(records))
}

fn unique_external_records(records: Vec<ExternalSessionRecord>) -> Vec<ExternalSessionRecord> {
    let mut unique = HashMap::<(Provider, String), ExternalSessionRecord>::new();
    for record in records {
        let key = (record.summary.provider, record.summary.id.clone());
        match unique.get(&key) {
            Some(existing) if existing.summary.last_activity >= record.summary.last_activity => {}
            _ => {
                unique.insert(key, record);
            }
        }
    }
    let mut records = unique.into_values().collect::<Vec<_>>();
    records.sort_by_key(|record| std::cmp::Reverse(record.summary.last_activity));
    records
}

fn stored_records(records: &[ExternalSessionRecord]) -> Vec<StoredExternalSessionRecord> {
    records
        .iter()
        .map(|record| StoredExternalSessionRecord {
            summary: record.summary.clone(),
            file_path: record.file_path.display().to_string(),
        })
        .collect()
}

fn restored_records(source: &StoredExternalHistorySource) -> Vec<ExternalSessionRecord> {
    source
        .records
        .iter()
        .map(|record| ExternalSessionRecord {
            summary: record.summary.clone(),
            file_path: PathBuf::from(&record.file_path),
        })
        .collect()
}

fn source_matches(
    source: &StoredExternalHistorySource,
    fingerprint: &ExternalFileFingerprint,
) -> bool {
    source.parser_version == EXTERNAL_HISTORY_PARSER_VERSION
        && source.file_identity == fingerprint.file_identity
        && source.file_size == fingerprint.file_size
        && source.modified_nanos == fingerprint.modified_nanos
}

fn persist_source(
    storage: &Storage,
    provider: Provider,
    source_path: &Path,
    fingerprint: &ExternalFileFingerprint,
    scan_offset: u64,
    records: &[ExternalSessionRecord],
) -> iowb_storage::Result<()> {
    storage.upsert_external_history_source(&StoredExternalHistorySource {
        provider,
        source_path: source_path.display().to_string(),
        file_identity: fingerprint.file_identity.clone(),
        file_size: fingerprint.file_size,
        modified_nanos: fingerprint.modified_nanos,
        scan_offset,
        parser_version: EXTERNAL_HISTORY_PARSER_VERSION,
        records: stored_records(records),
    })
}

fn sync_claude_sources(
    home: &Path,
    storage: &Storage,
    records: &mut Vec<ExternalSessionRecord>,
) -> iowb_storage::Result<()> {
    let root = home.join(".claude/projects");
    if !root.is_dir() {
        return Ok(());
    }
    let paths = files_below(&root, 2, "jsonl")
        .into_iter()
        .filter(|path| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("agent-"))
        })
        .collect::<Vec<_>>();
    let retained = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    for path in paths {
        let Some(fingerprint) = external_file_fingerprint(&path) else {
            continue;
        };
        let source_path = path.display().to_string();
        let stored = storage.external_history_source(Provider::Claude, &source_path)?;
        if let Some(stored) = stored
            .as_ref()
            .filter(|source| source_matches(source, &fingerprint))
        {
            records.extend(restored_records(stored));
            continue;
        }

        let (discovered, scan_offset) = if let Some(stored) = stored.filter(|source| {
            source.parser_version == EXTERNAL_HISTORY_PARSER_VERSION
                && source.file_identity == fingerprint.file_identity
                && source.file_size <= fingerprint.file_size
                && source.scan_offset <= source.file_size
                && !source.records.is_empty()
        }) {
            let mut builders = stored
                .records
                .iter()
                .map(|record| {
                    let summary = &record.summary;
                    (
                        summary.id.clone(),
                        SessionBuilder {
                            id: summary.id.clone(),
                            project_path: summary.project_path.clone(),
                            title: Some(summary.title.clone()),
                            first_user: None,
                            message_count: summary.message_count,
                            last_activity: Some(summary.last_activity),
                            model: summary.model.clone(),
                        },
                    )
                })
                .collect::<HashMap<_, _>>();
            let offset = for_each_json_line_from(&path, stored.scan_offset, |entry| {
                update_claude_builder(&mut builders, entry)
            });
            (
                finish_claude_builders(builders, &path, modified_time(&path)),
                offset,
            )
        } else {
            let mut builders = HashMap::new();
            let offset = for_each_json_line_from(&path, 0, |entry| {
                update_claude_builder(&mut builders, entry)
            });
            (
                finish_claude_builders(builders, &path, modified_time(&path)),
                offset,
            )
        };
        persist_source(
            storage,
            Provider::Claude,
            &path,
            &fingerprint,
            scan_offset,
            &discovered,
        )?;
        records.extend(discovered);
    }
    storage.prune_external_history_sources(Provider::Claude, &retained)
}

fn sync_codex_sources(
    home: &Path,
    storage: &Storage,
    records: &mut Vec<ExternalSessionRecord>,
) -> iowb_storage::Result<()> {
    let codex_dir = home.join(".codex");
    let (source_path, fingerprint) =
        if let Some(index) = newest_matching_file(&codex_dir, "state_", "sqlite") {
            let fingerprint = codex_sqlite_fingerprint(&index);
            (index, fingerprint)
        } else {
            let root = codex_dir.join("sessions");
            (root.clone(), external_tree_fingerprint(&root, 8, "jsonl"))
        };
    let retained = vec![source_path.display().to_string()];
    if let Some(stored) = storage
        .external_history_source(Provider::Codex, &retained[0])?
        .filter(|source| source_matches(source, &fingerprint))
    {
        records.extend(restored_records(&stored));
        return storage.prune_external_history_sources(Provider::Codex, &retained);
    }

    let mut discovered = Vec::new();
    discover_codex(home, &mut discovered);
    persist_source(
        storage,
        Provider::Codex,
        &source_path,
        &fingerprint,
        fingerprint.file_size,
        &discovered,
    )?;
    records.extend(discovered);
    storage.prune_external_history_sources(Provider::Codex, &retained)
}

fn codex_sqlite_fingerprint(database_path: &Path) -> ExternalFileFingerprint {
    // SQLite may keep fresh thread metadata exclusively in WAL for a long
    // time, so the durable cursor must include both files.
    let wal_path = PathBuf::from(format!("{}-wal", database_path.display()));
    let mut hasher = DefaultHasher::new();
    let mut total_size = 0u64;
    let mut modified_nanos = None;
    for path in [database_path, wal_path.as_path()] {
        let Some(fingerprint) = external_file_fingerprint(path) else {
            continue;
        };
        path.hash(&mut hasher);
        fingerprint.file_identity.hash(&mut hasher);
        fingerprint.file_size.hash(&mut hasher);
        fingerprint.modified_nanos.hash(&mut hasher);
        total_size = total_size.saturating_add(fingerprint.file_size);
        modified_nanos = modified_nanos.max(fingerprint.modified_nanos);
    }
    ExternalFileFingerprint {
        file_identity: Some(format!("sqlite:{:016x}", hasher.finish())),
        file_size: total_size,
        modified_nanos,
    }
}

fn external_tree_fingerprint(
    root: &Path,
    max_depth: usize,
    extension: &str,
) -> ExternalFileFingerprint {
    let mut hasher = DefaultHasher::new();
    let mut total_size = 0u64;
    let mut modified_nanos = None;
    for path in files_below(root, max_depth, extension) {
        let Some(fingerprint) = external_file_fingerprint(&path) else {
            continue;
        };
        path.hash(&mut hasher);
        fingerprint.file_identity.hash(&mut hasher);
        fingerprint.file_size.hash(&mut hasher);
        fingerprint.modified_nanos.hash(&mut hasher);
        total_size = total_size.saturating_add(fingerprint.file_size);
        modified_nanos = modified_nanos.max(fingerprint.modified_nanos);
    }
    ExternalFileFingerprint {
        file_identity: Some(format!("tree:{:016x}", hasher.finish())),
        file_size: total_size,
        modified_nanos,
    }
}

pub(crate) fn load_external_messages(record: &ExternalSessionRecord) -> Vec<ChatMessage> {
    match record.summary.provider {
        Provider::Claude => load_claude_messages(record),
        Provider::Codex => load_codex_messages(record),
        Provider::Gemini => load_gemini_messages(record),
    }
}

pub(crate) fn count_external_messages(record: &ExternalSessionRecord) -> usize {
    match record.summary.provider {
        Provider::Codex => count_codex_messages(record),
        Provider::Claude | Provider::Gemini => record.summary.message_count,
    }
}

pub(crate) fn same_project_path(left: &str, right: &str) -> bool {
    let left_path = Path::new(left);
    let right_path = Path::new(right);
    match (left_path.canonicalize(), right_path.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => normalize_path(left) == normalize_path(right),
    }
}
