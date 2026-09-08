use std::{
    env, fs,
    net::IpAddr,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use iowb_core::AppConfig;
use iowb_fs::WorkspacePathValidator;
use serde::Serialize;
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Debug, Parser)]
#[command(name = "io-workbench")]
#[command(bin_name = "io-workbench")]
#[command(version, about = "Rust-first workspace server and client suite")]
#[command(
    after_help = "Examples:\n  io-workbench doctor\n  io-workbench doctor --require-running --json\n  io-workbench setup"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long, env = "IO_WORKBENCH_HOST", global = true)]
    host: Option<IpAddr>,

    #[arg(long, env = "IO_WORKBENCH_PORT", global = true)]
    port: Option<u16>,

    #[arg(long, env = "IO_WORKBENCH_CONFIG_DIR", global = true)]
    config_dir: Option<PathBuf>,

    #[arg(long, env = "IO_WORKBENCH_WORKSPACE_ROOT", global = true)]
    workspace_root: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Start,
    Status,
    /// Inspect local configuration, optional host tools, and the running health endpoint.
    Doctor(DoctorArgs),
    /// Print a safe, non-mutating first-run and repair checklist.
    Setup,
    Sandbox {
        project_path: PathBuf,
    },
    ImportLegacy {
        #[arg(long)]
        from: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    Version,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    /// Emit a stable JSON report on stdout.
    #[arg(long)]
    json: bool,

    /// Exit unsuccessfully when an advisory warning is found as well as an error.
    #[arg(long)]
    strict: bool,

    /// Do not request the configured server's /health endpoint.
    #[arg(long)]
    skip_health: bool,

    /// Treat a missing or invalid /health response as an error instead of an advisory warning.
    #[arg(long, conflicts_with = "skip_health")]
    require_running: bool,

    /// Maximum time to wait for the local health endpoint, in milliseconds.
    #[arg(long, default_value_t = DEFAULT_DOCTOR_TIMEOUT_MS, value_parser = clap::value_parser!(u64).range(100..=10_000))]
    timeout_ms: u64,
}

const DEFAULT_DOCTOR_TIMEOUT_MS: u64 = 1_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum DoctorCheckStatus {
    Ok,
    Warning,
    Error,
    Skipped,
}

impl DoctorCheckStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct DoctorCheck {
    name: String,
    status: DoctorCheckStatus,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    remedy: Option<String>,
}

impl DoctorCheck {
    fn ok(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: DoctorCheckStatus::Ok,
            message: message.into(),
            remedy: None,
        }
    }

    fn warning(
        name: impl Into<String>,
        message: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: DoctorCheckStatus::Warning,
            message: message.into(),
            remedy: Some(remedy.into()),
        }
    }

    fn error(
        name: impl Into<String>,
        message: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: DoctorCheckStatus::Error,
            message: message.into(),
            remedy: Some(remedy.into()),
        }
    }

    fn skipped(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: DoctorCheckStatus::Skipped,
            message: message.into(),
            remedy: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct DoctorSummary {
    errors: usize,
    warnings: usize,
    skipped: usize,
}

#[derive(Debug, Clone, Serialize)]
struct DoctorReport {
    schema: &'static str,
    product: &'static str,
    version: &'static str,
    server: String,
    ok: bool,
    strict_ok: bool,
    summary: DoctorSummary,
    checks: Vec<DoctorCheck>,
}

pub fn run() -> anyhow::Result<()> {
    init_tracing();

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let cli = Cli::parse();
        let config = apply_overrides(AppConfig::from_env()?, &cli);

        match cli.command.unwrap_or(Command::Start) {
            Command::Start => iowb_server::serve(config).await,
            Command::Status => {
                print_status(&config);
                Ok(())
            }
            Command::Doctor(args) => run_doctor(&config, &args).await,
            Command::Setup => {
                print_setup_checklist(&config);
                Ok(())
            }
            Command::Sandbox { project_path } => {
                run_sandbox_check(&config, &project_path).await?;
                Ok(())
            }
            Command::ImportLegacy { from, dry_run } => {
                import_legacy_data(&config, from, dry_run)?;
                Ok(())
            }
            Command::Version => {
                println!("io-workbench {}", iowb_server::VERSION);
                Ok(())
            }
        }
    })
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("iowb=info"));
    fmt().with_env_filter(filter).init();
}

fn apply_overrides(mut config: AppConfig, cli: &Cli) -> AppConfig {
    if let Some(host) = cli.host {
        config.host = host;
    }
    if let Some(port) = cli.port {
        config.port = port;
    }
    if let Some(config_dir) = &cli.config_dir {
        config.config_dir = config_dir.clone();
        config.database_path = config.config_dir.join(iowb_protocol::DATABASE_FILE_NAME);
    }
    if let Some(workspace_root) = &cli.workspace_root {
        config.workspace_root = workspace_root.clone();
    }

    config
}

fn print_status(config: &AppConfig) {
    println!("product: io-workbench");
    println!("version: {}", iowb_server::VERSION);
    println!("server: http://{}:{}", config.host, config.port);
    println!("config_dir: {}", config.config_dir.display());
    println!("database: {}", config.database_path.display());
    println!("workspace_root: {}", config.workspace_root.display());
    println!("auth_required: {}", config.auth_required);
    println!("otp_auth: {}", config.otp_secret.is_some());
}

async fn run_doctor(config: &AppConfig, args: &DoctorArgs) -> anyhow::Result<()> {
    let report = collect_doctor_report(config, args).await;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_doctor_report(&report);
    }

    let passed = if args.strict {
        report.strict_ok
    } else {
        report.ok
    };
    if passed {
        return Ok(());
    }

    let mode = if args.strict {
        "strict doctor"
    } else {
        "doctor"
    };
    anyhow::bail!(
        "{mode} found {} error(s) and {} warning(s)",
        report.summary.errors,
        report.summary.warnings
    );
}

async fn collect_doctor_report(config: &AppConfig, args: &DoctorArgs) -> DoctorReport {
    let mut checks = vec![
        check_current_binary(),
        check_config_directory(&config.config_dir),
        check_database_path(&config.database_path),
        check_workspace_root(&config.workspace_root),
        check_listener_security(config),
    ];
    checks.extend(check_optional_tools());
    checks.push(if args.skip_health {
        DoctorCheck::skipped("health", "health request skipped by --skip-health")
    } else {
        check_running_health(config, args.timeout_ms, args.require_running).await
    });

    let summary = summarize_checks(&checks);
    DoctorReport {
        schema: "io-workbench.doctor/v1",
        product: "io-workbench",
        version: iowb_server::VERSION,
        server: health_url(config),
        ok: summary.errors == 0,
        strict_ok: summary.errors == 0 && summary.warnings == 0,
        summary,
        checks,
    }
}

fn summarize_checks(checks: &[DoctorCheck]) -> DoctorSummary {
    DoctorSummary {
        errors: checks
            .iter()
            .filter(|check| check.status == DoctorCheckStatus::Error)
            .count(),
        warnings: checks
            .iter()
            .filter(|check| check.status == DoctorCheckStatus::Warning)
            .count(),
        skipped: checks
            .iter()
            .filter(|check| check.status == DoctorCheckStatus::Skipped)
            .count(),
    }
}

fn print_doctor_report(report: &DoctorReport) {
    println!("io-workbench doctor");
    println!("version: {}", report.version);
    println!("server: {}", report.server);
    for check in &report.checks {
        println!(
            "{}: {} — {}",
            check.status.label(),
            check.name,
            check.message
        );
        if let Some(remedy) = &check.remedy {
            println!("  repair: {remedy}");
        }
    }
    println!(
        "summary: {} error(s), {} warning(s), {} skipped",
        report.summary.errors, report.summary.warnings, report.summary.skipped
    );
    if report.ok {
        if report.strict_ok {
            println!("result: ready");
        } else {
            println!("result: core checks pass; review warnings");
        }
    } else {
        println!("result: repair required");
    }
}

fn print_setup_checklist(config: &AppConfig) {
    println!("io-workbench setup (no changes made)");
    println!("configured server: {}", server_url(config));
    println!("configuration directory: {}", config.config_dir.display());
    println!("workspace boundary: {}", config.workspace_root.display());
    println!();
    println!("1. Start the host: io-workbench start");
    println!("2. Verify it: io-workbench doctor --require-running");
    println!(
        "3. Open {} and complete first-user setup.",
        server_url(config)
    );
    println!("4. Add an existing project inside the workspace boundary.");
    println!("5. Install and authenticate any provider CLI you plan to use on this host.");
    println!("6. Run one small non-destructive provider task before relying on it.");
    println!();
    println!("Repair guidance:");
    println!("  • Missing config/workspace paths: review the doctor remedy, then start the host.");
    println!(
        "  • Missing optional tools: rerun the official installer or install the tool with its supported method."
    );
    println!(
        "  • Health failure: start the configured service, then rerun io-workbench doctor --require-running."
    );
    println!(
        "  • After an upgrade: rerun io-workbench doctor --require-running before reconnecting remote clients."
    );
    println!();
    println!(
        "Use `io-workbench doctor --json --require-running` for automation. It makes no changes."
    );
}

fn check_current_binary() -> DoctorCheck {
    match env::current_exe() {
        Ok(path) if path.is_file() => {
            DoctorCheck::ok("binary", format!("running executable: {}", path.display()))
        }
        Ok(path) => DoctorCheck::warning(
            "binary",
            format!(
                "current executable is not a regular file: {}",
                path.display()
            ),
            "Reinstall the release binary for this user, then run `io-workbench doctor` again.",
        ),
        Err(error) => DoctorCheck::warning(
            "binary",
            format!("could not resolve the running executable: {error}"),
            "Run the installed `io-workbench` command from a normal user shell and retry.",
        ),
    }
}

fn check_config_directory(path: &Path) -> DoctorCheck {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => match fs::read_dir(path) {
            Ok(_) => DoctorCheck::ok(
                "config_directory",
                format!("readable directory: {}", path.display()),
            ),
            Err(error) => DoctorCheck::error(
                "config_directory",
                format!("cannot read {}: {error}", path.display()),
                "Choose a user-owned directory with `--config-dir`, then restart the service.",
            ),
        },
        Ok(_) => DoctorCheck::error(
            "config_directory",
            format!("expected a directory but found a file: {}", path.display()),
            "Choose an empty user-owned directory with `--config-dir`; do not overwrite the existing file.",
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DoctorCheck::warning(
            "config_directory",
            format!("not created yet: {}", path.display()),
            "Start io-workbench once to create it, or choose another path with `--config-dir`.",
        ),
        Err(error) => DoctorCheck::error(
            "config_directory",
            format!("cannot inspect {}: {error}", path.display()),
            "Choose a readable user-owned directory with `--config-dir`, then restart the service.",
        ),
    }
}

fn check_database_path(path: &Path) -> DoctorCheck {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            match fs::OpenOptions::new().read(true).write(true).open(path) {
                Ok(_) => DoctorCheck::ok(
                    "database",
                    format!("readable/writable database file: {}", path.display()),
                ),
                Err(error) => DoctorCheck::error(
                    "database",
                    format!(
                        "cannot open {} for read/write access: {error}",
                        path.display()
                    ),
                    "Restore the file permissions or select a different `--config-dir`; do not delete the database to repair it.",
                ),
            }
        }
        Ok(_) => DoctorCheck::error(
            "database",
            format!(
                "expected a database file but found a directory: {}",
                path.display()
            ),
            "Move the directory aside only after making a backup, then choose a valid `--config-dir`.",
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DoctorCheck::warning(
            "database",
            format!("not created yet: {}", path.display()),
            "Start io-workbench once to initialize its database. Do not create an empty file by hand.",
        ),
        Err(error) => DoctorCheck::error(
            "database",
            format!("cannot inspect {}: {error}", path.display()),
            "Check the configured directory and file permissions; preserve the existing data before changing paths.",
        ),
    }
}

fn check_workspace_root(path: &Path) -> DoctorCheck {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => match fs::read_dir(path) {
            Ok(_) => DoctorCheck::ok(
                "workspace_root",
                format!("readable workspace boundary: {}", path.display()),
            ),
            Err(error) => DoctorCheck::error(
                "workspace_root",
                format!("cannot read {}: {error}", path.display()),
                "Choose a readable project parent with `--workspace-root`, then restart the service.",
            ),
        },
        Ok(_) => DoctorCheck::error(
            "workspace_root",
            format!("expected a directory but found a file: {}", path.display()),
            "Choose a directory that contains only the projects this host should be allowed to access.",
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DoctorCheck::warning(
            "workspace_root",
            format!("not created yet: {}", path.display()),
            "Create or select the intended project parent with `--workspace-root`; the first server start can create an empty directory.",
        ),
        Err(error) => DoctorCheck::error(
            "workspace_root",
            format!("cannot inspect {}: {error}", path.display()),
            "Choose a readable project parent with `--workspace-root`, then restart the service.",
        ),
    }
}

fn check_listener_security(config: &AppConfig) -> DoctorCheck {
    let auth_enabled =
        config.auth_required || config.local_token.is_some() || config.otp_secret.is_some();
    if !auth_enabled && !config.host.is_loopback() {
        return DoctorCheck::error(
            "listener_security",
            format!(
                "{} is reachable beyond loopback while io-workbench authentication is disabled",
                health_url(config)
            ),
            "Enable IO_WORKBENCH_AUTH_REQUIRED or configure a token/OTP before exposing this host.",
        );
    }
    if !auth_enabled {
        return DoctorCheck::warning(
            "listener_security",
            "authentication is disabled for a loopback-only host".to_string(),
            "Keep the listener on loopback, or enable IO_WORKBENCH_AUTH_REQUIRED before remote access.",
        );
    }
    if !config.host.is_loopback() {
        return DoctorCheck::warning(
            "listener_security",
            format!("{} is not loopback-only", health_url(config)),
            "Use a VPN, authenticated HTTPS/WSS reverse proxy, or tunnel and verify remote access separately.",
        );
    }

    DoctorCheck::ok(
        "listener_security",
        "authenticated loopback listener configured".to_string(),
    )
}

fn check_optional_tools() -> Vec<DoctorCheck> {
    [
        (
            "cli.git",
            "git",
            "Install Git before using repository and commit tools.",
        ),
        (
            "cli.codex",
            "codex",
            "Install and authenticate Codex on this host, then run a small provider task.",
        ),
        (
            "cli.claude",
            "claude",
            "Install and authenticate Claude Code on this host, then run a small provider task.",
        ),
        (
            "cli.gemini",
            "gemini",
            "Install and authenticate Gemini CLI on this host, then run a small provider task.",
        ),
        (
            "cli.io_gateway",
            "io-gateway",
            "IO Gateway is optional. Install and configure it separately only when you plan to use it.",
        ),
    ]
    .into_iter()
    .map(|(check_name, command, remedy)| match find_command_in_path(command) {
        Some(path) => DoctorCheck::ok(check_name, format!("found `{command}` at {}", path.display())),
        None => DoctorCheck::warning(
            check_name,
            format!("`{command}` was not found on PATH"),
            remedy,
        ),
    })
    .collect()
}

async fn check_running_health(
    config: &AppConfig,
    timeout_ms: u64,
    require_running: bool,
) -> DoctorCheck {
    let url = health_url(config);
    let failure = |message: String| {
        if require_running {
            DoctorCheck::error(
                "health",
                message,
                format!(
                    "Start the configured host, then retry `io-workbench doctor --require-running`. Expected {url}"
                ),
            )
        } else {
            DoctorCheck::warning(
                "health",
                message,
                format!(
                    "Start the configured host, then retry `io-workbench doctor --require-running`. Expected {url}"
                ),
            )
        }
    };

    let client = match reqwest::Client::builder()
        // Doctor checks the local endpoint selected by this host configuration.
        // Do not let a shell-level HTTP proxy turn it into a remote request or
        // make a healthy local service appear unavailable.
        .no_proxy()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
    {
        Ok(client) => client,
        Err(error) => return failure(format!("could not construct health client: {error}")),
    };
    let response = match client.get(&url).send().await {
        Ok(response) => response,
        Err(error) => return failure(format!("did not reach {url}: {error}")),
    };
    if !response.status().is_success() {
        return failure(format!(
            "{url} returned HTTP {} instead of a successful health response",
            response.status()
        ));
    }

    match response.json::<iowb_protocol::HealthResponse>().await {
        Ok(health) if health.service == "io-workbench" => DoctorCheck::ok(
            "health",
            format!("reachable io-workbench {} at {url}", health.version),
        ),
        Ok(health) => failure(format!(
            "{url} returned a health response for unexpected service `{}`",
            health.service
        )),
        Err(error) => failure(format!(
            "{url} returned an invalid health response: {error}"
        )),
    }
}

fn health_url(config: &AppConfig) -> String {
    format!("{}/health", server_url(config))
}

fn server_url(config: &AppConfig) -> String {
    let probe_host = match config.host {
        IpAddr::V4(address) if address.is_unspecified() => {
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        }
        IpAddr::V6(address) if address.is_unspecified() => {
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        }
        address => address,
    };
    match probe_host {
        IpAddr::V4(address) => format!("http://{address}:{}", config.port),
        IpAddr::V6(address) => format!("http://[{address}]:{}", config.port),
    }
}

fn find_command_in_path(command: &str) -> Option<PathBuf> {
    let candidates = command_candidates(command);
    command_search_directories()
        .into_iter()
        .find_map(|directory| {
            candidates
                .iter()
                .map(|candidate| directory.join(candidate))
                .find(|candidate| is_executable_file(candidate))
        })
}

fn command_search_directories() -> Vec<PathBuf> {
    let mut directories = env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .collect::<Vec<_>>();

    if let Some(prefix) = env::var_os("IO_WORKBENCH_NPM_PREFIX").map(PathBuf::from) {
        add_command_directory(&mut directories, prefix.clone());
        add_command_directory(&mut directories, prefix.join("bin"));
    }
    if let Some(home) = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
    {
        add_command_directory(&mut directories, home.join(".local/bin"));
        add_command_directory(&mut directories, home.join(".npm-global/bin"));
        add_command_directory(&mut directories, home.join(".npm/bin"));
        add_command_directory(&mut directories, home.join(".volta/bin"));
    }
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        add_command_directory(
            &mut directories,
            local_app_data.join("io-workbench").join("npm"),
        );
        add_command_directory(
            &mut directories,
            local_app_data.join("Programs").join("io-workbench"),
        );
    }

    directories
}

fn add_command_directory(directories: &mut Vec<PathBuf>, directory: PathBuf) {
    if !directories.iter().any(|existing| existing == &directory) {
        directories.push(directory);
    }
}

fn command_candidates(command: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let extension = Path::new(command).extension().is_some();
        if extension {
            return vec![command.to_string()];
        }
        let extensions = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        return extensions
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!("{command}{extension}"))
            .collect();
    }

    #[cfg(not(windows))]
    {
        vec![command.to_string()]
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn import_legacy_data(
    config: &AppConfig,
    from: Option<PathBuf>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let source = from.unwrap_or_else(default_legacy_config_dir);
    if !source.exists() {
        anyhow::bail!("legacy config directory not found: {}", source.display());
    }
    if !source.is_dir() {
        anyhow::bail!(
            "legacy config path is not a directory: {}",
            source.display()
        );
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let target = config
        .config_dir
        .join("legacy-imports")
        .join(format!("web-ai-cli-{stamp}"));
    let mut copied = 0usize;
    let mut bytes = 0u64;
    collect_copy_stats(&source, &mut copied, &mut bytes)?;

    println!("source: {}", source.display());
    println!("target: {}", target.display());
    println!("files: {copied}");
    println!("bytes: {bytes}");
    if dry_run {
        println!("dry_run: true");
        return Ok(());
    }

    copy_dir_recursive(&source, &target)?;
    println!("imported legacy data into {}", target.display());
    println!("original legacy data was not modified");
    Ok(())
}

fn default_legacy_config_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".web-ai-cli")
}

fn collect_copy_stats(path: &Path, files: &mut usize, bytes: &mut u64) -> anyhow::Result<()> {
    for entry in fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            collect_copy_stats(&entry.path(), files, bytes)?;
        } else if metadata.is_file() {
            *files += 1;
            *bytes += metadata.len();
        }
    }
    Ok(())
}

fn copy_dir_recursive(source: &Path, target: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(target).with_context(|| format!("failed to create {}", target.display()))?;
    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        }
    }
    Ok(())
}

async fn run_sandbox_check(config: &AppConfig, project_path: &Path) -> anyhow::Result<()> {
    let validator = WorkspacePathValidator::new(config.workspace_root.clone());
    let validation = validator.validate(project_path, false).await;

    if validation.valid {
        println!(
            "valid workspace path: {}",
            validation
                .resolved_path
                .as_deref()
                .unwrap_or_else(|| project_path.to_str().unwrap_or("<invalid utf-8>"))
        );
        return Ok(());
    }

    anyhow::bail!(
        "invalid workspace path: {}",
        validation
            .error
            .as_deref()
            .context("validator returned no error")?
    );
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr, Ipv6Addr},
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    static TEMP_PATH_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_temp_path(label: &str) -> PathBuf {
        let counter = TEMP_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!("iowb-cli-{label}-{}-{counter}", std::process::id()))
    }

    fn test_config(host: IpAddr, port: u16) -> AppConfig {
        let config_dir = unique_temp_path("config");
        AppConfig {
            host,
            port,
            database_path: config_dir.join("io-workbench.db"),
            workspace_root: unique_temp_path("workspace"),
            config_dir,
            auth_required: true,
            local_token: None,
            otp_secret: None,
            max_sessions: 100,
            max_scan_depth: 6,
            max_file_read_bytes: 2 * 1024 * 1024,
        }
    }

    #[test]
    fn health_url_uses_loopback_for_wildcard_listeners() {
        let config = test_config(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8787);
        assert_eq!(server_url(&config), "http://127.0.0.1:8787");
        assert_eq!(health_url(&config), "http://127.0.0.1:8787/health");

        let config = test_config(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 8787);
        assert_eq!(server_url(&config), "http://[::1]:8787");
        assert_eq!(health_url(&config), "http://[::1]:8787/health");

        let config = test_config(IpAddr::V6(Ipv6Addr::LOCALHOST), 8123);
        assert_eq!(health_url(&config), "http://[::1]:8123/health");
    }

    #[test]
    fn directory_checks_warn_for_uninitialized_paths_and_error_for_files() {
        let path = unique_temp_path("directory-check");
        let check = check_config_directory(&path);
        assert_eq!(check.status, DoctorCheckStatus::Warning);

        fs::write(&path, "not a directory").expect("write test file");
        let check = check_workspace_root(&path);
        assert_eq!(check.status, DoctorCheckStatus::Error);
        fs::remove_file(path).expect("remove test file");
    }

    #[test]
    fn report_summary_separates_advisories_from_failures() {
        let checks = vec![
            DoctorCheck::ok("one", "ready"),
            DoctorCheck::warning("two", "optional tool missing", "install it"),
            DoctorCheck::error("three", "invalid path", "choose another path"),
            DoctorCheck::skipped("four", "not requested"),
        ];
        let summary = summarize_checks(&checks);
        assert_eq!(summary.errors, 1);
        assert_eq!(summary.warnings, 1);
        assert_eq!(summary.skipped, 1);
    }

    #[test]
    fn doctor_flags_parse_with_machine_readable_and_required_health_modes() {
        let cli = Cli::try_parse_from([
            "io-workbench",
            "doctor",
            "--json",
            "--require-running",
            "--timeout-ms",
            "2500",
        ])
        .expect("doctor command parses");

        match cli.command.expect("subcommand") {
            Command::Doctor(args) => {
                assert!(args.json);
                assert!(args.require_running);
                assert_eq!(args.timeout_ms, 2_500);
            }
            _ => panic!("expected doctor command"),
        }
    }

    #[tokio::test]
    async fn health_check_accepts_a_valid_workbench_health_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind health test listener");
        let port = listener.local_addr().expect("listener address").port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept health request");
            let mut request = [0_u8; 1024];
            let _ = stream
                .read(&mut request)
                .await
                .expect("read health request");
            let body = r#"{"status":"ok","service":"io-workbench","version":"test","config_dir":"/tmp","database_path":"/tmp/io-workbench.db","server_time":"2026-01-01T00:00:00Z"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write health response");
        });

        let config = test_config(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let check = check_running_health(&config, 1_000, true).await;
        assert_eq!(check.status, DoctorCheckStatus::Ok, "{}", check.message);
        server.await.expect("health server finishes");
    }
}
