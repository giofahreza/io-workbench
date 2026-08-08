use std::{env, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const RAG_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub(crate) struct RagClient {
    base_url: String,
    client: Client,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RagQueryRequest {
    pub(crate) project_id: String,
    pub(crate) run_id: String,
    pub(crate) task_id: String,
    pub(crate) phase: String,
    pub(crate) query: String,
    pub(crate) requirements: Vec<Value>,
    pub(crate) known_files: Vec<String>,
    pub(crate) validation_error: Option<Value>,
    pub(crate) scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectIndexRequest {
    pub(crate) project_id: String,
    pub(crate) project_path: String,
    pub(crate) run_id: Option<String>,
    pub(crate) include_globs: Vec<String>,
    pub(crate) exclude_globs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextRef {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) scope: String,
    #[serde(default)]
    pub(crate) score: f64,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RagQueryResponse {
    #[serde(default)]
    pub(crate) context_refs: Vec<ContextRef>,
    #[serde(default)]
    pub(crate) global_standards: Vec<String>,
    #[serde(default)]
    pub(crate) project_patterns: Vec<String>,
    #[serde(default)]
    pub(crate) relevant_files: Vec<String>,
    #[serde(default)]
    pub(crate) test_conventions: Vec<String>,
    #[serde(default)]
    pub(crate) risk_notes: Vec<String>,
    #[serde(default)]
    pub(crate) prompt_context: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskResultIngestRequest {
    pub(crate) project_id: String,
    pub(crate) run_id: String,
    pub(crate) task_id: String,
    pub(crate) requirements: Vec<Value>,
    pub(crate) changed_files: Vec<String>,
    pub(crate) test_files: Vec<String>,
    pub(crate) commands: Vec<String>,
    pub(crate) validation: Value,
    pub(crate) summary: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValidationErrorIngestRequest {
    pub(crate) project_id: String,
    pub(crate) run_id: String,
    pub(crate) task_id: String,
    pub(crate) phase: String,
    pub(crate) command: String,
    pub(crate) exit_code: Option<i64>,
    pub(crate) output: String,
    pub(crate) validation: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromotionCandidatesRequest {
    pub(crate) project_id: String,
    pub(crate) limit: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromotionApproveRequest {
    pub(crate) project_id: String,
    pub(crate) candidate_ids: Vec<String>,
    pub(crate) reviewer: String,
    pub(crate) notes: String,
}

impl RagClient {
    pub(crate) fn from_env() -> Option<Result<Self>> {
        let base_url = env::var("IO_WORKBENCH_RAG_URL").ok()?;
        Some(Self::new(base_url))
    }

    pub(crate) fn new(base_url: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(RAG_TIMEOUT)
            .build()
            .context("failed to build RAG HTTP client")?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        })
    }

    pub(crate) async fn query(&self, request: &RagQueryRequest) -> Result<RagQueryResponse> {
        self.post_json(
            "/rag/query",
            request,
            TraceContext::new(
                &request.project_id,
                Some(&request.run_id),
                Some(&request.task_id),
                "query",
            ),
        )
        .await
    }

    pub(crate) async fn index_project(&self, request: &ProjectIndexRequest) -> Result<Value> {
        self.post_json(
            "/rag/projects/index",
            request,
            TraceContext::new(
                &request.project_id,
                request.run_id.as_deref(),
                None,
                "project_index",
            ),
        )
        .await
    }

    pub(crate) async fn ingest_task_result(
        &self,
        request: &TaskResultIngestRequest,
    ) -> Result<Value> {
        self.post_json(
            "/rag/ingest/task-result",
            request,
            TraceContext::new(
                &request.project_id,
                Some(&request.run_id),
                Some(&request.task_id),
                "task_result",
            ),
        )
        .await
    }

    pub(crate) async fn ingest_validation_error(
        &self,
        request: &ValidationErrorIngestRequest,
    ) -> Result<Value> {
        self.post_json(
            "/rag/ingest/validation-error",
            request,
            TraceContext::new(
                &request.project_id,
                Some(&request.run_id),
                Some(&request.task_id),
                "validation_error",
            ),
        )
        .await
    }

    pub(crate) async fn promotion_candidates(
        &self,
        request: &PromotionCandidatesRequest,
    ) -> Result<Value> {
        self.post_json(
            "/rag/promote/candidates",
            request,
            TraceContext::new(&request.project_id, None, None, "promotion_candidates"),
        )
        .await
    }

    pub(crate) async fn approve_promotions(
        &self,
        request: &PromotionApproveRequest,
    ) -> Result<Value> {
        self.post_json(
            "/rag/promote/approve",
            request,
            TraceContext::new(&request.project_id, None, None, "promotion_approve"),
        )
        .await
    }

    async fn post_json<T, R>(
        &self,
        path: &str,
        request: &T,
        trace_context: TraceContext,
    ) -> Result<R>
    where
        T: Serialize + ?Sized,
        R: for<'de> Deserialize<'de>,
    {
        let response = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header("traceparent", trace_context.traceparent())
            .header("x-iowb-project-id", trace_context.project_id)
            .header("x-iowb-run-id", trace_context.run_id.unwrap_or_default())
            .header("x-iowb-task-id", trace_context.task_id.unwrap_or_default())
            .header("x-iowb-rag-operation", trace_context.operation)
            .json(request)
            .send()
            .await
            .with_context(|| format!("RAG request failed for {path}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("RAG request {path} returned {status}: {text}");
        }
        serde_json::from_str(&text).with_context(|| format!("invalid RAG response for {path}"))
    }
}

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
