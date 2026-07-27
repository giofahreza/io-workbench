use std::{
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use clap::{Parser, Subcommand};
use iowb_core::AppConfig;
use iowb_fs::WorkspacePathValidator;
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Debug, Parser)]
#[command(name = "io-workbench")]
#[command(bin_name = "io-workbench")]
#[command(version, about = "Rust-first workspace server and client suite")]
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
