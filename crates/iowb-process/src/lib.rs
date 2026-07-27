use std::{
    collections::HashMap,
    io::{Read, Write},
    path::PathBuf,
    process::Stdio,
    sync::{Arc, mpsc as std_mpsc},
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

#[derive(Clone)]
pub struct ProcessManager {
    processes: Arc<RwLock<HashMap<String, Arc<ProcessRecord>>>>,
    events: broadcast::Sender<ProcessEvent>,
}

struct ProcessRecord {
    info: ProcessInfo,
    control_tx: ProcessControlSender,
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

        self.processes.write().await.insert(
            id.clone(),
            Arc::new(ProcessRecord {
                info,
                control_tx: ProcessControlSender::Async {
                    input_tx,
                    kill_tx: Mutex::new(Some(kill_tx)),
                },
            }),
        );

        if let Some(stdin) = stdin {
            spawn_input_writer(self.events.clone(), id.clone(), stdin, input_rx);
        }

        if let Some(stdout) = stdout {
            spawn_output_reader(
                self.events.clone(),
                id.clone(),
                ProcessStream::Stdout,
                stdout,
            );
        }

        if let Some(stderr) = stderr {
            spawn_output_reader(
                self.events.clone(),
                id.clone(),
                ProcessStream::Stderr,
                stderr,
            );
        }

        let events = self.events.clone();
        let processes = Arc::clone(&self.processes);
        let process_id = id.clone();
        tokio::spawn(async move {
            tokio::select! {
                status = child.wait() => {
                    match status {
                        Ok(status) => {
                            let _ = events.send(ProcessEvent::Exited {
                                process_id: process_id.clone(),
                                code: status.code(),
                            });
                        }
                        Err(error) => {
                            let _ = events.send(ProcessEvent::Failed {
                                process_id: process_id.clone(),
                                message: error.to_string(),
                            });
                        }
                    }
                }
                _ = kill_rx => {
                    let kill_result = child.kill().await;
                    let _ = events.send(ProcessEvent::Exited {
                        process_id: process_id.clone(),
                        code: None,
                    });
                    if let Err(error) = kill_result {
                        let _ = events.send(ProcessEvent::Failed {
                            process_id: process_id.clone(),
                            message: error.to_string(),
                        });
                    }
                }
            }
            processes.write().await.remove(&process_id);
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

        self.processes.write().await.insert(
            id.clone(),
            Arc::new(ProcessRecord {
                info,
                control_tx: ProcessControlSender::Blocking(control_tx),
            }),
        );

        spawn_pty_reader(self.events.clone(), id.clone(), reader);
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
        tokio::task::spawn_blocking(move || {
            match child.wait() {
                Ok(status) => {
                    let code = i32::try_from(status.exit_code()).ok();
                    let _ = events.send(ProcessEvent::Exited {
                        process_id: process_id.clone(),
                        code,
                    });
                }
                Err(error) => {
                    let _ = events.send(ProcessEvent::Failed {
                        process_id: process_id.clone(),
                        message: error.to_string(),
                    });
                }
            }
            tokio::runtime::Handle::current().spawn(async move {
                processes.write().await.remove(&process_id);
            });
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

fn spawn_output_reader<R>(
    events: broadcast::Sender<ProcessEvent>,
    process_id: String,
    stream: ProcessStream,
    reader: R,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut reader = reader;
        let mut buffer = vec![0_u8; PROCESS_OUTPUT_CHUNK_BYTES];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    let _ = events.send(ProcessEvent::Output {
                        process_id: process_id.clone(),
                        stream,
                        data: String::from_utf8_lossy(&buffer[..read]).into_owned(),
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
    });
}

fn spawn_pty_reader(
    events: broadcast::Sender<ProcessEvent>,
    process_id: String,
    mut reader: Box<dyn Read + Send>,
) {
    std::thread::spawn(move || {
        let mut buffer = vec![0_u8; PROCESS_OUTPUT_CHUNK_BYTES];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let _ = events.send(ProcessEvent::Output {
                        process_id: process_id.clone(),
                        stream: ProcessStream::Stdout,
                        data: String::from_utf8_lossy(&buffer[..read]).into_owned(),
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
    });
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
    use tokio::time::{Duration, timeout};

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
}
