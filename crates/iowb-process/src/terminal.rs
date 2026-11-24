use super::*;
use iowb_protocol::{TerminalShellInfo, TerminalStartRequest, TerminalStartResponse};
use std::{env, ffi::OsStr, path::Path};

fn executable(command: &str, path: &OsStr) -> Option<PathBuf> {
    let requested = Path::new(command);
    if command.is_empty() || matches!(requested.file_stem()?.to_str()?, "nologin" | "false") {
        return None;
    }
    let candidates = if requested.is_absolute() {
        vec![requested.to_path_buf()]
    } else if requested.components().count() == 1 {
        env::split_paths(path)
            .filter(|directory| directory.is_absolute())
            .map(|directory| directory.join(requested))
            .collect()
    } else {
        return None;
    };
    candidates.into_iter().find(|candidate| {
        let Ok(metadata) = candidate.metadata() else {
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
    })
}

#[cfg(unix)]
fn account_shell() -> Option<String> {
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 || result.is_null() {
            return None;
        }
        let shell = unsafe { (*result).pw_shell };
        if shell.is_null() {
            return None;
        }
        return unsafe { std::ffi::CStr::from_ptr(shell) }
            .to_str()
            .ok()
            .map(str::to_owned);
    }
}

#[cfg(not(unix))]
fn account_shell() -> Option<String> {
    None
}

fn select_shell(
    configured: Option<&str>,
    args: Vec<String>,
    environment: Option<&str>,
    account: Option<&str>,
    path: &OsStr,
    safe: bool,
) -> Result<TerminalShellInfo> {
    let describe =
        |command: PathBuf, args: Vec<String>, source: &str| -> Result<TerminalShellInfo> {
            Ok(TerminalShellInfo {
                command: command
                    .to_str()
                    .ok_or_else(|| ProcessError::InvalidShell("shell path is not UTF-8".into()))?
                    .into(),
                args,
                source: source.into(),
                safe,
            })
        };
    #[cfg(unix)]
    let fallbacks = [
        "/bin/bash",
        "/usr/bin/bash",
        "/bin/sh",
        "/usr/bin/sh",
        "bash",
        "sh",
    ];
    #[cfg(not(unix))]
    let fallbacks = ["pwsh.exe", "powershell.exe", "cmd.exe"];

    if !safe {
        if let Some(configured) = configured {
            let command = executable(configured, path).ok_or_else(|| ProcessError::InvalidShell(
                "IO_WORKBENCH_TERMINAL_SHELL must name an executable shell, not a directory, login-denying program, or shell expression. Open a safe terminal to recover.".into()
            ))?;
            return describe(command, args, "configured");
        }
        if !args.is_empty() {
            return Err(ProcessError::InvalidShell(
                "IO_WORKBENCH_TERMINAL_ARGS requires IO_WORKBENCH_TERMINAL_SHELL".into(),
            ));
        }
        for (candidate, source) in [(environment, "environment"), (account, "account")] {
            if let Some(command) = candidate.and_then(|candidate| executable(candidate, path)) {
                return describe(command, Vec::new(), source);
            }
        }
    }
    for candidate in fallbacks {
        if let Some(command) = executable(candidate, path) {
            let name = command.file_stem().and_then(OsStr::to_str).unwrap_or("");
            let args = if !safe {
                Vec::new()
            } else {
                match name {
                    "bash" => vec!["--noprofile".into(), "--norc".into(), "-i".into()],
                    "sh" => vec!["-i".into()],
                    "cmd" => vec!["/D".into()],
                    _ => vec!["-NoLogo".into(), "-NoProfile".into()],
                }
            };
            return describe(command, args, if safe { "safe" } else { "fallback" });
        }
    }
    Err(ProcessError::InvalidShell(
        "no usable terminal shell was found on the Workbench host".into(),
    ))
}

impl ProcessManager {
    pub async fn start_terminal(
        &self,
        request: TerminalStartRequest,
    ) -> Result<TerminalStartResponse> {
        let configured = env::var("IO_WORKBENCH_TERMINAL_SHELL")
            .ok()
            .filter(|value| !value.is_empty());
        let args = if request.safe {
            Vec::new()
        } else {
            env::var("IO_WORKBENCH_TERMINAL_ARGS")
                .ok()
                .map(|value| serde_json::from_str::<Vec<String>>(&value))
                .transpose()
                .map_err(|_| {
                    ProcessError::InvalidShell(
                        "IO_WORKBENCH_TERMINAL_ARGS must be a JSON array of argument strings"
                            .into(),
                    )
                })?
                .unwrap_or_default()
        };
        #[cfg(unix)]
        let environment = env::var("SHELL").ok();
        #[cfg(not(unix))]
        let environment = env::var("COMSPEC").ok();
        let shell = select_shell(
            configured.as_deref(),
            args,
            environment.as_deref(),
            account_shell().as_deref(),
            &env::var_os("PATH").unwrap_or_default(),
            request.safe,
        )?;
        let process = self
            .start_with_options(
                ProcessStartRequest {
                    command: shell.command.clone(),
                    args: shell.args.clone(),
                    cwd: request.cwd,
                    pty: true,
                    cols: request.cols,
                    rows: request.rows,
                },
                request.safe,
            )
            .await?;
        Ok(TerminalStartResponse { process, shell })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn host_preference_and_configured_arguments_are_preserved() {
        let path = OsStr::new("/usr/bin:/bin");
        let shell = select_shell(
            None,
            vec![],
            Some("/usr/bin/zsh"),
            Some("/bin/sh"),
            path,
            false,
        )
        .unwrap();
        if Path::new("/usr/bin/zsh").exists() {
            assert_eq!(shell.command, "/usr/bin/zsh");
        }
        let shell = select_shell(
            Some("/bin/sh"),
            vec!["-l".into()],
            Some("/bin/bash"),
            None,
            path,
            false,
        )
        .unwrap();
        assert_eq!(shell.args, ["-l"]);
        assert_eq!(shell.source, "configured");
    }

    #[test]
    fn invalid_implicit_shells_fall_back_but_explicit_errors_are_visible() {
        let path = OsStr::new("/usr/bin:/bin");
        for invalid in [
            "/missing/shell",
            "/bin",
            "/usr/bin/false",
            "/usr/sbin/nologin",
            "/bin/sh -c touch /tmp/not-executed",
        ] {
            let shell =
                select_shell(None, vec![], Some(invalid), Some("/bin/sh"), path, false).unwrap();
            assert_eq!(shell.command, "/bin/sh");
            assert_eq!(shell.source, "account");
            assert!(select_shell(Some(invalid), vec![], None, None, path, false).is_err());
        }
    }

    #[test]
    fn executable_paths_with_spaces_are_not_evaluated_as_shell_code() {
        use std::os::unix::fs::PermissionsExt;
        let root = env::temp_dir().join(format!("iowb-shell-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let command = root.join("shell with spaces; literal");
        std::fs::write(&command, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
        let shell = select_shell(
            Some(command.to_str().unwrap()),
            vec![],
            None,
            None,
            OsStr::new(""),
            false,
        )
        .unwrap();
        assert_eq!(shell.command, command.to_str().unwrap());
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            select_shell(
                Some(command.to_str().unwrap()),
                vec![],
                None,
                None,
                OsStr::new(""),
                false
            )
            .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn safe_mode_ignores_a_broken_profile_without_reusing_its_arguments() {
        let shell = select_shell(
            Some("/missing/profile"),
            vec!["-c".into(), "exit 1".into()],
            None,
            None,
            OsStr::new("/usr/bin:/bin"),
            true,
        )
        .unwrap();
        assert!(shell.safe);
        assert_eq!(shell.source, "safe");
        assert_eq!(shell.args, ["--noprofile", "--norc", "-i"]);
    }
}
