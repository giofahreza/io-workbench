use std::{env, path::PathBuf, sync::Once, time::Instant};

use axum::{
    Extension, Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    routing::{get, post, put},
};
use chrono::Utc;
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
use rusqlite::{
    Connection, OptionalExtension, params, params_from_iter,
    types::{Value as SqlValue, ValueRef},
};
use serde::Deserialize;
use serde_json::{Map, Value};
use sqlx::{
    AnyPool, Column, Executor, Row, TypeInfo,
    any::{AnyPoolOptions, AnyRow},
};

use crate::{AuthenticatedUser, Result, ServerError};

const DEFAULT_QUERY_MAX_ROWS: usize = 1000;
const MAX_QUERY_MAX_ROWS: usize = 5000;
const DEFAULT_TABLE_PAGE_SIZE: usize = 50;
const MAX_TABLE_PAGE_SIZE: usize = 500;
const DEFAULT_TRANSFER_MAX_ROWS: usize = 10_000;
const MAX_TRANSFER_MAX_ROWS: usize = 100_000;
static SQLX_ANY_DRIVERS: Once = Once::new();

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
) -> Result<Json<Value>> {
    let existing_password = if let Some(connection_id) = request.existing_connection_id {
        state
            .storage
            .get_database_connection(&user.0.id, connection_id)?
            .and_then(|connection| connection.password)
    } else {
        None
    };
    let input = normalize_connection_input(&state, request.connection, existing_password).await?;
    let result = test_connection_input(&input).await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "result": result,
    })))
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
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn test_saved_connection(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    AxumPath(connection_id): AxumPath<i64>,
) -> Result<Json<Value>> {
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
            })))
        }
        Err(error) => {
            let message = error
                .body
                .details
                .clone()
                .unwrap_or(error.body.error.clone());
            let updated = state.storage.record_database_connection_test(
                &user.0.id,
                connection_id,
                DatabaseTestStatus::Error,
                &message,
            )?;
            Err(ServerError::with_details(
                StatusCode::BAD_REQUEST,
                "connection test failed",
                serde_json::json!({
                    "message": message,
                    "connection": updated,
                    "result": {
                        "status": "error",
                        "message": message
                    }
                })
                .to_string(),
            ))
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
) -> Result<Json<Value>> {
    let sql = request.sql.trim();
    if sql.is_empty() {
        return Err(ServerError::new(StatusCode::BAD_REQUEST, "SQL is required"));
    }

    let connection = load_connection(&state, &user.0.id, connection_id)?;
    let max_rows = request
        .max_rows
        .filter(|value| *value > 0)
        .map(|value| value.min(MAX_QUERY_MAX_ROWS))
        .unwrap_or(DEFAULT_QUERY_MAX_ROWS);
    let mut result = if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        execute_sqlite_query(&connection, sql, max_rows)?
    } else {
        execute_live_query(&connection, sql, max_rows).await?
    };
    result.database_name = request.database_name.or_else(|| Some("main".to_string()));
    result.schema_name = request.schema_name;

    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "context": {
            "databaseName": result.database_name,
            "schemaName": result.schema_name,
        },
        "result": result,
    })))
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
    data.database_name = query.database_name.or_else(|| Some("main".to_string()));
    data.schema_name = query.schema_name;

    Ok(Json(serde_json::json!({
        "success": true,
        "session": database_session(&connection.profile),
        "data": data,
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

    let source_connection = load_connection(&state, &user.0.id, request.source.connection_id)?;
    let target_connection = load_connection(&state, &user.0.id, request.target.connection_id)?;

    let now = Utc::now();
    let mut job = DatabaseTransferJob {
        id: new_id("dbjob"),
        job_type: "table-transfer".to_string(),
        mode: request.mode,
        status: DatabaseTransferJobStatus::Running,
        source: enrich_transfer_endpoint(request.source, &source_connection.profile),
        target: enrich_transfer_endpoint(request.target, &target_connection.profile),
        progress: progress(0, 1, "Starting transfer"),
        logs: vec![DatabaseTransferJobLogEntry {
            timestamp: now,
            level: "info".to_string(),
            message: "Transfer job created".to_string(),
            details: None,
        }],
        warnings: Vec::new(),
        error: None,
        result: None,
        created_at: now,
        updated_at: now,
        started_at: Some(now),
        finished_at: None,
    };

    match run_database_transfer(
        &source_connection,
        &target_connection,
        &job.source,
        &job.target,
        job.mode,
    )
    .await
    {
        Ok((result, warnings)) => {
            let finished_at = Utc::now();
            job.status = DatabaseTransferJobStatus::Succeeded;
            job.progress = progress(1, 1, "Transfer completed");
            job.warnings = warnings;
            job.logs.push(DatabaseTransferJobLogEntry {
                timestamp: finished_at,
                level: "info".to_string(),
                message: "Transfer completed".to_string(),
                details: None,
            });
            job.updated_at = finished_at;
            job.finished_at = Some(finished_at);
            job.result = Some(result);
        }
        Err(error) => {
            let finished_at = Utc::now();
            let message = error.body.details.clone().unwrap_or(error.body.error);
            job.status = DatabaseTransferJobStatus::Failed;
            job.progress = progress(0, 1, "Transfer failed");
            job.logs.push(DatabaseTransferJobLogEntry {
                timestamp: finished_at,
                level: "error".to_string(),
                message: message.clone(),
                details: None,
            });
            job.error = Some(DatabaseTransferJobError {
                message,
                code: None,
                category: Some("transfer".to_string()),
                retryable: false,
                details: None,
            });
            job.updated_at = finished_at;
            job.finished_at = Some(finished_at);
        }
    }

    state
        .storage
        .upsert_database_transfer_job(&user.0.id, &job)?;

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "success": true,
            "job": job,
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
                "truncated": snapshot.truncated,
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
            if snapshot.truncated {
                job.warnings.push(DatabaseTransferJobWarning {
                    message: format!(
                        "Export wrote the first {} rows; increase IO_WORKBENCH_DATABASE_TRANSFER_MAX_ROWS for larger exports",
                        snapshot.rows.len()
                    ),
                    scope: Some(endpoint.table_name.clone()),
                    code: Some("row-limit".to_string()),
                });
            }
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
    let message = error.body.details.clone().unwrap_or(error.body.error);
    job.status = DatabaseTransferJobStatus::Failed;
    job.progress = progress(0, 1, "Job failed");
    job.logs.push(DatabaseTransferJobLogEntry {
        timestamp: finished_at,
        level: "error".to_string(),
        message: message.clone(),
        details: None,
    });
    job.error = Some(DatabaseTransferJobError {
        message,
        code: None,
        category: Some(job.job_type.clone()),
        retryable: false,
        details: None,
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
    let max_rows = transfer_max_rows();
    if rows.len() > max_rows {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "import has {} rows, exceeding IO_WORKBENCH_DATABASE_TRANSFER_MAX_ROWS ({max_rows})",
                rows.len()
            ),
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_server_error)?;
    }
    let conn = Connection::open(path).map_err(sqlite_server_error)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(sqlite_server_error)?;
    let version: String = conn
        .query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .map_err(sqlite_server_error)?;
    Ok(DatabaseTestResult {
        status: DatabaseTestStatus::Success,
        message: format!("SQLite connection successful ({version})"),
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
            supports_schemas: true,
            supports_multiple_databases: true,
            supports_foreign_keys: true,
        },
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => DatabaseCapabilities {
            supports_schemas: false,
            supports_multiple_databases: true,
            supports_foreign_keys: true,
        },
    }
}

fn sqlite_capabilities() -> DatabaseCapabilities {
    DatabaseCapabilities {
        supports_schemas: false,
        supports_multiple_databases: false,
        supports_foreign_keys: true,
    }
}

async fn test_live_connection(connection: &StoredDatabaseConnection) -> Result<DatabaseTestResult> {
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
    let name = query
        .name
        .clone()
        .unwrap_or_else(|| default_database_name(&connection));
    let details = match object_type {
        DatabaseObjectType::Database | DatabaseObjectType::Schema => {
            let objects = live_objects(
                &connection,
                query.database_name.as_deref(),
                query.schema_name.as_deref(),
            )
            .await?;
            let relational_schema = if query.include_relational.unwrap_or(false) {
                Some(
                    live_relational_schema(
                        &connection,
                        &name,
                        query.schema_name.as_deref(),
                        &objects,
                    )
                    .await?,
                )
            } else {
                None
            };
            DatabaseObjectDetails {
                database_name: query
                    .database_name
                    .or_else(|| Some(default_database_name(&connection))),
                schema_name: query.schema_name,
                name,
                object_type,
                columns: Vec::new(),
                primary_key: Vec::new(),
                foreign_keys: Vec::new(),
                relational_schema,
                databases: live_databases(&connection).await?,
                schemas: live_schemas(&connection, None).await?,
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
                query.database_name.as_deref(),
                query.schema_name.as_deref(),
                table_name,
                object_type,
            )
            .await?;
            DatabaseObjectDetails {
                database_name: query
                    .database_name
                    .or_else(|| Some(default_database_name(&connection))),
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

struct LiveTableDescription {
    columns: Vec<DatabaseObjectColumn>,
    foreign_keys: Vec<DatabaseForeignKey>,
}

async fn live_pool(connection: &StoredDatabaseConnection) -> Result<AnyPool> {
    SQLX_ANY_DRIVERS.call_once(sqlx::any::install_default_drivers);
    AnyPoolOptions::new()
        .max_connections(4)
        .connect(&connection_url(connection)?)
        .await
        .map_err(sqlx_server_error)
}

fn connection_url(connection: &StoredDatabaseConnection) -> Result<String> {
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
    let database = connection
        .profile
        .database_name
        .as_deref()
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
    let pool = live_pool(connection).await?;
    let current = default_database_name(connection);
    let sql = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
            "SELECT datname AS name FROM pg_database WHERE datistemplate = false ORDER BY datname"
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
    let pool = live_pool(connection).await?;
    let _ = database_name;
    let rows = sqlx::query(
        r#"
        SELECT schema_name
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
    let pool = live_pool(connection).await?;
    match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
            let schema = schema_name.unwrap_or("public");
            let rows = sqlx::query(
                r#"
                SELECT table_name, table_type
                FROM information_schema.tables
                WHERE table_schema = ?
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
                        database_name: Some(default_database_name(connection)),
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
    let pool = live_pool(connection).await?;
    let expected_table_types: &[&str] = match object_type {
        DatabaseObjectType::View => &["VIEW"],
        _ => &["BASE TABLE"],
    };

    let (database, schema) = live_scope(connection, database_name, schema_name);
    let columns_sql = match connection.profile.db_type {
        SupportedDatabaseType::Postgresql => {
            r#"
            SELECT column_name, data_type, udt_name, is_nullable, column_default
            FROM information_schema.columns
            WHERE table_schema = ? AND table_name = ?
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

    let table_type = live_table_type(&pool, &schema, table_name).await?;
    if !expected_table_types.contains(&table_type.as_str()) {
        return Err(ServerError::new(StatusCode::NOT_FOUND, "Table not found"));
    }

    let _ = database;
    Ok(LiveTableDescription {
        columns,
        foreign_keys,
    })
}

async fn live_table_type(pool: &AnyPool, schema: &str, table_name: &str) -> Result<String> {
    let row = sqlx::query(
        r#"
        SELECT table_type
        FROM information_schema.tables
        WHERE table_schema = ? AND table_name = ?
        "#,
    )
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
            SELECT kcu.column_name
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
              ON tc.constraint_name = kcu.constraint_name
             AND tc.table_schema = kcu.table_schema
            WHERE tc.constraint_type = 'PRIMARY KEY'
              AND tc.table_schema = ?
              AND tc.table_name = ?
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
              tc.constraint_name,
              kcu.column_name,
              ccu.table_schema AS referenced_table_schema,
              ccu.table_name AS referenced_table_name,
              ccu.column_name AS referenced_column_name
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
              ON tc.constraint_name = kcu.constraint_name
             AND tc.table_schema = kcu.table_schema
            JOIN information_schema.constraint_column_usage ccu
              ON ccu.constraint_name = tc.constraint_name
             AND ccu.table_schema = tc.table_schema
            WHERE tc.constraint_type = 'FOREIGN KEY'
              AND tc.table_schema = ?
              AND tc.table_name = ?
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
        scope_type: DatabaseObjectType::Database,
        scope_name: scope_name.to_string(),
        tables,
        relationships,
    })
}

async fn execute_live_query(
    connection: &StoredDatabaseConnection,
    sql: &str,
    max_rows: usize,
) -> Result<DatabaseQueryResult> {
    let start = Instant::now();
    let pool = live_pool(connection).await?;
    let statement_type = classify_statement(sql);
    if matches!(
        statement_type,
        DatabaseQueryStatementType::Select | DatabaseQueryStatementType::Other
    ) {
        let rows = sqlx::query(sql)
            .fetch_all(&pool)
            .await
            .map_err(sqlx_server_error)?;
        let columns = rows.first().map(any_columns).unwrap_or_default();
        let result_truncated = rows.len() > max_rows;
        let output = rows
            .into_iter()
            .take(max_rows)
            .map(|row| any_row_to_json_map(&row))
            .collect::<Vec<_>>();
        return Ok(DatabaseQueryResult {
            sql: sql.to_string(),
            statement_type,
            row_count: output.len(),
            returned_row_count: output.len(),
            result_truncated,
            max_rows,
            rows: output,
            columns,
            notices: Vec::new(),
            duration_ms: start.elapsed().as_millis(),
            meta: None,
            database_name: None,
            schema_name: None,
        });
    }

    let done = pool.execute(sql).await.map_err(sqlx_server_error)?;
    let affected = usize::try_from(done.rows_affected()).unwrap_or(usize::MAX);
    Ok(DatabaseQueryResult {
        sql: sql.to_string(),
        statement_type,
        row_count: affected,
        returned_row_count: 0,
        result_truncated: false,
        max_rows,
        rows: Vec::new(),
        columns: Vec::new(),
        notices: Vec::new(),
        duration_ms: start.elapsed().as_millis(),
        meta: Some(serde_json::json!({ "changedRows": affected })),
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
    let pool = live_pool(connection).await?;
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
    let rows = sqlx::query(&format!("SELECT * FROM {table_ref} LIMIT ? OFFSET ?"))
        .bind(i64::try_from(limit + 1).unwrap_or(i64::MAX))
        .bind(i64::try_from(offset).unwrap_or(i64::MAX))
        .fetch_all(&pool)
        .await
        .map_err(sqlx_server_error)?;
    let total_row_count = if include_total_count {
        let row = sqlx::query(&format!("SELECT COUNT(*) FROM {table_ref}"))
            .fetch_one(&pool)
            .await
            .map_err(sqlx_server_error)?;
        any_i64(&row, 0).map(|count| count.max(0) as usize)
    } else {
        None
    };
    let mut output = rows
        .into_iter()
        .map(|row| any_row_to_json_map(&row))
        .collect::<Vec<_>>();
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

fn any_columns(row: &AnyRow) -> Vec<DatabaseObjectColumn> {
    row.columns()
        .iter()
        .map(|column| DatabaseObjectColumn {
            name: column.name().to_string(),
            data_type: Some(column.type_info().name().to_string()),
            native_type: Some(column.type_info().name().to_string()),
            nullable: None,
            default_value: None,
            extra: None,
            is_primary_key: false,
        })
        .collect()
}

fn any_row_to_json_map(row: &AnyRow) -> Map<String, Value> {
    let mut item = Map::new();
    for (index, column) in row.columns().iter().enumerate() {
        item.insert(column.name().to_string(), any_value_to_json(row, index));
    }
    item
}

fn any_value_to_json(row: &AnyRow, index: usize) -> Value {
    if let Ok(value) = row.try_get::<Option<String>, _>(index) {
        return value.map(Value::String).unwrap_or(Value::Null);
    }
    if let Ok(value) = row.try_get::<Option<i64>, _>(index) {
        return value
            .map(|value| Value::Number(value.into()))
            .unwrap_or(Value::Null);
    }
    if let Ok(value) = row.try_get::<Option<i32>, _>(index) {
        return value
            .map(|value| Value::Number(value.into()))
            .unwrap_or(Value::Null);
    }
    if let Ok(value) = row.try_get::<Option<i16>, _>(index) {
        return value
            .map(|value| Value::Number(value.into()))
            .unwrap_or(Value::Null);
    }
    if let Ok(value) = row.try_get::<Option<f64>, _>(index) {
        return value
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or(Value::Null);
    }
    if let Ok(value) = row.try_get::<Option<f32>, _>(index) {
        return value
            .and_then(|value| serde_json::Number::from_f64(f64::from(value)))
            .map(Value::Number)
            .unwrap_or(Value::Null);
    }
    if let Ok(value) = row.try_get::<Option<bool>, _>(index) {
        return value.map(Value::Bool).unwrap_or(Value::Null);
    }
    Value::String("<unsupported>".to_string())
}

fn any_string(row: &AnyRow, index: usize) -> Option<String> {
    row.try_get::<Option<String>, _>(index).ok().flatten()
}

fn any_i64(row: &AnyRow, index: usize) -> Option<i64> {
    row.try_get::<Option<i64>, _>(index)
        .ok()
        .flatten()
        .or_else(|| {
            row.try_get::<Option<i32>, _>(index)
                .ok()
                .flatten()
                .map(i64::from)
        })
}

fn sqlite_connection(connection: &StoredDatabaseConnection) -> Result<Connection> {
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
            meta: Some(serde_json::json!({ "changedRows": changed })),
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
    while let Some(row) = rows.next().map_err(sqlite_server_error)? {
        if output.len() >= max_rows {
            return Ok(DatabaseQueryResult {
                sql: sql.to_string(),
                statement_type,
                row_count: output.len(),
                returned_row_count: output.len(),
                result_truncated: true,
                max_rows,
                rows: output,
                columns,
                notices: Vec::new(),
                duration_ms: start.elapsed().as_millis(),
                meta: None,
                database_name: None,
                schema_name: None,
            });
        }
        output.push(row_to_json_map(row, &column_names)?);
    }

    Ok(DatabaseQueryResult {
        sql: sql.to_string(),
        statement_type,
        row_count: output.len(),
        returned_row_count: output.len(),
        result_truncated: false,
        max_rows,
        rows: output,
        columns,
        notices: Vec::new(),
        duration_ms: start.elapsed().as_millis(),
        meta: None,
        database_name: None,
        schema_name: None,
    })
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
    truncated: bool,
}

async fn run_database_transfer(
    source_connection: &StoredDatabaseConnection,
    target_connection: &StoredDatabaseConnection,
    source: &DatabaseTransferEndpoint,
    target: &DatabaseTransferEndpoint,
    mode: DatabaseTransferMode,
) -> Result<(DatabaseTransferJobResult, Vec<DatabaseTransferJobWarning>)> {
    if source_connection.profile.db_type == SupportedDatabaseType::Sqlite
        && target_connection.profile.db_type == SupportedDatabaseType::Sqlite
    {
        return run_sqlite_transfer(source_connection, target_connection, source, target, mode)
            .map(|result| (result, Vec::new()));
    }

    let snapshot = read_transfer_source(source_connection, source).await?;
    if snapshot.columns.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Source table has no columns",
        ));
    }

    let target_exists = transfer_target_exists(target_connection, target).await?;
    if !target_exists {
        create_transfer_target_table(target_connection, target, &snapshot.columns).await?;
    }

    let copied_row_count = if mode == DatabaseTransferMode::SchemaOnly {
        0
    } else {
        insert_transfer_rows(target_connection, target, &snapshot.columns, &snapshot.rows).await?
    };

    let mut warnings = Vec::new();
    if snapshot.truncated {
        warnings.push(DatabaseTransferJobWarning {
            message: format!(
                "Transfer copied the first {} rows; increase IO_WORKBENCH_DATABASE_TRANSFER_MAX_ROWS for larger copies",
                snapshot.rows.len()
            ),
            scope: Some(source.table_name.clone()),
            code: Some("row-limit".to_string()),
        });
    }

    Ok((
        DatabaseTransferJobResult {
            created_table: !target_exists,
            copied_row_count,
            failed_row_count: 0,
            ignored_source_columns: Vec::new(),
            mapped_column_count: snapshot.columns.len(),
            column_failures: Vec::new(),
            row_failures: Vec::new(),
        },
        warnings,
    ))
}

async fn read_transfer_source(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
) -> Result<TransferSourceSnapshot> {
    let max_rows = transfer_max_rows();
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_connection(connection)?;
        let table =
            describe_sqlite_table_inner(&conn, &endpoint.table_name, DatabaseObjectType::Table)?;
        let sql = format!(
            "SELECT * FROM {} LIMIT ?1",
            quote_identifier(&endpoint.table_name)
        );
        let mut stmt = conn.prepare(&sql).map_err(sqlite_server_error)?;
        let column_names = stmt
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut rows = stmt
            .query(params![i64::try_from(max_rows + 1).unwrap_or(i64::MAX)])
            .map_err(sqlite_server_error)?;
        let mut output = Vec::new();
        while let Some(row) = rows.next().map_err(sqlite_server_error)? {
            output.push(row_to_json_map(row, &column_names)?);
            if output.len() > max_rows {
                break;
            }
        }
        let truncated = output.len() > max_rows;
        if truncated {
            output.truncate(max_rows);
        }
        return Ok(TransferSourceSnapshot {
            columns: table.columns,
            rows: output,
            truncated,
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
    let data = read_live_table_data(
        connection,
        endpoint.database_name.as_deref(),
        endpoint.schema_name.as_deref(),
        &endpoint.table_name,
        max_rows + 1,
        0,
        false,
    )
    .await?;
    let mut rows = data.rows;
    let truncated = rows.len() > max_rows;
    if truncated {
        rows.truncate(max_rows);
    }
    Ok(TransferSourceSnapshot {
        columns: table.columns,
        rows,
        truncated,
    })
}

async fn transfer_target_exists(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
) -> Result<bool> {
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_connection(connection)?;
        return sqlite_table_exists(&conn, &endpoint.table_name);
    }

    let pool = live_pool(connection).await?;
    let (_database, schema) = live_scope(
        connection,
        endpoint.database_name.as_deref(),
        endpoint.schema_name.as_deref(),
    );
    let row = sqlx::query(
        r#"
        SELECT 1
        FROM information_schema.tables
        WHERE table_schema = ? AND table_name = ?
        "#,
    )
    .bind(&schema)
    .bind(&endpoint.table_name)
    .fetch_optional(&pool)
    .await
    .map_err(sqlx_server_error)?;
    Ok(row.is_some())
}

async fn create_transfer_target_table(
    connection: &StoredDatabaseConnection,
    endpoint: &DatabaseTransferEndpoint,
    columns: &[DatabaseObjectColumn],
) -> Result<()> {
    let sql = build_transfer_create_table_sql(connection.profile.db_type, endpoint, columns);
    if connection.profile.db_type == SupportedDatabaseType::Sqlite {
        let conn = sqlite_connection(connection)?;
        conn.execute_batch(&sql).map_err(sqlite_server_error)?;
        return Ok(());
    }

    let pool = live_pool(connection).await?;
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
        let mut conn = sqlite_connection(connection)?;
        let tx = conn.transaction().map_err(sqlite_server_error)?;
        for row in rows {
            let sql = build_transfer_insert_sql(connection.profile.db_type, endpoint, columns, row);
            tx.execute(&sql, []).map_err(sqlite_server_error)?;
        }
        tx.commit().map_err(sqlite_server_error)?;
        return Ok(rows.len());
    }

    let pool = live_pool(connection).await?;
    for row in rows {
        let sql = build_transfer_insert_sql(connection.profile.db_type, endpoint, columns, row);
        pool.execute(sql.as_str())
            .await
            .map_err(sqlx_server_error)?;
    }
    Ok(rows.len())
}

fn transfer_max_rows() -> usize {
    env::var("IO_WORKBENCH_DATABASE_TRANSFER_MAX_ROWS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_TRANSFER_MAX_ROWS)
        .clamp(1, MAX_TRANSFER_MAX_ROWS)
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
                .map(transfer_value_literal)
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
            if native.contains("int") {
                "INTEGER"
            } else if native.contains("real")
                || native.contains("float")
                || native.contains("double")
                || native.contains("decimal")
                || native.contains("numeric")
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
            } else if native.contains("int") {
                "BIGINT"
            } else if native.contains("real")
                || native.contains("float")
                || native.contains("double")
                || native.contains("decimal")
                || native.contains("numeric")
            {
                "DOUBLE PRECISION"
            } else {
                "TEXT"
            }
        }
        SupportedDatabaseType::Mysql | SupportedDatabaseType::Mariadb => {
            if native.contains("bool") {
                "BOOLEAN"
            } else if native.contains("int") {
                "BIGINT"
            } else if native.contains("real")
                || native.contains("float")
                || native.contains("double")
                || native.contains("decimal")
                || native.contains("numeric")
            {
                "DOUBLE"
            } else {
                "TEXT"
            }
        }
    }
}

fn transfer_value_literal(value: &Value) -> String {
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
        Value::String(value) => format!("'{}'", value.replace('\'', "''")),
        Value::Array(_) | Value::Object(_) => {
            format!(
                "'{}'",
                serde_json::to_string(value)
                    .unwrap_or_default()
                    .replace('\'', "''")
            )
        }
    }
}

fn run_sqlite_transfer(
    source_connection: &StoredDatabaseConnection,
    target_connection: &StoredDatabaseConnection,
    source: &DatabaseTransferEndpoint,
    target: &DatabaseTransferEndpoint,
    mode: DatabaseTransferMode,
) -> Result<DatabaseTransferJobResult> {
    let source_conn = sqlite_connection(source_connection)?;
    let mut target_conn = sqlite_connection(target_connection)?;
    let source_table =
        describe_sqlite_table_inner(&source_conn, &source.table_name, DatabaseObjectType::Table)?;
    if source_table.columns.is_empty() {
        return Err(ServerError::new(
            StatusCode::BAD_REQUEST,
            "Source table has no columns",
        ));
    }

    let target_exists = sqlite_table_exists(&target_conn, &target.table_name)?;
    if !target_exists {
        let create_sql = build_create_table_sql(&target.table_name, &source_table.columns);
        target_conn
            .execute_batch(&create_sql)
            .map_err(sqlite_server_error)?;
    }

    if mode == DatabaseTransferMode::SchemaOnly {
        return Ok(DatabaseTransferJobResult {
            created_table: !target_exists,
            copied_row_count: 0,
            failed_row_count: 0,
            ignored_source_columns: Vec::new(),
            mapped_column_count: source_table.columns.len(),
            column_failures: Vec::new(),
            row_failures: Vec::new(),
        });
    }

    let column_names = source_table
        .columns
        .iter()
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    let select_sql = format!("SELECT * FROM {}", quote_identifier(&source.table_name));
    let mut select_stmt = source_conn
        .prepare(&select_sql)
        .map_err(sqlite_server_error)?;
    let insert_sql = build_insert_sql(&target.table_name, &column_names);
    let tx = target_conn.transaction().map_err(sqlite_server_error)?;
    let copied_row_count = {
        let mut insert_stmt = tx.prepare(&insert_sql).map_err(sqlite_server_error)?;
        let mut rows = select_stmt.query([]).map_err(sqlite_server_error)?;
        let mut copied_row_count = 0;
        while let Some(row) = rows.next().map_err(sqlite_server_error)? {
            let values = (0..column_names.len())
                .map(|index| row.get::<_, SqlValue>(index))
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(sqlite_server_error)?;
            insert_stmt
                .execute(params_from_iter(values.iter()))
                .map_err(sqlite_server_error)?;
            copied_row_count += 1;
        }
        copied_row_count
    };
    tx.commit().map_err(sqlite_server_error)?;

    Ok(DatabaseTransferJobResult {
        created_table: !target_exists,
        copied_row_count,
        failed_row_count: 0,
        ignored_source_columns: Vec::new(),
        mapped_column_count: column_names.len(),
        column_failures: Vec::new(),
        row_failures: Vec::new(),
    })
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

fn build_create_table_sql(table_name: &str, columns: &[DatabaseObjectColumn]) -> String {
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
            quote_identifier(&column.name),
            column
                .native_type
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("TEXT")
        );
        if single_primary_key && column.is_primary_key {
            definition.push_str(" PRIMARY KEY");
        }
        if column.nullable == Some(false) && !(single_primary_key && column.is_primary_key) {
            definition.push_str(" NOT NULL");
        }
        if let Some(default_value) = column
            .default_value
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            definition.push_str(" DEFAULT ");
            definition.push_str(default_value);
        }
        definitions.push(definition);
    }
    if primary_key_columns.len() > 1 {
        definitions.push(format!(
            "PRIMARY KEY ({})",
            primary_key_columns
                .iter()
                .map(|column| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    format!(
        "CREATE TABLE IF NOT EXISTS {} ({})",
        quote_identifier(table_name),
        definitions.join(", ")
    )
}

fn build_insert_sql(table_name: &str, column_names: &[String]) -> String {
    let columns = column_names
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders = (0..column_names.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {} ({columns}) VALUES ({placeholders})",
        quote_identifier(table_name)
    )
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
        ValueRef::Blob(value) => Value::String(format!("<blob {} bytes>", value.len())),
    }
}

fn classify_statement(sql: &str) -> DatabaseQueryStatementType {
    let keyword = sql
        .trim_start()
        .split(|ch: char| ch.is_whitespace() || ch == ';' || ch == '(')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match keyword.as_str() {
        "select" | "with" | "pragma" => DatabaseQueryStatementType::Select,
        "insert" => DatabaseQueryStatementType::Insert,
        "update" => DatabaseQueryStatementType::Update,
        "delete" => DatabaseQueryStatementType::Delete,
        "create" | "alter" | "drop" | "truncate" | "reindex" | "vacuum" => {
            DatabaseQueryStatementType::Ddl
        }
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
    endpoint.database_name = endpoint.database_name.or_else(|| Some("main".to_string()));
    endpoint
}

fn progress(current: usize, total: usize, message: &str) -> Value {
    let percentage = if total == 0 {
        0.0
    } else {
        (current as f64 / total as f64) * 100.0
    };
    serde_json::json!({
        "current": current,
        "total": total,
        "percentage": percentage,
        "message": message,
    })
}

fn sqlite_server_error(error: rusqlite::Error) -> ServerError {
    ServerError::with_details(StatusCode::BAD_REQUEST, "database error", error.to_string())
}

fn sqlx_server_error(error: sqlx::Error) -> ServerError {
    ServerError::with_details(StatusCode::BAD_REQUEST, "database error", error.to_string())
}

fn io_server_error(error: std::io::Error) -> ServerError {
    ServerError::with_details(
        StatusCode::BAD_REQUEST,
        "database filesystem error",
        error.to_string(),
    )
}
