#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TerminalSnapshotQuery {
    #[serde(default)]
    after_sequence: u64,
}

async fn start_terminal(
    State(state): State<AppState>,
    Json(request): Json<iowb_protocol::TerminalStartRequest>,
) -> Result<Json<iowb_protocol::TerminalStartResponse>> {
    if let Some(cwd) = request.cwd.as_ref() {
        let path = state
            .path_validator
            .validate_path(PathBuf::from(cwd), false)
            .await?;
        if !path.is_dir() {
            return Err(CoreError::InvalidInput(
                "terminal working directory must be a directory".into(),
            )
            .into());
        }
    }
    Ok(Json(state.processes.start_terminal(request).await?))
}

async fn terminal_snapshot(
    State(state): State<AppState>,
    AxumPath(process_id): AxumPath<String>,
    Query(query): Query<TerminalSnapshotQuery>,
) -> Result<Json<iowb_protocol::TerminalSnapshotResponse>> {
    Ok(Json(
        state
            .processes
            .terminal_snapshot(&process_id, query.after_sequence)
            .await?,
    ))
}
