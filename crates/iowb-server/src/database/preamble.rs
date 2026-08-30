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
