# Codex model settings and token accounting

Workbench's built-in Codex launchers load model defaults from an existing
`$CODEX_HOME/default.config.toml` (`~/.codex` when `CODEX_HOME` is unset).
This matches the profile-file layout supported by the installed Codex CLI.
If that file is absent, Codex uses its ordinary configuration.

Set `IO_WORKBENCH_CODEX_PROFILE=work` in the Workbench server environment to
select `work.config.toml`. An explicitly selected missing or malformed profile
fails the launch with a configuration error. Set `IO_WORKBENCH_CODEX_PROFILE=`
to disable profile loading. Restart the server after changing its environment.
Profile file changes are read on the next launch.

This imports these model defaults through explicit `-c` arguments for both
`codex exec` and `codex app-server`:

- `model`, `model_provider`, `model_catalog_json`
- `model_reasoning_effort`, `model_reasoning_summary`, `model_verbosity`
- `service_tier`, `model_context_window`, `model_auto_compact_token_limit`

The installed app-server does not accept `--profile`. This is a limited import
of model defaults, not a full profile merge: authentication, provider definitions,
MCP servers, hooks, permissions, and other profile settings are not copied.
Native provider definitions must already be available in Codex's base config.
Gateway authentication and provider routing continue to come from Workbench's
gateway settings. Custom `*_ARGS_JSON` launch templates retain full control of
their arguments and do not receive these defaults.

Chat selections for model, effort, and fast mode take precedence over profile
defaults. Live turns also pass these defaults through the thread and turn APIs
so saved resume settings do not silently override them.
Turning Thinking on preserves the selected Codex effort. When effort
is omitted, Codex uses its configured default. A model catalog can change tools
and instructions; new threads are the clearest way to compare a changed catalog,
since resumed threads retain their saved history and instructions.

App-server usage saved for a run sums its inference requests using changes in
the thread's cumulative counters. The previous thread total is subtracted on
resume, and repeated updates do not add usage twice. Usage observed before a
failure or interruption is retained. Missing resume baselines, legacy last-only
reports, and counter regressions are marked partial. Missing usage stays missing.
Existing historical usage rows are not recalculated.

Input totals already include cached input, and output totals already include
reasoning. Those subsets must not be added again when comparing token totals.
Corrected accounting may increase the usage shown by Workbench because the old
implementation retained only the last inference request of each turn.

These changes align settings and improve reporting. The token savings for a
particular workload still require a comparison with the same model, effort,
tools, instructions, and starting context. A process restart alone does not
establish a prompt-cache miss; app-server process lifetime is unchanged here.
