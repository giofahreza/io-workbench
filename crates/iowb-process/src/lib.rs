use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex as StdMutex, mpsc as std_mpsc},
    time::Duration,
};

use chrono::Utc;
use iowb_protocol::{ProcessInfo, ProcessStartRequest, ProcessStartResponse, ProcessStream};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{Mutex, RwLock, broadcast, mpsc, oneshot},
};
use uuid::Uuid;

const PROCESS_EVENT_CAPACITY: usize = 512;
const PROCESS_INPUT_CAPACITY: usize = 256;
const PROCESS_OUTPUT_CHUNK_BYTES: usize = 8192;
// Keep enough recent output for a client that reconnects to an active PTY,
// without letting a long-running terminal consume unbounded server memory.
const PROCESS_OUTPUT_REPLAY_BYTES: usize = 64 * 1024;
// A shell can leave an inherited pipe open after its own process exits. Drain
// ordinary process output briefly, then stop its readers so a terminal exit
// cannot be delayed indefinitely or followed by stale output events.
const PROCESS_OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
// A background descendant can keep a slave PTY open after its shell exits.
// Give the reader a short chance to drain, then gate further chunks so exit
// remains prompt and can never be followed by stale terminal output.
const PTY_OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("process not found")]
    NotFound,
    #[error("process command is empty")]
    EmptyCommand,
    #[error("process event stream is closed")]
    EventStreamClosed,
    #[error("process input stream is closed")]
    InputClosed,
}

pub type Result<T> = std::result::Result<T, ProcessError>;

#[derive(Debug, Clone)]
pub enum ProcessEvent {
    Output {
        process_id: String,
        stream: ProcessStream,
        data: String,
    },
    Exited {
        process_id: String,
        code: Option<i32>,
    },
    Failed {
        process_id: String,
        message: String,
    },
}

/// A bounded, in-memory output chunk that can be replayed to a client that
/// reconnects to an active process. It deliberately is not persisted: a
/// process and its terminal screen both end when the server stops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutputChunk {
    pub stream: ProcessStream,
    pub data: String,
}

#[derive(Clone)]
pub struct ProcessManager {
    processes: Arc<RwLock<HashMap<String, Arc<ProcessRecord>>>>,
    events: broadcast::Sender<ProcessEvent>,
}

struct ProcessRecord {
    info: ProcessInfo,
    control_tx: ProcessControlSender,
    output: Arc<StdMutex<ProcessOutputBuffer>>,
}

#[derive(Default)]
struct ProcessOutputBuffer {
    chunks: VecDeque<ProcessOutputChunk>,
    byte_len: usize,
}

impl ProcessOutputBuffer {
    fn push(&mut self, stream: ProcessStream, data: String) {
        self.byte_len = self.byte_len.saturating_add(data.len());
        self.chunks.push_back(ProcessOutputChunk { stream, data });
        while self.byte_len > PROCESS_OUTPUT_REPLAY_BYTES {
            let Some(removed) = self.chunks.pop_front() else {
                break;
            };
            self.byte_len = self.byte_len.saturating_sub(removed.data.len());
        }
    }

    fn snapshot(&self) -> Vec<ProcessOutputChunk> {
        self.chunks.iter().cloned().collect()
    }
}

enum ProcessControlSender {
    Async {
        input_tx: mpsc::Sender<Vec<u8>>,
        kill_tx: Mutex<Option<oneshot::Sender<()>>>,
    },
    Blocking(std_mpsc::Sender<ProcessControl>),
}

enum ProcessControl {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Kill,
}

impl ProcessManager {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(PROCESS_EVENT_CAPACITY);
        Self {
            processes: Arc::new(RwLock::new(HashMap::new())),
            events,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProcessEvent> {
        self.events.subscribe()
    }

    pub async fn start(&self, request: ProcessStartRequest) -> Result<ProcessStartResponse> {
        if request.command.trim().is_empty() {
            return Err(ProcessError::EmptyCommand);
        }

        if request.pty {
            return self.start_pty(request).await;
        }

        let id = format!("proc_{}", Uuid::new_v4().simple());
        let started_at = Utc::now();
        let cwd = request.cwd.as_ref().map(PathBuf::from);

        let mut command = Command::new(&request.command);
        command
            .args(&request.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The local Termux host keeps its loopback bearer in its own process
        // environment. A user terminal (and every CLI it starts) must never
        // inherit that host credential.
        command.env_remove("IO_WORKBENCH_TOKEN");

        if let Some(cwd) = &cwd {
            command.current_dir(cwd);
        }

        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (input_tx, input_rx) = mpsc::channel(PROCESS_INPUT_CAPACITY);
        let (kill_tx, kill_rx) = oneshot::channel();

        let info = ProcessInfo {
            id: id.clone(),
            command: request.command,
            args: request.args,
            cwd: cwd.map(|path| path.display().to_string()),
            started_at,
            pty: false,
        };

        let output = Arc::new(StdMutex::new(ProcessOutputBuffer::default()));
        self.processes.write().await.insert(
            id.clone(),
            Arc::new(ProcessRecord {
                info,
                control_tx: ProcessControlSender::Async {
                    input_tx,
                    kill_tx: Mutex::new(Some(kill_tx)),
                },
                output: Arc::clone(&output),
            }),
        );

        if let Some(stdin) = stdin {
            spawn_input_writer(self.events.clone(), id.clone(), stdin, input_rx);
        }

        let stdout_reader = stdout.map(|stdout| {
            spawn_output_reader(
                self.events.clone(),
                id.clone(),
                ProcessStream::Stdout,
                stdout,
                Arc::clone(&output),
            )
        });

        let stderr_reader = stderr.map(|stderr| {
            spawn_output_reader(
                self.events.clone(),
                id.clone(),
                ProcessStream::Stderr,
                stderr,
                Arc::clone(&output),
            )
        });

        let events = self.events.clone();
        let processes = Arc::clone(&self.processes);
        let process_id = id.clone();
        tokio::spawn(async move {
            let completion = tokio::select! {
                status = child.wait() => {
                    match status {
                        Ok(status) => ProcessCompletion::Exited(status.code()),
                        Err(error) => ProcessCompletion::Failed(error.to_string()),
                    }
                }
                _ = kill_rx => {
                    ProcessCompletion::Killed(child.kill().await.err().map(|error| error.to_string()))
                }
            };
            processes.write().await.remove(&process_id);
            drain_output_reader(stdout_reader).await;
            drain_output_reader(stderr_reader).await;

            match completion {
                ProcessCompletion::Exited(code) => {
                    let _ = events.send(ProcessEvent::Exited { process_id, code });
                }
                ProcessCompletion::Killed(error) => {
                    let _ = events.send(ProcessEvent::Exited {
                        process_id: process_id.clone(),
                        code: None,
                    });
                    if let Some(message) = error {
                        let _ = events.send(ProcessEvent::Failed {
                            process_id,
                            message,
                        });
                    }
                }
                ProcessCompletion::Failed(message) => {
                    let _ = events.send(ProcessEvent::Failed {
                        process_id,
                        message,
                    });
                }
            }
        });

        Ok(ProcessStartResponse { id, started_at })
    }

    async fn start_pty(&self, request: ProcessStartRequest) -> Result<ProcessStartResponse> {
        let id = format!("proc_{}", Uuid::new_v4().simple());
        let started_at = Utc::now();
        let cwd = request.cwd.as_ref().map(PathBuf::from);
        let (control_tx, control_rx) = std_mpsc::channel();
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: request.rows.max(1),
            cols: request.cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut command = CommandBuilder::new(&request.command);
        command.args(&request.args);
        // See the matching tokio Command call above. CommandBuilder starts
        // with the parent environment, so remove the host-only bearer before
        // the PTY child is spawned.
        command.env_remove("IO_WORKBENCH_TOKEN");
        configure_pty_environment(&mut command);
        if let Some(cwd) = &cwd {
            command.cwd(cwd);
        }

        let mut child = pair.slave.spawn_command(command)?;
        let killer = child.clone_killer();
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let master = pair.master;

        let info = ProcessInfo {
            id: id.clone(),
            command: request.command,
            args: request.args,
            cwd: cwd.map(|path| path.display().to_string()),
            started_at,
            pty: true,
        };

        let output = Arc::new(StdMutex::new(ProcessOutputBuffer::default()));
        self.processes.write().await.insert(
            id.clone(),
            Arc::new(ProcessRecord {
                info,
                control_tx: ProcessControlSender::Blocking(control_tx),
                output: Arc::clone(&output),
            }),
        );

        let output_gate = Arc::new(StdMutex::new(true));
        let reader_done = spawn_pty_reader(
            self.events.clone(),
            id.clone(),
            reader,
            Arc::clone(&output_gate),
            Arc::clone(&output),
        );
        spawn_pty_control(
            self.events.clone(),
            id.clone(),
            writer,
            master,
            killer,
            control_rx,
        );

        let events = self.events.clone();
        let processes = Arc::clone(&self.processes);
        let process_id = id.clone();
        tokio::spawn(async move {
            let completion = match tokio::task::spawn_blocking(move || child.wait()).await {
                Ok(Ok(status)) => ProcessCompletion::Exited(i32::try_from(status.exit_code()).ok()),
                Ok(Err(error)) => ProcessCompletion::Failed(error.to_string()),
                Err(error) => ProcessCompletion::Failed(format!("PTY wait task failed: {error}")),
            };

            // Dropping the control sender closes the master PTY once its
            // control thread wakes. Most readers then finish immediately,
            // but a background descendant can retain the slave indefinitely.
            // Bound that drain and close the output gate before publishing
            // exit, so clients never see a final chunk after their terminal
            // is marked closed.
            processes.write().await.remove(&process_id);
            let _ = tokio::time::timeout(PTY_OUTPUT_DRAIN_TIMEOUT, reader_done).await;

            let mut output_open = output_gate
                .lock()
                .expect("PTY output gate lock must not be poisoned");
            *output_open = false;
            match completion {
                ProcessCompletion::Exited(code) => {
                    let _ = events.send(ProcessEvent::Exited { process_id, code });
                }
                ProcessCompletion::Killed(_) => unreachable!("PTY control handles termination"),
                ProcessCompletion::Failed(message) => {
                    let _ = events.send(ProcessEvent::Failed {
                        process_id,
                        message,
                    });
                }
            }
            drop(output_open);
        });

        Ok(ProcessStartResponse { id, started_at })
    }

    async fn send_control(&self, process_id: &str, control: ProcessControl) -> Result<()> {
        let record = {
            let processes = self.processes.read().await;
            processes.get(process_id).map(Arc::clone)
        }
        .ok_or(ProcessError::NotFound)?;

        match (&record.control_tx, control) {
            (ProcessControlSender::Async { input_tx, .. }, ProcessControl::Input(data)) => input_tx
                .send(data)
                .await
                .map_err(|_| ProcessError::InputClosed),
            (ProcessControlSender::Async { kill_tx, .. }, ProcessControl::Kill) => {
                if let Some(kill_tx) = kill_tx.lock().await.take() {
                    let _ = kill_tx.send(());
                }
                Ok(())
            }
            (ProcessControlSender::Async { .. }, ProcessControl::Resize { .. }) => Ok(()),
            (ProcessControlSender::Blocking(tx), control) => {
                tx.send(control).map_err(|_| ProcessError::InputClosed)
            }
        }
    }

    pub async fn list(&self) -> Vec<ProcessInfo> {
        self.processes
            .read()
            .await
            .values()
            .map(|record| record.info.clone())
            .collect()
    }

    /// Returns the recent output for an active process. This is used only to
    /// restore a terminal view after a client reconnects; normal output still
    /// flows through the live event stream.
    pub async fn output_snapshot(&self, process_id: &str) -> Result<Vec<ProcessOutputChunk>> {
        let output = {
            let processes = self.processes.read().await;
            processes
                .get(process_id)
                .map(|record| Arc::clone(&record.output))
        }
        .ok_or(ProcessError::NotFound)?;
        Ok(output
            .lock()
            .expect("process output buffer lock must not be poisoned")
            .snapshot())
    }

    pub async fn abort(&self, process_id: &str) -> Result<()> {
        self.send_control(process_id, ProcessControl::Kill).await
    }

    pub async fn write_input(&self, process_id: &str, data: impl Into<Vec<u8>>) -> Result<()> {
        self.send_control(process_id, ProcessControl::Input(data.into()))
            .await
    }

    pub async fn resize_terminal(&self, process_id: &str, cols: u16, rows: u16) -> Result<()> {
        self.send_control(process_id, ProcessControl::Resize { cols, rows })
            .await
    }
}

impl From<anyhow::Error> for ProcessError {
    fn from(error: anyhow::Error) -> Self {
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            return ProcessError::Io(std::io::Error::new(io.kind(), io.to_string()));
        }
        ProcessError::Io(std::io::Error::other(error.to_string()))
    }
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

enum ProcessCompletion {
    Exited(Option<i32>),
    Killed(Option<String>),
    Failed(String),
}

async fn drain_output_reader(reader: Option<tokio::task::JoinHandle<()>>) {
    let Some(mut reader) = reader else {
        return;
    };
    if tokio::time::timeout(PROCESS_OUTPUT_DRAIN_TIMEOUT, &mut reader)
        .await
        .is_err()
    {
        reader.abort();
        let _ = reader.await;
    }
}

fn spawn_output_reader<R>(
    events: broadcast::Sender<ProcessEvent>,
    process_id: String,
    stream: ProcessStream,
    reader: R,
    output: Arc<StdMutex<ProcessOutputBuffer>>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut reader = reader;
        let mut buffer = vec![0_u8; PROCESS_OUTPUT_CHUNK_BYTES];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    let data = String::from_utf8_lossy(&buffer[..read]).into_owned();
                    output
                        .lock()
                        .expect("process output buffer lock must not be poisoned")
                        .push(stream, data.clone());
                    let _ = events.send(ProcessEvent::Output {
                        process_id: process_id.clone(),
                        stream,
                        data,
                    });
                }
                Err(error) => {
                    let _ = events.send(ProcessEvent::Failed {
                        process_id: process_id.clone(),
                        message: error.to_string(),
                    });
                    break;
                }
            }
        }
    })
}

fn spawn_pty_reader(
    events: broadcast::Sender<ProcessEvent>,
    process_id: String,
    mut reader: Box<dyn Read + Send>,
    output_gate: Arc<StdMutex<bool>>,
    output: Arc<StdMutex<ProcessOutputBuffer>>,
) -> oneshot::Receiver<()> {
    let (done_tx, done_rx) = oneshot::channel();
    std::thread::spawn(move || {
        let mut buffer = vec![0_u8; PROCESS_OUTPUT_CHUNK_BYTES];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let output_open = output_gate
                        .lock()
                        .expect("PTY output gate lock must not be poisoned");
                    if *output_open {
                        let data = String::from_utf8_lossy(&buffer[..read]).into_owned();
                        output
                            .lock()
                            .expect("process output buffer lock must not be poisoned")
                            .push(ProcessStream::Stdout, data.clone());
                        let _ = events.send(ProcessEvent::Output {
                            process_id: process_id.clone(),
                            stream: ProcessStream::Stdout,
                            data,
                        });
                    }
                }
                Err(error) if is_normal_pty_close(&error) => break,
                Err(error) => {
                    let _ = events.send(ProcessEvent::Failed {
                        process_id: process_id.clone(),
                        message: error.to_string(),
                    });
                    break;
                }
            }
        }
        let _ = done_tx.send(());
    });
    done_rx
}

fn is_normal_pty_close(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        // Unix PTY masters report EIO when the slave closes. It is their
        // end-of-stream signal, not a user-visible terminal failure.
        error.raw_os_error() == Some(5)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

fn configure_pty_environment(command: &mut CommandBuilder) {
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "io-workbench");
}

fn spawn_pty_control(
    events: broadcast::Sender<ProcessEvent>,
    process_id: String,
    mut writer: Box<dyn Write + Send>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    mut killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    control_rx: std_mpsc::Receiver<ProcessControl>,
) {
    std::thread::spawn(move || {
        loop {
            match control_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(ProcessControl::Input(data)) => {
                    if let Err(error) = writer.write_all(&data).and_then(|_| writer.flush()) {
                        let _ = events.send(ProcessEvent::Failed {
                            process_id: process_id.clone(),
                            message: error.to_string(),
                        });
                        break;
                    }
                }
                Ok(ProcessControl::Resize { cols, rows }) => {
                    if let Err(error) = master.resize(PtySize {
                        rows: rows.max(1),
                        cols: cols.max(1),
                        pixel_width: 0,
                        pixel_height: 0,
                    }) {
                        let _ = events.send(ProcessEvent::Failed {
                            process_id: process_id.clone(),
                            message: error.to_string(),
                        });
                    }
                }
                Ok(ProcessControl::Kill) => {
                    if let Err(error) = killer.kill() {
                        let _ = events.send(ProcessEvent::Failed {
                            process_id: process_id.clone(),
                            message: error.to_string(),
                        });
                    }
                    break;
                }
                Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}

fn spawn_input_writer(
    events: broadcast::Sender<ProcessEvent>,
    process_id: String,
    mut stdin: tokio::process::ChildStdin,
    mut input_rx: mpsc::Receiver<Vec<u8>>,
) {
    tokio::spawn(async move {
        while let Some(data) = input_rx.recv().await {
            if let Err(error) = stdin.write_all(&data).await {
                let _ = events.send(ProcessEvent::Failed {
                    process_id: process_id.clone(),
                    message: error.to_string(),
                });
                break;
            }
            if let Err(error) = stdin.flush().await {
                let _ = events.send(ProcessEvent::Failed {
                    process_id: process_id.clone(),
                    message: error.to_string(),
                });
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration, Instant, timeout};

    #[tokio::test]
    async fn writes_process_input_to_child_stdin() {
        let manager = ProcessManager::new();
        let mut events = manager.subscribe();
        let started = manager
            .start(ProcessStartRequest {
                command: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    "read line; printf 'got:%s\\n' \"$line\"".to_string(),
                ],
                cwd: None,
                pty: false,
                cols: 80,
                rows: 24,
            })
            .await
            .expect("process starts");

        manager
            .write_input(&started.id, b"hello\n".to_vec())
            .await
            .expect("stdin accepts input");

        let mut saw_output = false;
        for _ in 0..8 {
            match timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("event arrives")
                .expect("event stream open")
            {
                ProcessEvent::Output {
                    process_id,
                    stream: ProcessStream::Stdout,
                    data,
                } if process_id == started.id && data.contains("got:hello") => {
                    saw_output = true;
                    break;
                }
                _ => {}
            }
        }

        assert!(saw_output);
    }

    #[tokio::test]
    async fn pty_process_accepts_input_and_emits_output() {
        let manager = ProcessManager::new();
        let mut events = manager.subscribe();
        let started = manager
            .start(ProcessStartRequest {
                command: "/bin/sh".to_string(),
                args: vec![
                    "-lc".to_string(),
                    "read line; printf 'pty:%s\\n' \"$line\"".to_string(),
                ],
                cwd: None,
                pty: true,
                cols: 80,
                rows: 24,
            })
            .await
            .expect("pty process starts");

        manager
            .write_input(&started.id, b"hello\n".to_vec())
            .await
            .expect("pty stdin accepts input");
        manager
            .resize_terminal(&started.id, 100, 30)
            .await
            .expect("pty resize succeeds");

        let mut saw_output = false;
        for _ in 0..12 {
            match timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("event arrives")
                .expect("event stream open")
            {
                ProcessEvent::Output {
                    process_id, data, ..
                } if process_id == started.id && data.contains("pty:hello") => {
                    saw_output = true;
                    break;
                }
                _ => {}
            }
        }

        assert!(saw_output);
    }

    #[tokio::test]
    async fn pty_exit_is_published_after_final_output() {
        let manager = ProcessManager::new();
        let mut events = manager.subscribe();
        let started = manager
            .start(ProcessStartRequest {
                command: "/bin/sh".to_string(),
                args: vec![
                    "-lc".to_string(),
                    "printf 'PTY_FINAL_OUTPUT\\n'".to_string(),
                ],
                cwd: None,
                pty: true,
                cols: 80,
                rows: 24,
            })
            .await
            .expect("PTY process starts");

        let mut saw_final_output = false;
        for _ in 0..12 {
            match timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("event arrives")
                .expect("event stream open")
            {
                ProcessEvent::Output {
                    process_id, data, ..
                } if process_id == started.id && data.contains("PTY_FINAL_OUTPUT") => {
                    saw_final_output = true;
                }
                ProcessEvent::Exited { process_id, .. } if process_id == started.id => {
                    assert!(
                        saw_final_output,
                        "terminal exit must not precede its final PTY output"
                    );
                    return;
                }
                ProcessEvent::Failed {
                    process_id,
                    message,
                } if process_id == started.id => panic!("PTY process failed: {message}"),
                _ => {}
            }
        }

        panic!("did not receive terminal exit for the PTY process");
    }

    #[tokio::test]
    async fn pty_exit_stays_prompt_when_a_background_child_keeps_the_slave_open() {
        let manager = ProcessManager::new();
        let mut events = manager.subscribe();
        let started = manager
            .start(ProcessStartRequest {
                command: "/bin/sh".to_string(),
                args: vec![
                    "-lc".to_string(),
                    "(sleep 2; printf 'PTY_LATE_OUTPUT\\n') & printf 'PTY_EARLY_OUTPUT\\n'"
                        .to_string(),
                ],
                cwd: None,
                pty: true,
                cols: 80,
                rows: 24,
            })
            .await
            .expect("PTY process starts");

        let mut saw_early_output = false;
        let exit_deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = exit_deadline
                .checked_duration_since(Instant::now())
                .expect("PTY exit must be prompt despite its background child");
            match timeout(remaining, events.recv())
                .await
                .expect("event arrives before the exit deadline")
                .expect("event stream open")
            {
                ProcessEvent::Output {
                    process_id, data, ..
                } if process_id == started.id && data.contains("PTY_EARLY_OUTPUT") => {
                    saw_early_output = true;
                }
                ProcessEvent::Exited { process_id, .. } if process_id == started.id => break,
                ProcessEvent::Failed {
                    process_id,
                    message,
                } if process_id == started.id => panic!("PTY process failed: {message}"),
                _ => {}
            }
        }
        assert!(
            saw_early_output,
            "terminal exit must retain already-read output"
        );

        // The child still owns the slave for roughly another second. The
        // output gate must prevent that late data from following exit.
        let late_output_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let Some(remaining) = late_output_deadline.checked_duration_since(Instant::now())
            else {
                break;
            };
            match timeout(remaining, events.recv()).await {
                Err(_) => break,
                Ok(Ok(ProcessEvent::Output {
                    process_id, data, ..
                })) if process_id == started.id && data.contains("PTY_LATE_OUTPUT") => {
                    panic!("terminal output must not follow its exit event");
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => panic!("event stream failed: {error}"),
            }
        }
    }

    #[tokio::test]
    async fn pipe_exit_is_published_after_stdout_and_stderr() {
        let manager = ProcessManager::new();
        let mut events = manager.subscribe();
        let started = manager
            .start(ProcessStartRequest {
                command: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    "printf 'PIPE_STDOUT\\n'; printf 'PIPE_STDERR\\n' >&2".to_string(),
                ],
                cwd: None,
                pty: false,
                cols: 80,
                rows: 24,
            })
            .await
            .expect("pipe process starts");

        let mut saw_stdout = false;
        let mut saw_stderr = false;
        for _ in 0..12 {
            match timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("event arrives")
                .expect("event stream open")
            {
                ProcessEvent::Output {
                    process_id,
                    stream: ProcessStream::Stdout,
                    data,
                } if process_id == started.id && data.contains("PIPE_STDOUT") => {
                    saw_stdout = true;
                }
                ProcessEvent::Output {
                    process_id,
                    stream: ProcessStream::Stderr,
                    data,
                } if process_id == started.id && data.contains("PIPE_STDERR") => {
                    saw_stderr = true;
                }
                ProcessEvent::Exited { process_id, .. } if process_id == started.id => {
                    assert!(saw_stdout, "process exit must follow stdout output");
                    assert!(saw_stderr, "process exit must follow stderr output");
                    return;
                }
                ProcessEvent::Failed {
                    process_id,
                    message,
                } if process_id == started.id => panic!("pipe process failed: {message}"),
                _ => {}
            }
        }

        panic!("did not receive process exit for the pipe process");
    }

    #[tokio::test]
    async fn pty_process_gets_browser_terminal_environment() {
        let manager = ProcessManager::new();
        let mut events = manager.subscribe();
        let started = manager
            .start(ProcessStartRequest {
                command: "/bin/sh".to_string(),
                args: vec![
                    "-lc".to_string(),
                    "printf 'term:%s colorterm:%s term_program:%s\\n' \"$TERM\" \"$COLORTERM\" \"$TERM_PROGRAM\"".to_string(),
                ],
                cwd: None,
                pty: true,
                cols: 80,
                rows: 24,
            })
            .await
            .expect("pty process starts");

        let mut saw_output = false;
        for _ in 0..12 {
            match timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("event arrives")
                .expect("event stream open")
            {
                ProcessEvent::Output {
                    process_id, data, ..
                } if process_id == started.id
                    && data.contains("term:xterm-256color")
                    && data.contains("colorterm:truecolor")
                    && data.contains("term_program:io-workbench") =>
                {
                    saw_output = true;
                    break;
                }
                _ => {}
            }
        }

        assert!(saw_output);
    }

    #[tokio::test]
    async fn active_pty_output_can_be_replayed_after_a_client_reconnect() {
        let manager = ProcessManager::new();
        let mut events = manager.subscribe();
        let started = manager
            .start(ProcessStartRequest {
                command: "/bin/sh".to_string(),
                args: vec![
                    "-lc".to_string(),
                    "printf 'PTY_REPLAY_MARKER'; read line".to_string(),
                ],
                cwd: None,
                pty: true,
                cols: 80,
                rows: 24,
            })
            .await
            .expect("PTY starts");

        for _ in 0..12 {
            match timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("event arrives")
                .expect("event stream open")
            {
                ProcessEvent::Output {
                    process_id, data, ..
                } if process_id == started.id && data.contains("PTY_REPLAY_MARKER") => break,
                ProcessEvent::Failed {
                    process_id,
                    message,
                } if process_id == started.id => panic!("PTY process failed: {message}"),
                _ => continue,
            }
        }

        let replay = manager
            .output_snapshot(&started.id)
            .await
            .expect("active PTY has a replay buffer");
        assert!(
            replay
                .iter()
                .any(|chunk| chunk.data.contains("PTY_REPLAY_MARKER")),
            "the active PTY transcript should include output produced before a client reconnects"
        );

        manager
            .write_input(&started.id, b"\n".to_vec())
            .await
            .expect("PTY accepts cleanup input");
    }

    #[test]
    fn process_output_replay_keeps_a_bounded_tail() {
        let mut output = ProcessOutputBuffer::default();
        output.push(
            ProcessStream::Stdout,
            "a".repeat(PROCESS_OUTPUT_REPLAY_BYTES),
        );
        output.push(ProcessStream::Stderr, "b".to_string());

        let replay = output.snapshot();
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].stream, ProcessStream::Stderr);
        assert_eq!(replay[0].data, "b");
    }
}
