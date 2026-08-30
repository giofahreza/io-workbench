async fn persist_run_attempt_usage(
    context: &AgentStartContext,
    status: iowb_protocol::SessionRuntimeStatus,
    usage: Option<&NormalizedRunUsage>,
) -> Option<SessionLifetimeTokenUsage> {
    let Some(attempt_id) = context.attempt_id.as_deref() else {
        return None;
    };
    let status = runtime_status_label(status);
    let (usage_value, raw, source, completeness) = if let Some(usage) = usage {
        (
            Some(&usage.usage),
            usage.raw_usage_json.as_deref(),
            Some(usage.source),
            usage.completeness,
        )
    } else {
        (
            None,
            None,
            Some("provider"),
            TokenUsageCompleteness::Missing,
        )
    };
    match context.storage.finish_chat_run_attempt(
        attempt_id,
        status,
        usage_value,
        raw,
        source,
        completeness,
    ) {
        Ok(lifetime) => lifetime,
        Err(error) => {
            warn!(
                error = %error,
                attempt_id,
                session_id = %context.session_id,
                "failed to persist chat run token usage"
            );
            None
        }
    }
}
