use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const IOWB_RAG_PLUGIN_ABI_VERSION: u32 = 1;
pub const IOWB_RAG_PROTOCOL_VERSION: u32 = 1;
pub const IOWB_RAG_VERSION_SYMBOL: &[u8] = b"iowb_rag_plugin_version\0";
pub const IOWB_RAG_CALL_SYMBOL: &[u8] = b"iowb_rag_call\0";
pub const IOWB_RAG_FREE_SYMBOL: &[u8] = b"iowb_rag_free\0";

pub type IowbRagPluginVersionFn = unsafe extern "C" fn() -> u32;
pub type IowbRagPluginCallFn = unsafe extern "C" fn(*const u8, usize, *mut IowbRagBuffer) -> i32;
pub type IowbRagPluginFreeFn = unsafe extern "C" fn(*mut u8, usize, usize);

#[repr(C)]
#[derive(Debug, Default)]
pub struct IowbRagBuffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub cap: usize,
}

impl IowbRagBuffer {
    pub fn is_empty(&self) -> bool {
        self.ptr.is_null() || self.len == 0
    }
}

pub fn buffer_from_vec(bytes: Vec<u8>) -> IowbRagBuffer {
    let mut bytes = bytes;
    let buffer = IowbRagBuffer {
        ptr: bytes.as_mut_ptr(),
        len: bytes.len(),
        cap: bytes.capacity(),
    };
    std::mem::forget(bytes);
    buffer
}

/// # Safety
///
/// `ptr`, `len`, and `cap` must have been produced by [`buffer_from_vec`] in
/// the same dynamic library/allocation domain.
pub unsafe fn drop_buffer(ptr: *mut u8, len: usize, cap: usize) {
    if !ptr.is_null() {
        unsafe {
            drop(Vec::from_raw_parts(ptr, len, cap));
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRagPluginRequest {
    pub protocol_version: u32,
    pub command: NativeRagCommand,
    pub trace: NativeRagTraceContext,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRagPluginResponse {
    pub protocol_version: u32,
    pub ok: bool,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub error: Option<String>,
}

impl NativeRagPluginResponse {
    pub fn success(payload: impl Into<Value>) -> Self {
        Self {
            protocol_version: IOWB_RAG_PROTOCOL_VERSION,
            ok: true,
            payload: payload.into(),
            error: None,
        }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            protocol_version: IOWB_RAG_PROTOCOL_VERSION,
            ok: false,
            payload: Value::Null,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeRagCommand {
    Health,
    ProjectIndex,
    Query,
    TaskResult,
    ValidationError,
    PromotionCandidates,
    PromotionApprove,
}

impl NativeRagCommand {
    pub fn operation(self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::ProjectIndex => "project_index",
            Self::Query => "query",
            Self::TaskResult => "task_result",
            Self::ValidationError => "validation_error",
            Self::PromotionCandidates => "promotion_candidates",
            Self::PromotionApprove => "promotion_approve",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRagTraceContext {
    pub traceparent: String,
    pub project_id: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    pub operation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagQueryRequest {
    pub project_id: String,
    pub run_id: String,
    pub task_id: String,
    pub phase: String,
    pub query: String,
    #[serde(default)]
    pub known_files: Vec<String>,
    #[serde(default)]
    pub validation_error: Option<Value>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIndexRequest {
    pub project_id: String,
    pub project_path: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub include_globs: Vec<String>,
    #[serde(default)]
    pub exclude_globs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextRef {
    pub id: String,
    pub source: String,
    pub scope: String,
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagQueryResponse {
    #[serde(default)]
    pub context_refs: Vec<ContextRef>,
    #[serde(default)]
    pub global_standards: Vec<String>,
    #[serde(default)]
    pub project_patterns: Vec<String>,
    #[serde(default)]
    pub relevant_files: Vec<String>,
    #[serde(default)]
    pub test_conventions: Vec<String>,
    #[serde(default)]
    pub risk_notes: Vec<String>,
    #[serde(default)]
    pub prompt_context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResultIngestRequest {
    pub project_id: String,
    pub run_id: String,
    pub task_id: String,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub test_files: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub validation: Value,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationErrorIngestRequest {
    pub project_id: String,
    pub run_id: String,
    pub task_id: String,
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub validation: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotionCandidatesRequest {
    pub project_id: String,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotionApproveRequest {
    pub project_id: String,
    #[serde(default)]
    pub candidate_ids: Vec<String>,
    #[serde(default)]
    pub reviewer: String,
    #[serde(default)]
    pub notes: String,
}
