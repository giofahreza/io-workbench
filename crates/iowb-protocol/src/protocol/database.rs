#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SupportedDatabaseType {
    Postgresql,
    Mysql,
    Mariadb,
    Sqlite,
}

impl SupportedDatabaseType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Postgresql => "postgresql",
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
            Self::Sqlite => "sqlite",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseTestStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseObjectType {
    Connection,
    Database,
    Schema,
    Table,
    View,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseQueryStatementType {
    Select,
    Insert,
    Update,
    Delete,
    Ddl,
    Other,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseTransferMode {
    TableCopy,
    SchemaOnly,
    SchemaAndData,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseTransferJobStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConnectionInput {
    pub name: String,
    #[serde(rename = "type")]
    pub db_type: SupportedDatabaseType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "filePath", skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(rename = "showAllDatabases", default)]
    pub show_all_databases: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConnectionProfile {
    pub id: i64,
    pub name: String,
    #[serde(rename = "type")]
    pub db_type: SupportedDatabaseType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "filePath", skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(rename = "showAllDatabases")]
    pub show_all_databases: bool,
    #[serde(rename = "hasPassword")]
    pub has_password: bool,
    #[serde(rename = "lastTestStatus", skip_serializing_if = "Option::is_none")]
    pub last_test_status: Option<DatabaseTestStatus>,
    #[serde(rename = "lastTestMessage", skip_serializing_if = "Option::is_none")]
    pub last_test_message: Option<String>,
    #[serde(rename = "lastTestedAt", skip_serializing_if = "Option::is_none")]
    pub last_tested_at: Option<DateTime<Utc>>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTestConnectionRequest {
    #[serde(
        rename = "existingConnectionId",
        skip_serializing_if = "Option::is_none"
    )]
    pub existing_connection_id: Option<i64>,
    pub connection: DatabaseConnectionInput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTestResult {
    pub status: DatabaseTestStatus,
    pub message: String,
    #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseCapabilities {
    #[serde(rename = "supportsDatabases", default)]
    pub supports_databases: bool,
    #[serde(rename = "supportsSchemas", default)]
    pub supports_schemas: bool,
    #[serde(rename = "supportsViews", default)]
    pub supports_views: bool,
    #[serde(rename = "supportsIndexes", default)]
    pub supports_indexes: bool,
    #[serde(rename = "supportsMultipleDatabases", default)]
    pub supports_multiple_databases: bool,
    #[serde(rename = "supportsForeignKeys", default)]
    pub supports_foreign_keys: bool,
    #[serde(rename = "supportsParameterizedQueries", default)]
    pub supports_parameterized_queries: bool,
    #[serde(rename = "supportsOffset", default)]
    pub supports_offset: bool,
    #[serde(rename = "supportedObjectTypes", default)]
    pub supported_object_types: Vec<DatabaseObjectType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSessionInfo {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "connectionId")]
    pub connection_id: i64,
    #[serde(rename = "type")]
    pub db_type: SupportedDatabaseType,
    pub capabilities: DatabaseCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseExplorerNode {
    pub id: String,
    #[serde(rename = "type")]
    pub object_type: DatabaseObjectType,
    #[serde(rename = "connectionId")]
    pub connection_id: i64,
    pub name: String,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    #[serde(rename = "hasChildren")]
    pub has_children: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseNameSummary {
    pub name: String,
    #[serde(rename = "isDefault", default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseObjectSummary {
    pub name: String,
    #[serde(rename = "type")]
    pub object_type: DatabaseObjectType,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseObjectColumn {
    pub name: String,
    #[serde(rename = "dataType", skip_serializing_if = "Option::is_none")]
    pub data_type: Option<String>,
    #[serde(rename = "nativeType", skip_serializing_if = "Option::is_none")]
    pub native_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,
    #[serde(rename = "defaultValue", skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<String>,
    #[serde(rename = "isPrimaryKey", default)]
    pub is_primary_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseForeignKey {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "columnName")]
    pub column_name: String,
    #[serde(
        rename = "referencedSchemaName",
        skip_serializing_if = "Option::is_none"
    )]
    pub referenced_schema_name: Option<String>,
    #[serde(rename = "referencedTableName")]
    pub referenced_table_name: String,
    #[serde(rename = "referencedColumnName")]
    pub referenced_column_name: String,
    #[serde(rename = "onUpdate", skip_serializing_if = "Option::is_none")]
    pub on_update: Option<String>,
    #[serde(rename = "onDelete", skip_serializing_if = "Option::is_none")]
    pub on_delete: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseRelationalSchemaTable {
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub object_type: DatabaseObjectType,
    pub columns: Vec<DatabaseObjectColumn>,
    #[serde(rename = "isExternal", default)]
    pub is_external: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseRelationalSchemaRelationship {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "sourceDatabaseName", skip_serializing_if = "Option::is_none")]
    pub source_database_name: Option<String>,
    #[serde(rename = "sourceSchemaName", skip_serializing_if = "Option::is_none")]
    pub source_schema_name: Option<String>,
    #[serde(rename = "sourceTableName")]
    pub source_table_name: String,
    #[serde(rename = "sourceColumnName")]
    pub source_column_name: String,
    #[serde(rename = "targetDatabaseName", skip_serializing_if = "Option::is_none")]
    pub target_database_name: Option<String>,
    #[serde(rename = "targetSchemaName", skip_serializing_if = "Option::is_none")]
    pub target_schema_name: Option<String>,
    #[serde(rename = "targetTableName")]
    pub target_table_name: String,
    #[serde(rename = "targetColumnName")]
    pub target_column_name: String,
    #[serde(rename = "onUpdate", skip_serializing_if = "Option::is_none")]
    pub on_update: Option<String>,
    #[serde(rename = "onDelete", skip_serializing_if = "Option::is_none")]
    pub on_delete: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseRelationalSchema {
    #[serde(rename = "scopeType")]
    pub scope_type: DatabaseObjectType,
    #[serde(rename = "scopeName")]
    pub scope_name: String,
    pub tables: Vec<DatabaseRelationalSchemaTable>,
    pub relationships: Vec<DatabaseRelationalSchemaRelationship>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseObjectDetails {
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub object_type: DatabaseObjectType,
    #[serde(default)]
    pub columns: Vec<DatabaseObjectColumn>,
    #[serde(rename = "primaryKey", default)]
    pub primary_key: Vec<String>,
    #[serde(rename = "foreignKeys", default)]
    pub foreign_keys: Vec<DatabaseForeignKey>,
    #[serde(rename = "relationalSchema", skip_serializing_if = "Option::is_none")]
    pub relational_schema: Option<DatabaseRelationalSchema>,
    #[serde(default)]
    pub databases: Vec<DatabaseNameSummary>,
    #[serde(default)]
    pub schemas: Vec<DatabaseNameSummary>,
    #[serde(default)]
    pub objects: Vec<DatabaseObjectSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseQueryRequest {
    pub sql: String,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    #[serde(rename = "maxRows", skip_serializing_if = "Option::is_none")]
    pub max_rows: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseQueryResult {
    pub sql: String,
    #[serde(rename = "statementType")]
    pub statement_type: DatabaseQueryStatementType,
    #[serde(rename = "rowCount")]
    pub row_count: usize,
    #[serde(rename = "returnedRowCount")]
    pub returned_row_count: usize,
    #[serde(rename = "resultTruncated")]
    pub result_truncated: bool,
    #[serde(rename = "maxRows")]
    pub max_rows: usize,
    pub rows: Vec<serde_json::Map<String, Value>>,
    pub columns: Vec<DatabaseObjectColumn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
    #[serde(rename = "durationMs")]
    pub duration_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTableData {
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    #[serde(rename = "tableName")]
    pub table_name: String,
    pub offset: usize,
    pub limit: usize,
    #[serde(rename = "rowCount")]
    pub row_count: usize,
    #[serde(rename = "totalRowCount", skip_serializing_if = "Option::is_none")]
    pub total_row_count: Option<usize>,
    #[serde(rename = "exactTotalRowCount")]
    pub exact_total_row_count: bool,
    #[serde(rename = "hasMore")]
    pub has_more: bool,
    pub columns: Vec<DatabaseObjectColumn>,
    pub rows: Vec<serde_json::Map<String, Value>>,
    #[serde(rename = "durationMs")]
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferEndpoint {
    #[serde(rename = "connectionId")]
    pub connection_id: i64,
    #[serde(rename = "connectionName", skip_serializing_if = "Option::is_none")]
    pub connection_name: Option<String>,
    #[serde(rename = "connectionType", skip_serializing_if = "Option::is_none")]
    pub connection_type: Option<SupportedDatabaseType>,
    #[serde(rename = "databaseName", skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(rename = "schemaName", skip_serializing_if = "Option::is_none")]
    pub schema_name: Option<String>,
    #[serde(rename = "tableName")]
    pub table_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferRequest {
    pub mode: DatabaseTransferMode,
    pub source: DatabaseTransferEndpoint,
    pub target: DatabaseTransferEndpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferJobLogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferJobWarning {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferJobError {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferJobResult {
    #[serde(rename = "createdTable")]
    pub created_table: bool,
    #[serde(rename = "copiedRowCount")]
    pub copied_row_count: usize,
    #[serde(rename = "failedRowCount")]
    pub failed_row_count: usize,
    #[serde(rename = "ignoredSourceColumns")]
    pub ignored_source_columns: Vec<String>,
    #[serde(rename = "mappedColumnCount")]
    pub mapped_column_count: usize,
    #[serde(rename = "columnFailures")]
    pub column_failures: Vec<Value>,
    #[serde(rename = "rowFailures")]
    pub row_failures: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTransferJob {
    pub id: String,
    #[serde(rename = "type")]
    pub job_type: String,
    pub mode: DatabaseTransferMode,
    pub status: DatabaseTransferJobStatus,
    pub source: DatabaseTransferEndpoint,
    pub target: DatabaseTransferEndpoint,
    pub progress: Value,
    pub logs: Vec<DatabaseTransferJobLogEntry>,
    pub warnings: Vec<DatabaseTransferJobWarning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DatabaseTransferJobError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<DatabaseTransferJobResult>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
    #[serde(rename = "startedAt", skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(rename = "finishedAt", skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
}
