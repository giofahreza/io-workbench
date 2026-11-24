use super::*;
use iowb_protocol::TerminalSnapshotResponse;
use std::time::Instant;

const RETIRED_PROCESS_LIMIT: usize = 64;
const RETIRED_PROCESS_TTL: Duration = Duration::from_secs(300);

#[derive(Default)]
pub(super) struct RetiredProcesses {
    entries: HashMap<String, RetiredProcess>,
}

struct RetiredProcess {
    info: ProcessInfo,
    output: Arc<StdMutex<ProcessOutputBuffer>>,
    retired_at: Instant,
    result: Option<(Option<i32>, Option<String>)>,
}

impl RetiredProcesses {
    fn prune(&mut self) {
        self.entries
            .retain(|_, record| record.retired_at.elapsed() < RETIRED_PROCESS_TTL);
        while self.entries.len() > RETIRED_PROCESS_LIMIT {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, record)| record.retired_at)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                self.entries.remove(&id);
            }
        }
    }

    pub(super) fn retain(&mut self, record: &ProcessRecord) {
        self.entries.insert(
            record.info.id.clone(),
            RetiredProcess {
                info: record.info.clone(),
                output: Arc::clone(&record.output),
                retired_at: Instant::now(),
                result: None,
            },
        );
        self.prune();
    }

    pub(super) fn finish(&mut self, id: &str, completion: &ProcessCompletion) {
        if let Some(record) = self.entries.get_mut(id) {
            record.result = Some(match completion {
                ProcessCompletion::Exited(code) => (*code, None),
                ProcessCompletion::Killed(error) => (None, error.clone()),
                ProcessCompletion::Failed(error) => (None, Some(error.clone())),
            });
        }
    }
}

fn snapshot(
    info: &ProcessInfo,
    output: &ProcessOutputBuffer,
    result: Option<&(Option<i32>, Option<String>)>,
    after_sequence: u64,
) -> TerminalSnapshotResponse {
    TerminalSnapshotResponse {
        process: info.clone(),
        chunks: output
            .chunks
            .iter()
            .filter(|chunk| chunk.sequence > after_sequence)
            .cloned()
            .collect(),
        first_sequence: output
            .chunks
            .front()
            .map_or(output.sequence + 1, |chunk| chunk.sequence),
        last_sequence: output.sequence,
        running: result.is_none(),
        exit_code: result.and_then(|(code, _)| *code),
        error: result.and_then(|(_, error)| error.clone()),
    }
}

impl ProcessManager {
    pub async fn terminal_snapshot(
        &self,
        id: &str,
        after_sequence: u64,
    ) -> Result<TerminalSnapshotResponse> {
        let active = self.processes.read().await;
        if let Some(record) = active.get(id) {
            let output = record.output.lock().expect("process output lock");
            return Ok(snapshot(&record.info, &output, None, after_sequence));
        }
        drop(active);
        let mut retired = self.retired.lock().expect("retired process lock");
        retired.prune();
        let record = retired.entries.get(id).ok_or(ProcessError::NotFound)?;
        let output = record.output.lock().expect("process output lock");
        Ok(snapshot(
            &record.info,
            &output,
            record.result.as_ref(),
            after_sequence,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[cfg(unix)]
    async fn fast_exit_retains_final_output_status_and_sequence_cursor() {
        for pty in [false, true] {
            let manager = ProcessManager::new();
            let mut events = manager.subscribe();
            let started = manager
                .start(ProcessStartRequest {
                    command: "/bin/sh".into(),
                    args: vec!["-c".into(), "printf 'FINAL_OUTPUT'; exit 7".into()],
                    cwd: None,
                    pty,
                    cols: 80,
                    rows: 24,
                })
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if matches!(events.recv().await.unwrap(), ProcessEvent::Exited { process_id, .. } if process_id == started.id) {
                        break;
                    }
                }
            }).await.unwrap();
            assert!(manager.list().await.is_empty());
            let snapshot = manager.terminal_snapshot(&started.id, 0).await.unwrap();
            assert!(!snapshot.running);
            assert_eq!(snapshot.exit_code, Some(7));
            assert!(
                snapshot
                    .chunks
                    .iter()
                    .any(|chunk| chunk.data.contains("FINAL_OUTPUT"))
            );
            assert!(snapshot.last_sequence > 0);
            assert!(
                manager
                    .terminal_snapshot(&started.id, snapshot.last_sequence)
                    .await
                    .unwrap()
                    .chunks
                    .is_empty()
            );
            assert!(matches!(
                manager.write_input(&started.id, b"no".to_vec()).await,
                Err(ProcessError::NotFound)
            ));
        }
    }

    #[test]
    fn retired_history_is_time_and_count_bounded() {
        let mut retired = RetiredProcesses::default();
        for index in 0..=RETIRED_PROCESS_LIMIT {
            retired.entries.insert(
                index.to_string(),
                RetiredProcess {
                    info: ProcessInfo {
                        id: index.to_string(),
                        command: "sh".into(),
                        args: vec![],
                        cwd: None,
                        started_at: Utc::now(),
                        pty: true,
                    },
                    output: Arc::new(StdMutex::new(ProcessOutputBuffer::default())),
                    retired_at: Instant::now(),
                    result: Some((Some(0), None)),
                },
            );
        }
        retired.prune();
        assert_eq!(retired.entries.len(), RETIRED_PROCESS_LIMIT);
        retired
            .entries
            .values_mut()
            .for_each(|record| record.retired_at -= RETIRED_PROCESS_TTL);
        retired.prune();
        assert!(retired.entries.is_empty());
    }

    #[test]
    fn snapshot_watermarks_report_ring_eviction_and_final_errors() {
        let info = ProcessInfo {
            id: "snapshot-test".into(),
            command: "sh".into(),
            args: vec![],
            cwd: None,
            started_at: Utc::now(),
            pty: true,
        };
        let mut output = ProcessOutputBuffer::default();
        let empty = snapshot(&info, &output, None, 0);
        assert_eq!((empty.first_sequence, empty.last_sequence), (1, 0));
        assert!(empty.running);
        for _ in 0..20 {
            output.push(
                ProcessStream::Stdout,
                "x".repeat(PROCESS_OUTPUT_CHUNK_BYTES),
            );
        }
        let retained = snapshot(&info, &output, None, 0);
        assert!(retained.first_sequence > 1);
        assert_eq!(retained.last_sequence, 20);
        assert_eq!(
            retained.chunks.first().unwrap().sequence,
            retained.first_sequence
        );
        assert!(output.byte_len <= PROCESS_OUTPUT_REPLAY_BYTES);
        let result = (None, Some("wait failed".into()));
        let finished = snapshot(&info, &output, Some(&result), retained.last_sequence);
        assert!(finished.chunks.is_empty());
        assert!(!finished.running);
        assert_eq!(finished.error.as_deref(), Some("wait failed"));
    }
}
