async fn terminate_agent_process_tree(child: &mut tokio::process::Child, session_id: &str) {
    let process_id = child.id();
    let mut tree_signal_sent = false;

    #[cfg(unix)]
    if let Some(process_id) = process_id {
        match signal_agent_process_group(process_id, libc::SIGTERM) {
            Ok(()) => tree_signal_sent = true,
            Err(error) => {
                warn!(error = %error, session_id, process_id, "failed to terminate agent process group");
            }
        }
        tokio::time::sleep(AGENT_ABORT_TERM_GRACE).await;
        match signal_agent_process_group(process_id, libc::SIGKILL) {
            Ok(()) => tree_signal_sent = true,
            Err(error) => {
                warn!(error = %error, session_id, process_id, "failed to kill agent process group");
            }
        }
    }

    #[cfg(windows)]
    if let Some(process_id) = process_id {
        tree_signal_sent = Command::new("taskkill")
            .args(["/PID", &process_id.to_string(), "/T", "/F"])
            .status()
            .await
            .is_ok_and(|status| status.success());
    }

    if !tree_signal_sent {
        let _ = child.start_kill();
    }

    match tokio::time::timeout(AGENT_ABORT_REAP_TIMEOUT, child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            warn!(error = %error, session_id, "failed to reap aborted agent process");
        }
        Err(_) => {
            let _ = child.start_kill();
            warn!(session_id, "timed out reaping aborted agent process");
        }
    }
}

#[cfg(unix)]
fn signal_agent_process_group(process_id: u32, signal: i32) -> std::io::Result<()> {
    let process_group = i32::try_from(process_id)
        .map_err(|_| std::io::Error::other("agent process id exceeds i32"))?;
    // The child is spawned as its own process-group leader, so a negative PID
    // reaches launchers and every descendant that inherits the group.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

async fn drain_aborted_agent_output(output_rx: &mut mpsc::Receiver<AgentProcessEvent>) {
    output_rx.close();
    let started = Instant::now();
    while let Some(remaining) = AGENT_ABORT_OUTPUT_DRAIN_TIMEOUT.checked_sub(started.elapsed()) {
        match tokio::time::timeout(remaining, output_rx.recv()).await {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
}
