async fn cli_provider_status(AxumPath(provider): AxumPath<String>) -> Result<Json<Value>> {
    let provider = parse_provider_param(&provider)?;
    Ok(Json(provider_cli_status(provider).await))
}

async fn cli_overview() -> Json<Value> {
    let mut providers = serde_json::Map::new();
    for provider in [Provider::Claude, Provider::Codex, Provider::Gemini] {
        providers.insert(
            provider.as_str().to_string(),
            provider_cli_status(provider).await,
        );
    }
    Json(serde_json::json!({
        "success": true,
        "providers": providers,
    }))
}

async fn list_api_keys(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "apiKeys": state.storage.list_api_keys(&user.0.id)?,
    })))
}

async fn create_api_key(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<CreateApiKeyRequest>,
) -> Result<Json<Value>> {
    let key_name = request.key_name.trim();
    if key_name.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "keyName is required",
        ));
    }

    let api_key = generate_secret_token("iowb_key");
    let key_prefix = api_key.chars().take(18).collect::<String>();
    let record = state.storage.create_api_key(
        &user.0.id,
        key_name,
        &hash_secret_token(&api_key),
        &key_prefix,
    )?;
    let mut api_key_value = serde_json::to_value(record).unwrap_or_else(|_| serde_json::json!({}));
    if let Value::Object(map) = &mut api_key_value {
        map.insert("api_key".to_string(), Value::String(api_key));
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "apiKey": api_key_value,
    })))
}

async fn delete_api_key(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(key_id): AxumPath<i64>,
) -> Result<Json<Value>> {
    if !state.storage.delete_api_key(&user.0.id, key_id)? {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "API key not found"));
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn toggle_api_key(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(key_id): AxumPath<i64>,
    Json(request): Json<ToggleActiveRequest>,
) -> Result<Json<Value>> {
    if !state
        .storage
        .toggle_api_key(&user.0.id, key_id, request.is_active)?
    {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "API key not found"));
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn list_credentials(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<CredentialsQuery>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "credentials": state
            .storage
            .list_credentials(&user.0.id, query.credential_type.as_deref())?,
    })))
}

async fn create_credential(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<CreateCredentialRequest>,
) -> Result<Json<Value>> {
    let credential_name = request.credential_name.trim();
    let credential_type = request.credential_type.trim();
    let credential_value = request.credential_value.trim();
    if credential_name.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "credentialName is required",
        ));
    }
    if credential_type.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "credentialType is required",
        ));
    }
    if credential_value.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "credentialValue is required",
        ));
    }

    let credential = state.storage.create_credential(
        &user.0.id,
        credential_name,
        credential_type,
        credential_value,
        request.description.as_deref().map(str::trim),
    )?;

    Ok(Json(serde_json::json!({
        "success": true,
        "credential": credential,
    })))
}

async fn delete_credential(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(credential_id): AxumPath<i64>,
) -> Result<Json<Value>> {
    if !state.storage.delete_credential(&user.0.id, credential_id)? {
        return Err(ServerError::new(
            StatusCode::NOT_FOUND,
            "credential not found",
        ));
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn toggle_credential(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(credential_id): AxumPath<i64>,
    Json(request): Json<ToggleActiveRequest>,
) -> Result<Json<Value>> {
    if !state
        .storage
        .toggle_credential(&user.0.id, credential_id, request.is_active)?
    {
        return Err(ServerError::new(
            StatusCode::NOT_FOUND,
            "credential not found",
        ));
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

#[derive(Debug, Deserialize)]
struct CreateApiKeyRequest {
    #[serde(alias = "key_name", rename = "keyName")]
    key_name: String,
}

#[derive(Debug, Deserialize)]
struct GitConfigRequest {
    #[serde(alias = "git_name", rename = "gitName")]
    git_name: String,
    #[serde(alias = "git_email", rename = "gitEmail")]
    git_email: String,
}

#[derive(Debug, Deserialize)]
struct ToggleActiveRequest {
    #[serde(alias = "is_active", rename = "isActive")]
    is_active: bool,
}

#[derive(Debug, Deserialize)]
struct CredentialsQuery {
    #[serde(rename = "type")]
    credential_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateCredentialRequest {
    #[serde(alias = "credential_name", rename = "credentialName")]
    credential_name: String,
    #[serde(alias = "credential_type", rename = "credentialType")]
    credential_type: String,
    #[serde(alias = "credential_value", rename = "credentialValue")]
    credential_value: String,
    description: Option<String>,
}
