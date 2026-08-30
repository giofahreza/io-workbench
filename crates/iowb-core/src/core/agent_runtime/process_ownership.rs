fn isolate_agent_process(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);

    // A forced SIGKILL gives Rust no opportunity to run cleanup code. On
    // Linux, ask the kernel to kill the provider CLI when its server parent
    // disappears so startup recovery never overlaps an orphaned old turn.
    #[cfg(target_os = "linux")]
    unsafe {
        command.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn durable_agent_run_scope(database_path: &Path) -> String {
    let canonical_path = std::fs::canonicalize(database_path).unwrap_or_else(|_| {
        if database_path.is_absolute() {
            database_path.to_path_buf()
        } else {
            env::current_dir()
                .map(|current_dir| current_dir.join(database_path))
                .unwrap_or_else(|_| database_path.to_path_buf())
        }
    });
    hex::encode(Sha256::digest(
        canonical_path.as_os_str().as_encoded_bytes(),
    ))
}

#[cfg(target_os = "linux")]
fn process_start_time(process_id: libc::pid_t) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{process_id}/stat")).ok()?;
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "linux")]
fn current_process_identity() -> Option<(libc::pid_t, u64)> {
    let process_id = std::process::id() as libc::pid_t;
    process_start_time(process_id).map(|start_time| (process_id, start_time))
}

#[cfg(target_os = "linux")]
fn process_environment_value<'a>(environment: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let key = key.as_bytes();
    environment
        .split(|byte| *byte == 0)
        .find_map(|entry| entry.strip_prefix(key)?.strip_prefix(b"="))
}

#[cfg(target_os = "linux")]
fn marked_process_owner_is_alive(environment: &[u8]) -> bool {
    let owner_pid = process_environment_value(environment, DURABLE_AGENT_OWNER_PID_ENV)
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.parse::<libc::pid_t>().ok());
    let owner_start = process_environment_value(environment, DURABLE_AGENT_OWNER_START_ENV)
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.parse::<u64>().ok());

    match (owner_pid, owner_start) {
        (Some(owner_pid), Some(owner_start)) => process_start_time(owner_pid) == Some(owner_start),
        // New scoped processes always carry a complete owner identity. Treat
        // incomplete markers as live so cleanup fails closed rather than
        // risking termination of a process owned by another server.
        _ => true,
    }
}

/// Kill provider descendants left behind by a stopped server before a durable
/// continuation is launched. A process must match both the run and canonical
/// database path, and its recorded server owner must no longer be alive.
/// Linux `/proc` exposes these inherited markers even when the original
/// process-group leader has already exited.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OrphanedAgentRunCleanup {
    pub terminated_process_groups: usize,
    pub live_owner: bool,
}

#[cfg(target_os = "linux")]
pub fn terminate_orphaned_agent_run_processes(
    run_id: &str,
    database_path: impl AsRef<Path>,
) -> OrphanedAgentRunCleanup {
    let run_id = run_id.trim();
    if run_id.is_empty() {
        return OrphanedAgentRunCleanup::default();
    }
    let expected_scope = durable_agent_run_scope(database_path.as_ref());
    let current_process_group = unsafe { libc::getpgrp() };
    let mut process_groups = HashSet::<libc::pid_t>::new();
    let mut live_owner = false;
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return OrphanedAgentRunCleanup::default();
    };

    for entry in entries.flatten() {
        let Some(process_id) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        let Ok(environment) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        if process_environment_value(&environment, DURABLE_AGENT_RUN_ENV) != Some(run_id.as_bytes())
            || process_environment_value(&environment, DURABLE_AGENT_SCOPE_ENV)
                != Some(expected_scope.as_bytes())
        {
            continue;
        }
        if marked_process_owner_is_alive(&environment) {
            live_owner = true;
            continue;
        }
        let process_group = unsafe { libc::getpgid(process_id) };
        if process_group > 0 && process_group != current_process_group {
            process_groups.insert(process_group);
        }
    }

    if live_owner {
        return OrphanedAgentRunCleanup {
            terminated_process_groups: 0,
            live_owner: true,
        };
    }

    let mut terminated = 0;
    for process_group in process_groups {
        let result = unsafe { libc::kill(-process_group, libc::SIGKILL) };
        if result == 0 {
            terminated += 1;
            info!(
                run_id,
                process_group, "terminated orphaned durable agent process group"
            );
        } else {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                warn!(
                    error = %error,
                    run_id,
                    process_group,
                    "failed to terminate orphaned durable agent process group"
                );
            }
        }
    }
    OrphanedAgentRunCleanup {
        terminated_process_groups: terminated,
        live_owner,
    }
}

#[cfg(not(target_os = "linux"))]
pub fn terminate_orphaned_agent_run_processes(
    _run_id: &str,
    _database_path: impl AsRef<Path>,
) -> OrphanedAgentRunCleanup {
    OrphanedAgentRunCleanup::default()
}
