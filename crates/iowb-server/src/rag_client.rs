use std::{
    env,
    path::{Path, PathBuf},
    slice,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail, ensure};
use iowb_rag_plugin_api::{
    IOWB_RAG_CALL_SYMBOL, IOWB_RAG_FREE_SYMBOL, IOWB_RAG_PLUGIN_ABI_VERSION,
    IOWB_RAG_PROTOCOL_VERSION, IOWB_RAG_VERSION_SYMBOL, IowbRagBuffer, IowbRagPluginCallFn,
    IowbRagPluginFreeFn, IowbRagPluginVersionFn, NativeRagCommand, NativeRagPluginRequest,
    NativeRagPluginResponse, NativeRagTraceContext,
};
pub(crate) use iowb_rag_plugin_api::{
    ProjectIndexRequest, PromotionApproveRequest, PromotionCandidatesRequest, RagQueryRequest,
    RagQueryResponse, TaskResultIngestRequest, ValidationErrorIngestRequest,
};
use libloading::Library;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const RAG_MODE_ENV: &str = "IO_WORKBENCH_RAG_MODE";
const RAG_PLUGIN_ENV: &str = "IO_WORKBENCH_RAG_PLUGIN";
const RAG_PLUGIN_PATH_ENV: &str = "IO_WORKBENCH_RAG_PLUGIN_PATH";

#[derive(Clone)]
pub(crate) struct RagClient {
    plugin: NativeRagPlugin,
}

#[derive(Clone)]
struct NativeRagPlugin {
    inner: Arc<NativeRagPluginInner>,
}

struct NativeRagPluginInner {
    _library: Library,
    call: IowbRagPluginCallFn,
    free: IowbRagPluginFreeFn,
    path: PathBuf,
    call_lock: Mutex<()>,
}

unsafe impl Send for NativeRagPluginInner {}
unsafe impl Sync for NativeRagPluginInner {}

impl RagClient {
    pub(crate) fn from_env() -> Option<Result<Self>> {
        Some(match select_rag_plugin_from_env()? {
            Ok(path) => NativeRagPlugin::load(path).map(Self::native),
            Err(error) => Err(error),
        })
    }

    pub(crate) fn configured_descriptor() -> Option<String> {
        configured_descriptor_from_env()
    }

    pub(crate) fn descriptor(&self) -> String {
        format!("native-plugin:{}", self.plugin.inner.path.display())
    }

    fn native(plugin: NativeRagPlugin) -> Self {
        Self { plugin }
    }

    pub(crate) async fn query(&self, request: &RagQueryRequest) -> Result<RagQueryResponse> {
        self.post_json(
            NativeRagCommand::Query,
            request,
            TraceContext::new(
                &request.project_id,
                Some(&request.run_id),
                Some(&request.task_id),
                NativeRagCommand::Query.operation(),
            ),
        )
        .await
    }

    pub(crate) async fn index_project(&self, request: &ProjectIndexRequest) -> Result<Value> {
        self.post_json(
            NativeRagCommand::ProjectIndex,
            request,
            TraceContext::new(
                &request.project_id,
                request.run_id.as_deref(),
                None,
                NativeRagCommand::ProjectIndex.operation(),
            ),
        )
        .await
    }

    pub(crate) async fn ingest_task_result(
        &self,
        request: &TaskResultIngestRequest,
    ) -> Result<Value> {
        self.post_json(
            NativeRagCommand::TaskResult,
            request,
            TraceContext::new(
                &request.project_id,
                Some(&request.run_id),
                Some(&request.task_id),
                NativeRagCommand::TaskResult.operation(),
            ),
        )
        .await
    }

    pub(crate) async fn ingest_validation_error(
        &self,
        request: &ValidationErrorIngestRequest,
    ) -> Result<Value> {
        self.post_json(
            NativeRagCommand::ValidationError,
            request,
            TraceContext::new(
                &request.project_id,
                Some(&request.run_id),
                Some(&request.task_id),
                NativeRagCommand::ValidationError.operation(),
            ),
        )
        .await
    }

    pub(crate) async fn promotion_candidates(
        &self,
        request: &PromotionCandidatesRequest,
    ) -> Result<Value> {
        self.post_json(
            NativeRagCommand::PromotionCandidates,
            request,
            TraceContext::new(
                &request.project_id,
                None,
                None,
                NativeRagCommand::PromotionCandidates.operation(),
            ),
        )
        .await
    }

    pub(crate) async fn approve_promotions(
        &self,
        request: &PromotionApproveRequest,
    ) -> Result<Value> {
        self.post_json(
            NativeRagCommand::PromotionApprove,
            request,
            TraceContext::new(
                &request.project_id,
                None,
                None,
                NativeRagCommand::PromotionApprove.operation(),
            ),
        )
        .await
    }

    async fn post_json<T, R>(
        &self,
        command: NativeRagCommand,
        request: &T,
        trace_context: TraceContext,
    ) -> Result<R>
    where
        T: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        self.plugin.call(command, request, trace_context).await
    }
}

impl NativeRagPlugin {
    fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let library = unsafe {
            Library::new(&path)
                .with_context(|| format!("failed to load native RAG plugin {}", path.display()))?
        };
        let version = unsafe {
            *library
                .get::<IowbRagPluginVersionFn>(IOWB_RAG_VERSION_SYMBOL)
                .with_context(|| {
                    format!(
                        "native RAG plugin {} does not export iowb_rag_plugin_version",
                        path.display()
                    )
                })?
        };
        let abi_version = unsafe { version() };
        ensure!(
            abi_version == IOWB_RAG_PLUGIN_ABI_VERSION,
            "native RAG plugin {} ABI version {abi_version} is incompatible with host ABI {}",
            path.display(),
            IOWB_RAG_PLUGIN_ABI_VERSION
        );
        let call = unsafe {
            *library
                .get::<IowbRagPluginCallFn>(IOWB_RAG_CALL_SYMBOL)
                .with_context(|| {
                    format!(
                        "native RAG plugin {} does not export iowb_rag_call",
                        path.display()
                    )
                })?
        };
        let free = unsafe {
            *library
                .get::<IowbRagPluginFreeFn>(IOWB_RAG_FREE_SYMBOL)
                .with_context(|| {
                    format!(
                        "native RAG plugin {} does not export iowb_rag_free",
                        path.display()
                    )
                })?
        };
        Ok(Self {
            inner: Arc::new(NativeRagPluginInner {
                _library: library,
                call,
                free,
                path,
                call_lock: Mutex::new(()),
            }),
        })
    }

    async fn call<T, R>(
        &self,
        command: NativeRagCommand,
        request: &T,
        trace_context: TraceContext,
    ) -> Result<R>
    where
        T: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let request = NativeRagPluginRequest {
            protocol_version: IOWB_RAG_PROTOCOL_VERSION,
            command,
            trace: trace_context.native_trace_context(),
            payload: serde_json::to_value(request).context("failed to serialize RAG request")?,
        };
        let request_bytes =
            serde_json::to_vec(&request).context("failed to encode native RAG request")?;
        let plugin = self.clone();
        let response_bytes =
            tokio::task::spawn_blocking(move || plugin.call_blocking(command, &request_bytes))
                .await
                .context("native RAG plugin task failed")??;
        let response: NativeRagPluginResponse = serde_json::from_slice(&response_bytes)
            .with_context(|| {
                format!(
                    "native RAG plugin {} returned invalid JSON for {}",
                    self.inner.path.display(),
                    command.operation()
                )
            })?;
        ensure!(
            response.protocol_version == IOWB_RAG_PROTOCOL_VERSION,
            "native RAG plugin {} protocol version {} is incompatible with host protocol {}",
            self.inner.path.display(),
            response.protocol_version,
            IOWB_RAG_PROTOCOL_VERSION
        );
        if !response.ok {
            bail!(
                "native RAG plugin {} failed {}: {}",
                self.inner.path.display(),
                command.operation(),
                response
                    .error
                    .unwrap_or_else(|| "unknown error".to_string())
            );
        }
        serde_json::from_value(response.payload).with_context(|| {
            format!(
                "native RAG plugin {} returned invalid payload for {}",
                self.inner.path.display(),
                command.operation()
            )
        })
    }

    fn call_blocking(&self, command: NativeRagCommand, request: &[u8]) -> Result<Vec<u8>> {
        let _guard = self
            .inner
            .call_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("native RAG plugin call lock is poisoned"))?;
        let mut response = IowbRagBuffer::default();
        let status = unsafe {
            (self.inner.call)(
                request.as_ptr(),
                request.len(),
                &mut response as *mut IowbRagBuffer,
            )
        };
        let response_bytes = unsafe { self.take_response_buffer(response) };
        if status != 0 {
            let message = String::from_utf8_lossy(&response_bytes);
            bail!(
                "native RAG plugin {} returned status {status} for {}: {message}",
                self.inner.path.display(),
                command.operation()
            );
        }
        ensure!(
            !response_bytes.is_empty(),
            "native RAG plugin {} returned an empty response for {}",
            self.inner.path.display(),
            command.operation()
        );
        Ok(response_bytes)
    }

    unsafe fn take_response_buffer(&self, response: IowbRagBuffer) -> Vec<u8> {
        if response.is_empty() {
            return Vec::new();
        }
        let bytes = unsafe { slice::from_raw_parts(response.ptr, response.len).to_vec() };
        unsafe {
            (self.inner.free)(response.ptr, response.len, response.cap);
        }
        bytes
    }
}

fn select_rag_plugin_from_env() -> Option<Result<PathBuf>> {
    let mode = env::var(RAG_MODE_ENV)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    match mode.as_deref() {
        Some("off" | "false" | "disabled" | "none") => None,
        Some("native" | "native-plugin" | "plugin" | "edge") => Some(
            plugin_path_from_env().with_context(|| {
                format!(
                    "{RAG_MODE_ENV}=native-plugin requires {RAG_PLUGIN_ENV} or {RAG_PLUGIN_PATH_ENV}"
                )
            }),
        ),
        Some(other) => Some(Err(anyhow::anyhow!(
            "unsupported {RAG_MODE_ENV} value {other:?}; expected native-plugin or off"
        ))),
        None => plugin_path_from_env().map(Ok),
    }
}

fn plugin_path_from_env() -> Option<PathBuf> {
    env::var(RAG_PLUGIN_ENV)
        .ok()
        .or_else(|| env::var(RAG_PLUGIN_PATH_ENV).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn configured_descriptor_from_env() -> Option<String> {
    let mode = env::var(RAG_MODE_ENV)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    match mode.as_deref() {
        Some("off" | "false" | "disabled" | "none") => None,
        Some("native" | "native-plugin" | "plugin" | "edge") => Some(
            plugin_path_from_env()
                .map(|path| format!("native-plugin:{}", path.display()))
                .unwrap_or_else(|| {
                    format!("native-plugin:<missing {RAG_PLUGIN_ENV} or {RAG_PLUGIN_PATH_ENV}>")
                }),
        ),
        Some(other) => Some(format!("<unsupported {RAG_MODE_ENV}={other}>")),
        None => plugin_path_from_env().map(|path| format!("native-plugin:{}", path.display())),
    }
}

#[derive(Clone)]
struct TraceContext {
    project_id: String,
    run_id: Option<String>,
    task_id: Option<String>,
    operation: String,
}

impl TraceContext {
    fn new(project_id: &str, run_id: Option<&str>, task_id: Option<&str>, operation: &str) -> Self {
        Self {
            project_id: project_id.to_string(),
            run_id: run_id.map(str::to_string),
            task_id: task_id.map(str::to_string),
            operation: operation.to_string(),
        }
    }

    fn traceparent(&self) -> String {
        rag_traceparent(
            &self.project_id,
            self.run_id.as_deref(),
            self.task_id.as_deref(),
            &self.operation,
        )
    }

    fn native_trace_context(&self) -> NativeRagTraceContext {
        NativeRagTraceContext {
            traceparent: self.traceparent(),
            project_id: self.project_id.clone(),
            run_id: self.run_id.clone(),
            task_id: self.task_id.clone(),
            operation: self.operation.clone(),
        }
    }
}

pub(crate) fn rag_traceparent(
    project_id: &str,
    run_id: Option<&str>,
    task_id: Option<&str>,
    operation: &str,
) -> String {
    let seed = format!(
        "{}:{}:{}:{}",
        project_id,
        run_id.unwrap_or(""),
        task_id.unwrap_or(""),
        operation
    );
    let digest = Sha256::digest(seed.as_bytes());
    let trace_id = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut span_id = digest[16..24]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if span_id == "0000000000000000" {
        span_id = "0000000000000001".to_string();
    }
    format!("00-{trace_id}-{span_id}-01")
}

pub(crate) fn error_record(error: impl ToString) -> Value {
    json!({
        "error": error.to_string(),
    })
}
