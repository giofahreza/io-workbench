type AppServerWriter = Box<dyn tokio::io::AsyncWrite + Unpin + Send>;
type AppServerReader = Box<dyn tokio::io::AsyncRead + Unpin + Send>;
struct AppServerLines {
    lines: tokio::io::Lines<BufReader<AppServerReader>>,
    pending: std::collections::VecDeque<String>,
}

impl AppServerLines {
    fn new(reader: AppServerReader) -> Self {
        Self {
            lines: BufReader::new(reader).lines(),
            pending: Default::default(),
        }
    }

    async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        if let Some(line) = self.pending.pop_front() {
            return Ok(Some(line));
        }
        self.lines.next_line().await
    }

    fn prepend(&mut self, messages: Vec<String>) {
        for message in messages.into_iter().rev() {
            self.pending.push_front(message);
        }
    }
}

struct AppServerConnection {
    child: Option<Child>,
    bridge: Option<tokio::task::JoinHandle<()>>,
    stderr: AppServerStderr,
    shared: bool,
}

impl AppServerConnection {
    async fn close(&mut self) {
        if let Some(bridge) = self.bridge.take() {
            bridge.abort();
            let _ = bridge.await;
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
            let _ = timeout(Duration::from_secs(1), child.wait()).await;
        }
    }
}

impl Drop for AppServerConnection {
    fn drop(&mut self) {
        if let Some(bridge) = self.bridge.take() {
            bridge.abort();
        }
    }
}

impl CodexAppServerClient {
    async fn connect(
        &self,
        options: &CodexAppServerLaunchOptions,
        operation: &'static str,
    ) -> Result<(AppServerConnection, AppServerWriter, AppServerLines)> {
        if options.transport != CodexAppServerTransport::Stdio {
            if !options.env.is_empty() {
                return Err(CoreError::InvalidInput(
                    "Shared Codex runtimes cannot receive per-run credentials or environment overrides".into(),
                ));
            }
            #[cfg(unix)]
            {
                let socket = match &options.transport {
                    CodexAppServerTransport::UnixSocket(path) => Some(path.clone()),
                    CodexAppServerTransport::NativeDaemon => {
                        self.native_daemon_socket(options.codex_home.as_deref())
                            .await?
                    }
                    CodexAppServerTransport::Stdio => None,
                };
                if let Some(socket) = socket {
                    return self.connect_socket(&socket).await;
                }
            }
        }
        let mut child = app_server_command(&self.command, options)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stderr = start_stderr_capture(&mut child, operation);
        let stdin: AppServerWriter = Box::new(child.stdin.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdin was unavailable",
            ))
        })?);
        let stdout: AppServerReader = Box::new(child.stdout.take().ok_or_else(|| {
            CoreError::Io(std::io::Error::other(
                "Codex app-server stdout was unavailable",
            ))
        })?);
        Ok((
            AppServerConnection {
                child: Some(child),
                bridge: None,
                stderr,
                shared: false,
            },
            stdin,
            AppServerLines::new(stdout),
        ))
    }

    #[cfg(unix)]
    async fn native_daemon_socket(
        &self,
        codex_home: Option<&std::path::Path>,
    ) -> Result<Option<PathBuf>> {
        let mut command = Command::new(&self.command);
        command
            .args(["app-server", "daemon", "start"])
            .env("PATH", augmented_user_path())
            .env_remove("IO_WORKBENCH_TOKEN")
            .env_remove("IOWB_IO_GATEWAY_API_KEY")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        if let Some(home) = codex_home {
            command.env("CODEX_HOME", home);
        }
        let output = timeout(
            self.request_timeout.max(Duration::from_secs(30)),
            command.output(),
        )
        .await
        .map_err(|_| {
            live_turn_setup_timeout(
                "start native daemon",
                self.request_timeout.max(Duration::from_secs(30)),
            )
        })??;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("unrecognized subcommand 'daemon'")
                || stderr.contains("unexpected argument 'daemon'")
            {
                return Ok(None);
            }
            return Err(CoreError::InvalidInput(
                "Could not start the native Codex daemon. Run `codex app-server daemon start` as the Workbench service user to diagnose it. The existing CLI was not stopped.".into(),
            ));
        }
        let response: Value = serde_json::from_slice(&output.stdout)?;
        response
            .get("socketPath")
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .map(Some)
            .ok_or_else(|| {
                CoreError::InvalidInput(
                    "Native Codex daemon did not return its control socket".into(),
                )
            })
    }

    #[cfg(unix)]
    async fn connect_socket(
        &self,
        socket: &std::path::Path,
    ) -> Result<(AppServerConnection, AppServerWriter, AppServerLines)> {
        use std::os::unix::fs::MetadataExt;
        if !socket.is_absolute() {
            return Err(CoreError::InvalidInput(
                "Codex control socket path must be absolute".into(),
            ));
        }
        let metadata = tokio::fs::metadata(socket).await?;
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(CoreError::Forbidden(
                "Codex control socket belongs to another user".into(),
            ));
        }
        let stream = tokio::net::UnixStream::connect(socket).await?;
        let (websocket, _) = timeout(
            self.request_timeout,
            tokio_tungstenite::client_async("ws://localhost/", stream),
        )
        .await
        .map_err(|_| live_turn_setup_timeout("connect native socket", self.request_timeout))?
        .map_err(|error| CoreError::Io(std::io::Error::other(error)))?;
        let (client, bridge_io) = tokio::io::duplex(128 * 1024);
        let (reader, writer) = tokio::io::split(client);
        let stderr = Arc::new(Mutex::new(String::new()));
        let diagnostics = stderr.clone();
        let bridge = tokio::spawn(async move {
            if let Err(error) = bridge_websocket(websocket, bridge_io).await {
                append_stderr_bounded(&mut *diagnostics.lock().await, &error.to_string());
            }
        });
        Ok((
            AppServerConnection {
                child: None,
                bridge: Some(bridge),
                stderr,
                shared: true,
            },
            Box::new(writer),
            AppServerLines::new(Box::new(reader)),
        ))
    }
}

fn validate_shared_codex_home(result: &Value, home: Option<&std::path::Path>) -> Result<()> {
    let expected = home
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
    let actual = result
        .get("codexHome")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let same = expected
        .as_ref()
        .zip(actual.as_ref())
        .is_some_and(|(expected, actual)| {
            std::fs::canonicalize(expected)
                .ok()
                .zip(std::fs::canonicalize(actual).ok())
                .is_some_and(|(expected, actual)| expected == actual)
        });
    if !same {
        return Err(CoreError::Conflict("Codex control socket uses a different CODEX_HOME. Connect Workbench to the same Codex environment as the CLI; no thread was changed.".into()));
    }
    Ok(())
}

#[cfg(unix)]
async fn bridge_websocket(
    mut websocket: tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    bridge_io: tokio::io::DuplexStream,
) -> Result<()> {
    use tokio_tungstenite::tungstenite::Message;
    let (reader, mut writer) = tokio::io::split(bridge_io);
    let mut lines = BufReader::new(reader).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                websocket.send(Message::Text(line.into())).await
                    .map_err(|error| CoreError::Io(std::io::Error::other(error)))?;
            }
            message = websocket.next() => {
                let Some(message) = message else { break };
                match message.map_err(|error| CoreError::Io(std::io::Error::other(error)))? {
                    Message::Text(text) => {
                        let value: Value = serde_json::from_str(&text)?;
                        let mut encoded = serde_json::to_vec(&value)?;
                        encoded.push(b'\n');
                        writer.write_all(&encoded).await?;
                    }
                    Message::Ping(payload) => {
                        websocket.send(Message::Pong(payload)).await
                            .map_err(|error| CoreError::Io(std::io::Error::other(error)))?;
                    }
                    Message::Close(_) => break,
                    Message::Pong(_) => {},
                    _ => return Err(CoreError::InvalidInput("Codex sent a non-text protocol frame".into())),
                }
            }
        }
    }
    Ok(())
}
