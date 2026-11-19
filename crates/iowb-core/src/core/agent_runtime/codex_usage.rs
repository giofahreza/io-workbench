struct CodexTurnUsageTracker {
    previous_total: Option<SessionTokenUsage>,
    accumulated: SessionTokenUsage,
    incomplete: bool,
    has_usage: bool,
    has_cumulative_usage: bool,
}

impl Default for CodexTurnUsageTracker {
    fn default() -> Self {
        Self {
            previous_total: Some(SessionTokenUsage::default()),
            accumulated: SessionTokenUsage::default(),
            incomplete: false,
            has_usage: false,
            has_cumulative_usage: false,
        }
    }
}

impl CodexTurnUsageTracker {
    fn set_baseline(&mut self, token_usage: &Value) {
        if !self.has_usage
            && let Some(total) = token_usage.get("total")
        {
            self.previous_total = Some(normalize_codex_app_server_token_usage(total).usage);
        }
    }

    fn update(&mut self, token_usage: &Value) -> Option<NormalizedRunUsage> {
        let last = normalize_codex_app_server_token_usage(token_usage).usage;
        if let Some(total_value) = token_usage.get("total") {
            if !self.has_cumulative_usage {
                // A legacy last-only report may overlap this first cumulative
                // report. Replace that estimate rather than adding it twice.
                self.accumulated = SessionTokenUsage::default();
                self.has_cumulative_usage = true;
            }
            let total = normalize_codex_app_server_token_usage(total_value).usage;
            let previous = self.previous_total.get_or_insert_with(|| {
                // Older servers may omit the resume baseline. Recover what is
                // known, but don't claim that a possibly coalesced first update
                // accounts for every response in this turn.
                self.incomplete = true;
                codex_usage_difference(&total, &last)
            });
            if total.used < previous.used
                || total.input < previous.input
                || total.output < previous.output
            {
                // Never charge a stale update or counter reset as new usage.
                // Preserve the observed lower bound and disclose incompleteness.
                self.incomplete = true;
            } else {
                let mut delta = codex_usage_difference(&total, previous);
                if delta.used > 0
                    && !total_value.as_object().is_some_and(|object| {
                        object.contains_key("cacheWriteInputTokens")
                            || object.contains_key("cache_write_input_tokens")
                    })
                {
                    delta.cache_creation = last.cache_creation;
                }
                add_codex_usage(&mut self.accumulated, &delta);
                *previous = total;
            }
        } else {
            // Legacy last-only/flat payloads have no request identity or
            // cumulative counter, so summing them would double-count repeats.
            if !self.has_cumulative_usage {
                self.accumulated = last;
                self.previous_total = None;
            }
            self.incomplete = true;
        }
        self.has_usage = true;
        if self.accumulated.used == 0 {
            return None;
        }
        let usage = &self.accumulated;
        let raw = serde_json::json!({
            "input_tokens": usage.input,
            "output_tokens": usage.output,
            "cached_input_tokens": usage.cache_read,
            "cache_write_input_tokens": usage.cache_creation,
            "reasoning_output_tokens": usage.reasoning,
            "total_tokens": usage.used,
        });
        let mut normalized = normalize_codex_app_server_token_usage(&raw);
        if self.incomplete {
            normalized.completeness = TokenUsageCompleteness::Partial;
        }
        Some(normalized)
    }
}

fn codex_usage_difference(
    total: &SessionTokenUsage,
    previous: &SessionTokenUsage,
) -> SessionTokenUsage {
    SessionTokenUsage {
        used: total.used.saturating_sub(previous.used),
        input: total.input.saturating_sub(previous.input),
        output: total.output.saturating_sub(previous.output),
        cache_creation: total.cache_creation.saturating_sub(previous.cache_creation),
        cache_read: total.cache_read.saturating_sub(previous.cache_read),
        reasoning: total.reasoning.saturating_sub(previous.reasoning),
        cost_usd: 0.0,
    }
}

fn add_codex_usage(total: &mut SessionTokenUsage, delta: &SessionTokenUsage) {
    total.used = total.used.saturating_add(delta.used);
    total.input = total.input.saturating_add(delta.input);
    total.output = total.output.saturating_add(delta.output);
    total.cache_creation = total.cache_creation.saturating_add(delta.cache_creation);
    total.cache_read = total.cache_read.saturating_add(delta.cache_read);
    total.reasoning = total.reasoning.saturating_add(delta.reasoning);
}
