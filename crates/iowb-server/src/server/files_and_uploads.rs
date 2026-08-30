async fn project_sessions(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
) -> Result<Json<Vec<SessionSummary>>> {
    let project = state.projects.find_by_ref(&project_name)?;
    Ok(Json(state.sessions.list_for_project(&project.path).await?))
}

async fn delete_project(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
) -> Result<Json<PlaceholderResponse>> {
    let deleted = state.projects.delete_by_ref(&project_name)?;
    if !deleted {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "project not found"));
    }
    publish_projects(&state).await;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "project removed from io-workbench index".to_string(),
    }))
}

#[derive(Debug, Deserialize)]
struct RenameProjectRequest {
    name: String,
}

async fn rename_project(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<RenameProjectRequest>,
) -> Result<Json<ProjectSummary>> {
    let mut project = state.projects.find_by_ref(&project_name)?;
    let name = request.name.trim();
    if name.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "project name is required",
        ));
    }
    if name.len() > 200 || name.chars().any(char::is_control) {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "project name must be 200 printable characters or fewer",
        ));
    }
    if let Some(existing) = state.storage.find_project_by_name(name)?
        && existing.id != project.id
    {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "another project already uses that name",
        ));
    }

    project.name = name.to_string();
    project.updated_at = Utc::now();
    state.storage.upsert_project(&project)?;
    project.sessions = state.sessions.list_for_project(&project.path).await?;
    publish_projects(&state).await;
    Ok(Json(project))
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    path: Option<String>,
    #[serde(rename = "filePath")]
    file_path: Option<String>,
    #[serde(rename = "dirPath")]
    dir_path: Option<String>,
    #[serde(rename = "maxDepth")]
    max_depth: Option<usize>,
}

impl FileQuery {
    fn requested_path(&self) -> &str {
        self.dir_path
            .as_deref()
            .or(self.file_path.as_deref())
            .or(self.path.as_deref())
            .unwrap_or("")
    }
}

async fn list_project_files(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<Vec<FileEntry>>> {
    let project = state.projects.find_by_ref(&project_name)?;
    Ok(Json(
        state
            .files
            .list_tree_with_depth(
                project.path,
                query.requested_path(),
                query.max_depth.unwrap_or(state.config.max_scan_depth),
            )
            .await?,
    ))
}

async fn read_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<FileContentResponse>> {
    let project = state.projects.find_by_ref(&project_name)?;
    Ok(Json(
        state
            .files
            .read_file(project.path, query.requested_path())
            .await?,
    ))
}

async fn stream_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Query(query): Query<FileQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    let project = state.projects.find_by_ref(&project_name)?;
    let file = state
        .files
        .resolve_file(project.path, query.requested_path())
        .await?;
    let range_header = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    let requested_range = parse_file_range(range_header, file.size)?;
    let (status, start, end) = match requested_range {
        Some(range) => (StatusCode::PARTIAL_CONTENT, range.start, range.end),
        None => (StatusCode::OK, 0, file.size.saturating_sub(1)),
    };
    let content_length = if file.size == 0 { 0 } else { end - start + 1 };
    let mut disk_file = tokio::fs::File::open(&file.path)
        .await
        .map_err(FsError::Io)?;
    if start > 0 {
        disk_file
            .seek(SeekFrom::Start(start))
            .await
            .map_err(FsError::Io)?;
    }

    let content_type = file
        .mime_type
        .as_deref()
        .filter(|mime| !mime.trim().is_empty())
        .unwrap_or("application/octet-stream");
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, content_length.to_string());
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{}", file.size),
        );
    }
    builder
        .body(file_response_body(disk_file, content_length))
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to build file response",
                error.to_string(),
            )
        })
}

async fn write_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<WriteFileRequestCompat>,
) -> Result<Json<FileContentResponse>> {
    let project = state.projects.find_by_ref(&project_name)?;
    Ok(Json(
        state
            .files
            .write_file(project.path, request.file_path, &request.content)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct WriteFileRequestCompat {
    #[serde(alias = "path", rename = "filePath")]
    file_path: String,
    content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileByteRange {
    start: u64,
    end: u64,
}

fn parse_file_range(header_value: Option<&str>, file_size: u64) -> Result<Option<FileByteRange>> {
    let Some(header_value) = header_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let Some(range_value) = header_value.strip_prefix("bytes=") else {
        return Err(ServerError::new(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "unsupported range unit",
        ));
    };
    let range_spec = range_value.split(',').next().unwrap_or_default().trim();
    let Some((start_raw, end_raw)) = range_spec.split_once('-') else {
        return Err(ServerError::new(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "invalid range",
        ));
    };
    if file_size == 0 {
        return Err(ServerError::new(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "requested range is not satisfiable",
        ));
    }
    if start_raw.trim().is_empty() {
        let suffix_length = end_raw
            .trim()
            .parse::<u64>()
            .map_err(|_| ServerError::new(StatusCode::RANGE_NOT_SATISFIABLE, "invalid range"))?;
        if suffix_length == 0 {
            return Err(ServerError::new(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "requested range is not satisfiable",
            ));
        }
        let start = file_size.saturating_sub(suffix_length);
        return Ok(Some(FileByteRange {
            start,
            end: file_size - 1,
        }));
    }

    let start = start_raw
        .trim()
        .parse::<u64>()
        .map_err(|_| ServerError::new(StatusCode::RANGE_NOT_SATISFIABLE, "invalid range"))?;
    if start >= file_size {
        return Err(ServerError::new(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "requested range is not satisfiable",
        ));
    }
    let end = if end_raw.trim().is_empty() {
        file_size - 1
    } else {
        end_raw
            .trim()
            .parse::<u64>()
            .map_err(|_| ServerError::new(StatusCode::RANGE_NOT_SATISFIABLE, "invalid range"))?
            .min(file_size - 1)
    };
    if end < start {
        return Err(ServerError::new(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "requested range is not satisfiable",
        ));
    }
    Ok(Some(FileByteRange { start, end }))
}

fn file_response_body(file: tokio::fs::File, remaining: u64) -> Body {
    const CHUNK_SIZE: u64 = 64 * 1024;
    let stream =
        futures_util::stream::try_unfold((file, remaining), |(mut file, remaining)| async move {
            if remaining == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            let read_len = remaining.min(CHUNK_SIZE) as usize;
            let mut buffer = vec![0; read_len];
            let bytes_read = file.read(&mut buffer).await?;
            if bytes_read == 0 {
                return Ok(None);
            }
            buffer.truncate(bytes_read);
            Ok(Some((
                Bytes::from(buffer),
                (file, remaining.saturating_sub(bytes_read as u64)),
            )))
        });
    Body::from_stream(stream)
}

async fn create_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<CreateFileRequest>,
) -> Result<Json<FileEntry>> {
    let project = state.projects.find_by_ref(&project_name)?;
    Ok(Json(
        state
            .files
            .create_path(
                project.path,
                request.file_path,
                &request.content,
                request.directory,
            )
            .await?,
    ))
}

async fn rename_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<RenameFileRequest>,
) -> Result<Json<FileEntry>> {
    let project = state.projects.find_by_ref(&project_name)?;
    Ok(Json(
        state
            .files
            .rename_path(project.path, request.old_path, request.new_path)
            .await?,
    ))
}

async fn rename_project_files_batch(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<BatchRenameFileRequest>,
) -> Result<Json<Vec<FileEntry>>> {
    let project = state.projects.find_by_ref(&project_name)?;
    let mut renamed = Vec::with_capacity(request.entries.len());
    for entry in request.entries {
        renamed.push(
            state
                .files
                .rename_path(project.path.clone(), entry.old_path, entry.new_path)
                .await?,
        );
    }
    Ok(Json(renamed))
}

async fn copy_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<CopyFileRequest>,
) -> Result<Json<FileEntry>> {
    let project = state.projects.find_by_ref(&project_name)?;
    Ok(Json(
        state
            .files
            .copy_path(project.path, request.source_path, request.target_path)
            .await?,
    ))
}

async fn copy_project_files_batch(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<BatchCopyFileRequest>,
) -> Result<Json<Vec<FileEntry>>> {
    let project = state.projects.find_by_ref(&project_name)?;
    let mut copied = Vec::with_capacity(request.entries.len());
    for entry in request.entries {
        copied.push(
            state
                .files
                .copy_path(project.path.clone(), entry.source_path, entry.target_path)
                .await?,
        );
    }
    Ok(Json(copied))
}

async fn delete_project_file(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<DeleteFileRequest>,
) -> Result<Json<PlaceholderResponse>> {
    let project = state.projects.find_by_ref(&project_name)?;
    state
        .files
        .delete_path(project.path, request.file_path)
        .await?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "file deleted".to_string(),
    }))
}

async fn delete_project_files_batch(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    Json(request): Json<BatchDeleteFileRequest>,
) -> Result<Json<PlaceholderResponse>> {
    let project = state.projects.find_by_ref(&project_name)?;
    let count = request.paths.len();
    for path in request.paths {
        state.files.delete_path(project.path.clone(), path).await?;
    }
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: format!("deleted: {count} item(s)"),
    }))
}

async fn files_upload(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    multipart: Multipart,
) -> Result<Json<Value>> {
    let project = state.projects.find_by_ref(&project_name)?;
    let (fields, files) =
        collect_multipart_files(multipart, MAX_UPLOAD_FILES, MAX_UPLOAD_FILE_BYTES).await?;
    if files.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No files provided",
        ));
    }

    let target_path = fields
        .get("targetPath")
        .map(String::as_str)
        .unwrap_or("")
        .trim();
    let relative_paths = fields
        .get("relativePaths")
        .and_then(|value| serde_json::from_str::<Vec<String>>(value).ok())
        .unwrap_or_default();

    let mut uploaded = Vec::new();
    for (index, file) in files.into_iter().enumerate() {
        let file_name = relative_paths
            .get(index)
            .map(String::as_str)
            .unwrap_or(file.file_name.as_str())
            .trim();
        if file_name.is_empty() {
            continue;
        }

        let destination = if target_path.is_empty() || matches!(target_path, "." | "./") {
            PathBuf::from(file_name)
        } else {
            PathBuf::from(target_path).join(file_name)
        };
        let size = file.bytes.len();
        let entry = state
            .files
            .write_bytes(&project.path, &destination, &file.bytes)
            .await?;
        uploaded.push(serde_json::json!({
            "name": file_name,
            "path": entry.path,
            "size": size,
            "mimeType": file.content_type,
        }));
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "files": uploaded,
        "targetPath": target_path,
        "message": format!("Uploaded {} file(s) successfully", uploaded.len()),
    })))
}

async fn upload_images(
    State(state): State<AppState>,
    AxumPath(project_name): AxumPath<String>,
    multipart: Multipart,
) -> Result<Json<Value>> {
    let project = state.projects.find_by_ref(&project_name)?;
    let (_fields, files) =
        collect_multipart_files(multipart, MAX_UPLOAD_IMAGES, MAX_UPLOAD_IMAGE_BYTES).await?;
    if files.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No image files provided",
        ));
    }

    let upload_root = PathBuf::from(".io-workbench/chat-images");
    let mut images = Vec::new();
    for file in files {
        let mime_type = file
            .content_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        if !is_allowed_image_mime(&mime_type) {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Invalid file type. Only JPEG, PNG, GIF, WebP, and SVG are allowed.",
            ));
        }
        let extension = Path::new(&file.file_name)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{extension}"))
            .unwrap_or_default();
        let safe_name = format!("{}{}", new_id("chat-image"), extension);
        let destination = upload_root.join(safe_name);
        let entry = state
            .files
            .write_bytes(&project.path, &destination, &file.bytes)
            .await?;
        images.push(serde_json::json!({
            "name": file.file_name,
            "path": entry.path,
            "size": file.bytes.len(),
            "mimeType": mime_type,
        }));
    }

    Ok(Json(serde_json::json!({ "images": images })))
}

async fn audio_transcribe(multipart: Multipart) -> Result<Json<Value>> {
    let command = env::var("IO_WORKBENCH_TRANSCRIBE_COMMAND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "audio transcription is not configured; set IO_WORKBENCH_TRANSCRIBE_COMMAND",
            )
        })?;
    let (_fields, files) = collect_multipart_files(multipart, 1, MAX_UPLOAD_FILE_BYTES).await?;
    let file = files
        .into_iter()
        .next()
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "audio file is required"))?;
    let mime_type = file
        .content_type
        .clone()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let temp_path = env::temp_dir().join(format!("{}.audio", new_id("iowb")));
    tokio::fs::write(&temp_path, &file.bytes)
        .await
        .map_err(FsError::Io)?;

    let args = transcribe_args(&temp_path, &file.file_name, &mime_type)?;
    let output = Command::new(&command)
        .args(args)
        .output()
        .await
        .map_err(|error| {
            ServerError::with_details(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to run transcription command",
                error.to_string(),
            )
        });
    let _ = tokio::fs::remove_file(&temp_path).await;
    let output = output?;
    if !output.status.success() {
        return Err(ServerError::with_details(
            StatusCode::BAD_GATEWAY,
            "transcription command failed",
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "text": String::from_utf8_lossy(&output.stdout).trim(),
    })))
}

fn transcribe_args(path: &Path, filename: &str, mime_type: &str) -> Result<Vec<String>> {
    let template = env::var("IO_WORKBENCH_TRANSCRIBE_ARGS_JSON")
        .ok()
        .unwrap_or_else(|| serde_json::json!(["{audio_path}"]).to_string());
    let args = serde_json::from_str::<Vec<String>>(&template).map_err(|error| {
        ServerError::with_details(
            StatusCode::BAD_REQUEST,
            "invalid IO_WORKBENCH_TRANSCRIBE_ARGS_JSON",
            error.to_string(),
        )
    })?;
    Ok(args
        .into_iter()
        .map(|arg| {
            arg.replace("{audio_path}", &path.display().to_string())
                .replace("{filename}", filename)
                .replace("{mime_type}", mime_type)
        })
        .collect())
}

struct UploadedPart {
    file_name: String,
    content_type: Option<String>,
    bytes: Bytes,
}

async fn collect_multipart_files(
    mut multipart: Multipart,
    max_files: usize,
    max_file_bytes: usize,
) -> Result<(HashMap<String, String>, Vec<UploadedPart>)> {
    let mut fields = HashMap::new();
    let mut files = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(multipart_server_error)?
    {
        let name = field.name().unwrap_or("").to_string();
        let file_name = field.file_name().map(str::to_string);
        let content_type = field.content_type().map(str::to_string);

        if let Some(file_name) = file_name {
            if files.len() >= max_files {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    format!("Too many files. Maximum is {max_files} files."),
                ));
            }
            let bytes = field.bytes().await.map_err(multipart_server_error)?;
            if bytes.len() > max_file_bytes {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    format!("File too large. Maximum size is {max_file_bytes} bytes."),
                ));
            }
            files.push(UploadedPart {
                file_name,
                content_type,
                bytes,
            });
        } else {
            let value = field.text().await.map_err(multipart_server_error)?;
            fields.insert(name, value);
        }
    }

    Ok((fields, files))
}

fn is_allowed_image_mime(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp" | "image/svg+xml"
    )
}

async fn browse_filesystem(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
) -> Result<Json<BrowseFilesystemResponse>> {
    let path = query
        .path
        .as_deref()
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| state.config.workspace_root.to_str().unwrap_or("~"));
    let entries = state.files.browse_entries(path).await?;
    Ok(Json(BrowseFilesystemResponse {
        path: path.to_string(),
        entries,
    }))
}

async fn create_folder(
    State(state): State<AppState>,
    Json(request): Json<CreateFolderRequest>,
) -> Result<Json<PlaceholderResponse>> {
    let path = state
        .path_validator
        .validate_path(PathBuf::from(request.path), false)
        .await?;
    tokio::fs::create_dir_all(path).await.map_err(FsError::Io)?;
    Ok(Json(PlaceholderResponse {
        implemented: true,
        message: "folder created".to_string(),
    }))
}

#[derive(Debug, Deserialize)]
struct CreateFolderRequest {
    path: String,
}
