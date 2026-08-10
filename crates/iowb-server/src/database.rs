use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Once, OnceLock},
    time::{Duration, Instant},
};

use axum::{
    Extension, Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use futures_util::StreamExt;
use iowb_core::AppState;
use iowb_protocol::{
    DatabaseCapabilities, DatabaseConnectionInput, DatabaseConnectionProfile, DatabaseExplorerNode,
    DatabaseForeignKey, DatabaseNameSummary, DatabaseObjectColumn, DatabaseObjectDetails,
    DatabaseObjectSummary, DatabaseObjectType, DatabaseQueryRequest, DatabaseQueryResult,
    DatabaseQueryStatementType, DatabaseRelationalSchema, DatabaseRelationalSchemaRelationship,
    DatabaseRelationalSchemaTable, DatabaseSessionInfo, DatabaseTableData,
    DatabaseTestConnectionRequest, DatabaseTestResult, DatabaseTestStatus,
    DatabaseTransferEndpoint, DatabaseTransferJob, DatabaseTransferJobError,
    DatabaseTransferJobLogEntry, DatabaseTransferJobResult, DatabaseTransferJobStatus,
    DatabaseTransferJobWarning, DatabaseTransferMode, DatabaseTransferRequest,
    SupportedDatabaseType, new_id,
};
use iowb_storage::StoredDatabaseConnection;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, types::ValueRef};
use serde::Deserialize;
use serde_json::{Map, Value};
use sqlx::{
    AnyPool, Column, Either, Executor, MySqlPool, PgPool, Row, TypeInfo,
    any::{AnyPoolOptions, AnyRow},
    mysql::{MySqlPoolOptions, MySqlRow},
    postgres::{PgPoolOptions, PgRow, PgTypeKind},
};
use tokio::sync::Mutex;

use crate::{AuthenticatedUser, Result, ServerError};

const DEFAULT_QUERY_MAX_ROWS: usize = 1000;
const MAX_QUERY_MAX_ROWS: usize = 5000;
const DEFAULT_TABLE_PAGE_SIZE: usize = 50;
const MAX_TABLE_PAGE_SIZE: usize = 500;
const TRANSFER_ROW_BATCH_SIZE: usize = 200;
const MAX_ROW_FAILURE_DETAILS: usize = 50;
const LIVE_POOL_CACHE_MAX_ENTRIES: usize = 12;
const LIVE_POOL_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
static SQLX_ANY_DRIVERS: Once = Once::new();
static LIVE_POOL_CACHE: OnceLock<Mutex<HashMap<String, CachedLivePool>>> = OnceLock::new();
static TYPED_LIVE_POOL_CACHE: OnceLock<Mutex<HashMap<String, CachedTypedLivePool>>> =
    OnceLock::new();

struct CachedLivePool {
    connection_id: i64,
    pool: AnyPool,
    last_used: Instant,
}

#[derive(Clone)]
enum TypedLivePool {
    Postgresql(PgPool),
    Mysql(MySqlPool),
}

impl TypedLivePool {
    fn is_closed(&self) -> bool {
        match self {
            Self::Postgresql(pool) => pool.is_closed(),
            Self::Mysql(pool) => pool.is_closed(),
        }
    }
}

struct CachedTypedLivePool {
    connection_id: i64,
    pool: TypedLivePool,
    last_used: Instant,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/database", get(database_status))
        .route(
            "/api/database/connections",
            get(list_connections).post(create_connection),
        )
        .route(
            "/api/database/connections/test",
            post(test_unsaved_connection),
        )
        .route(
            "/api/database/connections/{connection_id}",
            put(update_connection).delete(delete_connection),
        )
        .route(
            "/api/database/connections/{connection_id}/test",
            post(test_saved_connection),
        )
        .route(
            "/api/database/connections/{connection_id}/explorer",
            get(explorer),
        )
        .route(
            "/api/database/connections/{connection_id}/object-details",
            get(object_details),
        )
        .route(
            "/api/database/connections/{connection_id}/query",
            post(execute_query),
        )
        .route(
            "/api/database/connections/{connection_id}/table-data",
            get(table_data),
        )
        .route(
            "/api/database/connections/{connection_id}/table-data/paste",
            post(paste_table_rows),
        )
        .route(
            "/api/database/connections/{connection_id}/table-data/row-edit",
            post(edit_table_row),
        )
        .route(
            "/api/database/connections/{connection_id}/table-data/row-add",
            post(add_table_row),
        )
        .route(
            "/api/database/connections/{connection_id}/table-data/row-delete",
            post(delete_table_row),
        )
        .route(
            "/api/database/connections/{connection_id}/table-data/rows-delete",
            post(delete_table_rows),
        )
        .route(
            "/api/database/connections/{connection_id}/export/portable",
            post(export_portable_table),
        )
        .route(
            "/api/database/connections/{connection_id}/import/portable",
            post(import_portable_table),
        )
        .route("/api/database/transfers", post(create_transfer))
        .route("/api/database/export", post(export_table))
        .route("/api/database/import", post(import_table))
        .route("/api/database/jobs", get(list_jobs))
        .route("/api/database/jobs/{job_id}", get(get_job))
}

async fn database_status() -> Json<Value> {
    Json(serde_json::json!({
        "success": true,
        "adapters": [
            { "type": "sqlite", "implemented": true },
            { "type": "postgresql", "implemented": true },
            { "type": "mysql", "implemented": true },
            { "type": "mariadb", "implemented": true }
        ]
    }))
}

async fn list_connections(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "success": true,
        "connections": state.storage.list_database_connections(&user.0.id)?,
    })))
}

async fn create_connection(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(input): Json<DatabaseConnectionInput>,
) -> Result<Json<Value>> {
    let input = normalize_connection_input(&state, input, None).await?;
    let connection = state
        .storage
        .create_database_connection(&user.0.id, &input)?
        .profile;
    Ok(Json(serde_json::json!({
        "success": true,
        "connection": connection,
    })))
}

async fn test_unsaved_connection(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<DatabaseTestConnectionRequest>,
) -> Result<Response> {
    let existing_password = if let Some(connection_id) = request.existing_connection_id {
        state
            .storage
            .get_database_connection(&user.0.id, connection_id)?
            .and_then(|connection| connection.password)
    } else {
        None
    };
    let input = normalize_connection_input(&state, request.connection, existing_password).await?;
    match test_connection_input(&input).await {
        Ok(result) => Ok(Json(serde_json::json!({
            "success": true,
            "result": result,
        }))
        .into_response()),
        Err(error) => Ok(database_error_response(
            error,
            "Failed to test database connection",
            None,
        )),
    }
}

async fn update_connection(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(input): Json<DatabaseConnectionInput>,
) -> Result<Json<Value>> {
    let existing = state
        .storage
        .get_database_connection(&user.0.id, connection_id)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Connection not found"))?;
    let input = normalize_connection_input(&state, input, existing.password).await?;
    let connection = state
        .storage
        .update_database_connection(&user.0.id, connection_id, &input)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Connection not found"))?
        .profile;
    evict_live_pools(connection_id).await;
    Ok(Json(serde_json::json!({
        "success": true,
        "connection": connection,
    })))
}

async fn delete_connection(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
) -> Result<Json<Value>> {
    if !state
        .storage
        .delete_database_connection(&user.0.id, connection_id)?
    {
        return Err(ServerError::new(
            StatusCode::NOT_FOUND,
            "Connection not found",
        ));
    }
    evict_live_pools(connection_id).await;
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn test_saved_connection(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
) -> Result<Response> {
    let connection = state
        .storage
        .get_database_connection(&user.0.id, connection_id)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Connection not found"))?;

    match test_connection_record(&connection).await {
        Ok(result) => {
            let updated = state
                .storage
                .record_database_connection_test(
                    &user.0.id,
                    connection_id,
                    DatabaseTestStatus::Success,
                    &result.message,
                )?
                .unwrap_or(connection.profile);
            Ok(Json(serde_json::json!({
                "success": true,
                "connection": updated,
                "result": result,
            }))
            .into_response())
        }
        Err(error) => {
            let message = database_error_message(&error);
            let updated = state
                .storage
                .record_database_connection_test(
                    &user.0.id,
                    connection_id,
                    DatabaseTestStatus::Error,
                    &message,
                )?
                .unwrap_or(connection.profile);
            Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": message,
                    "connection": updated,
                    "result": {
                        "status": "error",
                        "message": message,
                    }
                })),
            )
                .into_response())
        }
    }
}

#[derive(Debug, Deserialize)]
struct ExplorerQuery {
    #[serde(rename = "nodeType")]
    node_type: Option<String>,
    #[serde(rename = "databaseName")]
    database_name: Option<String>,
    #[serde(rename = "schemaName")]
    schema_name: Option<String>,
}

async fn explorer(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Query(query): Query<ExplorerQuery>,
) -> Result<Json<Value>> {
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    if connection.profile.db_type != SupportedDatabaseType::Sqlite {
        return explorer_live(connection, query).await;
    }

    let node_type = query.node_type.as_deref().unwrap_or("connection");
    let session = database_session(&connection.profile);
    let capabilities = sqlite_capabilities();
    let nodes = match node_type {
        "connection" => vec![DatabaseExplorerNode {
            id: format!("database:{connection_id}:main::main"),
            object_type: DatabaseObjectType::Database,
            connection_id,
            name: "main".to_string(),
            database_name: Some("main".to_string()),
            schema_name: None,
            has_children: true,
            description: Some("default".to_string()),
        }],
        "database" | "schema" => sqlite_objects(&connection)?
            .into_iter()
            .map(|object| DatabaseExplorerNode {
                id: format!(
                    "{}:{connection_id}:{}::{}",
                    database_object_type_as_str(object.object_type),
                    object.database_name.as_deref().unwrap_or("main"),
                    object.name
                ),
                object_type: object.object_type,
                connection_id,
                name: object.name,
                database_name: query
                    .database_name
                    .clone()
                    .or_else(|| Some("main".to_string())),
                schema_name: query.schema_name.clone(),
                has_children: false,
                description: Some(database_object_type_as_str(object.object_type).to_string()),
            })
            .collect(),
        _ => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Unsupported explorer node type",
            ));
        }
    };

    Ok(Json(serde_json::json!({
        "success": true,
        "session": session,
        "nodes": nodes,
        "capabilities": capabilities,
    })))
}

#[derive(Debug, Deserialize)]
struct ObjectDetailsQuery {
    #[serde(rename = "objectType")]
    object_type: String,
    #[serde(rename = "databaseName")]
    database_name: Option<String>,
    #[serde(rename = "schemaName")]
    schema_name: Option<String>,
    name: Option<String>,
    #[serde(rename = "includeRelational")]
    include_relational: Option<bool>,
}

async fn object_details(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Query(query): Query<ObjectDetailsQuery>,
) -> Result<Json<Value>> {
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    if connection.profile.db_type != SupportedDatabaseType::Sqlite {
        return object_details_live(connection, query).await;
    }

    let object_type = parse_object_type(&query.object_type)?;
    let name = query.name.clone().unwrap_or_else(|| "main".to_string());
    let details = match object_type {
        DatabaseObjectType::Database => {
            let objects = sqlite_objects(&connection)?;
            let relational_schema = if query.include_relational.unwrap_or(false) {
                Some(sqlite_relational_schema(&connection, &objects)?)
            } else {
                None
            };
            DatabaseObjectDetails {
                database_name: Some(name.clone()),
                schema_name: query.schema_name.clone(),
                name,
                object_type,
                columns: Vec::new(),
                primary_key: Vec::new(),
                foreign_keys: Vec::new(),
                relational_schema,
                databases: vec![DatabaseNameSummary {
                    name: "main".to_string(),
                    is_default: true,
                }],
                schemas: Vec::new(),
                objects,
            }
        }
        DatabaseObjectType::Table | DatabaseObjectType::View => {
            let table_name = query
                .name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| {
                    ServerError::new(StatusCode::BAD_REQUEST, "Object name is required")
                })?;
            let table = describe_sqlite_table(&connection, table_name, object_type)?;
            DatabaseObjectDetails {
                database_name: query.database_name.or_else(|| Some("main".to_string())),
                schema_name: query.schema_name,
                name: table_name.to_string(),
                object_type,
                primary_key: table
                    .columns
                    .iter()
                    .filter(|column| column.is_primary_key)
                    .map(|column| column.name.clone())
                    .collect(),
                foreign_keys: table.foreign_keys,
                columns: table.columns,
                relational_schema: None,
                databases: Vec::new(),
                schemas: Vec::new(),
                objects: Vec::new(),
            }
        }
        _ => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Unsupported database object type",
            ));
        }
    };

    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "details": details,
    })))
}

async fn execute_query(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabaseQueryRequest>,
) -> Result<Response> {
    let sql = request.sql.trim();
    if sql.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "SQL is required",
                "code": "INVALID_SQL",
                "category": "validation",
                "retryable": false,
                "context": {
                    "databaseName": request.database_name,
                    "schemaName": request.schema_name,
                },
            })),
        )
            .into_response());
    }

    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let max_rows = request
        .max_rows
        .filter(|value| *value > 0)
        .map(|value| value.min(MAX_QUERY_MAX_ROWS))
        .unwrap_or(DEFAULT_QUERY_MAX_ROWS);
    let database_name = match connection.profile.db_type {
        SupportedDatabaseType::Sqlite => None,
        SupportedDatabaseType::Postgresql
        | SupportedDatabaseType::Mysql
        | SupportedDatabaseType::Mariadb => request
            .database_name
            .or_else(|| connection.profile.database_name.clone()),
    };
    let schema_name = if connection.profile.db_type == SupportedDatabaseType::Postgresql {
        request.schema_name
    } else {
        None
    };
    let execution = if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        execute_sqlite_query(&connection, sql, max_rows)
    } else {
        execute_live_query(
            &connection,
            sql,
            max_rows,
            database_name.as_deref(),
            schema_name.as_deref(),
        )
        .await
    };
    let mut result = match execution {
        Ok(result) => result,
        Err(error) => {
            return Ok(database_error_response(
                error,
                "Failed to execute database query",
                Some(serde_json::json!({
                    "databaseName": database_name,
                    "schemaName": schema_name,
                })),
            ));
        }
    };
    result.database_name = database_name;
    result.schema_name = schema_name;

    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "context": {
            "databaseName": result.database_name,
            "schemaName": result.schema_name,
        },
        "result": result,
    }))
    .into_response())
}

#[derive(Debug, Deserialize)]
struct TableDataQuery {
    #[serde(rename = "databaseName")]
    database_name: Option<String>,
    #[serde(rename = "schemaName")]
    schema_name: Option<String>,
    #[serde(rename = "tableName")]
    table_name: String,
    limit: Option<usize>,
    offset: Option<usize>,
    #[serde(rename = "includeTotalCount")]
    include_total_count: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct DatabaseFileJobRequest {
    #[serde(rename = "connectionId")]
    connection_id: i64,
    #[serde(rename = "databaseName")]
    database_name: Option<String>,
    #[serde(rename = "schemaName")]
    schema_name: Option<String>,
    #[serde(rename = "tableName")]
    table_name: String,
    #[serde(rename = "filePath")]
    file_path: String,
}

#[derive(Debug, Deserialize)]
struct DatabaseTableScopeRequest {
    #[serde(rename = "databaseName")]
    database_name: Option<String>,
    #[serde(rename = "schemaName")]
    schema_name: Option<String>,
    #[serde(rename = "tableName")]
    table_name: String,
}

#[derive(Debug, Deserialize)]
struct DatabaseRowEditRequest {
    #[serde(flatten)]
    scope: DatabaseTableScopeRequest,
    #[serde(rename = "primaryKeyValues")]
    primary_key_values: Map<String, Value>,
    #[serde(rename = "columnValues")]
    column_values: Map<String, Value>,
    #[serde(rename = "originalValues", default)]
    original_values: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct DatabaseRowAddRequest {
    #[serde(flatten)]
    scope: DatabaseTableScopeRequest,
    #[serde(rename = "columnValues")]
    column_values: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct DatabaseRowDeleteRequest {
    #[serde(flatten)]
    scope: DatabaseTableScopeRequest,
    #[serde(rename = "primaryKeyValues")]
    primary_key_values: Map<String, Value>,
    #[serde(rename = "originalValues", default)]
    original_values: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct DatabaseRowsDeleteRequest {
    #[serde(flatten)]
    scope: DatabaseTableScopeRequest,
    #[serde(rename = "primaryKeyRows", default)]
    primary_key_rows: Vec<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
struct DatabasePasteColumn {
    name: String,
}

#[derive(Debug, Deserialize)]
struct DatabaseClipboardPayload {
    #[serde(rename = "type")]
    clipboard_type: String,
    #[serde(rename = "createdAt")]
    _created_at: Option<String>,
    #[serde(rename = "sourceConnectionId")]
    _source_connection_id: Option<i64>,
    #[serde(rename = "sourceDatabaseName")]
    _source_database_name: Option<String>,
    #[serde(rename = "sourceSchemaName")]
    _source_schema_name: Option<String>,
    #[serde(rename = "sourceTableName")]
    _source_table_name: Option<String>,
    #[serde(default)]
    columns: Vec<DatabaseObjectColumn>,
    #[serde(default)]
    rows: Vec<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
struct DatabasePasteRequest {
    #[serde(flatten)]
    scope: DatabaseTableScopeRequest,
    #[serde(default)]
    clipboard: Option<DatabaseClipboardPayload>,
    #[serde(rename = "sourceColumns", default)]
    source_columns: Vec<DatabasePasteColumn>,
    #[serde(rename = "sourceRows", default)]
    source_rows: Vec<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
struct DatabasePortableExportRequest {
    #[serde(flatten)]
    scope: DatabaseTableScopeRequest,
    #[serde(rename = "includeData", default)]
    include_data: bool,
}

#[derive(Debug, Deserialize)]
struct DatabasePortableImportRequest {
    #[serde(rename = "databaseName")]
    database_name: Option<String>,
    #[serde(rename = "schemaName")]
    schema_name: Option<String>,
    #[serde(rename = "tableName")]
    table_name: Option<String>,
    payload: Value,
}

async fn table_data(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Query(query): Query<TableDataQuery>,
) -> Result<Json<Value>> {
    if query.table_name.trim().is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "tableName is required",
        ));
    }

    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let limit = query
        .limit
        .filter(|value| *value > 0)
        .map(|value| value.min(MAX_TABLE_PAGE_SIZE))
        .unwrap_or(DEFAULT_TABLE_PAGE_SIZE);
    let offset = query.offset.unwrap_or(0);
    let mut data = if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        read_sqlite_table_data(
            &connection,
            &query.table_name,
            limit,
            offset,
            query.include_total_count.unwrap_or(false),
        )?
    } else {
        read_live_table_data(
            &connection,
            query.database_name.as_deref(),
            query.schema_name.as_deref(),
            &query.table_name,
            limit,
            offset,
            query.include_total_count.unwrap_or(false),
        )
        .await?
    };
    data.database_name = query
        .database_name
        .or_else(|| Some(default_database_name(&connection)));
    data.schema_name = query.schema_name;

    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "data": data,
    })))
}

async fn paste_table_rows(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabasePasteRequest>,
) -> Result<Response> {
    validate_table_scope(&request.scope)?;
    let (source_column_names, source_rows) = if let Some(clipboard) = request.clipboard {
        if clipboard.clipboard_type != "rows"
            || clipboard.columns.is_empty()
            || clipboard.rows.is_empty()
        {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "A structured row clipboard payload is required",
                    "code": "INVALID_CLIPBOARD",
                    "category": "validation",
                    "retryable": false,
                })),
            )
                .into_response());
        }
        (
            clipboard
                .columns
                .into_iter()
                .map(|column| column.name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>(),
            clipboard.rows,
        )
    } else {
        if request.source_rows.is_empty() {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "A structured row clipboard payload is required",
                    "code": "INVALID_CLIPBOARD",
                    "category": "validation",
                    "retryable": false,
                })),
            )
                .into_response());
        }
        let names = if request.source_columns.is_empty() {
            request
                .source_rows
                .iter()
                .flat_map(|row| row.keys().cloned())
                .fold(Vec::<String>::new(), |mut names, name| {
                    if !names.iter().any(|candidate| candidate == &name) {
                        names.push(name);
                    }
                    names
                })
        } else {
            request
                .source_columns
                .into_iter()
                .map(|column| column.name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect()
        };
        (names, request.source_rows)
    };

    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let metadata = database_table_metadata(
        &connection,
        request.scope.database_name.as_deref(),
        request.scope.schema_name.as_deref(),
        &request.scope.table_name,
    )
    .await?;
    let mappings = build_column_mappings(&source_column_names, &metadata.columns);
    let used_source_columns = mappings
        .iter()
        .map(|(_, source)| source.clone())
        .collect::<Vec<_>>();
    let ignored_source_columns = source_column_names
        .iter()
        .filter(|name| !used_source_columns.iter().any(|used| used == *name))
        .cloned()
        .collect::<Vec<_>>();
    let mut column_failures = metadata
        .columns
        .iter()
        .filter(|column| {
            column.nullable == Some(false)
                && column.default_value.is_none()
                && !column.is_primary_key
                && !mappings.iter().any(|(target, _)| target == &column.name)
        })
        .map(|column| {
            serde_json::json!({
                "columnName": column.name,
                "message": "Required target column has no matching source column",
            })
        })
        .collect::<Vec<_>>();

    if mappings.is_empty() {
        column_failures.push(serde_json::json!({
            "message": "No compatible columns were found between the clipboard payload and the target table",
        }));
    }
    if mappings.is_empty() || !column_failures.is_empty() {
        let error = if mappings.is_empty() {
            "No compatible columns available for paste"
        } else {
            "Target table is missing required column mappings"
        };
        let result = serde_json::json!({
            "databaseName": request.scope.database_name,
            "schemaName": request.scope.schema_name,
            "tableName": request.scope.table_name,
            "attemptedRowCount": source_rows.len(),
            "insertedRowCount": 0,
            "failedRowCount": 0,
            "mappings": mappings.iter().map(|(target, source)| serde_json::json!({
                "targetColumnName": target,
                "sourceColumnName": source,
            })).collect::<Vec<_>>(),
            "ignoredSourceColumns": ignored_source_columns,
            "columnFailures": column_failures,
            "rowFailures": [],
        });
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": error, "result": result })),
        )
            .into_response());
    }

    let mut inserted_row_count = 0usize;
    let mut row_failures = Vec::new();
    for (row_index, source_row) in source_rows.iter().enumerate() {
        let values = mappings
            .iter()
            .map(|(target, source)| {
                (
                    target.clone(),
                    source_row.get(source).cloned().unwrap_or(Value::Null),
                )
            })
            .collect::<Map<_, _>>();
        match insert_database_row(
            &connection,
            request.scope.database_name.as_deref(),
            request.scope.schema_name.as_deref(),
            &request.scope.table_name,
            &metadata,
            &values,
        )
        .await
        {
            Ok(_) => inserted_row_count += 1,
            Err(error) => row_failures.push(serde_json::json!({
                "rowIndex": row_index,
                "message": database_error_message(&error),
                "code": error.body.code,
            })),
        }
    }

    let failed_row_count = row_failures.len();
    let result = serde_json::json!({
        "databaseName": request.scope.database_name,
        "schemaName": request.scope.schema_name,
        "tableName": request.scope.table_name,
        "attemptedRowCount": source_rows.len(),
        "insertedRowCount": inserted_row_count,
        "failedRowCount": failed_row_count,
        "mappings": mappings.iter().map(|(target, source)| serde_json::json!({
            "targetColumnName": target,
            "sourceColumnName": source,
        })).collect::<Vec<_>>(),
        "ignoredSourceColumns": ignored_source_columns,
        "columnFailures": column_failures,
        "rowFailures": row_failures,
    });
    Ok(Json(serde_json::json!({
        "success": failed_row_count == 0,
        "session": database_session(&connection.profile),
        "result": result,
    }))
    .into_response())
}

async fn edit_table_row(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabaseRowEditRequest>,
) -> Result<Json<Value>> {
    validate_table_scope(&request.scope)?;
    if request.primary_key_values.is_empty() || request.column_values.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "primaryKeyValues and columnValues must be non-empty objects",
        ));
    }
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let row = update_database_row(
        &connection,
        &request.scope,
        &request.primary_key_values,
        &request.column_values,
        &request.original_values,
    )
    .await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "row": row,
    })))
}

async fn add_table_row(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabaseRowAddRequest>,
) -> Result<Json<Value>> {
    validate_table_scope(&request.scope)?;
    if request.column_values.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "columnValues must be a non-empty object",
        ));
    }
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let metadata = database_table_metadata(
        &connection,
        request.scope.database_name.as_deref(),
        request.scope.schema_name.as_deref(),
        &request.scope.table_name,
    )
    .await?;
    let row = insert_database_row(
        &connection,
        request.scope.database_name.as_deref(),
        request.scope.schema_name.as_deref(),
        &request.scope.table_name,
        &metadata,
        &request.column_values,
    )
    .await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "row": row,
    })))
}

async fn delete_table_row(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabaseRowDeleteRequest>,
) -> Result<Json<Value>> {
    validate_table_scope(&request.scope)?;
    if request.primary_key_values.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "primaryKeyValues must be a non-empty object",
        ));
    }
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    delete_database_row(
        &connection,
        &request.scope,
        &request.primary_key_values,
        &request.original_values,
    )
    .await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "deletedRowCount": 1,
    })))
}

async fn delete_table_rows(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabaseRowsDeleteRequest>,
) -> Result<Json<Value>> {
    validate_table_scope(&request.scope)?;
    if request.primary_key_rows.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "primaryKeyRows must be a non-empty array",
        ));
    }
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let mut deleted_row_count = 0usize;
    let mut row_failures = Vec::new();
    for (row_index, primary_key_values) in request.primary_key_rows.iter().enumerate() {
        match delete_database_row(&connection, &request.scope, primary_key_values, &Map::new())
            .await
        {
            Ok(_) => deleted_row_count += 1,
            Err(error) => row_failures.push(serde_json::json!({
                "rowIndex": row_index,
                "message": error.body.details.clone().unwrap_or(error.body.error),
            })),
        }
    }
    Ok(Json(serde_json::json!({
        "success": row_failures.is_empty(),
        "session": database_session(&connection.profile),
        "deletedRowCount": deleted_row_count,
        "failedRowCount": row_failures.len(),
        "rowFailures": row_failures,
    })))
}

async fn export_portable_table(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabasePortableExportRequest>,
) -> Result<Json<Value>> {
    validate_table_scope(&request.scope)?;
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let metadata = database_table_metadata(
        &connection,
        request.scope.database_name.as_deref(),
        request.scope.schema_name.as_deref(),
        &request.scope.table_name,
    )
    .await?;
    let endpoint = DatabaseTransferEndpoint {
        connection_id,
        connection_name: Some(connection.profile.name.clone()),
        connection_type: Some(connection.profile.db_type),
        database_name: request
            .scope
            .database_name
            .clone()
            .or_else(|| Some(default_database_name(&connection))),
        schema_name: request.scope.schema_name.clone(),
        table_name: request.scope.table_name.clone(),
    };
    let snapshot = if request.include_data {
        Some(read_transfer_source(&connection, &endpoint).await?)
    } else {
        None
    };
    let mut payload = serde_json::json!({
        "format": "web-ai-cli/database-portable-v1",
        "type": if request.include_data { "table-schema-and-data" } else { "table-schema" },
        "exportedAt": Utc::now(),
        "source": {
            "connectionType": connection.profile.db_type,
            "databaseName": endpoint.database_name,
            "schemaName": endpoint.schema_name,
            "tableName": endpoint.table_name,
            "objectType": "table",
        },
        "table": {
            "name": request.scope.table_name,
            "type": "table",
            "columns": metadata.columns.iter().map(canonical_portable_column).collect::<Vec<_>>(),
            "primaryKey": metadata.primary_key,
        },
    });
    if let Some(snapshot) = snapshot {
        payload
            .as_object_mut()
            .expect("portable payload must be an object")
            .insert(
                "rows".to_string(),
                Value::Array(snapshot.rows.into_iter().map(Value::Object).collect()),
            );
    }
    Ok(Json(
        serde_json::json!({ "success": true, "payload": payload }),
    ))
}

async fn import_portable_table(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
    Json(request): Json<DatabasePortableImportRequest>,
) -> Result<Json<Value>> {
    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let portable = parse_portable_table_payload(&request.payload)?;
    let table_name = request
        .table_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&portable.table_name)
        .to_string();
    if table_name.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "A target table name is required for portable imports",
        ));
    }
    let endpoint = DatabaseTransferEndpoint {
        connection_id,
        connection_name: Some(connection.profile.name.clone()),
        connection_type: Some(connection.profile.db_type),
        database_name: request
            .database_name
            .clone()
            .or_else(|| Some(default_database_name(&connection))),
        schema_name: request.schema_name.clone(),
        table_name: table_name.clone(),
    };
    if transfer_target_exists(&connection, &endpoint).await? {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            format!("Target table already exists: {table_name}"),
        ));
    }
    ensure_transfer_target_schema(&connection, &endpoint).await?;
    create_transfer_target_table(&connection, &endpoint, &portable.columns).await?;
    let mut imported_row_count = 0usize;
    let mut failed_row_count = 0usize;
    let mut row_failures = Vec::new();
    if portable.include_data {
        for (row_index, row) in portable.rows.iter().enumerate() {
            match insert_transfer_rows(
                &connection,
                &endpoint,
                &portable.columns,
                std::slice::from_ref(row),
            )
            .await
            {
                Ok(count) => imported_row_count += count,
                Err(error) => {
                    failed_row_count += 1;
                    if row_failures.len() < MAX_ROW_FAILURE_DETAILS {
                        row_failures.push(serde_json::json!({
                            "rowIndex": row_index,
                            "message": database_error_message(&error),
                            "code": error.body.code,
                        }));
                    }
                }
            }
        }
    }
    let result = serde_json::json!({
        "databaseName": endpoint.database_name,
        "schemaName": endpoint.schema_name,
        "tableName": endpoint.table_name,
        "createdTable": true,
        "importedRowCount": imported_row_count,
        "failedRowCount": failed_row_count,
        "ignoredSourceColumns": [],
        "mappedColumnCount": portable.columns.len(),
        "columnFailures": [],
        "rowFailures": row_failures,
    });
    Ok(Json(serde_json::json!({
        "success": result["failedRowCount"].as_u64() == Some(0),
        "session": database_session(&connection.profile),
        "result": result,
    })))
}

async fn create_transfer(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<DatabaseTransferRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    if request.source.table_name.trim().is_empty() || request.target.table_name.trim().is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "source and target table names are required",
        ));
    }
    if request.source.connection_id == request.target.connection_id
        && request.source.database_name == request.target.database_name
        && request.source.schema_name == request.target.schema_name
        && request.source.table_name == request.target.table_name
    {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Source and target tables must be different",
        ));
    }

    let source_connection = load_connection(&state, &user.0.id, request.source.connection_id)?;
    let target_connection = load_connection(&state, &user.0.id, request.target.connection_id)?;

    let now = Utc::now();
    let job = DatabaseTransferJob {
        id: new_id("dbjob"),
        job_type: "table-transfer".to_string(),
        mode: request.mode,
        status: DatabaseTransferJobStatus::Running,
        source: enrich_transfer_endpoint(request.source, &source_connection.profile),
        target: enrich_transfer_endpoint(request.target, &target_connection.profile),
        progress: progress(0, 1, "Queued"),
        logs: vec![DatabaseTransferJobLogEntry {
            timestamp: now,
            level: "info".to_string(),
            message: "Transfer queued".to_string(),
            details: None,
        }],
        warnings: Vec::new(),
        error: None,
        result: None,
        created_at: now,
        updated_at: now,
        started_at: None,
        finished_at: None,
    };

    state
        .storage
        .upsert_database_transfer_job(&user.0.id, &job)?;

    let response_job = job.clone();
    let task_state = state.clone();
    let user_id = user.0.id.clone();
    tokio::spawn(async move {
        let mut job = job;
        let started_at = Utc::now();
        job.started_at = Some(started_at);
        job.updated_at = started_at;
        let storage = task_state.storage.clone();
        let persist_user_id = user_id.clone();
        let transfer_result = {
            let mut persist_job = move |updated_job: &DatabaseTransferJob| {
                let _ = storage.upsert_database_transfer_job(&persist_user_id, updated_job);
            };
            run_database_transfer(
                &source_connection,
                &target_connection,
                &mut job,
                &mut persist_job,
            )
            .await
        };

        match transfer_result {
            Ok(result) => {
                let finished_at = Utc::now();
                let completion_message = match job.mode {
                    DatabaseTransferMode::TableCopy => "Table copy completed",
                    DatabaseTransferMode::SchemaOnly => "Schema copy completed",
                    DatabaseTransferMode::SchemaAndData => "Schema and data copy completed",
                };
                job.status = DatabaseTransferJobStatus::Succeeded;
                job.progress = progress(1, 1, completion_message);
                job.logs.push(DatabaseTransferJobLogEntry {
                    timestamp: finished_at,
                    level: "info".to_string(),
                    message: completion_message.to_string(),
                    details: None,
                });
                job.updated_at = finished_at;
                job.finished_at = Some(finished_at);
                job.result = Some(result);
            }
            Err(error) => fail_database_job(&mut job, error),
        }
        let _ = task_state
            .storage
            .upsert_database_transfer_job(&user_id, &job);
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "success": true,
            "job": response_job,
        })),
    ))
}

async fn list_jobs(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "success": true,
        "jobs": state.storage.list_database_transfer_jobs(&user.0.id)?,
    })))
}

async fn get_job(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(job_id): AxumPath<String>,
) -> Result<Json<Value>> {
    let job = state
        .storage
        .get_database_transfer_job(&user.0.id, &job_id)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Transfer job not found"))?;
    Ok(Json(serde_json::json!({
        "success": true,
        "job": job,
    })))
}

async fn export_table(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<DatabaseFileJobRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    if request.table_name.trim().is_empty() || request.file_path.trim().is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "tableName and filePath are required",
        ));
    }
    let connection = load_connection(&state, &user.0.id, request.connection_id)?;
    let output_path = state
        .path_validator
        .validate_path(PathBuf::from(&request.file_path), true)
        .await?;
    let endpoint = DatabaseTransferEndpoint {
        connection_id: request.connection_id,
        connection_name: Some(connection.profile.name.clone()),
        connection_type: Some(connection.profile.db_type),
        database_name: request
            .database_name
            .or_else(|| Some(default_database_name(&connection))),
        schema_name: request.schema_name,
        table_name: request.table_name,
    };
    let now = Utc::now();
    let mut job = database_file_job("data-export", &endpoint, &endpoint, now);

    match read_transfer_source(&connection, &endpoint).await {
        Ok(snapshot) => {
            let finished_at = Utc::now();
            let export = serde_json::json!({
                "format": "io-workbench.table-export.v1",
                "connection": connection.profile,
                "table": endpoint,
                "columns": snapshot.columns,
                "rows": snapshot.rows,
                "truncated": false,
                "exportedAt": finished_at,
            });
            if let Some(parent) = output_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(io_server_error)?;
            }
            let content = serde_json::to_vec_pretty(&export).map_err(|error| {
                ServerError::with_details(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to encode export",
                    error.to_string(),
                )
            })?;
            tokio::fs::write(&output_path, content)
                .await
                .map_err(io_server_error)?;
            job.status = DatabaseTransferJobStatus::Succeeded;
            job.progress = progress(1, 1, "Export completed");
            job.updated_at = finished_at;
            job.finished_at = Some(finished_at);
            job.result = Some(DatabaseTransferJobResult {
                created_table: false,
                copied_row_count: snapshot.rows.len(),
                failed_row_count: 0,
                ignored_source_columns: Vec::new(),
                mapped_column_count: snapshot.columns.len(),
                column_failures: Vec::new(),
                row_failures: Vec::new(),
            });
            job.logs.push(DatabaseTransferJobLogEntry {
                timestamp: finished_at,
                level: "info".to_string(),
                message: format!("Exported table to {}", output_path.display()),
                details: None,
            });
        }
        Err(error) => fail_database_job(&mut job, error),
    }

    state
        .storage
        .upsert_database_transfer_job(&user.0.id, &job)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "success": job.status == DatabaseTransferJobStatus::Succeeded,
            "job": job,
        })),
    ))
}

async fn import_table(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(request): Json<DatabaseFileJobRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    if request.table_name.trim().is_empty() || request.file_path.trim().is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "tableName and filePath are required",
        ));
    }
    let connection = load_connection(&state, &user.0.id, request.connection_id)?;
    let input_path = state
        .path_validator
        .validate_path(PathBuf::from(&request.file_path), false)
        .await?;
    let endpoint = DatabaseTransferEndpoint {
        connection_id: request.connection_id,
        connection_name: Some(connection.profile.name.clone()),
        connection_type: Some(connection.profile.db_type),
        database_name: request
            .database_name
            .or_else(|| Some(default_database_name(&connection))),
        schema_name: request.schema_name,
        table_name: request.table_name,
    };
    let now = Utc::now();
    let mut job = database_file_job("data-import", &endpoint, &endpoint, now);

    match read_import_rows(&input_path).await {
        Ok(rows) => {
            let columns = infer_import_columns(&rows);
            let result = async {
                let target_exists = transfer_target_exists(&connection, &endpoint).await?;
                if !target_exists {
                    create_transfer_target_table(&connection, &endpoint, &columns).await?;
                }
                let copied = insert_transfer_rows(&connection, &endpoint, &columns, &rows).await?;
                Ok::<_, ServerError>((target_exists, copied))
            }
            .await;
            match result {
                Ok((target_exists, copied)) => {
                    let finished_at = Utc::now();
                    job.status = DatabaseTransferJobStatus::Succeeded;
                    job.progress = progress(1, 1, "Import completed");
                    job.updated_at = finished_at;
                    job.finished_at = Some(finished_at);
                    job.result = Some(DatabaseTransferJobResult {
                        created_table: !target_exists,
                        copied_row_count: copied,
                        failed_row_count: 0,
                        ignored_source_columns: Vec::new(),
                        mapped_column_count: columns.len(),
                        column_failures: Vec::new(),
                        row_failures: Vec::new(),
                    });
                    job.logs.push(DatabaseTransferJobLogEntry {
                        timestamp: finished_at,
                        level: "info".to_string(),
                        message: format!("Imported rows from {}", input_path.display()),
                        details: None,
                    });
                }
                Err(error) => fail_database_job(&mut job, error),
            }
        }
        Err(error) => fail_database_job(&mut job, error),
    }

    state
        .storage
        .upsert_database_transfer_job(&user.0.id, &job)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "success": job.status == DatabaseTransferJobStatus::Succeeded,
            "job": job,
        })),
    ))
}

fn database_file_job(
    job_type: &str,
    source: &DatabaseTransferEndpoint,
    target: &DatabaseTransferEndpoint,
    now: chrono::DateTime<Utc>,
) -> DatabaseTransferJob {
    DatabaseTransferJob {
        id: new_id("dbjob"),
        job_type: job_type.to_string(),
        mode: DatabaseTransferMode::SchemaAndData,
        status: DatabaseTransferJobStatus::Running,
        source: source.clone(),
        target: target.clone(),
        progress: progress(0, 1, "Job started"),
        logs: vec![DatabaseTransferJobLogEntry {
            timestamp: now,
            level: "info".to_string(),
            message: format!("{job_type} job created"),
            details: None,
        }],
        warnings: Vec::new(),
        error: None,
        result: None,
        created_at: now,
        updated_at: now,
        started_at: Some(now),
        finished_at: None,
    }
}

fn fail_database_job(job: &mut DatabaseTransferJob, error: ServerError) {
    let finished_at = Utc::now();
    let body = error.body;
    let details = body.details.map(|details| {
        serde_json::from_str::<Value>(&details).unwrap_or_else(|_| Value::String(details))
    });
    let message = body.error;
    let code = body.code;
    job.status = DatabaseTransferJobStatus::Failed;
    job.logs.push(DatabaseTransferJobLogEntry {
        timestamp: finished_at,
        level: "error".to_string(),
        message: message.clone(),
        details: code.clone(),
    });
    job.error = Some(DatabaseTransferJobError {
        message,
        code,
        category: body.category,
        retryable: body.retryable.unwrap_or(false),
        details,
    });
    job.updated_at = finished_at;
    job.finished_at = Some(finished_at);
}

async fn read_import_rows(path: &PathBuf) -> Result<Vec<Map<String, Value>>> {
    let content = tokio::fs::read(path).await.map_err(io_server_error)?;
    let value = serde_json::from_slice::<Value>(&content).map_err(|error| {
        ServerError::with_details(
            StatusCode::BAD_REQUEST,
            "import file must be valid JSON",
            error.to_string(),
        )
    })?;
    let rows_value = value.get("rows").cloned().unwrap_or(value);
    let rows = rows_value
        .as_array()
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "import JSON must be an array or an object with a rows array",
            )
        })?
        .iter()
        .map(|row| {
            row.as_object().cloned().ok_or_else(|| {
                ServerError::new(StatusCode::BAD_REQUEST, "import rows must be JSON objects")
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if rows.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "import file has no rows",
        ));
    }
    Ok(rows)
}

fn infer_import_columns(rows: &[Map<String, Value>]) -> Vec<DatabaseObjectColumn> {
    let mut names = Vec::new();
    for row in rows {
        for key in row.keys() {
            if !names.contains(key) {
                names.push(key.clone());
            }
        }
    }
    names
        .into_iter()
        .map(|name| DatabaseObjectColumn {
            name,
            data_type: Some("text".to_string()),
            native_type: Some("TEXT".to_string()),
            nullable: Some(true),
            default_value: None,
            extra: None,
            is_primary_key: false,
        })
        .collect()
}

#[derive(Clone)]
struct DatabaseTableMetadata {
    columns: Vec<DatabaseObjectColumn>,
    primary_key: Vec<String>,
}

struct PortableTablePayload {
    table_name: String,
    columns: Vec<DatabaseObjectColumn>,
    rows: Vec<Map<String, Value>>,
    include_data: bool,
}

fn validate_table_scope(scope: &DatabaseTableScopeRequest) -> Result<()> {
    if scope.table_name.trim().is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "tableName is required",
        ));
    }
    Ok(())
}

async fn database_table_metadata(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    table_name: &str,
) -> Result<DatabaseTableMetadata> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let description = describe_sqlite_table(connection, table_name, DatabaseObjectType::Table)?;
        let primary_key = description
            .columns
            .iter()
            .filter(|column| column.is_primary_key)
            .map(|column| column.name.clone())
            .collect();
        return Ok(DatabaseTableMetadata {
            columns: description.columns,
            primary_key,
        });
    }

    let description = describe_live_table(
        connection,
        database_name,
        schema_name,
        table_name,
        DatabaseObjectType::Table,
    )
    .await?;
    let primary_key = description
        .columns
        .iter()
        .filter(|column| column.is_primary_key)
        .map(|column| column.name.clone())
        .collect();
    Ok(DatabaseTableMetadata {
        columns: description.columns,
        primary_key,
    })
}

fn build_column_mappings(
    source_columns: &[String],
    target_columns: &[DatabaseObjectColumn],
) -> Vec<(String, String)> {
    target_columns
        .iter()
        .filter_map(|target| {
            source_columns
                .iter()
                .find(|source| *source == &target.name)
                .or_else(|| {
                    let target_key = normalize_database_column_key(&target.name);
                    source_columns
                        .iter()
                        .find(|source| normalize_database_column_key(source) == target_key)
                })
                .map(|source| (target.name.clone(), source.clone()))
        })
        .collect()
}

fn normalize_database_column_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn canonical_portable_column(column: &DatabaseObjectColumn) -> Value {
    serde_json::json!({
        "name": column.name,
        "typeFamily": portable_type_family(column),
        "dataType": column.data_type,
        "nativeType": column.native_type,
        "nullable": column.nullable.unwrap_or(true),
        "defaultValue": column.default_value,
        "isPrimaryKey": column.is_primary_key,
    })
}

fn portable_type_family(column: &DatabaseObjectColumn) -> &'static str {
    let data_type = column
        .native_type
        .as_deref()
        .or(column.data_type.as_deref())
        .unwrap_or("")
        .to_ascii_lowercase();
    if data_type.contains("bool") {
        "boolean"
    } else if data_type.contains("bigint") {
        "bigint"
    } else if data_type.contains("int") {
        "integer"
    } else if data_type.contains("decimal") || data_type.contains("numeric") {
        "numeric"
    } else if data_type.contains("real")
        || data_type.contains("float")
        || data_type.contains("double")
    {
        "float"
    } else if data_type.contains("json") {
        "json"
    } else if data_type.contains("uuid") {
        "uuid"
    } else if data_type.contains("blob")
        || data_type.contains("binary")
        || data_type.contains("bytea")
    {
        "binary"
    } else if data_type.contains("timestamp") || data_type.contains("datetime") {
        "datetime"
    } else if data_type == "date" {
        "date"
    } else if data_type == "time" {
        "time"
    } else {
        "text"
    }
}

fn parse_portable_table_payload(payload: &Value) -> Result<PortableTablePayload> {
    let object = payload.as_object().ok_or_else(|| {
        ServerError::new(StatusCode::BAD_REQUEST, "A portable payload is required")
    })?;
    if object.get("format").and_then(Value::as_str) != Some("web-ai-cli/database-portable-v1") {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Unsupported portable table payload",
        ));
    }
    let payload_type = object.get("type").and_then(Value::as_str).unwrap_or("");
    let include_data = match payload_type {
        "table-schema" => false,
        "table-schema-and-data" => true,
        _ => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Unsupported portable table payload type",
            ));
        }
    };
    let table = object
        .get("table")
        .and_then(Value::as_object)
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Portable table is missing"))?;
    let table_name = table
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let columns = table
        .get("columns")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ServerError::new(
                StatusCode::BAD_REQUEST,
                "Portable table columns are missing",
            )
        })?
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let name = entry.get("name")?.as_str()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let type_family = entry
                .get("typeFamily")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let data_type = entry
                .get("dataType")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    type_family
                        .map(portable_type_family_name)
                        .map(str::to_string)
                });
            Some(DatabaseObjectColumn {
                name,
                data_type,
                native_type: entry
                    .get("nativeType")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                nullable: entry.get("nullable").and_then(Value::as_bool),
                default_value: entry
                    .get("defaultValue")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                extra: None,
                is_primary_key: entry
                    .get("isPrimaryKey")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect::<Vec<_>>();
    if columns.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Portable table has no columns",
        ));
    }
    let rows = if include_data {
        object
            .get("rows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| row.as_object().cloned())
            .collect()
    } else {
        Vec::new()
    };
    Ok(PortableTablePayload {
        table_name,
        columns,
        rows,
        include_data,
    })
}

fn portable_type_family_name(type_family: &str) -> &'static str {
    match type_family {
        "boolean" => "boolean",
        "bigint" => "bigint",
        "integer" => "integer",
        "numeric" => "numeric",
        "float" => "double",
        "json" => "json",
        "uuid" => "uuid",
        "binary" => "binary",
        "datetime" => "datetime",
        "date" => "date",
        "time" => "time",
        _ => "text",
    }
}

async fn update_database_row(
    connection: &StoredDatabaseConnection,
    scope: &DatabaseTableScopeRequest,
    primary_key_values: &Map<String, Value>,
    column_values: &Map<String, Value>,
    original_values: &Map<String, Value>,
) -> Result<Option<Map<String, Value>>> {
    let metadata = database_table_metadata(
        connection,
        scope.database_name.as_deref(),
        scope.schema_name.as_deref(),
        &scope.table_name,
    )
    .await?;
    if metadata.primary_key.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Table has no primary key. Row editing is not supported.",
        ));
    }
    let primary_keys = canonical_primary_key_values(&metadata, primary_key_values)?;
    let current = select_database_row(connection, scope, &primary_keys)
        .await?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Row not found"))?;
    ensure_row_is_current(&current, original_values)?;
    let updates = canonical_column_values(&metadata, column_values, false)?;
    if updates.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No non-primary-key columns provided for update",
        ));
    }
    let endpoint = database_scope_endpoint(connection, scope);
    let set_sql = updates
        .iter()
        .map(|(column, value)| {
            format!(
                "{} = {}",
                quote_transfer_identifier(connection.profile.db_type, column),
                transfer_value_literal(connection.profile.db_type, value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let where_sql = build_database_where_clause(connection.profile.db_type, &primary_keys);
    let sql = format!(
        "UPDATE {} SET {set_sql} WHERE {where_sql}",
        transfer_table_ref(connection.profile.db_type, &endpoint)
    );
    execute_database_mutation(connection, scope.database_name.as_deref(), &sql).await?;
    select_database_row(connection, scope, &primary_keys).await
}

async fn insert_database_row(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    table_name: &str,
    metadata: &DatabaseTableMetadata,
    column_values: &Map<String, Value>,
) -> Result<Option<Map<String, Value>>> {
    let mut values = canonical_column_values(metadata, column_values, true)?;
    values.retain(|(name, value)| {
        let column = metadata.columns.iter().find(|column| column.name == *name);
        !(value.is_null()
            && column.is_some_and(|column| {
                column.default_value.is_some()
                    || column
                        .extra
                        .as_deref()
                        .is_some_and(|extra| extra.to_ascii_lowercase().contains("auto_increment"))
            }))
    });
    if values.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "No valid column values provided for insert",
        ));
    }
    let scope = DatabaseTableScopeRequest {
        database_name: database_name.map(str::to_string),
        schema_name: schema_name.map(str::to_string),
        table_name: table_name.to_string(),
    };
    let endpoint = database_scope_endpoint(connection, &scope);
    let columns = values
        .iter()
        .map(|(name, _)| quote_transfer_identifier(connection.profile.db_type, name))
        .collect::<Vec<_>>()
        .join(", ");
    let sql_values = values
        .iter()
        .map(|(_, value)| transfer_value_literal(connection.profile.db_type, value))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO {} ({columns}) VALUES ({sql_values})",
        transfer_table_ref(connection.profile.db_type, &endpoint)
    );
    execute_database_mutation(connection, database_name, &sql).await?;

    let provided = values.into_iter().collect::<Map<_, _>>();
    let primary_keys = metadata
        .primary_key
        .iter()
        .filter_map(|column| {
            find_json_map_value(&provided, column).map(|value| (column.clone(), value.clone()))
        })
        .collect::<Map<_, _>>();
    if !metadata.primary_key.is_empty() && primary_keys.len() == metadata.primary_key.len() {
        return select_database_row(connection, &scope, &primary_keys).await;
    }
    Ok(None)
}

async fn delete_database_row(
    connection: &StoredDatabaseConnection,
    scope: &DatabaseTableScopeRequest,
    primary_key_values: &Map<String, Value>,
    original_values: &Map<String, Value>,
) -> Result<()> {
    let metadata = database_table_metadata(
        connection,
        scope.database_name.as_deref(),
        scope.schema_name.as_deref(),
        &scope.table_name,
    )
    .await?;
    if metadata.primary_key.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Table has no primary key. Row deletion is not supported.",
        ));
    }
    let primary_keys = canonical_primary_key_values(&metadata, primary_key_values)?;
    let current = select_database_row(connection, scope, &primary_keys)
        .await?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Row not found"))?;
    ensure_row_is_current(&current, original_values)?;
    let endpoint = database_scope_endpoint(connection, scope);
    let sql = format!(
        "DELETE FROM {} WHERE {}",
        transfer_table_ref(connection.profile.db_type, &endpoint),
        build_database_where_clause(connection.profile.db_type, &primary_keys),
    );
    execute_database_mutation(connection, scope.database_name.as_deref(), &sql).await?;
    Ok(())
}

fn canonical_primary_key_values(
    metadata: &DatabaseTableMetadata,
    values: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    metadata
        .primary_key
        .iter()
        .map(|column| {
            find_json_map_value(values, column)
                .cloned()
                .map(|value| (column.clone(), value))
                .ok_or_else(|| {
                    ServerError::new(
                        StatusCode::BAD_REQUEST,
                        format!("Primary key value for \"{column}\" is missing"),
                    )
                })
        })
        .collect()
}

fn canonical_column_values(
    metadata: &DatabaseTableMetadata,
    values: &Map<String, Value>,
    include_primary_keys: bool,
) -> Result<Vec<(String, Value)>> {
    let mut output: Vec<(String, Value)> = Vec::new();
    for (requested_name, value) in values {
        let Some(column) = metadata
            .columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(requested_name))
        else {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                format!("Unknown table column: {requested_name}"),
            ));
        };
        if !include_primary_keys && column.is_primary_key {
            continue;
        }
        if output
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(&column.name))
        {
            continue;
        }
        output.push((column.name.clone(), value.clone()));
    }
    Ok(output)
}

fn find_json_map_value<'a>(values: &'a Map<String, Value>, column: &str) -> Option<&'a Value> {
    values.get(column).or_else(|| {
        values
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(column))
            .map(|(_, value)| value)
    })
}

fn ensure_row_is_current(
    current: &Map<String, Value>,
    original_values: &Map<String, Value>,
) -> Result<()> {
    if original_values.is_empty() {
        return Ok(());
    }
    let changed = original_values.iter().any(|(column, original)| {
        find_json_map_value(current, column).is_none_or(|value| value != original)
    });
    if changed {
        return Err(ServerError::new(
            StatusCode::CONFLICT,
            "The row changed after it was loaded. Refresh before saving or deleting it.",
        ));
    }
    Ok(())
}

fn database_scope_endpoint(
    connection: &StoredDatabaseConnection,
    scope: &DatabaseTableScopeRequest,
) -> DatabaseTransferEndpoint {
    DatabaseTransferEndpoint {
        connection_id: connection.profile.id,
        connection_name: Some(connection.profile.name.clone()),
        connection_type: Some(connection.profile.db_type),
        database_name: scope
            .database_name
            .clone()
            .or_else(|| Some(default_database_name(connection))),
        schema_name: scope.schema_name.clone(),
        table_name: scope.table_name.clone(),
    }
}

fn build_database_where_clause(
    db_type: SupportedDatabaseType,
    values: &Map<String, Value>,
) -> String {
    values
        .iter()
        .map(|(column, value)| {
            let column = quote_transfer_identifier(db_type, column);
            if value.is_null() {
                format!("{column} IS NULL")
            } else {
                format!("{column} = {}", transfer_value_literal(db_type, value))
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

async fn select_database_row(
    connection: &StoredDatabaseConnection,
    scope: &DatabaseTableScopeRequest,
    primary_keys: &Map<String, Value>,
) -> Result<Option<Map<String, Value>>> {
    let endpoint = database_scope_endpoint(connection, scope);
    let sql = format!(
        "SELECT * FROM {} WHERE {} LIMIT 1",
        transfer_table_ref(connection.profile.db_type, &endpoint),
        build_database_where_clause(connection.profile.db_type, primary_keys),
    );
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_connection(connection)?;
        let mut statement = conn.prepare(&sql).map_err(sqlite_server_error)?;
        let column_names = statement
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut rows = statement.query([]).map_err(sqlite_server_error)?;
        return rows
            .next()
            .map_err(sqlite_server_error)?
            .map(|row| row_to_json_map(row, &column_names))
            .transpose();
    }
    Ok(
        fetch_live_row_maps(connection, scope.database_name.as_deref(), &sql)
            .await?
            .into_iter()
            .next(),
    )
}

async fn execute_database_mutation(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    sql: &str,
) -> Result<u64> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_connection(connection)?;
        let affected = conn.execute(sql, []).map_err(sqlite_server_error)?;
        return Ok(affected as u64);
    }
    let pool = live_pool_for_database(connection, database_name).await?;
    let result = pool.execute(sql).await.map_err(sqlx_server_error)?;
    Ok(result.rows_affected())
}

async fn normalize_connection_input(
    state: &AppState,
    mut input: DatabaseConnectionInput,
    existing_password: Option<String>,
) -> Result<DatabaseConnectionInput> {
    input.name = input.name.trim().to_string();
    if input.name.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Connection name is required",
        ));
    }

    if input.password.as_deref().is_none_or(str::is_empty) {
        input.password = existing_password;
    }

    match input.db_type {
        SupportedDatabaseType::Sqlite => {
            let file_path = input
                .file_path
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ServerError::new(StatusCode::BAD_REQUEST, "SQLite file path is required")
                })?;
            let resolved = state
                .path_validator
                .validate_path(PathBuf::from(file_path), false)
                .await?;
            input.host = None;
            input.port = None;
            input.username = None;
            input.password = None;
            input.database_name = None;
            input.file_path = Some(resolved.display().to_string());
            input.show_all_databases = false;
        }
        SupportedDatabaseType::Postgresql
        | SupportedDatabaseType::Mysql
        | SupportedDatabaseType::Mariadb => {
            input.host = trim_optional(input.host);
            input.username = trim_optional(input.username);
            input.database_name = trim_optional(input.database_name);
            if input.host.is_none() {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Host is required",
                ));
            }
            if input.username.is_none() {
                return Err(ServerError::new(
                    StatusCode::BAD_REQUEST,
                    "Username is required",
                ));
            }
            if input.port.is_none() {
                input.port = Some(if input.db_type == SupportedDatabaseType::Postgresql {
                    5432
                } else {
                    3306
                });
            }
            input.file_path = None;
            input.show_all_databases =
                input.db_type == SupportedDatabaseType::Postgresql && input.show_all_databases;
        }
    }

    Ok(input)
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn load_connection(
    state: &AppState,
    user_id: &str,
    connection_id: i64,
) -> Result<StoredDatabaseConnection> {
    state
        .storage
        .get_database_connection(user_id, connection_id)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Connection not found"))
}

fn ensure_sqlite_connection(connection: &StoredDatabaseConnection) -> Result<PathBuf> {
    if connection.profile.db_type != SupportedDatabaseType::Sqlite {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "{} connections are supported for live browsing and queries; table transfer currently requires SQLite connections",
                connection.profile.db_type.as_str()
            ),
        ));
    }

    connection
        .profile
        .file_path
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "SQLite file path is required"))
}

async fn test_connection_input(input: &DatabaseConnectionInput) -> Result<DatabaseTestResult> {
    if input.db_type == SupportedDatabaseType::Sqlite {
        let path = input.file_path.as_ref().map(PathBuf::from).ok_or_else(|| {
            ServerError::new(StatusCode::BAD_REQUEST, "SQLite file path is required")
        })?;
        return test_sqlite_path(&path);
    }

    let profile = DatabaseConnectionProfile {
        id: 0,
        name: input.name.clone(),
        db_type: input.db_type,
        host: input.host.clone(),
        port: input.port,
        username: input.username.clone(),
        database_name: input.database_name.clone(),
        file_path: None,
        show_all_databases: input.show_all_databases,
        has_password: input
            .password
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        last_test_status: None,
        last_test_message: None,
        last_tested_at: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let connection = StoredDatabaseConnection {
        profile,
        password: input.password.clone(),
    };
    test_live_connection(&connection).await
}

async fn test_connection_record(
    connection: &StoredDatabaseConnection,
) -> Result<DatabaseTestResult> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let path = ensure_sqlite_connection(connection)?;
        return test_sqlite_path(&path);
    }
    test_live_connection(connection).await
}

fn test_sqlite_path(path: &PathBuf) -> Result<DatabaseTestResult> {
    let started_at = Instant::now();
    if !path.exists() {
        return Err(ServerError::database(
            StatusCode::BAD_REQUEST,
            format!("SQLite database file does not exist: {}", path.display()),
            None,
            "SQLITE_CANTOPEN",
            "connection",
            false,
        ));
    }
    if !path.is_file() {
        return Err(ServerError::database(
            StatusCode::BAD_REQUEST,
            format!("SQLite path is not a file: {}", path.display()),
            None,
            "SQLITE_CANTOPEN",
            "connection",
            false,
        ));
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(sqlite_server_error)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(sqlite_server_error)?;
    let version: String = conn
        .query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .map_err(sqlite_server_error)?;
    Ok(DatabaseTestResult {
        status: DatabaseTestStatus::Success,
        message: format!("SQLite connection successful ({version})"),
        duration_ms: Some(started_at.elapsed().as_millis()),
    })
}

fn database_session(profile: &DatabaseConnectionProfile) -> DatabaseSessionInfo {
    DatabaseSessionInfo {
        session_id: format!("{}:{}", profile.db_type.as_str(), profile.id),
        connection_id: profile.id,
        db_type: profile.db_type,
        capabilities: database_capabilities(profile.db_type),
    }
}

fn database_capabilities(db_type: SupportedDatabaseType) -> DatabaseCapabilities {
    match db_type {
        SupportedDatabaseType::Sqlite => sqlite_capabilities(),
        SupportedDatabaseType::Postgresql => DatabaseCapabilities {
            supports_databases: true,
            supports_schemas: true,
            supports_views: true,
            supports_indexes: true,
            supports_multiple_databases: true,
            supports_foreign_keys: true,
            supports_parameterized_queries: true,
            supports_offset: true,
            supported_object_types: vec![DatabaseObjectType::Table, DatabaseObjectType::View],
        },
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => DatabaseCapabilities {
            supports_databases: true,
            supports_schemas: false,
            supports_views: true,
            supports_indexes: true,
            supports_multiple_databases: true,
            supports_foreign_keys: true,
            supports_parameterized_queries: true,
            supports_offset: true,
            supported_object_types: vec![DatabaseObjectType::Table, DatabaseObjectType::View],
        },
    }
}

fn sqlite_capabilities() -> DatabaseCapabilities {
    DatabaseCapabilities {
        supports_databases: false,
        supports_schemas: false,
        supports_views: true,
        supports_indexes: true,
        supports_multiple_databases: false,
        supports_foreign_keys: true,
        supports_parameterized_queries: true,
        supports_offset: true,
        supported_object_types: vec![DatabaseObjectType::Table, DatabaseObjectType::View],
    }
}

async fn test_live_connection(connection: &StoredDatabaseConnection) -> Result<DatabaseTestResult> {
    let started_at = Instant::now();
    let pool = live_pool(connection).await?;
    let version_sql = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => "SELECT version() AS version",
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            "SELECT VERSION() AS version"
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let row = sqlx::query(version_sql)
        .fetch_one(&pool)
        .await
        .map_err(sqlx_server_error)?;
    let version = any_string(&row, 0).unwrap_or_else(|| "connected".to_string());
    Ok(DatabaseTestResult {
        status: DatabaseTestStatus::Success,
        message: format!(
            "{} connection successful ({version})",
            connection.profile.db_type.as_str()
        ),
        duration_ms: Some(started_at.elapsed().as_millis()),
    })
}

async fn explorer_live(
    connection: StoredDatabaseConnection,
    query: ExplorerQuery,
) -> Result<Json<Value>> {
    let node_type = query.node_type.as_deref().unwrap_or("connection");
    let session = database_session(&connection.profile);
    let capabilities = database_capabilities(connection.profile.db_type);
    let nodes = match node_type {
        "connection" => live_database_nodes(&connection).await?,
        "database" if capabilities.supports_schemas => {
            live_schema_nodes(&connection, query.database_name.as_deref()).await?
        }
        "database" | "schema" => {
            live_object_nodes(
                &connection,
                query.database_name.as_deref(),
                query.schema_name.as_deref(),
            )
            .await?
        }
        _ => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Unsupported explorer node type",
            ));
        }
    };

    Ok(Json(serde_json::json!({
        "success": true,
        "session": session,
        "nodes": nodes,
        "capabilities": capabilities,
    })))
}

async fn object_details_live(
    connection: StoredDatabaseConnection,
    query: ObjectDetailsQuery,
) -> Result<Json<Value>> {
    let object_type = parse_object_type(&query.object_type)?;
    let resolved_database_name = if object_type == DatabaseObjectType::Database {
        query.name.clone().or_else(|| query.database_name.clone())
    } else {
        query.database_name.clone()
    };
    let resolved_schema_name = if object_type == DatabaseObjectType::Schema {
        query.name.clone().or_else(|| query.schema_name.clone())
    } else {
        query.schema_name.clone()
    };
    let name = query
        .name
        .clone()
        .or_else(|| resolved_database_name.clone())
        .unwrap_or_else(|| default_database_name(&connection));
    let details = match object_type {
        DatabaseObjectType::Database | DatabaseObjectType::Schema => {
            let objects = live_objects(
                &connection,
                resolved_database_name.as_deref(),
                resolved_schema_name.as_deref(),
            )
            .await?;
            let relational_schema = if query.include_relational.unwrap_or(false) {
                Some(
                    live_relational_schema(
                        &connection,
                        object_type,
                        &name,
                        resolved_schema_name.as_deref(),
                        &objects,
                    )
                    .await?,
                )
            } else {
                None
            };
            DatabaseObjectDetails {
                database_name: resolved_database_name
                    .clone()
                    .or_else(|| Some(default_database_name(&connection))),
                schema_name: resolved_schema_name.clone(),
                name,
                object_type,
                columns: Vec::new(),
                primary_key: Vec::new(),
                foreign_keys: Vec::new(),
                relational_schema,
                databases: live_databases(&connection).await?,
                schemas: live_schemas(&connection, resolved_database_name.as_deref()).await?,
                objects,
            }
        }
        DatabaseObjectType::Table | DatabaseObjectType::View => {
            let table_name = query
                .name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| {
                    ServerError::new(StatusCode::BAD_REQUEST, "Object name is required")
                })?;
            let table = describe_live_table(
                &connection,
                resolved_database_name.as_deref(),
                resolved_schema_name.as_deref(),
                table_name,
                object_type,
            )
            .await?;
            DatabaseObjectDetails {
                database_name: resolved_database_name
                    .or_else(|| Some(default_database_name(&connection))),
                schema_name: resolved_schema_name,
                name: table_name.to_string(),
                object_type,
                primary_key: table
                    .columns
                    .iter()
                    .filter(|column| column.is_primary_key)
                    .map(|column| column.name.clone())
                    .collect(),
                foreign_keys: table.foreign_keys,
                columns: table.columns,
                relational_schema: None,
                databases: Vec::new(),
                schemas: Vec::new(),
                objects: Vec::new(),
            }
        }
        _ => {
            return Err(ServerError::new(
                StatusCode::BAD_REQUEST,
                "Unsupported database object type",
            ));
        }
    };

    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "details": details,
    })))
}

struct LiveTableDescription {
    columns: Vec<DatabaseObjectColumn>,
    foreign_keys: Vec<DatabaseForeignKey>,
}

async fn live_pool(connection: &StoredDatabaseConnection) -> Result<AnyPool> {
    live_pool_for_database(connection, None).await
}

async fn live_pool_for_database(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
) -> Result<AnyPool> {
    SQLX_ANY_DRIVERS.call_once(sqlx::any::install_default_drivers);
    let url = connection_url_for_database(connection, database_name)?;
    if connection.profile.id <= 0 {
        return connect_live_pool(&url).await;
    }

    let cache_key = format!("{}:{url}", connection.profile.id);
    let now = Instant::now();
    {
        let mut cache = live_pool_cache().lock().await;
        cache.retain(|_, entry| {
            !entry.pool.is_closed()
                && now.saturating_duration_since(entry.last_used) <= LIVE_POOL_CACHE_TTL
        });
        if let Some(entry) = cache.get_mut(&cache_key) {
            entry.last_used = now;
            return Ok(entry.pool.clone());
        }
    }

    let pool = connect_live_pool(&url).await?;
    let mut cache = live_pool_cache().lock().await;
    if let Some(entry) = cache.get_mut(&cache_key) {
        entry.last_used = now;
        return Ok(entry.pool.clone());
    }
    if cache.len() >= LIVE_POOL_CACHE_MAX_ENTRIES {
        if let Some(oldest_key) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest_key);
        }
    }
    cache.insert(
        cache_key,
        CachedLivePool {
            connection_id: connection.profile.id,
            pool: pool.clone(),
            last_used: now,
        },
    );
    Ok(pool)
}

async fn connect_live_pool(url: &str) -> Result<AnyPool> {
    AnyPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Some(LIVE_POOL_CACHE_TTL))
        .connect(url)
        .await
        .map_err(sqlx_server_error)
}

fn live_pool_cache() -> &'static Mutex<HashMap<String, CachedLivePool>> {
    LIVE_POOL_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn typed_live_pool_for_database(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
) -> Result<TypedLivePool> {
    let url = connection_url_for_database(connection, database_name)?;
    if connection.profile.id <= 0 {
        return connect_typed_live_pool(connection.profile.db_type, &url).await;
    }

    let cache_key = format!("{}:{url}", connection.profile.id);
    let now = Instant::now();
    {
        let mut cache = typed_live_pool_cache().lock().await;
        cache.retain(|_, entry| {
            !entry.pool.is_closed()
                && now.saturating_duration_since(entry.last_used) <= LIVE_POOL_CACHE_TTL
        });
        if let Some(entry) = cache.get_mut(&cache_key) {
            entry.last_used = now;
            return Ok(entry.pool.clone());
        }
    }

    let pool = connect_typed_live_pool(connection.profile.db_type, &url).await?;
    let mut cache = typed_live_pool_cache().lock().await;
    if let Some(entry) = cache.get_mut(&cache_key) {
        entry.last_used = now;
        return Ok(entry.pool.clone());
    }
    if cache.len() >= LIVE_POOL_CACHE_MAX_ENTRIES {
        if let Some(oldest_key) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest_key);
        }
    }
    cache.insert(
        cache_key,
        CachedTypedLivePool {
            connection_id: connection.profile.id,
            pool: pool.clone(),
            last_used: now,
        },
    );
    Ok(pool)
}

async fn connect_typed_live_pool(
    db_type: SupportedDatabaseType,
    url: &str,
) -> Result<TypedLivePool> {
    match db_type {
        SupportedDatabaseType::Postgresql => PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Some(LIVE_POOL_CACHE_TTL))
            .connect(url)
            .await
            .map(TypedLivePool::Postgresql)
            .map_err(sqlx_server_error),
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => MySqlPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Some(LIVE_POOL_CACHE_TTL))
            .connect(url)
            .await
            .map(TypedLivePool::Mysql)
            .map_err(sqlx_server_error),
        SupportedDatabaseType::Sqlite => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "SQLite does not use a live SQLx pool",
        )),
    }
}

fn typed_live_pool_cache() -> &'static Mutex<HashMap<String, CachedTypedLivePool>> {
    TYPED_LIVE_POOL_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn evict_live_pools(connection_id: i64) {
    if let Some(cache) = LIVE_POOL_CACHE.get() {
        cache
            .lock()
            .await
            .retain(|_, entry| entry.connection_id != connection_id);
    }
    if let Some(cache) = TYPED_LIVE_POOL_CACHE.get() {
        cache
            .lock()
            .await
            .retain(|_, entry| entry.connection_id != connection_id);
    }
}

fn connection_url_for_database(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
) -> Result<String> {
    let host = connection
        .profile
        .host
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Host is required"))?;
    let username = connection
        .profile
        .username
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ServerError::new(StatusCode::BAD_REQUEST, "Username is required"))?;
    let password = connection.password.as_deref().unwrap_or("");
    let port = connection
        .profile
        .port
        .unwrap_or(match connection.profile.db_type {
            SupportedDatabaseType::Postgresql => 5432,
            SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => 3306,
            SupportedDatabaseType::Sqlite => unreachable!(),
        });
    let database = database_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(connection.profile.database_name.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(match connection.profile.db_type {
            SupportedDatabaseType::Postgresql => "postgres",
            SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => "mysql",
            SupportedDatabaseType::Sqlite => unreachable!(),
        });
    let scheme = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => "postgres",
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => "mysql",
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    Ok(format!(
        "{scheme}://{}:{}@{}:{}/{}",
        url_encode(username),
        url_encode(password),
        host,
        port,
        url_encode(database)
    ))
}

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect::<Vec<_>>(),
        })
        .collect()
}

fn default_database_name(connection: &StoredDatabaseConnection) -> String {
    connection
        .profile
        .database_name
        .clone()
        .unwrap_or_else(|| match connection.profile.db_type {
            SupportedDatabaseType::Postgresql => "postgres".to_string(),
            SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => "mysql".to_string(),
            SupportedDatabaseType::Sqlite => "main".to_string(),
        })
}

async fn live_database_nodes(
    connection: &StoredDatabaseConnection,
) -> Result<Vec<DatabaseExplorerNode>> {
    Ok(live_databases(connection)
        .await?
        .into_iter()
        .map(|database| DatabaseExplorerNode {
            id: format!("database:{}:{}:", connection.profile.id, database.name),
            object_type: DatabaseObjectType::Database,
            connection_id: connection.profile.id,
            name: database.name.clone(),
            database_name: Some(database.name),
            schema_name: None,
            has_children: true,
            description: database.is_default.then_some("default".to_string()),
        })
        .collect())
}

async fn live_schema_nodes(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
) -> Result<Vec<DatabaseExplorerNode>> {
    Ok(live_schemas(connection, database_name)
        .await?
        .into_iter()
        .map(|schema| DatabaseExplorerNode {
            id: format!(
                "schema:{}:{}:{}",
                connection.profile.id,
                database_name.unwrap_or_else(|| {
                    connection
                        .profile
                        .database_name
                        .as_deref()
                        .unwrap_or("postgres")
                }),
                schema.name
            ),
            object_type: DatabaseObjectType::Schema,
            connection_id: connection.profile.id,
            name: schema.name.clone(),
            database_name: Some(
                database_name
                    .map(str::to_string)
                    .unwrap_or_else(|| default_database_name(connection)),
            ),
            schema_name: Some(schema.name),
            has_children: true,
            description: schema.is_default.then_some("default".to_string()),
        })
        .collect())
}

async fn live_object_nodes(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
) -> Result<Vec<DatabaseExplorerNode>> {
    Ok(live_objects(connection, database_name, schema_name)
        .await?
        .into_iter()
        .map(|object| DatabaseExplorerNode {
            id: format!(
                "{}:{}:{}:{}:{}",
                database_object_type_as_str(object.object_type),
                connection.profile.id,
                object.database_name.as_deref().unwrap_or(""),
                object.schema_name.as_deref().unwrap_or(""),
                object.name
            ),
            object_type: object.object_type,
            connection_id: connection.profile.id,
            name: object.name,
            database_name: object.database_name,
            schema_name: object.schema_name,
            has_children: false,
            description: Some(database_object_type_as_str(object.object_type).to_string()),
        })
        .collect())
}

async fn live_databases(connection: &StoredDatabaseConnection) -> Result<Vec<DatabaseNameSummary>> {
    let current = default_database_name(connection);
    if connection.profile.db_type == SupportedDatabaseType::Postgresql
        && !connection.profile.show_all_databases
    {
        return Ok(vec![DatabaseNameSummary {
            name: current,
            is_default: true,
        }]);
    }
    let pool = live_pool(connection).await?;
    let sql = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
            "SELECT datname::text AS name FROM pg_database WHERE datistemplate = false ORDER BY datname"
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            "SELECT schema_name AS name FROM information_schema.schemata ORDER BY schema_name"
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let rows = sqlx::query(sql)
        .fetch_all(&pool)
        .await
        .map_err(sqlx_server_error)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| any_string(&row, 0))
        .map(|name| DatabaseNameSummary {
            is_default: name == current,
            name,
        })
        .collect())
}

async fn live_schemas(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
) -> Result<Vec<DatabaseNameSummary>> {
    if connection.profile.db_type != SupportedDatabaseType::Postgresql {
        return Ok(Vec::new());
    }
    let pool = live_pool_for_database(connection, database_name).await?;
    let rows = sqlx::query(
        r#"
        SELECT schema_name::text AS schema_name
        FROM information_schema.schemata
        WHERE schema_name NOT IN ('pg_catalog', 'information_schema')
          AND schema_name NOT LIKE 'pg_toast%'
        ORDER BY schema_name
        "#,
    )
    .fetch_all(&pool)
    .await
    .map_err(sqlx_server_error)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| any_string(&row, 0))
        .map(|name| DatabaseNameSummary {
            is_default: name == "public",
            name,
        })
        .collect())
}

async fn live_objects(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
) -> Result<Vec<DatabaseObjectSummary>> {
    let pool = live_pool_for_database(connection, database_name).await?;
    match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
            let database = database_name
                .or(connection.profile.database_name.as_deref())
                .map(str::to_string)
                .unwrap_or_else(|| default_database_name(connection));
            let schema = schema_name.unwrap_or("public");
            let rows = sqlx::query(
                r#"
                SELECT table_name::text, table_type::text
                FROM information_schema.tables
                WHERE table_schema = $1
                  AND table_type IN ('BASE TABLE', 'VIEW')
                ORDER BY table_type, table_name
                "#,
            )
            .bind(schema)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            Ok(rows
                .into_iter()
                .filter_map(|row| {
                    let name = any_string(&row, 0)?;
                    let object_type = if any_string(&row, 1).as_deref() == Some("VIEW") {
                        DatabaseObjectType::View
                    } else {
                        DatabaseObjectType::Table
                    };
                    Some(DatabaseObjectSummary {
                        name,
                        object_type,
                        database_name: Some(database.clone()),
                        schema_name: Some(schema.to_string()),
                    })
                })
                .collect())
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            let database = database_name
                .or(connection.profile.database_name.as_deref())
                .map(str::to_string)
                .unwrap_or_else(|| default_database_name(connection));
            let rows = sqlx::query(
                r#"
                SELECT table_name, table_type
                FROM information_schema.tables
                WHERE table_schema = ?
                  AND table_type IN ('BASE TABLE', 'VIEW')
                ORDER BY table_type, table_name
                "#,
            )
            .bind(&database)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
            Ok(rows
                .into_iter()
                .filter_map(|row| {
                    let name = any_string(&row, 0)?;
                    let object_type = if any_string(&row, 1).as_deref() == Some("VIEW") {
                        DatabaseObjectType::View
                    } else {
                        DatabaseObjectType::Table
                    };
                    Some(DatabaseObjectSummary {
                        name,
                        object_type,
                        database_name: Some(database.clone()),
                        schema_name: None,
                    })
                })
                .collect())
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    }
}

async fn describe_live_table(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    table_name: &str,
    object_type: DatabaseObjectType,
) -> Result<LiveTableDescription> {
    let pool = live_pool_for_database(connection, database_name).await?;
    let expected_table_types: &[&str] = match object_type {
        DatabaseObjectType::View => &["VIEW"],
        _ => &["BASE TABLE"],
    };

    let (database, schema) = live_scope(connection, database_name, schema_name);
    let columns_sql = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
            r#"
            SELECT
              column_name::text,
              data_type::text,
              udt_name::text,
              is_nullable::text,
              column_default::text
            FROM information_schema.columns
            WHERE table_schema = $1 AND table_name = $2
            ORDER BY ordinal_position
            "#
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            r#"
            SELECT column_name, data_type, column_type, is_nullable, column_default
            FROM information_schema.columns
            WHERE table_schema = ? AND table_name = ?
            ORDER BY ordinal_position
            "#
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let columns_rows = sqlx::query(columns_sql)
        .bind(&schema)
        .bind(table_name)
        .fetch_all(&pool)
        .await
        .map_err(sqlx_server_error)?;
    if columns_rows.is_empty() {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "Table not found"));
    }

    let primary_keys =
        live_primary_keys(&pool, connection.profile.db_type, &schema, table_name).await?;
    let foreign_keys =
        live_foreign_keys(&pool, connection.profile.db_type, &schema, table_name).await?;
    let columns = columns_rows
        .into_iter()
        .filter_map(|row| {
            let name = any_string(&row, 0)?;
            let data_type = any_string(&row, 1);
            let native_type = any_string(&row, 2).or_else(|| data_type.clone());
            let nullable = any_string(&row, 3).map(|value| value.eq_ignore_ascii_case("yes"));
            Some(DatabaseObjectColumn {
                is_primary_key: primary_keys.contains(&name),
                name,
                data_type,
                native_type,
                nullable,
                default_value: any_string(&row, 4),
                extra: None,
            })
        })
        .collect::<Vec<_>>();

    let table_type =
        live_table_type(&pool, connection.profile.db_type, &schema, table_name).await?;
    if !expected_table_types.contains(&table_type.as_str()) {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "Table not found"));
    }

    let _ = database;
    Ok(LiveTableDescription {
        columns,
        foreign_keys,
    })
}

async fn live_table_type(
    pool: &AnyPool,
    db_type: SupportedDatabaseType,
    schema: &str,
    table_name: &str,
) -> Result<String> {
    let sql = match db_type {
        SupportedDatabaseType::Postgresql => {
            r#"
            SELECT table_type::text
            FROM information_schema.tables
            WHERE table_schema = $1 AND table_name = $2
            "#
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            r#"
            SELECT table_type
            FROM information_schema.tables
            WHERE table_schema = ? AND table_name = ?
            "#
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let row = sqlx::query(sql)
        .bind(schema)
        .bind(table_name)
        .fetch_optional(pool)
        .await
        .map_err(sqlx_server_error)?
        .ok_or_else(|| ServerError::new(StatusCode::NOT_FOUND, "Table not found"))?;
    Ok(any_string(&row, 0).unwrap_or_else(|| "BASE TABLE".to_string()))
}

async fn live_primary_keys(
    pool: &AnyPool,
    db_type: SupportedDatabaseType,
    schema: &str,
    table_name: &str,
) -> Result<Vec<String>> {
    let sql = match db_type {
        SupportedDatabaseType::Postgresql => {
            r#"
            SELECT kcu.column_name::text
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
              ON tc.constraint_name = kcu.constraint_name
             AND tc.table_schema = kcu.table_schema
            WHERE tc.constraint_type = 'PRIMARY KEY'
              AND tc.table_schema = $1
              AND tc.table_name = $2
            ORDER BY kcu.ordinal_position
            "#
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            r#"
            SELECT kcu.column_name
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
              ON tc.constraint_name = kcu.constraint_name
             AND tc.table_schema = kcu.table_schema
             AND tc.table_name = kcu.table_name
            WHERE tc.constraint_type = 'PRIMARY KEY'
              AND tc.table_schema = ?
              AND tc.table_name = ?
            ORDER BY kcu.ordinal_position
            "#
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let rows = sqlx::query(sql)
        .bind(schema)
        .bind(table_name)
        .fetch_all(pool)
        .await
        .map_err(sqlx_server_error)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| any_string(&row, 0))
        .collect())
}

async fn live_foreign_keys(
    pool: &AnyPool,
    db_type: SupportedDatabaseType,
    schema: &str,
    table_name: &str,
) -> Result<Vec<DatabaseForeignKey>> {
    let sql = match db_type {
        SupportedDatabaseType::Postgresql => {
            r#"
            SELECT
              tc.constraint_name::text,
              kcu.column_name::text,
              ccu.table_schema::text AS referenced_table_schema,
              ccu.table_name::text AS referenced_table_name,
              ccu.column_name::text AS referenced_column_name
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
              ON tc.constraint_name = kcu.constraint_name
             AND tc.table_schema = kcu.table_schema
            JOIN information_schema.constraint_column_usage ccu
              ON ccu.constraint_name = tc.constraint_name
             AND ccu.table_schema = tc.table_schema
            WHERE tc.constraint_type = 'FOREIGN KEY'
              AND tc.table_schema = $1
              AND tc.table_name = $2
            ORDER BY tc.constraint_name, kcu.ordinal_position
            "#
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            r#"
            SELECT
              kcu.constraint_name,
              kcu.column_name,
              kcu.referenced_table_schema,
              kcu.referenced_table_name,
              kcu.referenced_column_name
            FROM information_schema.key_column_usage kcu
            WHERE kcu.table_schema = ?
              AND kcu.table_name = ?
              AND kcu.referenced_table_name IS NOT NULL
            ORDER BY kcu.constraint_name, kcu.ordinal_position
            "#
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let rows = sqlx::query(sql)
        .bind(schema)
        .bind(table_name)
        .fetch_all(pool)
        .await
        .map_err(sqlx_server_error)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            Some(DatabaseForeignKey {
                name: any_string(&row, 0),
                column_name: any_string(&row, 1)?,
                referenced_schema_name: any_string(&row, 2),
                referenced_table_name: any_string(&row, 3)?,
                referenced_column_name: any_string(&row, 4)?,
                on_update: None,
                on_delete: None,
            })
        })
        .collect())
}

async fn live_relational_schema(
    connection: &StoredDatabaseConnection,
    scope_type: DatabaseObjectType,
    scope_name: &str,
    schema_name: Option<&str>,
    objects: &[DatabaseObjectSummary],
) -> Result<DatabaseRelationalSchema> {
    let mut tables = Vec::new();
    let mut relationships = Vec::new();
    for object in objects.iter().filter(|object| {
        matches!(
            object.object_type,
            DatabaseObjectType::Table | DatabaseObjectType::View
        )
    }) {
        let table = describe_live_table(
            connection,
            object.database_name.as_deref(),
            object.schema_name.as_deref().or(schema_name),
            &object.name,
            object.object_type,
        )
        .await?;
        for foreign_key in &table.foreign_keys {
            relationships.push(DatabaseRelationalSchemaRelationship {
                id: format!(
                    "{}:{}:{}",
                    object.name, foreign_key.column_name, foreign_key.referenced_table_name
                ),
                name: foreign_key.name.clone(),
                source_database_name: object.database_name.clone(),
                source_schema_name: object.schema_name.clone(),
                source_table_name: object.name.clone(),
                source_column_name: foreign_key.column_name.clone(),
                target_database_name: object.database_name.clone(),
                target_schema_name: foreign_key.referenced_schema_name.clone(),
                target_table_name: foreign_key.referenced_table_name.clone(),
                target_column_name: foreign_key.referenced_column_name.clone(),
                on_update: foreign_key.on_update.clone(),
                on_delete: foreign_key.on_delete.clone(),
            });
        }
        tables.push(DatabaseRelationalSchemaTable {
            database_name: object.database_name.clone(),
            schema_name: object.schema_name.clone(),
            name: object.name.clone(),
            object_type: object.object_type,
            columns: table.columns,
            is_external: false,
        });
    }

    Ok(DatabaseRelationalSchema {
        scope_type,
        scope_name: scope_name.to_string(),
        tables,
        relationships,
    })
}

async fn execute_live_query(
    connection: &StoredDatabaseConnection,
    sql: &str,
    max_rows: usize,
    database_name: Option<&str>,
    schema_name: Option<&str>,
) -> Result<DatabaseQueryResult> {
    match typed_live_pool_for_database(connection, database_name).await? {
        TypedLivePool::Postgresql(pool) => {
            execute_postgres_query(&pool, sql, max_rows, schema_name).await
        }
        TypedLivePool::Mysql(pool) => execute_mysql_query(&pool, sql, max_rows).await,
    }
}

async fn execute_postgres_query(
    pool: &PgPool,
    sql: &str,
    max_rows: usize,
    schema_name: Option<&str>,
) -> Result<DatabaseQueryResult> {
    let start = Instant::now();
    let mut database = pool.acquire().await.map_err(sqlx_server_error)?;
    if let Some(schema_name) = schema_name.map(str::trim).filter(|value| !value.is_empty()) {
        let scope_sql = format!(
            "SET search_path TO {}, public",
            quote_identifier(schema_name)
        );
        sqlx::query(&scope_sql)
            .execute(&mut *database)
            .await
            .map_err(sqlx_server_error)?;
    }

    let statement_type = classify_statement(sql);
    let described_columns = (&mut *database)
        .describe(sql)
        .await
        .map(|description| postgres_columns(description.columns()))
        .unwrap_or_default();
    let mut rows = Vec::new();
    let mut affected_rows = 0u64;
    let mut stream = sqlx::raw_sql(sql).fetch_many(&mut *database);
    while let Some(step) = stream.next().await {
        match step.map_err(sqlx_server_error)? {
            Either::Left(result) => {
                affected_rows = affected_rows.saturating_add(result.rows_affected());
            }
            Either::Right(row) => rows.push(row),
        }
    }
    drop(stream);

    let columns = rows
        .first()
        .map(|row| postgres_columns(row.columns()))
        .filter(|columns| !columns.is_empty())
        .unwrap_or(described_columns);
    let row_count = if columns.is_empty() {
        usize::try_from(affected_rows).unwrap_or(usize::MAX)
    } else {
        rows.len()
    };
    let result_truncated = rows.len() > max_rows;
    let output = rows
        .into_iter()
        .take(max_rows)
        .map(|row| postgres_row_to_json_map(&row))
        .collect::<Vec<_>>();
    let returned_row_count = output.len();
    Ok(DatabaseQueryResult {
        sql: sql.to_string(),
        statement_type,
        row_count,
        returned_row_count,
        result_truncated,
        max_rows,
        rows: output,
        columns,
        notices: Vec::new(),
        duration_ms: start.elapsed().as_millis(),
        meta: Some(query_result_meta(
            returned_row_count,
            result_truncated,
            max_rows,
            None,
        )),
        database_name: None,
        schema_name: None,
    })
}

async fn execute_mysql_query(
    pool: &MySqlPool,
    sql: &str,
    max_rows: usize,
) -> Result<DatabaseQueryResult> {
    let start = Instant::now();
    let mut database = pool.acquire().await.map_err(sqlx_server_error)?;
    let statement_type = classify_statement(sql);
    let described_columns = (&mut *database)
        .describe(sql)
        .await
        .map(|description| mysql_columns(description.columns()))
        .unwrap_or_default();
    let mut rows = Vec::new();
    let mut affected_rows = 0u64;
    let mut last_insert_id = 0u64;
    let mut stream = sqlx::raw_sql(sql).fetch_many(&mut *database);
    while let Some(step) = stream.next().await {
        match step.map_err(sqlx_server_error)? {
            Either::Left(result) => {
                affected_rows = affected_rows.saturating_add(result.rows_affected());
                last_insert_id = result.last_insert_id();
            }
            Either::Right(row) => rows.push(row),
        }
    }
    drop(stream);

    let columns = rows
        .first()
        .map(|row| mysql_columns(row.columns()))
        .filter(|columns| !columns.is_empty())
        .unwrap_or(described_columns);
    let has_row_set = !columns.is_empty();
    let row_count = if has_row_set {
        rows.len()
    } else {
        usize::try_from(affected_rows).unwrap_or(usize::MAX)
    };
    let result_truncated = rows.len() > max_rows;
    let output = rows
        .into_iter()
        .take(max_rows)
        .map(|row| mysql_row_to_json_map(&row))
        .collect::<Vec<_>>();
    let returned_row_count = output.len();
    let warning_status = if has_row_set {
        None
    } else {
        let mut status = None;
        let mut warning_stream = sqlx::raw_sql("SHOW COUNT(*) WARNINGS").fetch_many(&mut *database);
        while let Some(step) = warning_stream.next().await {
            match step {
                Ok(Either::Left(_)) => {}
                Ok(Either::Right(row)) => {
                    let value = mysql_value_to_json(&row, 0);
                    status = value
                        .as_u64()
                        .or_else(|| value.as_str()?.parse::<u64>().ok());
                    break;
                }
                Err(_) => break,
            }
        }
        drop(warning_stream);
        status
    };
    let extra_meta = (!has_row_set).then(|| {
        serde_json::json!({
            "affectedRows": affected_rows,
            "insertId": last_insert_id,
            "warningStatus": warning_status,
        })
    });
    Ok(DatabaseQueryResult {
        sql: sql.to_string(),
        statement_type,
        row_count,
        returned_row_count,
        result_truncated,
        max_rows,
        rows: output,
        columns,
        notices: Vec::new(),
        duration_ms: start.elapsed().as_millis(),
        meta: Some(query_result_meta(
            returned_row_count,
            result_truncated,
            max_rows,
            extra_meta,
        )),
        database_name: None,
        schema_name: None,
    })
}

async fn read_live_table_data(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
    table_name: &str,
    limit: usize,
    offset: usize,
    include_total_count: bool,
) -> Result<DatabaseTableData> {
    let start = Instant::now();
    let table = match describe_live_table(
        connection,
        database_name,
        schema_name,
        table_name,
        DatabaseObjectType::Table,
    )
    .await
    {
        Ok(table) => table,
        Err(_) => {
            describe_live_table(
                connection,
                database_name,
                schema_name,
                table_name,
                DatabaseObjectType::View,
            )
            .await?
        }
    };
    let (_database, schema) = live_scope(connection, database_name, schema_name);
    let table_ref = live_table_ref(connection.profile.db_type, &schema, table_name);
    let fetch_limit = limit.saturating_add(1);
    let rows = fetch_live_row_maps(
        connection,
        database_name,
        &format!("SELECT * FROM {table_ref} LIMIT {fetch_limit} OFFSET {offset}"),
    )
    .await?;
    let total_row_count = if include_total_count {
        fetch_live_count(
            connection,
            database_name,
            &format!("SELECT COUNT(*) AS __iowb_count FROM {table_ref}"),
        )
        .await?
    } else {
        None
    };
    let mut output = rows;
    let has_more = output.len() > limit;
    if has_more {
        output.truncate(limit);
    }
    Ok(DatabaseTableData {
        database_name: database_name
            .map(str::to_string)
            .or_else(|| Some(default_database_name(connection))),
        schema_name: schema_name.map(str::to_string),
        table_name: table_name.to_string(),
        offset,
        limit,
        row_count: output.len(),
        total_row_count,
        exact_total_row_count: include_total_count,
        has_more,
        columns: table.columns,
        rows: output,
        duration_ms: start.elapsed().as_millis(),
    })
}

fn live_scope(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    schema_name: Option<&str>,
) -> (String, String) {
    let database = database_name
        .or(connection.profile.database_name.as_deref())
        .map(str::to_string)
        .unwrap_or_else(|| default_database_name(connection));
    let schema = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => schema_name.unwrap_or("public").to_string(),
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => database.clone(),
        SupportedDatabaseType::Sqlite => "main".to_string(),
    };
    (database, schema)
}

fn live_table_ref(db_type: SupportedDatabaseType, schema: &str, table_name: &str) -> String {
    match db_type {
        SupportedDatabaseType::Postgresql => {
            format!(
                "{}.{}",
                quote_identifier(schema),
                quote_identifier(table_name)
            )
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            format!(
                "{}.{}",
                quote_mysql_identifier(schema),
                quote_mysql_identifier(table_name)
            )
        }
        SupportedDatabaseType::Sqlite => quote_identifier(table_name),
    }
}

fn quote_mysql_identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

async fn fetch_live_row_maps(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    sql: &str,
) -> Result<Vec<Map<String, Value>>> {
    match typed_live_pool_for_database(connection, database_name).await? {
        TypedLivePool::Postgresql(pool) => sqlx::raw_sql(sql)
            .fetch_all(&pool)
            .await
            .map(|rows| {
                rows.into_iter()
                    .map(|row| postgres_row_to_json_map(&row))
                    .collect()
            })
            .map_err(sqlx_server_error),
        TypedLivePool::Mysql(pool) => sqlx::raw_sql(sql)
            .fetch_all(&pool)
            .await
            .map(|rows| {
                rows.into_iter()
                    .map(|row| mysql_row_to_json_map(&row))
                    .collect()
            })
            .map_err(sqlx_server_error),
    }
}

async fn fetch_live_count(
    connection: &StoredDatabaseConnection,
    database_name: Option<&str>,
    sql: &str,
) -> Result<Option<usize>> {
    let value = fetch_live_row_maps(connection, database_name, sql)
        .await?
        .into_iter()
        .next()
        .and_then(|row| row.get("__iowb_count").cloned());
    Ok(value.as_ref().and_then(json_value_to_usize))
}

fn json_value_to_usize(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .or_else(|| {
            value
                .as_i64()
                .filter(|value| *value >= 0)
                .and_then(|value| usize::try_from(value).ok())
        })
        .or_else(|| value.as_str()?.parse::<usize>().ok())
}

fn postgres_columns(columns: &[sqlx::postgres::PgColumn]) -> Vec<DatabaseObjectColumn> {
    columns
        .iter()
        .map(|column| DatabaseObjectColumn {
            name: column.name().to_string(),
            data_type: None,
            native_type: column
                .type_info()
                .oid()
                .map(|oid| oid.0.to_string())
                .or_else(|| Some(column.type_info().name().to_string())),
            nullable: None,
            default_value: None,
            extra: None,
            is_primary_key: false,
        })
        .collect()
}

fn mysql_columns(columns: &[sqlx::mysql::MySqlColumn]) -> Vec<DatabaseObjectColumn> {
    columns
        .iter()
        .map(|column| DatabaseObjectColumn {
            name: column.name().to_string(),
            data_type: None,
            native_type: Some(column.type_info().name().to_string()),
            nullable: None,
            default_value: None,
            extra: None,
            is_primary_key: false,
        })
        .collect()
}

fn postgres_row_to_json_map(row: &PgRow) -> Map<String, Value> {
    let mut item = Map::new();
    for (index, column) in row.columns().iter().enumerate() {
        item.insert(
            column.name().to_string(),
            postgres_value_to_json(row, index),
        );
    }
    item
}

fn postgres_value_to_json(row: &PgRow, index: usize) -> Value {
    let type_info = row.columns()[index].type_info();
    if let PgTypeKind::Array(element_type) = type_info.kind() {
        return postgres_raw_string(row, index)
            .map(|value| {
                if postgres_array_element_is_normalized(element_type.name()) {
                    parse_postgres_array(&value, element_type.name())
                } else {
                    Value::String(value)
                }
            })
            .unwrap_or(Value::Null);
    }

    let type_name = match type_info.kind() {
        PgTypeKind::Domain(base_type) => base_type.name(),
        _ => type_info.name(),
    }
    .to_ascii_uppercase();

    match type_name.as_str() {
        "BOOL" => row
            .try_get_unchecked::<Option<bool>, _>(index)
            .ok()
            .flatten()
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        "INT2" | "INT4" | "OID" | "XID" | "CID" => row
            .try_get_unchecked::<Option<i64>, _>(index)
            .ok()
            .flatten()
            .map(|value| Value::Number(value.into()))
            .unwrap_or_else(|| postgres_string_fallback(row, index)),
        "INT8" | "NUMERIC" | "DECIMAL" | "MONEY" => postgres_string_fallback(row, index),
        "FLOAT4" | "FLOAT8" => row
            .try_get_unchecked::<Option<f64>, _>(index)
            .ok()
            .flatten()
            .map(json_float_value)
            .unwrap_or(Value::Null),
        "BYTEA" => row
            .try_get_unchecked::<Option<Vec<u8>>, _>(index)
            .ok()
            .flatten()
            .map(|value| database_buffer_value(&value))
            .unwrap_or(Value::Null),
        "JSON" | "JSONB" => row
            .try_get_unchecked::<Option<Value>, _>(index)
            .ok()
            .flatten()
            .unwrap_or(Value::Null),
        "DATE" => row
            .try_get_unchecked::<Option<NaiveDate>, _>(index)
            .ok()
            .flatten()
            .map(|value| local_naive_to_iso(value.and_hms_opt(0, 0, 0).expect("midnight")))
            .map(Value::String)
            .unwrap_or(Value::Null),
        "TIMESTAMP" => row
            .try_get_unchecked::<Option<NaiveDateTime>, _>(index)
            .ok()
            .flatten()
            .map(local_naive_to_iso)
            .map(Value::String)
            .unwrap_or(Value::Null),
        "TIMESTAMPTZ" => row
            .try_get_unchecked::<Option<DateTime<Utc>>, _>(index)
            .ok()
            .flatten()
            .map(utc_datetime_to_iso)
            .map(Value::String)
            .unwrap_or(Value::Null),
        "POINT" => postgres_raw_string(row, index)
            .and_then(|value| parse_postgres_point(&value))
            .unwrap_or(Value::Null),
        "CIRCLE" => postgres_raw_string(row, index)
            .and_then(|value| parse_postgres_circle(&value))
            .unwrap_or(Value::Null),
        "INTERVAL" => postgres_raw_string(row, index)
            .map(|value| parse_postgres_interval(&value))
            .unwrap_or(Value::Null),
        _ => postgres_string_fallback(row, index),
    }
}

fn postgres_raw_string(row: &PgRow, index: usize) -> Option<String> {
    row.try_get_unchecked::<Option<String>, _>(index)
        .ok()
        .flatten()
}

fn postgres_string_fallback(row: &PgRow, index: usize) -> Value {
    postgres_raw_string(row, index)
        .map(Value::String)
        .or_else(|| {
            row.try_get_unchecked::<Option<Vec<u8>>, _>(index)
                .ok()
                .flatten()
                .map(|value| database_buffer_value(&value))
        })
        .unwrap_or(Value::Null)
}

fn postgres_array_element_is_normalized(type_name: &str) -> bool {
    matches!(
        type_name.to_ascii_uppercase().as_str(),
        "BOOL"
            | "BYTEA"
            | "INT2"
            | "INT4"
            | "OID"
            | "INT8"
            | "POINT"
            | "FLOAT4"
            | "FLOAT8"
            | "CHAR"
            | "VARCHAR"
            | "TEXT"
            | "BPCHAR"
            | "NAME"
            | "CIDR"
            | "MACADDR"
            | "INET"
            | "TIMESTAMP"
            | "DATE"
            | "TIMESTAMPTZ"
            | "INTERVAL"
            | "NUMERIC"
            | "JSON"
            | "JSONB"
            | "UUID"
            | "MONEY"
            | "TIME"
            | "TIMETZ"
    )
}

fn mysql_row_to_json_map(row: &MySqlRow) -> Map<String, Value> {
    let mut item = Map::new();
    for (index, column) in row.columns().iter().enumerate() {
        item.insert(column.name().to_string(), mysql_value_to_json(row, index));
    }
    item
}

fn mysql_value_to_json(row: &MySqlRow, index: usize) -> Value {
    let type_name = row.columns()[index].type_info().name().to_ascii_uppercase();
    match type_name.as_str() {
        "BOOLEAN" | "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "YEAR" => {
            mysql_integer_value(row, index, type_name.contains("UNSIGNED"))
        }
        "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "MEDIUMINT UNSIGNED" | "INT UNSIGNED"
        | "BIGINT UNSIGNED" => mysql_integer_value(row, index, true),
        "BIGINT" => mysql_integer_value(row, index, false),
        "FLOAT" | "DOUBLE" => row
            .try_get_unchecked::<Option<f64>, _>(index)
            .ok()
            .flatten()
            .map(json_float_value)
            .unwrap_or(Value::Null),
        "DECIMAL" => mysql_string_fallback(row, index),
        "JSON" => row
            .try_get_unchecked::<Option<Value>, _>(index)
            .ok()
            .flatten()
            .unwrap_or(Value::Null),
        "DATE" => row
            .try_get_unchecked::<Option<NaiveDate>, _>(index)
            .ok()
            .flatten()
            .map(|value| local_naive_to_iso(value.and_hms_opt(0, 0, 0).expect("midnight")))
            .map(Value::String)
            .unwrap_or(Value::Null),
        "DATETIME" | "TIMESTAMP" => row
            .try_get_unchecked::<Option<NaiveDateTime>, _>(index)
            .ok()
            .flatten()
            .map(local_naive_to_iso)
            .map(Value::String)
            .unwrap_or(Value::Null),
        "GEOMETRY" => row
            .try_get_unchecked::<Option<Vec<u8>>, _>(index)
            .ok()
            .flatten()
            .and_then(|value| parse_mysql_geometry(&value))
            .unwrap_or(Value::Null),
        "VECTOR" => row
            .try_get_unchecked::<Option<Vec<u8>>, _>(index)
            .ok()
            .flatten()
            .map(|value| parse_mysql_vector(&value))
            .unwrap_or(Value::Null),
        "BIT" | "BINARY" | "VARBINARY" | "TINYBLOB" | "BLOB" | "MEDIUMBLOB" | "LONGBLOB" => row
            .try_get_unchecked::<Option<Vec<u8>>, _>(index)
            .ok()
            .flatten()
            .map(|value| database_buffer_value(&value))
            .unwrap_or(Value::Null),
        _ => mysql_string_fallback(row, index),
    }
}

fn mysql_integer_value(row: &MySqlRow, index: usize, unsigned: bool) -> Value {
    if unsigned {
        return row
            .try_get_unchecked::<Option<u64>, _>(index)
            .ok()
            .flatten()
            .map(|value| Value::Number(value.into()))
            .unwrap_or_else(|| mysql_string_fallback(row, index));
    }
    row.try_get_unchecked::<Option<i64>, _>(index)
        .ok()
        .flatten()
        .map(|value| Value::Number(value.into()))
        .unwrap_or_else(|| mysql_string_fallback(row, index))
}

fn mysql_string_fallback(row: &MySqlRow, index: usize) -> Value {
    row.try_get_unchecked::<Option<String>, _>(index)
        .ok()
        .flatten()
        .map(Value::String)
        .or_else(|| {
            row.try_get_unchecked::<Option<Vec<u8>>, _>(index)
                .ok()
                .flatten()
                .map(|value| database_buffer_value(&value))
        })
        .unwrap_or(Value::Null)
}

fn json_float_value(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn local_naive_to_iso(value: NaiveDateTime) -> String {
    Local
        .from_local_datetime(&value)
        .earliest()
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|| value.and_utc())
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn utc_datetime_to_iso(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[derive(Debug, Clone, PartialEq)]
enum PostgresArrayValue {
    Null,
    Text(String),
    Array(Vec<PostgresArrayValue>),
}

struct PostgresArrayParser<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> PostgresArrayParser<'a> {
    fn parse(input: &'a str) -> Option<PostgresArrayValue> {
        let normalized = if input.starts_with('[') {
            input.split_once('=').map(|(_, value)| value)?
        } else {
            input
        };
        let mut parser = Self {
            input: normalized.as_bytes(),
            offset: 0,
        };
        let value = parser.parse_array()?;
        (parser.offset == parser.input.len()).then_some(value)
    }

    fn parse_array(&mut self) -> Option<PostgresArrayValue> {
        self.consume(b'{')?;
        let mut values = Vec::new();
        if self.peek() == Some(b'}') {
            self.offset += 1;
            return Some(PostgresArrayValue::Array(values));
        }
        loop {
            let value = if self.peek() == Some(b'{') {
                self.parse_array()?
            } else if self.peek() == Some(b'"') {
                PostgresArrayValue::Text(self.parse_quoted()?)
            } else {
                self.parse_unquoted()?
            };
            values.push(value);
            match self.peek()? {
                b',' | b';' => self.offset += 1,
                b'}' => {
                    self.offset += 1;
                    break;
                }
                _ => return None,
            }
        }
        Some(PostgresArrayValue::Array(values))
    }

    fn parse_quoted(&mut self) -> Option<String> {
        self.consume(b'"')?;
        let mut value = Vec::new();
        while let Some(byte) = self.peek() {
            self.offset += 1;
            match byte {
                b'"' => return String::from_utf8(value).ok(),
                b'\\' => {
                    value.push(self.peek()?);
                    self.offset += 1;
                }
                _ => value.push(byte),
            }
        }
        None
    }

    fn parse_unquoted(&mut self) -> Option<PostgresArrayValue> {
        let mut value = Vec::new();
        while let Some(byte) = self.peek() {
            if matches!(byte, b',' | b';' | b'}') {
                break;
            }
            self.offset += 1;
            if byte == b'\\' {
                value.push(self.peek()?);
                self.offset += 1;
            } else {
                value.push(byte);
            }
        }
        let value = String::from_utf8(value).ok()?;
        if value == "NULL" {
            Some(PostgresArrayValue::Null)
        } else {
            Some(PostgresArrayValue::Text(value))
        }
    }

    fn consume(&mut self, byte: u8) -> Option<()> {
        (self.peek()? == byte).then(|| self.offset += 1)
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.offset).copied()
    }
}

fn parse_postgres_array(value: &str, element_type: &str) -> Value {
    PostgresArrayParser::parse(value)
        .map(|value| postgres_array_value_to_json(value, element_type))
        .unwrap_or_else(|| Value::String(value.to_string()))
}

fn postgres_array_value_to_json(value: PostgresArrayValue, element_type: &str) -> Value {
    match value {
        PostgresArrayValue::Null => Value::Null,
        PostgresArrayValue::Text(value) => postgres_array_scalar_to_json(&value, element_type),
        PostgresArrayValue::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| postgres_array_value_to_json(value, element_type))
                .collect(),
        ),
    }
}

fn postgres_array_scalar_to_json(value: &str, element_type: &str) -> Value {
    match element_type.to_ascii_uppercase().as_str() {
        "BOOL" => Value::Bool(matches!(
            value,
            "TRUE" | "t" | "true" | "y" | "yes" | "on" | "1"
        )),
        "INT2" | "INT4" | "OID" => value
            .parse::<i64>()
            .map(|value| Value::Number(value.into()))
            .unwrap_or_else(|_| Value::String(value.to_string())),
        "INT8" => Value::String(value.to_string()),
        "FLOAT4" | "FLOAT8" | "NUMERIC" => value
            .parse::<f64>()
            .map(json_float_value)
            .unwrap_or(Value::Null),
        "BYTEA" => decode_postgres_bytea(value)
            .map(|value| database_buffer_value(&value))
            .unwrap_or(Value::Null),
        "JSON" | "JSONB" => {
            serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
        }
        "DATE" => NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()
            .and_then(|value| value.and_hms_opt(0, 0, 0))
            .map(local_naive_to_iso)
            .map(Value::String)
            .unwrap_or(Value::Null),
        "TIMESTAMP" => NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
            .map(local_naive_to_iso)
            .map(Value::String)
            .unwrap_or(Value::Null),
        "TIMESTAMPTZ" => DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%#z")
            .map(|value| utc_datetime_to_iso(value.with_timezone(&Utc)))
            .map(Value::String)
            .unwrap_or(Value::Null),
        "INTERVAL" => parse_postgres_interval(value),
        "POINT" => parse_postgres_point(value).unwrap_or(Value::Null),
        _ => Value::String(value.to_string()),
    }
}

fn decode_postgres_bytea(value: &str) -> Option<Vec<u8>> {
    let encoded = value.strip_prefix("\\x")?;
    if encoded.len() % 2 != 0 {
        return None;
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let high = (chunk[0] as char).to_digit(16)?;
            let low = (chunk[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

fn parse_postgres_point(value: &str) -> Option<Value> {
    let (x, y) = value
        .strip_prefix('(')?
        .strip_suffix(')')?
        .split_once(',')?;
    Some(serde_json::json!({
        "x": x.parse::<f64>().ok().map(json_float_value).unwrap_or(Value::Null),
        "y": y.parse::<f64>().ok().map(json_float_value).unwrap_or(Value::Null),
    }))
}

fn parse_postgres_circle(value: &str) -> Option<Value> {
    let value = value.strip_prefix("<(")?.strip_suffix('>')?;
    let (point, radius) = value
        .rsplit_once(") ,")
        .or_else(|| value.rsplit_once("),"))?;
    let point = parse_postgres_point(&format!("({point})"))?;
    let mut object = point.as_object()?.clone();
    object.insert(
        "radius".to_string(),
        radius
            .trim()
            .parse::<f64>()
            .ok()
            .map(json_float_value)
            .unwrap_or(Value::Null),
    );
    Some(Value::Object(object))
}

fn parse_postgres_interval(value: &str) -> Value {
    let mut object = Map::new();
    let mut tokens = value.split_whitespace().peekable();
    let mut invert = false;
    while let Some(token) = tokens.next() {
        if token == "@" {
            continue;
        }
        if token.eq_ignore_ascii_case("ago") {
            invert = true;
            continue;
        }
        if token.contains(':') {
            let negative = token.starts_with('-');
            let time = token.trim_start_matches(['-', '+']);
            let mut parts = time.split(':');
            let hours = parts
                .next()
                .and_then(|part| part.parse::<i64>().ok())
                .unwrap_or(0);
            let minutes = parts
                .next()
                .and_then(|part| part.parse::<i64>().ok())
                .unwrap_or(0);
            let seconds = parts
                .next()
                .and_then(|part| part.parse::<f64>().ok())
                .unwrap_or(0.0);
            let sign = if negative { -1.0 } else { 1.0 };
            insert_nonzero_number(&mut object, "hours", sign * hours as f64);
            insert_nonzero_number(&mut object, "minutes", sign * minutes as f64);
            let whole_seconds = seconds.trunc() * sign;
            let milliseconds = (seconds.fract() * 1_000_000.0).round() / 1000.0 * sign;
            insert_nonzero_number(&mut object, "seconds", whole_seconds);
            insert_nonzero_number(&mut object, "milliseconds", milliseconds);
            continue;
        }
        let Some(unit) = tokens.next() else {
            continue;
        };
        let number = token.parse::<f64>().unwrap_or(0.0);
        let key = match unit.trim_end_matches('s') {
            "year" => "years",
            "mon" => "months",
            "day" => "days",
            "hour" => "hours",
            "min" | "minute" => "minutes",
            "sec" | "second" => "seconds",
            _ => continue,
        };
        insert_nonzero_number(&mut object, key, number);
    }
    if invert {
        for value in object.values_mut() {
            if let Some(number) = value.as_f64() {
                *value = json_float_value(-number);
            }
        }
    }
    Value::Object(object)
}

fn insert_nonzero_number(object: &mut Map<String, Value>, key: &str, value: f64) {
    if value == 0.0 {
        return;
    }
    let rounded = value.round();
    let value = if (value - rounded).abs() < f64::EPSILON {
        Value::Number((rounded as i64).into())
    } else {
        json_float_value(value)
    };
    object.insert(key.to_string(), value);
}

struct MysqlGeometryParser<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> MysqlGeometryParser<'a> {
    fn parse(input: &'a [u8]) -> Option<Value> {
        if input.len() < 5 {
            return None;
        }
        let mut parser = Self { input, offset: 4 };
        parser.parse_geometry()
    }

    fn parse_geometry(&mut self) -> Option<Value> {
        let little_endian = self.read_u8()? != 0;
        let geometry_type = self.read_u32(little_endian)?;
        match geometry_type {
            1 => {
                let x = self.read_f64(little_endian)?;
                let y = self.read_f64(little_endian)?;
                Some(serde_json::json!({ "x": json_float_value(x), "y": json_float_value(y) }))
            }
            2 => {
                let count = usize::try_from(self.read_u32(little_endian)?).ok()?;
                let mut points = Vec::with_capacity(count);
                for _ in 0..count {
                    let x = self.read_f64(little_endian)?;
                    let y = self.read_f64(little_endian)?;
                    points.push(
                        serde_json::json!({ "x": json_float_value(x), "y": json_float_value(y) }),
                    );
                }
                Some(Value::Array(points))
            }
            3 => {
                let ring_count = usize::try_from(self.read_u32(little_endian)?).ok()?;
                let mut rings = Vec::with_capacity(ring_count);
                for _ in 0..ring_count {
                    let point_count = usize::try_from(self.read_u32(little_endian)?).ok()?;
                    let mut points = Vec::with_capacity(point_count);
                    for _ in 0..point_count {
                        let x = self.read_f64(little_endian)?;
                        let y = self.read_f64(little_endian)?;
                        points.push(serde_json::json!({ "x": json_float_value(x), "y": json_float_value(y) }));
                    }
                    rings.push(Value::Array(points));
                }
                Some(Value::Array(rings))
            }
            4..=7 => {
                let count = usize::try_from(self.read_u32(little_endian)?).ok()?;
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(self.parse_geometry().unwrap_or(Value::Null));
                }
                Some(Value::Array(values))
            }
            _ => None,
        }
    }

    fn read_u8(&mut self) -> Option<u8> {
        let value = *self.input.get(self.offset)?;
        self.offset += 1;
        Some(value)
    }

    fn read_u32(&mut self, little_endian: bool) -> Option<u32> {
        let bytes: [u8; 4] = self
            .input
            .get(self.offset..self.offset + 4)?
            .try_into()
            .ok()?;
        self.offset += 4;
        Some(if little_endian {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    fn read_f64(&mut self, little_endian: bool) -> Option<f64> {
        let bytes: [u8; 8] = self
            .input
            .get(self.offset..self.offset + 8)?
            .try_into()
            .ok()?;
        self.offset += 8;
        Some(if little_endian {
            f64::from_le_bytes(bytes)
        } else {
            f64::from_be_bytes(bytes)
        })
    }
}

fn parse_mysql_geometry(value: &[u8]) -> Option<Value> {
    MysqlGeometryParser::parse(value)
}

fn parse_mysql_vector(value: &[u8]) -> Value {
    Value::Array(
        value
            .chunks_exact(4)
            .map(|chunk| {
                let bytes: [u8; 4] = chunk.try_into().expect("four-byte vector chunk");
                json_float_value(f64::from(f32::from_le_bytes(bytes)))
            })
            .collect(),
    )
}

fn any_string(row: &AnyRow, index: usize) -> Option<String> {
    row.try_get::<Option<String>, _>(index).ok().flatten()
}

fn sqlite_connection(connection: &StoredDatabaseConnection) -> Result<Connection> {
    let path = ensure_sqlite_connection(connection)?;
    if !path.exists() {
        return Err(ServerError::database(
            StatusCode::BAD_REQUEST,
            format!("SQLite database file does not exist: {}", path.display()),
            None,
            "SQLITE_CANTOPEN",
            "connection",
            false,
        ));
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(sqlite_server_error)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(sqlite_server_error)?;
    Ok(conn)
}

fn sqlite_transfer_target_connection(connection: &StoredDatabaseConnection) -> Result<Connection> {
    let path = ensure_sqlite_connection(connection)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_server_error)?;
    }
    let conn = Connection::open(path).map_err(sqlite_server_error)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(sqlite_server_error)?;
    Ok(conn)
}

fn sqlite_objects(connection: &StoredDatabaseConnection) -> Result<Vec<DatabaseObjectSummary>> {
    let conn = sqlite_connection(connection)?;
    list_sqlite_objects(&conn)
}

fn list_sqlite_objects(conn: &Connection) -> Result<Vec<DatabaseObjectSummary>> {
    let mut stmt = conn
        .prepare(
            r#"
            SELECT name, type
            FROM sqlite_master
            WHERE type IN ('table', 'view')
              AND name NOT LIKE 'sqlite_%'
            ORDER BY type ASC, name ASC
            "#,
        )
        .map_err(sqlite_server_error)?;
    let rows = stmt
        .query_map([], |row| {
            let object_type = match row.get::<_, String>(1)?.as_str() {
                "view" => DatabaseObjectType::View,
                _ => DatabaseObjectType::Table,
            };
            Ok(DatabaseObjectSummary {
                name: row.get(0)?,
                object_type,
                database_name: Some("main".to_string()),
                schema_name: None,
            })
        })
        .map_err(sqlite_server_error)?;

    let mut objects = Vec::new();
    for row in rows {
        objects.push(row.map_err(sqlite_server_error)?);
    }
    Ok(objects)
}

struct SqliteTableDescription {
    columns: Vec<DatabaseObjectColumn>,
    foreign_keys: Vec<DatabaseForeignKey>,
}

fn describe_sqlite_table(
    connection: &StoredDatabaseConnection,
    table_name: &str,
    object_type: DatabaseObjectType,
) -> Result<SqliteTableDescription> {
    let conn = sqlite_connection(connection)?;
    describe_sqlite_table_inner(&conn, table_name, object_type)
}

fn describe_sqlite_table_inner(
    conn: &Connection,
    table_name: &str,
    object_type: DatabaseObjectType,
) -> Result<SqliteTableDescription> {
    let expected_sql_type = match object_type {
        DatabaseObjectType::View => "view",
        _ => "table",
    };
    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE name = ?1 AND type = ?2",
            params![table_name, expected_sql_type],
            |_| Ok(()),
        )
        .optional()
        .map_err(sqlite_server_error)?
        .is_some();
    if !exists {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "Table not found"));
    }

    let mut columns_stmt = conn
        .prepare(&format!(
            "PRAGMA table_info({})",
            quote_identifier(table_name)
        ))
        .map_err(sqlite_server_error)?;
    let column_rows = columns_stmt
        .query_map([], |row| {
            let native_type: String = row.get(2)?;
            let not_null = row.get::<_, i64>(3)? == 1;
            let default_value: Option<String> = row.get(4)?;
            let primary_key_position = row.get::<_, i64>(5)?;
            Ok(DatabaseObjectColumn {
                name: row.get(1)?,
                data_type: Some(native_type.to_lowercase()),
                native_type: Some(native_type),
                nullable: Some(!not_null),
                default_value,
                extra: None,
                is_primary_key: primary_key_position > 0,
            })
        })
        .map_err(sqlite_server_error)?;

    let mut columns = Vec::new();
    for row in column_rows {
        columns.push(row.map_err(sqlite_server_error)?);
    }

    let mut fk_stmt = conn
        .prepare(&format!(
            "PRAGMA foreign_key_list({})",
            quote_identifier(table_name)
        ))
        .map_err(sqlite_server_error)?;
    let fk_rows = fk_stmt
        .query_map([], |row| {
            Ok(DatabaseForeignKey {
                name: None,
                column_name: row.get(3)?,
                referenced_schema_name: None,
                referenced_table_name: row.get(2)?,
                referenced_column_name: row.get(4)?,
                on_update: row.get(5)?,
                on_delete: row.get(6)?,
            })
        })
        .map_err(sqlite_server_error)?;

    let mut foreign_keys = Vec::new();
    for row in fk_rows {
        foreign_keys.push(row.map_err(sqlite_server_error)?);
    }

    Ok(SqliteTableDescription {
        columns,
        foreign_keys,
    })
}

fn sqlite_relational_schema(
    connection: &StoredDatabaseConnection,
    objects: &[DatabaseObjectSummary],
) -> Result<DatabaseRelationalSchema> {
    let conn = sqlite_connection(connection)?;
    let mut tables = Vec::new();
    let mut relationships = Vec::new();

    for object in objects.iter().filter(|object| {
        matches!(
            object.object_type,
            DatabaseObjectType::Table | DatabaseObjectType::View
        )
    }) {
        let table = describe_sqlite_table_inner(&conn, &object.name, object.object_type)?;
        for foreign_key in &table.foreign_keys {
            relationships.push(DatabaseRelationalSchemaRelationship {
                id: format!(
                    "{}:{}:{}",
                    object.name, foreign_key.column_name, foreign_key.referenced_table_name
                ),
                name: foreign_key.name.clone(),
                source_database_name: Some("main".to_string()),
                source_schema_name: None,
                source_table_name: object.name.clone(),
                source_column_name: foreign_key.column_name.clone(),
                target_database_name: Some("main".to_string()),
                target_schema_name: None,
                target_table_name: foreign_key.referenced_table_name.clone(),
                target_column_name: foreign_key.referenced_column_name.clone(),
                on_update: foreign_key.on_update.clone(),
                on_delete: foreign_key.on_delete.clone(),
            });
        }
        tables.push(DatabaseRelationalSchemaTable {
            database_name: Some("main".to_string()),
            schema_name: None,
            name: object.name.clone(),
            object_type: object.object_type,
            columns: table.columns,
            is_external: false,
        });
    }

    Ok(DatabaseRelationalSchema {
        scope_type: DatabaseObjectType::Database,
        scope_name: "main".to_string(),
        tables,
        relationships,
    })
}

fn execute_sqlite_query(
    connection: &StoredDatabaseConnection,
    sql: &str,
    max_rows: usize,
) -> Result<DatabaseQueryResult> {
    let start = Instant::now();
    let conn = sqlite_connection(connection)?;
    let statement_type = classify_statement(sql);
    let mut stmt = conn.prepare(sql).map_err(sqlite_server_error)?;
    let column_count = stmt.column_count();

    if column_count == 0 {
        let changed = stmt.execute([]).map_err(sqlite_server_error)?;
        let last_insert_row_id = conn.last_insert_rowid().to_string();
        return Ok(DatabaseQueryResult {
            sql: sql.to_string(),
            statement_type,
            row_count: changed,
            returned_row_count: 0,
            result_truncated: false,
            max_rows,
            rows: Vec::new(),
            columns: Vec::new(),
            notices: Vec::new(),
            duration_ms: start.elapsed().as_millis(),
            meta: Some(query_result_meta(
                0,
                false,
                max_rows,
                Some(serde_json::json!({
                    "changes": changed,
                    "lastInsertRowid": last_insert_row_id,
                })),
            )),
            database_name: None,
            schema_name: None,
        });
    }

    let column_names = stmt
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let columns = column_names
        .iter()
        .map(|name| DatabaseObjectColumn {
            name: name.clone(),
            data_type: None,
            native_type: None,
            nullable: None,
            default_value: None,
            extra: None,
            is_primary_key: false,
        })
        .collect::<Vec<_>>();
    let mut rows = stmt.query([]).map_err(sqlite_server_error)?;
    let mut output = Vec::new();
    let mut row_count = 0usize;
    while let Some(row) = rows.next().map_err(sqlite_server_error)? {
        row_count += 1;
        if output.len() < max_rows {
            output.push(row_to_json_map(row, &column_names)?);
        }
    }
    let returned_row_count = output.len();
    let result_truncated = row_count > returned_row_count;

    Ok(DatabaseQueryResult {
        sql: sql.to_string(),
        statement_type,
        row_count,
        returned_row_count,
        result_truncated,
        max_rows,
        rows: output,
        columns,
        notices: Vec::new(),
        duration_ms: start.elapsed().as_millis(),
        meta: Some(query_result_meta(
            returned_row_count,
            result_truncated,
            max_rows,
            None,
        )),
        database_name: None,
        schema_name: None,
    })
}

fn query_result_meta(
    returned_row_count: usize,
    result_truncated: bool,
    max_rows: usize,
    extra: Option<Value>,
) -> Value {
    let mut meta = match extra {
        Some(Value::Object(meta)) => meta,
        _ => Map::new(),
    };
    meta.insert(
        "returnedRowCount".to_string(),
        Value::from(returned_row_count),
    );
    meta.insert("resultTruncated".to_string(), Value::Bool(result_truncated));
    meta.insert("maxRows".to_string(), Value::from(max_rows));
    Value::Object(meta)
}

fn read_sqlite_table_data(
    connection: &StoredDatabaseConnection,
    table_name: &str,
    limit: usize,
    offset: usize,
    include_total_count: bool,
) -> Result<DatabaseTableData> {
    let start = Instant::now();
    let conn = sqlite_connection(connection)?;
    let table = describe_sqlite_table_inner(&conn, table_name, DatabaseObjectType::Table)
        .or_else(|_| describe_sqlite_table_inner(&conn, table_name, DatabaseObjectType::View))?;
    let total_row_count = if include_total_count {
        Some(count_sqlite_rows(&conn, table_name)?)
    } else {
        None
    };
    let sql = format!(
        "SELECT * FROM {} LIMIT ?1 OFFSET ?2",
        quote_identifier(table_name)
    );
    let mut stmt = conn.prepare(&sql).map_err(sqlite_server_error)?;
    let column_names = stmt
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut rows = stmt
        .query(params![limit as i64 + 1, offset as i64])
        .map_err(sqlite_server_error)?;
    let mut output = Vec::new();
    while let Some(row) = rows.next().map_err(sqlite_server_error)? {
        output.push(row_to_json_map(row, &column_names)?);
        if output.len() > limit {
            break;
        }
    }
    let has_more = output.len() > limit;
    if has_more {
        output.truncate(limit);
    }

    Ok(DatabaseTableData {
        database_name: None,
        schema_name: None,
        table_name: table_name.to_string(),
        offset,
        limit,
        row_count: output.len(),
        total_row_count,
        exact_total_row_count: include_total_count,
        has_more,
        columns: table.columns,
        rows: output,
        duration_ms: start.elapsed().as_millis(),
    })
}

fn count_sqlite_rows(conn: &Connection, table_name: &str) -> Result<usize> {
    let sql = format!("SELECT COUNT(*) FROM {}", quote_identifier(table_name));
    let count: i64 = conn
        .query_row(&sql, [], |row| row.get(0))
        .map_err(sqlite_server_error)?;
    Ok(count.max(0) as usize)
}

struct TransferSourceSnapshot {
    columns: Vec<DatabaseObjectColumn>,
    rows: Vec<Map<String, Value>>,
}

fn append_transfer_job_log<F>(
    job: &mut DatabaseTransferJob,
    level: &str,
    message: impl Into<String>,
    details: Option<String>,
    persist_job: &mut F,
) where
    F: FnMut(&DatabaseTransferJob),
{
    let timestamp = Utc::now();
    job.logs.push(DatabaseTransferJobLogEntry {
        timestamp,
        level: level.to_string(),
        message: message.into(),
        details,
    });
    job.updated_at = timestamp;
    persist_job(job);
}

fn set_transfer_job_progress<F>(
    job: &mut DatabaseTransferJob,
    current: usize,
    total: usize,
    message: impl AsRef<str>,
    persist_job: &mut F,
) where
    F: FnMut(&DatabaseTransferJob),
{
    job.progress = progress(current, total, message.as_ref());
    job.updated_at = Utc::now();
    persist_job(job);
}

fn add_transfer_job_warning<F>(
    job: &mut DatabaseTransferJob,
    message: impl Into<String>,
    scope: Option<String>,
    code: Option<String>,
    persist_job: &mut F,
) where
    F: FnMut(&DatabaseTransferJob),
{
    let message = message.into();
    job.warnings.push(DatabaseTransferJobWarning {
        message: message.clone(),
        scope: scope.clone(),
        code,
    });
    append_transfer_job_log(job, "warning", message, scope, persist_job);
}

async fn run_database_transfer<F>(
    source_connection: &StoredDatabaseConnection,
    target_connection: &StoredDatabaseConnection,
    job: &mut DatabaseTransferJob,
    persist_job: &mut F,
) -> Result<DatabaseTransferJobResult>
where
    F: FnMut(&DatabaseTransferJob) + Send,
{
    let source = job.source.clone();
    let target = job.target.clone();
    let mode = job.mode;

    append_transfer_job_log(
        job,
        "info",
        "Opening source and target connections",
        None,
        persist_job,
    );
    set_transfer_job_progress(job, 0, 4, "Opening connections", persist_job);
    append_transfer_job_log(
        job,
        "info",
        "Connections opened",
        Some(format!(
            "{} -> {}",
            source_connection.profile.db_type.as_str(),
            target_connection.profile.db_type.as_str()
        )),
        persist_job,
    );
    set_transfer_job_progress(job, 1, 4, "Inspecting source table", persist_job);

    let source_metadata = database_table_metadata(
        source_connection,
        source.database_name.as_deref(),
        source.schema_name.as_deref(),
        &source.table_name,
    )
    .await?;
    if source_metadata.columns.is_empty() {
        return Err(ServerError::database(
            StatusCode::BAD_REQUEST,
            "Source table has no columns",
            None,
            "NO_COMPATIBLE_COLUMNS",
            "validation",
            false,
        ));
    }
    append_transfer_job_log(
        job,
        "info",
        "Loaded source table model",
        Some(format!(
            "{} ({} column(s))",
            source.table_name,
            source_metadata.columns.len()
        )),
        persist_job,
    );

    for column in &source_metadata.columns {
        if column.default_value.is_some() {
            add_transfer_job_warning(
                job,
                format!("Skipped default value mapping for column {}", column.name),
                Some(column.name.clone()),
                Some("DEFAULT_SKIPPED".to_string()),
                persist_job,
            );
        }
    }

    let target_exists = transfer_target_exists(target_connection, &target).await?;
    let mut created_table = false;
    match mode {
        DatabaseTransferMode::TableCopy if !target_exists => {
            return Err(ServerError::database(
                StatusCode::NOT_FOUND,
                format!("Table not found: {}", target.table_name),
                None,
                "TABLE_NOT_FOUND",
                "metadata",
                false,
            ));
        }
        DatabaseTransferMode::SchemaOnly | DatabaseTransferMode::SchemaAndData if target_exists => {
            return Err(ServerError::database(
                StatusCode::BAD_REQUEST,
                format!("Target table already exists: {}", target.table_name),
                None,
                "TARGET_TABLE_EXISTS",
                "validation",
                false,
            ));
        }
        DatabaseTransferMode::SchemaOnly | DatabaseTransferMode::SchemaAndData => {
            set_transfer_job_progress(job, 2, 4, "Preparing target table", persist_job);
            create_transfer_target_table(target_connection, &target, &source_metadata.columns)
                .await?;
            created_table = true;
            append_transfer_job_log(
                job,
                "info",
                "Created target table",
                Some(target.table_name.clone()),
                persist_job,
            );
        }
        DatabaseTransferMode::TableCopy => {}
    }

    if mode == DatabaseTransferMode::SchemaOnly {
        return Ok(DatabaseTransferJobResult {
            created_table,
            copied_row_count: 0,
            failed_row_count: 0,
            ignored_source_columns: Vec::new(),
            mapped_column_count: source_metadata.columns.len(),
            column_failures: Vec::new(),
            row_failures: Vec::new(),
        });
    }

    set_transfer_job_progress(job, 3, 4, "Loading target table metadata", persist_job);
    let target_metadata = database_table_metadata(
        target_connection,
        target.database_name.as_deref(),
        target.schema_name.as_deref(),
        &target.table_name,
    )
    .await?;
    let source_column_names = source_metadata
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    let mappings = build_column_mappings(&source_column_names, &target_metadata.columns);
    let used_source_columns = mappings
        .iter()
        .map(|(_, source_name)| source_name.clone())
        .collect::<Vec<_>>();
    let ignored_source_columns = source_column_names
        .iter()
        .filter(|source_name| !used_source_columns.iter().any(|used| used == *source_name))
        .cloned()
        .collect::<Vec<_>>();
    let column_failures = target_metadata
        .columns
        .iter()
        .filter(|column| {
            column.nullable == Some(false)
                && column.default_value.is_none()
                && !column.is_primary_key
                && !mappings
                    .iter()
                    .any(|(target_name, _)| target_name == &column.name)
        })
        .map(|column| {
            serde_json::json!({
                "columnName": column.name,
                "message": "Required target column has no matching source column",
            })
        })
        .collect::<Vec<_>>();
    let mapping_details = serde_json::json!({
        "mappings": mappings.iter().map(|(target_name, source_name)| serde_json::json!({
            "targetColumnName": target_name,
            "sourceColumnName": source_name,
        })).collect::<Vec<_>>(),
        "ignoredSourceColumns": ignored_source_columns,
        "columnFailures": column_failures,
    });
    if mappings.is_empty() {
        return Err(ServerError::database(
            StatusCode::BAD_REQUEST,
            "No compatible columns were found between the source and target tables",
            Some(mapping_details.to_string()),
            "NO_COMPATIBLE_COLUMNS",
            "validation",
            false,
        ));
    }
    if !column_failures.is_empty() {
        return Err(ServerError::database(
            StatusCode::BAD_REQUEST,
            "Target table is missing required column mappings",
            Some(mapping_details.to_string()),
            "INCOMPATIBLE_TARGET_TABLE",
            "validation",
            false,
        ));
    }
    if !ignored_source_columns.is_empty() {
        add_transfer_job_warning(
            job,
            format!(
                "Ignored {} unmapped source column(s)",
                ignored_source_columns.len()
            ),
            Some(ignored_source_columns.join(", ")),
            Some("IGNORED_SOURCE_COLUMNS".to_string()),
            persist_job,
        );
    }

    let mapped_columns = mappings
        .iter()
        .filter_map(|(target_name, _)| {
            target_metadata
                .columns
                .iter()
                .find(|column| column.name == *target_name)
                .cloned()
        })
        .collect::<Vec<_>>();
    let mut copied_row_count = 0usize;
    let mut failed_row_count = 0usize;
    let mut row_failures = Vec::new();
    let mut processed_row_count = 0usize;
    let mut total_row_count = 0usize;
    let mut offset = 0usize;

    loop {
        let page = read_transfer_source_page(
            source_connection,
            &source,
            TRANSFER_ROW_BATCH_SIZE,
            offset,
            offset == 0,
        )
        .await?;
        if let Some(total) = page.total_row_count {
            total_row_count = total;
        }
        let page_row_count = page.rows.len();
        if page_row_count == 0 {
            break;
        }

        for (page_row_index, source_row) in page.rows.iter().enumerate() {
            let mapped_row = mappings
                .iter()
                .map(|(target_name, source_name)| {
                    (
                        target_name.clone(),
                        source_row.get(source_name).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect::<Map<_, _>>();
            match insert_transfer_rows(target_connection, &target, &mapped_columns, &[mapped_row])
                .await
            {
                Ok(count) => copied_row_count += count,
                Err(error) => {
                    failed_row_count += 1;
                    if row_failures.len() < MAX_ROW_FAILURE_DETAILS {
                        row_failures.push(serde_json::json!({
                            "rowIndex": offset + page_row_index,
                            "message": database_error_message(&error),
                            "code": error.body.code,
                        }));
                    }
                }
            }
        }

        processed_row_count = processed_row_count.saturating_add(page_row_count);
        offset = offset.checked_add(page_row_count).ok_or_else(|| {
            ServerError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "table row offset overflow",
            )
        })?;
        let progress_total = total_row_count.max(processed_row_count).max(1);
        set_transfer_job_progress(
            job,
            processed_row_count,
            progress_total,
            format!(
                "Copied {} of {} row(s)",
                processed_row_count, progress_total
            ),
            persist_job,
        );

        if !page.has_more {
            break;
        }
    }

    if failed_row_count > 0 {
        add_transfer_job_warning(
            job,
            format!("{failed_row_count} row(s) failed to copy"),
            Some(target.table_name.clone()),
            Some("ROW_COPY_FAILURES".to_string()),
            persist_job,
        );
    }

    Ok(DatabaseTransferJobResult {
        created_table,
        copied_row_count,
        failed_row_count,
        ignored_source_columns,
        mapped_column_count: mappings.len(),
        column_failures: Vec::new(),
        row_failures,
    })
}

async fn read_transfer_source_page(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
    limit: usize,
    offset: usize,
    include_total_count: bool,
) -> Result<DatabaseTableData> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        return read_sqlite_table_data(
            connection,
            &endpoint.table_name,
            limit,
            offset,
            include_total_count,
        );
    }

    read_live_table_data(
        connection,
        endpoint.database_name.as_deref(),
        endpoint.schema_name.as_deref(),
        &endpoint.table_name,
        limit,
        offset,
        include_total_count,
    )
    .await
}

async fn read_transfer_source(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
) -> Result<TransferSourceSnapshot> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_connection(connection)?;
        let table =
            describe_sqlite_table_inner(&conn, &endpoint.table_name, DatabaseObjectType::Table)?;
        let sql = format!("SELECT * FROM {}", quote_identifier(&endpoint.table_name));
        let mut stmt = conn.prepare(&sql).map_err(sqlite_server_error)?;
        let column_names = stmt
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut rows = stmt.query([]).map_err(sqlite_server_error)?;
        let mut output = Vec::new();
        while let Some(row) = rows.next().map_err(sqlite_server_error)? {
            output.push(row_to_json_map(row, &column_names)?);
        }
        return Ok(TransferSourceSnapshot {
            columns: table.columns,
            rows: output,
        });
    }

    let table = describe_live_table(
        connection,
        endpoint.database_name.as_deref(),
        endpoint.schema_name.as_deref(),
        &endpoint.table_name,
        DatabaseObjectType::Table,
    )
    .await?;
    let mut rows = Vec::new();
    let mut offset = 0usize;
    loop {
        let page = read_live_table_data(
            connection,
            endpoint.database_name.as_deref(),
            endpoint.schema_name.as_deref(),
            &endpoint.table_name,
            TRANSFER_ROW_BATCH_SIZE,
            offset,
            false,
        )
        .await?;
        let page_row_count = page.rows.len();
        rows.extend(page.rows);
        if !page.has_more || page_row_count == 0 {
            break;
        }
        offset = offset.checked_add(page_row_count).ok_or_else(|| {
            ServerError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "table row offset overflow",
            )
        })?;
    }
    Ok(TransferSourceSnapshot {
        columns: table.columns,
        rows,
    })
}

async fn transfer_target_exists(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
) -> Result<bool> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_transfer_target_connection(connection)?;
        return sqlite_table_exists(&conn, &endpoint.table_name);
    }

    let pool = live_pool_for_database(connection, endpoint.database_name.as_deref()).await?;
    let (_database, schema) = live_scope(
        connection,
        endpoint.database_name.as_deref(),
        endpoint.schema_name.as_deref(),
    );
    let exists_sql = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
            "SELECT 1 FROM information_schema.tables WHERE table_schema = $1 AND table_name = $2"
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            "SELECT 1 FROM information_schema.tables WHERE table_schema = ? AND table_name = ?"
        }
        SupportedDatabaseType::Sqlite => unreachable!(),
    };
    let row = sqlx::query(exists_sql)
        .bind(&schema)
        .bind(&endpoint.table_name)
        .fetch_optional(&pool)
        .await
        .map_err(sqlx_server_error)?;
    Ok(row.is_some())
}

async fn ensure_transfer_target_schema(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
) -> Result<()> {
    if connection.profile.db_type != SupportedDatabaseType::Postgresql {
        return Ok(());
    }
    let Some(schema_name) = endpoint
        .schema_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let pool = live_pool_for_database(connection, endpoint.database_name.as_deref()).await?;
    let sql = format!(
        "CREATE SCHEMA IF NOT EXISTS {}",
        quote_identifier(schema_name)
    );
    pool.execute(sql.as_str())
        .await
        .map_err(sqlx_server_error)?;
    Ok(())
}

async fn create_transfer_target_table(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
    columns: &[DatabaseObjectColumn],
) -> Result<()> {
    let sql = build_transfer_create_table_sql(connection.profile.db_type, endpoint, columns);
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_transfer_target_connection(connection)?;
        conn.execute_batch(&sql).map_err(sqlite_server_error)?;
        return Ok(());
    }

    ensure_transfer_target_schema(connection, endpoint).await?;
    let pool = live_pool_for_database(connection, endpoint.database_name.as_deref()).await?;
    pool.execute(sql.as_str())
        .await
        .map_err(sqlx_server_error)?;
    Ok(())
}

async fn insert_transfer_rows(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
    columns: &[DatabaseObjectColumn],
    rows: &[Map<String, Value>],
) -> Result<usize> {
    if rows.is_empty() {
        return Ok(0);
    }

    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let mut conn = sqlite_transfer_target_connection(connection)?;
        let tx = conn.transaction().map_err(sqlite_server_error)?;
        for row in rows {
            let sql = build_transfer_insert_sql(connection.profile.db_type, endpoint, columns, row);
            tx.execute(&sql, []).map_err(sqlite_server_error)?;
        }
        tx.commit().map_err(sqlite_server_error)?;
        return Ok(rows.len());
    }

    let pool = live_pool_for_database(connection, endpoint.database_name.as_deref()).await?;
    for row in rows {
        let sql = build_transfer_insert_sql(connection.profile.db_type, endpoint, columns, row);
        pool.execute(sql.as_str())
            .await
            .map_err(sqlx_server_error)?;
    }
    Ok(rows.len())
}

fn build_transfer_create_table_sql(
    db_type: SupportedDatabaseType,
    endpoint: &DatabaseTransferEndpoint,
    columns: &[DatabaseObjectColumn],
) -> String {
    let primary_key_columns = columns
        .iter()
        .filter(|column| column.is_primary_key)
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    let single_primary_key = primary_key_columns.len() == 1;
    let mut definitions = Vec::new();
    for column in columns {
        let mut definition = format!(
            "{} {}",
            quote_transfer_identifier(db_type, &column.name),
            transfer_column_type(db_type, column)
        );
        if single_primary_key && column.is_primary_key {
            definition.push_str(" PRIMARY KEY");
        }
        if column.nullable == Some(false) && !(single_primary_key && column.is_primary_key) {
            definition.push_str(" NOT NULL");
        }
        definitions.push(definition);
    }
    if primary_key_columns.len() > 1 {
        definitions.push(format!(
            "PRIMARY KEY ({})",
            primary_key_columns
                .iter()
                .map(|column| quote_transfer_identifier(db_type, column))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    format!(
        "CREATE TABLE IF NOT EXISTS {} ({})",
        transfer_table_ref(db_type, endpoint),
        definitions.join(", ")
    )
}

fn build_transfer_insert_sql(
    db_type: SupportedDatabaseType,
    endpoint: &DatabaseTransferEndpoint,
    columns: &[DatabaseObjectColumn],
    row: &Map<String, Value>,
) -> String {
    let column_names = columns
        .iter()
        .map(|column| quote_transfer_identifier(db_type, &column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let values = columns
        .iter()
        .map(|column| {
            row.get(&column.name)
                .map(|value| transfer_value_literal(db_type, value))
                .unwrap_or_else(|| "NULL".to_string())
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {} ({column_names}) VALUES ({values})",
        transfer_table_ref(db_type, endpoint)
    )
}

fn transfer_table_ref(
    db_type: SupportedDatabaseType,
    endpoint: &DatabaseTransferEndpoint,
) -> String {
    match db_type {
        SupportedDatabaseType::Postgresql => format!(
            "{}.{}",
            quote_identifier(endpoint.schema_name.as_deref().unwrap_or("public")),
            quote_identifier(&endpoint.table_name)
        ),
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            if let Some(database_name) = endpoint.database_name.as_deref() {
                format!(
                    "{}.{}",
                    quote_mysql_identifier(database_name),
                    quote_mysql_identifier(&endpoint.table_name)
                )
            } else {
                quote_mysql_identifier(&endpoint.table_name)
            }
        }
        SupportedDatabaseType::Sqlite => quote_identifier(&endpoint.table_name),
    }
}

fn quote_transfer_identifier(db_type: SupportedDatabaseType, value: &str) -> String {
    match db_type {
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            quote_mysql_identifier(value)
        }
        SupportedDatabaseType::Postgresql | SupportedDatabaseType::Sqlite => {
            quote_identifier(value)
        }
    }
}

fn transfer_column_type(
    db_type: SupportedDatabaseType,
    column: &DatabaseObjectColumn,
) -> &'static str {
    let native = column
        .native_type
        .as_deref()
        .or(column.data_type.as_deref())
        .unwrap_or("")
        .to_ascii_lowercase();
    match db_type {
        SupportedDatabaseType::Sqlite => {
            if native.contains("bool") || native.contains("int") {
                "INTEGER"
            } else if native.contains("decimal") || native.contains("numeric") {
                "NUMERIC"
            } else if native.contains("real")
                || native.contains("float")
                || native.contains("double")
            {
                "REAL"
            } else if native.contains("blob") || native.contains("binary") {
                "BLOB"
            } else {
                "TEXT"
            }
        }
        SupportedDatabaseType::Postgresql => {
            if native.contains("bool") {
                "BOOLEAN"
            } else if native.contains("bigint") {
                "BIGINT"
            } else if native.contains("int") {
                "INTEGER"
            } else if native.contains("decimal") || native.contains("numeric") {
                "NUMERIC"
            } else if native.contains("real")
                || native.contains("float")
                || native.contains("double")
            {
                "DOUBLE PRECISION"
            } else if native.contains("json") {
                "JSONB"
            } else if native.contains("uuid") {
                "UUID"
            } else if native.contains("blob")
                || native.contains("binary")
                || native.contains("bytea")
            {
                "BYTEA"
            } else if native.contains("timestamp") || native.contains("datetime") {
                "TIMESTAMP"
            } else if native == "date" {
                "DATE"
            } else if native == "time" {
                "TIME"
            } else {
                "TEXT"
            }
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            if native.contains("bool") {
                "BOOLEAN"
            } else if native.contains("bigint") {
                "BIGINT"
            } else if native.contains("int") {
                "INT"
            } else if native.contains("decimal") || native.contains("numeric") {
                "DECIMAL(38, 10)"
            } else if native.contains("real")
                || native.contains("float")
                || native.contains("double")
            {
                "DOUBLE"
            } else if native.contains("json") {
                if db_type == SupportedDatabaseType::Mysql {
                    "JSON"
                } else {
                    "LONGTEXT"
                }
            } else if native.contains("uuid") {
                "CHAR(36)"
            } else if native.contains("blob")
                || native.contains("binary")
                || native.contains("bytea")
            {
                "LONGBLOB"
            } else if native.contains("timestamp") || native.contains("datetime") {
                "DATETIME"
            } else if native == "date" {
                "DATE"
            } else if native == "time" {
                "TIME"
            } else {
                "TEXT"
            }
        }
    }
}

fn transfer_value_literal(db_type: SupportedDatabaseType, value: &Value) -> String {
    if let Some(encoded) = database_buffer_base64(value) {
        return match db_type {
            SupportedDatabaseType::Postgresql => {
                format!("decode('{encoded}', 'base64')")
            }
            SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
                format!("FROM_BASE64('{encoded}')")
            }
            SupportedDatabaseType::Sqlite => BASE64_STANDARD
                .decode(encoded)
                .map(|bytes| {
                    let hex = bytes
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>();
                    format!("X'{hex}'")
                })
                .unwrap_or_else(|_| "NULL".to_string()),
        };
    }
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(value) => {
            if *value {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        }
        Value::Number(value) => value.to_string(),
        Value::String(value) => database_text_literal(db_type, value),
        Value::Array(_) | Value::Object(_) => {
            database_text_literal(db_type, &serde_json::to_string(value).unwrap_or_default())
        }
    }
}

fn database_text_literal(db_type: SupportedDatabaseType, value: &str) -> String {
    match db_type {
        SupportedDatabaseType::Postgresql => {
            let mut tag = "iowb".to_string();
            while value.contains(&format!("${tag}$")) {
                tag.push('_');
            }
            format!("${tag}${value}${tag}$")
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            let hex = value
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            format!("CONVERT(X'{hex}' USING utf8mb4)")
        }
        SupportedDatabaseType::Sqlite => {
            let hex = value
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            format!("CAST(X'{hex}' AS TEXT)")
        }
    }
}

fn database_buffer_base64(value: &Value) -> Option<&str> {
    let object = value.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("buffer")
        || object.get("encoding").and_then(Value::as_str) != Some("base64")
    {
        return None;
    }
    object.get("value").and_then(Value::as_str)
}

fn sqlite_table_exists(conn: &Connection, table_name: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE name = ?1 AND type = 'table'",
            params![table_name],
            |_| Ok(()),
        )
        .optional()
        .map_err(sqlite_server_error)?
        .is_some())
}

fn row_to_json_map(row: &rusqlite::Row<'_>, column_names: &[String]) -> Result<Map<String, Value>> {
    let mut item = Map::new();
    for (index, name) in column_names.iter().enumerate() {
        item.insert(
            name.clone(),
            sqlite_value_to_json(row.get_ref(index).map_err(sqlite_server_error)?),
        );
    }
    Ok(item)
}

fn sqlite_value_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::Number(value.into()),
        ValueRef::Real(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        ValueRef::Text(value) => Value::String(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(value) => database_buffer_value(value),
    }
}

fn database_buffer_value(value: &[u8]) -> Value {
    serde_json::json!({
        "type": "buffer",
        "encoding": "base64",
        "value": BASE64_STANDARD.encode(value),
    })
}

fn classify_statement(sql: &str) -> DatabaseQueryStatementType {
    let keyword = sql
        .trim_start()
        .trim_start_matches('(')
        .split(|ch: char| ch.is_whitespace() || ch == ';')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match keyword.as_str() {
        "select" | "show" | "describe" | "pragma" | "with" | "explain" => {
            DatabaseQueryStatementType::Select
        }
        "insert" => DatabaseQueryStatementType::Insert,
        "update" => DatabaseQueryStatementType::Update,
        "delete" => DatabaseQueryStatementType::Delete,
        "create" | "alter" | "drop" | "truncate" => DatabaseQueryStatementType::Ddl,
        _ => DatabaseQueryStatementType::Other,
    }
}

fn parse_object_type(raw: &str) -> Result<DatabaseObjectType> {
    match raw {
        "database" => Ok(DatabaseObjectType::Database),
        "schema" => Ok(DatabaseObjectType::Schema),
        "table" => Ok(DatabaseObjectType::Table),
        "view" => Ok(DatabaseObjectType::View),
        _ => Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Unsupported database object type",
        )),
    }
}

fn database_object_type_as_str(object_type: DatabaseObjectType) -> &'static str {
    match object_type {
        DatabaseObjectType::Connection => "connection",
        DatabaseObjectType::Database => "database",
        DatabaseObjectType::Schema => "schema",
        DatabaseObjectType::Table => "table",
        DatabaseObjectType::View => "view",
    }
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn enrich_transfer_endpoint(
    mut endpoint: DatabaseTransferEndpoint,
    profile: &DatabaseConnectionProfile,
) -> DatabaseTransferEndpoint {
    endpoint.connection_name = Some(profile.name.clone());
    endpoint.connection_type = Some(profile.db_type);
    endpoint.database_name = endpoint.database_name.or_else(|| {
        profile.database_name.clone().or_else(|| {
            Some(
                match profile.db_type {
                    SupportedDatabaseType::Postgresql => "postgres",
                    SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => "mysql",
                    SupportedDatabaseType::Sqlite => "main",
                }
                .to_string(),
            )
        })
    });
    endpoint
}

fn progress(current: usize, total: usize, message: &str) -> Value {
    let safe_total = total.max(1);
    let safe_current = current.min(safe_total);
    let percentage = ((safe_current as f64 / safe_total as f64) * 100.0).round() as usize;
    serde_json::json!({
        "current": safe_current,
        "total": safe_total,
        "percentage": percentage,
        "message": message,
    })
}

fn database_error_message(error: &ServerError) -> String {
    error
        .body
        .details
        .as_deref()
        .filter(|details| !details.trim().is_empty())
        .unwrap_or(&error.body.error)
        .to_string()
}

fn database_error_response(
    error: ServerError,
    fallback_message: &str,
    context: Option<Value>,
) -> Response {
    let message = database_error_message(&error);
    let code = error.body.code.unwrap_or_else(|| {
        if error.status == StatusCode::NOT_FOUND {
            "NOT_FOUND".to_string()
        } else {
            "DATABASE_ERROR".to_string()
        }
    });
    let category = error.body.category.unwrap_or_else(|| {
        if error.status == StatusCode::BAD_REQUEST {
            "validation".to_string()
        } else {
            "unknown".to_string()
        }
    });
    let retryable = error.body.retryable.unwrap_or(false);
    let status = if error.status == StatusCode::NOT_FOUND
        || code == "TABLE_NOT_FOUND"
        || code == "SESSION_NOT_FOUND"
    {
        StatusCode::NOT_FOUND
    } else if matches!(
        category.as_str(),
        "connection" | "authentication" | "validation" | "metadata"
    ) {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    let mut payload = serde_json::json!({
        "error": if message.trim().is_empty() { fallback_message } else { &message },
        "code": code,
        "category": category,
        "retryable": retryable,
    });
    if let Some(context) = context {
        payload
            .as_object_mut()
            .expect("database error payload must be an object")
            .insert("context".to_string(), context);
    }
    (status, Json(payload)).into_response()
}

fn sqlite_server_error(error: rusqlite::Error) -> ServerError {
    let message = error.to_string();
    let raw_code = error
        .sqlite_error_code()
        .map(|code| format!("{code:?}"))
        .unwrap_or_default();
    let code = match raw_code.as_str() {
        "DatabaseBusy" => "SQLITE_BUSY",
        "DatabaseLocked" => "SQLITE_LOCKED",
        "ReadOnly" => "SQLITE_READONLY",
        "CannotOpen" => "SQLITE_CANTOPEN",
        "ConstraintViolation" => "SQLITE_CONSTRAINT",
        _ => "SQLITE_ERROR",
    };
    let retryable = matches!(code, "SQLITE_BUSY" | "SQLITE_LOCKED");
    let category = if matches!(
        code,
        "SQLITE_BUSY" | "SQLITE_LOCKED" | "SQLITE_READONLY" | "SQLITE_CANTOPEN"
    ) {
        "connection"
    } else {
        "execution"
    };
    ServerError::database(
        if category == "execution" {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::BAD_REQUEST
        },
        message,
        None,
        code,
        category,
        retryable,
    )
}

fn sqlx_server_error(error: sqlx::Error) -> ServerError {
    let vendor_code = match &error {
        sqlx::Error::Database(database_error) => database_error
            .code()
            .map(|code| normalize_sqlx_vendor_code(code.as_ref()).to_string()),
        _ => None,
    };
    let message = error.to_string();
    let normalized = message.to_ascii_lowercase();
    let code = vendor_code.unwrap_or_else(|| {
        if normalized.contains("connection refused") {
            "ECONNREFUSED".to_string()
        } else if normalized.contains("timed out") || normalized.contains("timeout") {
            "ETIMEDOUT".to_string()
        } else if normalized.contains("dns") || normalized.contains("name or service not known") {
            "ENOTFOUND".to_string()
        } else {
            "DATABASE_ERROR".to_string()
        }
    });
    let category = if code == "28P01"
        || code == "ER_ACCESS_DENIED_ERROR"
        || normalized.contains("access denied")
        || normalized.contains("authentication")
        || normalized.contains("password")
    {
        "authentication"
    } else if code == "3D000"
        || code == "ER_BAD_DB_ERROR"
        || normalized.contains("unknown database")
        || normalized.contains("does not exist")
        || normalized.contains("not found")
    {
        "metadata"
    } else if matches!(code.as_str(), "ECONNREFUSED" | "ETIMEDOUT" | "ENOTFOUND")
        || normalized.contains("connection")
    {
        "connection"
    } else {
        "execution"
    };
    let retryable = matches!(code.as_str(), "ECONNREFUSED" | "ETIMEDOUT" | "ENOTFOUND");
    ServerError::database(
        if category == "execution" {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::BAD_REQUEST
        },
        message,
        None,
        code,
        category,
        retryable,
    )
}

fn normalize_sqlx_vendor_code(code: &str) -> &str {
    match code {
        "1045" => "ER_ACCESS_DENIED_ERROR",
        "1048" => "ER_BAD_NULL_ERROR",
        "1049" => "ER_BAD_DB_ERROR",
        "1054" => "ER_BAD_FIELD_ERROR",
        "1062" => "ER_DUP_ENTRY",
        "1064" => "ER_PARSE_ERROR",
        "1146" => "ER_NO_SUCH_TABLE",
        "1205" => "ER_LOCK_WAIT_TIMEOUT",
        "1213" => "ER_LOCK_DEADLOCK",
        "1366" => "ER_TRUNCATED_WRONG_VALUE_FOR_FIELD",
        "1406" => "ER_DATA_TOO_LONG",
        "1451" => "ER_ROW_IS_REFERENCED_2",
        "1452" => "ER_NO_REFERENCED_ROW_2",
        _ => code,
    }
}

fn io_server_error(error: std::io::Error) -> ServerError {
    ServerError::database(
        StatusCode::BAD_REQUEST,
        error.to_string(),
        None,
        "DATABASE_FILESYSTEM_ERROR",
        "validation",
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn sqlite_test_connection(path: &std::path::Path) -> StoredDatabaseConnection {
        let now = Utc::now();
        StoredDatabaseConnection {
            profile: DatabaseConnectionProfile {
                id: 1,
                name: "test-sqlite".to_string(),
                db_type: SupportedDatabaseType::Sqlite,
                host: None,
                port: None,
                username: None,
                database_name: None,
                file_path: Some(path.to_string_lossy().into_owned()),
                show_all_databases: false,
                has_password: false,
                last_test_status: None,
                last_test_message: None,
                last_tested_at: None,
                created_at: now,
                updated_at: now,
            },
            password: None,
        }
    }

    fn test_transfer_job(
        source: DatabaseTransferEndpoint,
        target: DatabaseTransferEndpoint,
        mode: DatabaseTransferMode,
    ) -> DatabaseTransferJob {
        let now = Utc::now();
        DatabaseTransferJob {
            id: new_id("database-transfer-job-test"),
            job_type: "table-transfer".to_string(),
            mode,
            status: DatabaseTransferJobStatus::Running,
            source,
            target,
            progress: progress(0, 1, "Queued"),
            logs: Vec::new(),
            warnings: Vec::new(),
            error: None,
            result: None,
            created_at: now,
            updated_at: now,
            started_at: Some(now),
            finished_at: None,
        }
    }

    fn postgres_test_connection() -> Option<StoredDatabaseConnection> {
        if env::var("IOWB_RUN_LIVE_POSTGRES_TESTS").ok().as_deref() != Some("1") {
            return None;
        }
        let now = Utc::now();
        Some(StoredDatabaseConnection {
            profile: DatabaseConnectionProfile {
                id: -1,
                name: "test-postgresql".to_string(),
                db_type: SupportedDatabaseType::Postgresql,
                host: Some(
                    env::var("IOWB_TEST_POSTGRES_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
                ),
                port: Some(
                    env::var("IOWB_TEST_POSTGRES_PORT")
                        .ok()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(5432),
                ),
                username: Some(
                    env::var("IOWB_TEST_POSTGRES_USER")
                        .or_else(|_| env::var("USER"))
                        .unwrap_or_else(|_| "postgres".to_string()),
                ),
                database_name: Some(
                    env::var("IOWB_TEST_POSTGRES_DATABASE")
                        .unwrap_or_else(|_| "postgres".to_string()),
                ),
                file_path: None,
                show_all_databases: false,
                has_password: env::var_os("IOWB_TEST_POSTGRES_PASSWORD").is_some(),
                last_test_status: None,
                last_test_message: None,
                last_tested_at: None,
                created_at: now,
                updated_at: now,
            },
            password: env::var("IOWB_TEST_POSTGRES_PASSWORD").ok(),
        })
    }

    fn mysql_test_connection() -> Option<StoredDatabaseConnection> {
        if env::var("IOWB_RUN_LIVE_MYSQL_TESTS").ok().as_deref() != Some("1") {
            return None;
        }
        let now = Utc::now();
        Some(StoredDatabaseConnection {
            profile: DatabaseConnectionProfile {
                id: -2,
                name: "test-mysql".to_string(),
                db_type: SupportedDatabaseType::Mysql,
                host: Some(
                    env::var("IOWB_TEST_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
                ),
                port: Some(
                    env::var("IOWB_TEST_MYSQL_PORT")
                        .ok()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(3306),
                ),
                username: Some(
                    env::var("IOWB_TEST_MYSQL_USER").unwrap_or_else(|_| "root".to_string()),
                ),
                database_name: Some(
                    env::var("IOWB_TEST_MYSQL_DATABASE").unwrap_or_else(|_| "iowb".to_string()),
                ),
                file_path: None,
                show_all_databases: false,
                has_password: env::var_os("IOWB_TEST_MYSQL_PASSWORD").is_some(),
                last_test_status: None,
                last_test_message: None,
                last_tested_at: None,
                created_at: now,
                updated_at: now,
            },
            password: env::var("IOWB_TEST_MYSQL_PASSWORD").ok(),
        })
    }

    #[tokio::test]
    async fn postgres_live_queries_preserve_returning_rows_and_empty_columns() {
        let Some(connection) = postgres_test_connection() else {
            return;
        };
        let url =
            connection_url_for_database(&connection, connection.profile.database_name.as_deref())
                .expect("build live PostgreSQL URL");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live PostgreSQL");
        let table_name = new_id("iowb_database_parity").replace('-', "_");
        let table_ref = quote_identifier(&table_name);
        sqlx::raw_sql(&format!(
            r#"
            CREATE TEMP TABLE {table_ref} (
                id BIGSERIAL PRIMARY KEY,
                amount NUMERIC NOT NULL,
                payload JSONB NOT NULL,
                uid UUID NOT NULL,
                bytes BYTEA NOT NULL,
                numbers BIGINT[] NOT NULL,
                elapsed INTERVAL NOT NULL,
                location POINT NOT NULL,
                boundary CIRCLE NOT NULL
            )
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create PostgreSQL parity table");

        let insert = execute_postgres_query(
            &pool,
            &format!(
                r#"
                INSERT INTO {table_ref} (
                    amount, payload, uid, bytes, numbers, elapsed, location, boundary
                ) VALUES (
                    1234567890.123456789,
                    '{{"ok":true,"nested":[1,2]}}'::jsonb,
                    '123e4567-e89b-12d3-a456-426614174000'::uuid,
                    decode('AAEC/w==', 'base64'),
                    ARRAY[1::bigint, 9007199254740993::bigint],
                    '1 year 2 mons 3 days 04:05:06.789'::interval,
                    point(1.5, -2.25),
                    circle(point(1, 2), 3.5)
                )
                RETURNING id, amount, payload, uid, bytes, numbers, elapsed, location, boundary
                "#,
            ),
            1000,
            Some("public"),
        )
        .await
        .expect("execute INSERT RETURNING");
        assert_eq!(insert.statement_type, DatabaseQueryStatementType::Insert);
        assert_eq!(insert.row_count, 1);
        assert_eq!(insert.returned_row_count, 1);
        assert_eq!(insert.columns.len(), 9);
        let row = insert.rows.first().expect("returned PostgreSQL row");
        assert!(row.get("id").is_some_and(Value::is_string));
        assert_eq!(
            row.get("amount"),
            Some(&Value::String("1234567890.123456789".to_string())),
        );
        assert_eq!(
            row.get("payload"),
            Some(&serde_json::json!({"ok": true, "nested": [1, 2]}))
        );
        assert_eq!(
            row.get("uid"),
            Some(&Value::String(
                "123e4567-e89b-12d3-a456-426614174000".to_string(),
            )),
        );
        assert_eq!(
            row.get("bytes"),
            Some(&serde_json::json!({
                "type": "buffer",
                "encoding": "base64",
                "value": "AAEC/w==",
            })),
        );
        assert_eq!(
            row.get("numbers"),
            Some(&serde_json::json!(["1", "9007199254740993"]))
        );
        assert_eq!(
            row.get("elapsed"),
            Some(&serde_json::json!({
                "years": 1,
                "months": 2,
                "days": 3,
                "hours": 4,
                "minutes": 5,
                "seconds": 6,
                "milliseconds": 789,
            })),
        );
        assert_eq!(
            row.get("location"),
            Some(&serde_json::json!({"x": 1.5, "y": -2.25}))
        );
        assert_eq!(
            row.get("boundary"),
            Some(&serde_json::json!({"x": 1.0, "y": 2.0, "radius": 3.5})),
        );

        let empty = execute_postgres_query(
            &pool,
            &format!("SELECT amount, payload FROM {table_ref} WHERE FALSE"),
            1000,
            Some("public"),
        )
        .await
        .expect("execute empty PostgreSQL select");
        assert_eq!(empty.statement_type, DatabaseQueryStatementType::Select);
        assert_eq!(empty.row_count, 0);
        assert!(empty.rows.is_empty());
        assert_eq!(
            empty
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec!["amount", "payload"],
        );

        sqlx::raw_sql(&format!("DROP TABLE {table_ref}"))
            .execute(&pool)
            .await
            .expect("drop PostgreSQL parity table");
    }

    #[tokio::test]
    async fn mysql_live_queries_preserve_complex_values_empty_columns_and_meta() {
        let Some(connection) = mysql_test_connection() else {
            return;
        };
        let url =
            connection_url_for_database(&connection, connection.profile.database_name.as_deref())
                .expect("build live MySQL URL");
        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("connect to live MySQL");
        let table_name = new_id("iowb_database_parity").replace('-', "_");
        let table_ref = format!("`{}`", table_name.replace('`', "``"));
        sqlx::raw_sql(&format!(
            r#"
            CREATE TEMPORARY TABLE {table_ref} (
                id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
                amount DECIMAL(30, 9) NOT NULL,
                payload JSON NOT NULL,
                bytes BLOB NOT NULL,
                location POINT NOT NULL
            )
            "#,
        ))
        .execute(&pool)
        .await
        .expect("create MySQL parity table");

        let insert = execute_mysql_query(
            &pool,
            &format!(
                r#"
                INSERT INTO {table_ref} (amount, payload, bytes, location)
                VALUES (
                    123456789012345678901.123456789,
                    JSON_OBJECT('ok', TRUE, 'nested', JSON_ARRAY(1, 2)),
                    FROM_BASE64('AAEC/w=='),
                    ST_GeomFromText('POINT(1.5 -2.25)')
                )
                "#,
            ),
            1000,
        )
        .await
        .expect("execute MySQL insert");
        assert_eq!(insert.statement_type, DatabaseQueryStatementType::Insert);
        assert_eq!(insert.row_count, 1);
        assert!(insert.rows.is_empty());
        let insert_meta = insert
            .meta
            .as_ref()
            .and_then(Value::as_object)
            .expect("MySQL insert metadata");
        assert_eq!(
            insert_meta.get("affectedRows").and_then(Value::as_u64),
            Some(1)
        );
        assert!(
            insert_meta
                .get("insertId")
                .and_then(Value::as_u64)
                .is_some_and(|value| value > 0)
        );
        assert_eq!(
            insert_meta.get("warningStatus").and_then(Value::as_u64),
            Some(0)
        );

        let select = execute_mysql_query(
            &pool,
            &format!("SELECT id, amount, payload, bytes, location FROM {table_ref}"),
            1000,
        )
        .await
        .expect("execute MySQL select");
        assert_eq!(select.statement_type, DatabaseQueryStatementType::Select);
        assert_eq!(select.row_count, 1);
        assert_eq!(select.returned_row_count, 1);
        let row = select.rows.first().expect("returned MySQL row");
        assert_eq!(row.get("id").and_then(Value::as_u64), Some(1));
        assert_eq!(
            row.get("amount"),
            Some(&Value::String(
                "123456789012345678901.123456789".to_string(),
            )),
        );
        assert_eq!(
            row.get("payload"),
            Some(&serde_json::json!({"ok": true, "nested": [1, 2]}))
        );
        assert_eq!(
            row.get("bytes"),
            Some(&serde_json::json!({
                "type": "buffer",
                "encoding": "base64",
                "value": "AAEC/w==",
            })),
        );
        assert_eq!(
            row.get("location"),
            Some(&serde_json::json!({"x": 1.5, "y": -2.25}))
        );

        let empty = execute_mysql_query(
            &pool,
            &format!("SELECT amount, payload FROM {table_ref} WHERE FALSE"),
            1000,
        )
        .await
        .expect("execute empty MySQL select");
        assert_eq!(empty.row_count, 0);
        assert!(empty.rows.is_empty());
        assert_eq!(
            empty
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec!["amount", "payload"],
        );

        sqlx::raw_sql(&format!("DROP TEMPORARY TABLE {table_ref}"))
            .execute(&pool)
            .await
            .expect("drop MySQL parity table");
    }

    #[tokio::test]
    async fn sqlite_row_crud_enforces_optimistic_concurrency() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("database-test")));
        let sqlite = Connection::open(&path).expect("open test database");
        sqlite
            .execute_batch(
                r#"
                CREATE TABLE items (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    note TEXT
                );
                "#,
            )
            .expect("create test table");
        drop(sqlite);

        let connection = sqlite_test_connection(&path);
        let metadata = database_table_metadata(&connection, None, None, "items")
            .await
            .expect("load metadata");
        let inserted = insert_database_row(
            &connection,
            None,
            None,
            "items",
            &metadata,
            &serde_json::json!({"id": 1, "name": "Alpha", "note": null})
                .as_object()
                .expect("insert row object")
                .clone(),
        )
        .await
        .expect("insert row")
        .expect("inserted row result");
        assert_eq!(
            inserted.get("name"),
            Some(&Value::String("Alpha".to_string()))
        );

        let scope = DatabaseTableScopeRequest {
            database_name: None,
            schema_name: None,
            table_name: "items".to_string(),
        };
        let primary_key = serde_json::json!({"id": 1})
            .as_object()
            .expect("primary key object")
            .clone();
        let updated = update_database_row(
            &connection,
            &scope,
            &primary_key,
            serde_json::json!({"name": "Beta"})
                .as_object()
                .expect("update object"),
            &inserted,
        )
        .await
        .expect("update row")
        .expect("updated row result");
        assert_eq!(
            updated.get("name"),
            Some(&Value::String("Beta".to_string()))
        );

        let stale_error = update_database_row(
            &connection,
            &scope,
            &primary_key,
            serde_json::json!({"name": "Gamma"})
                .as_object()
                .expect("stale update object"),
            &inserted,
        )
        .await
        .expect_err("stale update must fail");
        assert_eq!(stale_error.status, StatusCode::CONFLICT);

        delete_database_row(&connection, &scope, &primary_key, &updated)
            .await
            .expect("delete row");
        let page =
            read_sqlite_table_data(&connection, "items", 50, 0, true).expect("read empty table");
        assert_eq!(page.row_count, 0);
        assert_eq!(page.total_row_count, Some(0));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn portable_payload_and_structured_column_mapping_match_web_ai_cli() {
        let payload = serde_json::json!({
            "format": "web-ai-cli/database-portable-v1",
            "type": "table-schema-and-data",
            "table": {
                "name": "people",
                "columns": [
                    {
                        "name": "user_id",
                        "dataType": "integer",
                        "nativeType": "INTEGER",
                        "nullable": false,
                        "isPrimaryKey": true
                    },
                    {
                        "name": "display_name",
                        "dataType": "text",
                        "nativeType": "TEXT",
                        "nullable": true
                    }
                ]
            },
            "rows": [{"user_id": 1, "display_name": "Ada"}]
        });

        let portable = parse_portable_table_payload(&payload).expect("parse portable payload");
        assert_eq!(portable.table_name, "people");
        assert!(portable.include_data);
        assert_eq!(portable.columns.len(), 2);
        assert_eq!(portable.rows.len(), 1);

        let mappings = build_column_mappings(
            &["User ID".to_string(), "DISPLAY-NAME".to_string()],
            &portable.columns,
        );
        assert_eq!(
            mappings,
            vec![
                ("user_id".to_string(), "User ID".to_string()),
                ("display_name".to_string(), "DISPLAY-NAME".to_string()),
            ],
        );
    }

    #[test]
    fn complex_value_normalizers_match_web_driver_shapes() {
        assert_eq!(
            parse_postgres_array("{1,2,NULL}", "INT8"),
            serde_json::json!(["1", "2", null]),
        );
        assert_eq!(
            parse_postgres_array("{{1.25,2.5},{3.75,4}}", "NUMERIC"),
            serde_json::json!([[1.25, 2.5], [3.75, 4.0]]),
        );
        assert_eq!(
            parse_postgres_array("{\"{\\\"ok\\\":true}\",NULL}", "JSONB"),
            serde_json::json!([{"ok": true}, null]),
        );
        assert_eq!(
            parse_postgres_point("(1.5,-2.25)"),
            Some(serde_json::json!({"x": 1.5, "y": -2.25})),
        );
        assert_eq!(
            parse_postgres_circle("<(1,2),3.5>"),
            Some(serde_json::json!({"x": 1.0, "y": 2.0, "radius": 3.5})),
        );
        assert_eq!(
            parse_postgres_interval("1 year 2 mons 3 days 04:05:06.789"),
            serde_json::json!({
                "years": 1,
                "months": 2,
                "days": 3,
                "hours": 4,
                "minutes": 5,
                "seconds": 6,
                "milliseconds": 789,
            }),
        );

        let mut geometry = vec![0, 0, 0, 0, 1];
        geometry.extend_from_slice(&1u32.to_le_bytes());
        geometry.extend_from_slice(&1.5f64.to_le_bytes());
        geometry.extend_from_slice(&(-2.25f64).to_le_bytes());
        assert_eq!(
            parse_mysql_geometry(&geometry),
            Some(serde_json::json!({"x": 1.5, "y": -2.25})),
        );
    }

    #[test]
    fn database_text_literals_preserve_quotes_backslashes_and_unicode() {
        let value = "quote ' slash \\ newline\nemoji 🧪";
        let postgres = database_text_literal(SupportedDatabaseType::Postgresql, value);
        assert!(postgres.starts_with("$iowb$"));
        assert!(postgres.ends_with("$iowb$"));
        assert!(postgres.contains(value));
        assert_eq!(
            database_text_literal(SupportedDatabaseType::Mysql, value),
            "CONVERT(X'71756f7465202720736c617368205c206e65776c696e650a656d6f6a6920f09fa7aa' USING utf8mb4)",
        );
        assert_eq!(
            database_text_literal(SupportedDatabaseType::Sqlite, value),
            "CAST(X'71756f7465202720736c617368205c206e65776c696e650a656d6f6a6920f09fa7aa' AS TEXT)",
        );
    }

    #[test]
    fn capabilities_match_web_adapter_contract() {
        let postgres =
            serde_json::to_value(database_capabilities(SupportedDatabaseType::Postgresql))
                .expect("serialize postgres capabilities");
        assert_eq!(postgres["supportsDatabases"], Value::Bool(true));
        assert_eq!(postgres["supportsSchemas"], Value::Bool(true));
        assert_eq!(postgres["supportsViews"], Value::Bool(true));
        assert_eq!(postgres["supportsIndexes"], Value::Bool(true));
        assert_eq!(postgres["supportsForeignKeys"], Value::Bool(true));
        assert_eq!(postgres["supportsParameterizedQueries"], Value::Bool(true));
        assert_eq!(postgres["supportsOffset"], Value::Bool(true));
        assert_eq!(
            postgres["supportedObjectTypes"],
            serde_json::json!(["table", "view"]),
        );

        let sqlite = database_capabilities(SupportedDatabaseType::Sqlite);
        assert!(!sqlite.supports_databases);
        assert!(!sqlite.supports_schemas);
        assert!(sqlite.supports_views);
        assert!(sqlite.supports_indexes);
    }

    #[test]
    fn query_and_binary_contracts_match_web_ai_cli() {
        assert!(matches!(
            classify_statement("SHOW TABLES"),
            DatabaseQueryStatementType::Select
        ));
        assert!(matches!(
            classify_statement("DESCRIBE users"),
            DatabaseQueryStatementType::Select
        ));
        assert!(matches!(
            classify_statement("EXPLAIN SELECT * FROM users"),
            DatabaseQueryStatementType::Select
        ));

        let binary = database_buffer_value(&[0, 1, 2, 255]);
        assert_eq!(binary["type"], Value::String("buffer".to_string()));
        assert_eq!(binary["encoding"], Value::String("base64".to_string()));
        assert_eq!(binary["value"], Value::String("AAEC/w==".to_string()));
        assert_eq!(
            transfer_value_literal(SupportedDatabaseType::Postgresql, &binary),
            "decode('AAEC/w==', 'base64')"
        );
        assert_eq!(
            transfer_value_literal(SupportedDatabaseType::Mysql, &binary),
            "FROM_BASE64('AAEC/w==')"
        );
        assert_eq!(
            transfer_value_literal(SupportedDatabaseType::Sqlite, &binary),
            "X'000102ff'"
        );

        let meta = query_result_meta(2, true, 2, Some(serde_json::json!({ "driver": "sqlite" })));
        assert_eq!(meta["returnedRowCount"], Value::from(2));
        assert_eq!(meta["resultTruncated"], Value::Bool(true));
        assert_eq!(meta["maxRows"], Value::from(2));
        assert_eq!(meta["driver"], Value::String("sqlite".to_string()));
    }

    #[test]
    fn sqlite_queries_do_not_create_missing_files_but_transfer_targets_can() {
        let directory = env::temp_dir().join(new_id("database-missing-file-test"));
        let path = directory.join("new.sqlite");
        let connection = sqlite_test_connection(&path);

        let error =
            sqlite_connection(&connection).expect_err("normal browsing must not create a file");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(!path.exists());

        drop(
            sqlite_transfer_target_connection(&connection)
                .expect("an explicit transfer target may create its SQLite file"),
        );
        assert!(path.exists());

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(directory);
    }

    #[test]
    fn sqlite_query_row_count_tracks_all_rows_before_truncation() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("database-query-test")));
        let sqlite = Connection::open(&path).expect("open query test database");
        sqlite
            .execute_batch(
                "CREATE TABLE records (id INTEGER PRIMARY KEY); INSERT INTO records VALUES (1), (2), (3);",
            )
            .expect("seed query test database");
        drop(sqlite);

        let result = execute_sqlite_query(
            &sqlite_test_connection(&path),
            "SELECT id FROM records ORDER BY id",
            2,
        )
        .expect("execute query");
        assert_eq!(result.row_count, 3);
        assert_eq!(result.returned_row_count, 2);
        assert!(result.result_truncated);
        assert_eq!(result.rows.len(), 2);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn sqlite_transfer_source_reads_beyond_former_row_limit() {
        let path = env::temp_dir().join(format!("{}.sqlite", new_id("database-transfer-test")));
        let mut sqlite = Connection::open(&path).expect("open transfer test database");
        sqlite
            .execute_batch("CREATE TABLE records (id INTEGER PRIMARY KEY, value TEXT NOT NULL);")
            .expect("create transfer test table");
        let transaction = sqlite.transaction().expect("begin insert transaction");
        {
            let mut statement = transaction
                .prepare("INSERT INTO records (id, value) VALUES (?1, ?2)")
                .expect("prepare insert");
            for id in 0..10_250i64 {
                statement
                    .execute(params![id, format!("row-{id}")])
                    .expect("insert transfer row");
            }
        }
        transaction.commit().expect("commit transfer rows");
        drop(sqlite);

        let connection = sqlite_test_connection(&path);
        let endpoint = DatabaseTransferEndpoint {
            connection_id: connection.profile.id,
            connection_name: Some(connection.profile.name.clone()),
            connection_type: Some(connection.profile.db_type),
            database_name: None,
            schema_name: None,
            table_name: "records".to_string(),
        };
        let snapshot = read_transfer_source(&connection, &endpoint)
            .await
            .expect("read complete transfer source");

        assert_eq!(snapshot.rows.len(), 10_250);
        assert_eq!(
            snapshot.rows.first().and_then(|row| row.get("id")),
            Some(&Value::from(0))
        );
        assert_eq!(
            snapshot.rows.last().and_then(|row| row.get("id")),
            Some(&Value::from(10_249)),
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn sqlite_transfer_copies_multiple_two_hundred_row_batches() {
        let source_path = env::temp_dir().join(format!("{}.sqlite", new_id("transfer-source")));
        let target_path = env::temp_dir().join(format!("{}.sqlite", new_id("transfer-target")));
        let mut source_sqlite = Connection::open(&source_path).expect("open transfer source");
        source_sqlite
            .execute_batch("CREATE TABLE records (id INTEGER PRIMARY KEY, value TEXT NOT NULL);")
            .expect("create transfer source table");
        let transaction = source_sqlite.transaction().expect("begin source seed");
        {
            let mut statement = transaction
                .prepare("INSERT INTO records (id, value) VALUES (?1, ?2)")
                .expect("prepare source seed");
            for id in 0..450i64 {
                statement
                    .execute(params![id, format!("row-{id}")])
                    .expect("insert source row");
            }
        }
        transaction.commit().expect("commit source seed");
        drop(source_sqlite);

        let source_connection = sqlite_test_connection(&source_path);
        let mut target_connection = sqlite_test_connection(&target_path);
        target_connection.profile.id = 2;
        target_connection.profile.name = "test-target-sqlite".to_string();
        let source_endpoint = DatabaseTransferEndpoint {
            connection_id: source_connection.profile.id,
            connection_name: Some(source_connection.profile.name.clone()),
            connection_type: Some(source_connection.profile.db_type),
            database_name: None,
            schema_name: None,
            table_name: "records".to_string(),
        };
        let target_endpoint = DatabaseTransferEndpoint {
            connection_id: target_connection.profile.id,
            connection_name: Some(target_connection.profile.name.clone()),
            connection_type: Some(target_connection.profile.db_type),
            database_name: None,
            schema_name: None,
            table_name: "records_copy".to_string(),
        };
        let mut job = test_transfer_job(
            source_endpoint,
            target_endpoint,
            DatabaseTransferMode::SchemaAndData,
        );
        let mut progress_updates = Vec::new();
        let result = {
            let mut persist_job = |updated_job: &DatabaseTransferJob| {
                progress_updates.push(updated_job.progress.clone());
            };
            run_database_transfer(
                &source_connection,
                &target_connection,
                &mut job,
                &mut persist_job,
            )
            .await
            .expect("copy multiple transfer batches")
        };

        assert!(result.created_table);
        assert_eq!(result.copied_row_count, 450);
        assert_eq!(result.failed_row_count, 0);
        assert!(progress_updates.iter().any(|update| {
            update["current"] == Value::from(200) && update["total"] == Value::from(450)
        }));
        assert!(progress_updates.iter().any(|update| {
            update["current"] == Value::from(400) && update["total"] == Value::from(450)
        }));
        assert!(progress_updates.iter().any(|update| {
            update["current"] == Value::from(450) && update["total"] == Value::from(450)
        }));

        let target_sqlite = Connection::open(&target_path).expect("open transfer target");
        let copied_count: i64 = target_sqlite
            .query_row("SELECT COUNT(*) FROM records_copy", [], |row| row.get(0))
            .expect("count copied rows");
        assert_eq!(copied_count, 450);
        drop(target_sqlite);

        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(target_path);
    }

    #[tokio::test]
    async fn sqlite_transfer_caps_failure_details_but_preserves_failed_count() {
        let source_path = env::temp_dir().join(format!("{}.sqlite", new_id("failure-source")));
        let target_path = env::temp_dir().join(format!("{}.sqlite", new_id("failure-target")));
        let mut source_sqlite = Connection::open(&source_path).expect("open failure source");
        source_sqlite
            .execute_batch("CREATE TABLE records (id INTEGER PRIMARY KEY, value TEXT NOT NULL);")
            .expect("create failure source table");
        let transaction = source_sqlite
            .transaction()
            .expect("begin failure source seed");
        {
            let mut statement = transaction
                .prepare("INSERT INTO records (id, value) VALUES (?1, 'duplicate')")
                .expect("prepare failure source seed");
            for id in 0..75i64 {
                statement.execute(params![id]).expect("insert failure row");
            }
        }
        transaction.commit().expect("commit failure source seed");
        drop(source_sqlite);

        let target_sqlite = Connection::open(&target_path).expect("open failure target");
        target_sqlite
            .execute_batch(
                "CREATE TABLE records_copy (id INTEGER PRIMARY KEY, value TEXT NOT NULL UNIQUE);\
                 INSERT INTO records_copy (id, value) VALUES (999, 'duplicate');",
            )
            .expect("seed conflicting target");
        drop(target_sqlite);

        let source_connection = sqlite_test_connection(&source_path);
        let mut target_connection = sqlite_test_connection(&target_path);
        target_connection.profile.id = 2;
        target_connection.profile.name = "test-target-sqlite".to_string();
        let source_endpoint = DatabaseTransferEndpoint {
            connection_id: source_connection.profile.id,
            connection_name: Some(source_connection.profile.name.clone()),
            connection_type: Some(source_connection.profile.db_type),
            database_name: None,
            schema_name: None,
            table_name: "records".to_string(),
        };
        let target_endpoint = DatabaseTransferEndpoint {
            connection_id: target_connection.profile.id,
            connection_name: Some(target_connection.profile.name.clone()),
            connection_type: Some(target_connection.profile.db_type),
            database_name: None,
            schema_name: None,
            table_name: "records_copy".to_string(),
        };
        let mut job = test_transfer_job(
            source_endpoint,
            target_endpoint,
            DatabaseTransferMode::TableCopy,
        );
        let result = run_database_transfer(
            &source_connection,
            &target_connection,
            &mut job,
            &mut |_| {},
        )
        .await
        .expect("complete transfer with row failures");

        assert_eq!(result.copied_row_count, 0);
        assert_eq!(result.failed_row_count, 75);
        assert_eq!(result.row_failures.len(), MAX_ROW_FAILURE_DETAILS);
        assert!(
            result
                .row_failures
                .iter()
                .all(|failure| failure["code"] == Value::String("SQLITE_CONSTRAINT".to_string()))
        );
        assert!(job.warnings.iter().any(|warning| {
            warning.code.as_deref() == Some("ROW_COPY_FAILURES")
                && warning.message == "75 row(s) failed to copy"
        }));
        assert!(job.logs.iter().any(|entry| {
            entry.level == "warning"
                && entry.message == "75 row(s) failed to copy"
                && entry.details.as_deref() == Some("records_copy")
        }));

        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(target_path);
    }

    #[test]
    fn failed_database_job_preserves_structured_error_metadata() {
        let endpoint = DatabaseTransferEndpoint {
            connection_id: 1,
            connection_name: Some("test".to_string()),
            connection_type: Some(SupportedDatabaseType::Sqlite),
            database_name: None,
            schema_name: None,
            table_name: "records".to_string(),
        };
        let mut job =
            test_transfer_job(endpoint.clone(), endpoint, DatabaseTransferMode::TableCopy);
        job.progress = progress(3, 4, "Loading target table metadata");
        fail_database_job(
            &mut job,
            ServerError::database(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to insert row",
                Some(serde_json::json!({ "operation": "transfer", "rowIndex": 7 }).to_string()),
                "SQLITE_CONSTRAINT",
                "execution",
                true,
            ),
        );

        let error = job.error.expect("structured job error");
        assert_eq!(error.message, "Failed to insert row");
        assert_eq!(error.code.as_deref(), Some("SQLITE_CONSTRAINT"));
        assert_eq!(error.category.as_deref(), Some("execution"));
        assert!(error.retryable);
        assert_eq!(
            error.details,
            Some(serde_json::json!({ "operation": "transfer", "rowIndex": 7 }))
        );
        assert_eq!(job.progress["current"], Value::from(3));
        assert_eq!(
            job.logs.last().and_then(|entry| entry.details.as_deref()),
            Some("SQLITE_CONSTRAINT")
        );
    }

    #[test]
    fn transfer_progress_matches_web_rounding_and_clamping() {
        assert_eq!(progress(2, 3, "Copying")["percentage"], Value::from(67));
        assert_eq!(progress(8, 4, "Copying")["current"], Value::from(4));
        assert_eq!(progress(0, 0, "Queued")["total"], Value::from(1));
    }

    #[test]
    fn mysql_vendor_codes_match_mysql2_symbolic_codes() {
        assert_eq!(normalize_sqlx_vendor_code("1045"), "ER_ACCESS_DENIED_ERROR");
        assert_eq!(normalize_sqlx_vendor_code("1049"), "ER_BAD_DB_ERROR");
        assert_eq!(normalize_sqlx_vendor_code("1062"), "ER_DUP_ENTRY");
        assert_eq!(normalize_sqlx_vendor_code("23505"), "23505");
    }
}
