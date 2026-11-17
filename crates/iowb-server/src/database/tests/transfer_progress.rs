    // These fixtures are private, create-new local files. The tests never use
    // live-engine credentials, an application server, or an Android device.
    struct TransferProgressFixture {
        source_path: std::path::PathBuf,
        target_path: std::path::PathBuf,
    }

    impl TransferProgressFixture {
        fn new(rows: usize, existing_target: bool) -> Self {
            let source_path = env::temp_dir().join(format!("{}.sqlite", new_id("progress-source")));
            let target_path = env::temp_dir().join(format!("{}.sqlite", new_id("progress-target")));
            for path in [&source_path, &target_path] {
                std::fs::OpenOptions::new().write(true).create_new(true).open(path)
                    .expect("reserve a fresh progress fixture file");
            }
            let fixture = Self { source_path, target_path };
            let mut source = Connection::open(&fixture.source_path).expect("open progress source");
            source.execute_batch("CREATE TABLE records (id TEXT PRIMARY KEY, value TEXT NOT NULL);")
                .expect("create progress source");
            let transaction = source.transaction().expect("seed transaction");
            {
                let mut insert = transaction.prepare("INSERT INTO records VALUES (?1, ?2)")
                    .expect("prepare progress seed");
                for index in 1..=rows {
                    insert.execute(params![format!("{index:03}"), format!("row-{index}")])
                        .expect("seed source row");
                }
            }
            transaction.commit().expect("commit source seed");
            if existing_target {
                Connection::open(&fixture.target_path).expect("open progress target")
                    .execute_batch("CREATE TABLE records_copy (id TEXT PRIMARY KEY, value TEXT NOT NULL);")
                    .expect("create existing progress target");
            }
            fixture
        }

        fn connections_and_job(&self, mode: DatabaseTransferMode)
            -> (StoredDatabaseConnection, StoredDatabaseConnection, DatabaseTransferJob)
        {
            let source = sqlite_test_connection(&self.source_path);
            let mut target = sqlite_test_connection(&self.target_path);
            target.profile.id = 2;
            let endpoint = |id, table: &str| DatabaseTransferEndpoint {
                connection_id: id,
                connection_name: None,
                connection_type: Some(SupportedDatabaseType::Sqlite),
                database_name: None,
                schema_name: None,
                table_name: table.to_string(),
            };
            let job = test_transfer_job(endpoint(1, "records"), endpoint(2, "records_copy"), mode);
            (source, target, job)
        }

        fn native_target_count(&self) -> Option<usize> {
            transfer_progress_native_count(&self.target_path)
        }
    }

    impl Drop for TransferProgressFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.source_path);
            let _ = std::fs::remove_file(&self.target_path);
        }
    }

    fn transfer_progress_native_count(path: &std::path::Path) -> Option<usize> {
        let native = Connection::open(path).expect("open independent checkpoint oracle");
        let exists: bool = native.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='records_copy')",
            [], |row| row.get(0),
        ).expect("check target existence");
        exists.then(|| native.query_row("SELECT COUNT(*) FROM records_copy", [], |row| row.get::<_, i64>(0))
            .expect("read committed target count") as usize)
    }

    fn assert_transfer_progress_snapshots(
        snapshots: &[(DatabaseTransferJob, Option<usize>)],
        total_rows: usize,
        seeded_target_rows: usize,
    ) {
        assert!(!snapshots.is_empty(), "the real worker must persist progress");
        let mut previous_percentage = None;
        let mut copy_started = false;
        for (job, native_count) in snapshots {
            let fields = job.progress.as_object().expect("progress object");
            assert!(fields.get("message").and_then(Value::as_str).is_some_and(|message| !message.is_empty()));
            if let Some(current) = fields.get("current").and_then(Value::as_u64) {
                let current = current as usize;
                assert_eq!(fields.get("total").and_then(Value::as_u64), Some(total_rows as u64));
                let percentage = fields.get("percentage").and_then(Value::as_u64).expect("copy percentage");
                assert!(percentage <= 100);
                if current < total_rows {
                    assert!(percentage < 100, "an unprocessed row must not display 100%");
                }
                if let Some(previous) = previous_percentage {
                    assert!(percentage >= previous, "copy percentage must never move backwards");
                }
                previous_percentage = Some(percentage);
                let result = job.result.as_ref().expect("copy counts and progress persist together");
                assert_eq!(current, result.copied_row_count + result.failed_row_count);
                assert_eq!(native_count.expect("copy target exists"), seeded_target_rows + result.copied_row_count,
                    "persisted copied count must reflect committed rows, not in-flight writes");
                if !copy_started {
                    assert_eq!(current, 0, "publish the copy phase before the first write");
                    copy_started = true;
                }
            } else {
                assert!(!copy_started, "the worker must not return to preparation after copying");
                for key in ["percentage", "current", "total"] {
                    assert!(!fields.contains_key(key), "preparation is indeterminate, not a competing numeric scale: {key}");
                }
                assert_eq!(native_count.unwrap_or(0), seeded_target_rows);
                if let Some(result) = &job.result {
                    assert_eq!(result.copied_row_count, 0);
                    assert_eq!(result.failed_row_count, 0);
                }
            }
        }
    }

    #[test]
    fn transfer_copy_progress_keeps_real_totals_and_does_not_round_early_completion() {
        let almost = transfer_copy_progress(400, 401, 397, 3);
        assert_eq!(almost["current"], Value::from(400));
        assert_eq!(almost["total"], Value::from(401));
        assert!(almost["percentage"].as_u64().unwrap() < 100);
        let complete = transfer_copy_progress(401, 401, 398, 3);
        assert_eq!(complete["percentage"], Value::from(100));
        let empty = transfer_copy_progress(0, 0, 0, 0);
        assert_eq!(empty["current"], Value::from(0));
        assert_eq!(empty["total"], Value::from(0), "do not invent one source row");
        assert!(empty["percentage"].as_u64().is_some_and(|percentage| percentage <= 100));
        // File/export jobs retain their established rounding/clamping contract.
        assert_eq!(progress(2, 3, "Copying")["percentage"], Value::from(67));
        assert_eq!(progress(0, 0, "Queued")["total"], Value::from(1));
    }

    #[test]
    fn transfer_copy_checkpoint_due_has_exact_first_time_and_batch_boundaries() {
        use std::time::Duration;
        assert!(transfer_copy_checkpoint_due(1, false, Duration::ZERO));
        assert!(!transfer_copy_checkpoint_due(2, false, Duration::ZERO));
        assert!(!transfer_copy_checkpoint_due(2, false, Duration::from_millis(499)));
        assert!(transfer_copy_checkpoint_due(2, false, Duration::from_millis(500)));
        assert!(transfer_copy_checkpoint_due(2, false, Duration::from_millis(501)));
        assert!(transfer_copy_checkpoint_due(200, true, Duration::ZERO));
        assert!(transfer_copy_checkpoint_due(401, true, Duration::ZERO));
    }

    #[tokio::test]
    async fn transfer_copy_progress_401_rows_persists_confirmed_counts_and_one_scale() {
        let fixture = TransferProgressFixture::new(401, false);
        let (source, target, mut job) = fixture.connections_and_job(DatabaseTransferMode::SchemaAndData);
        let mut snapshots = Vec::new();
        let result = run_database_transfer(&source, &target, &mut job, &mut |updated| {
            snapshots.push((updated.clone(), fixture.native_target_count()));
        }).await.expect("real 401-row transfer");
        assert_transfer_progress_snapshots(&snapshots, 401, 0);
        for current in [0, 1, 200, 400, 401] {
            assert!(snapshots.iter().any(|(saved, _)| saved.progress["current"] == Value::from(current)),
                "missing first-row or batch checkpoint at {current}");
        }
        assert!(result.created_table);
        assert_eq!(result.copied_row_count, 401);
        assert_eq!(result.failed_row_count, 0);
        assert_eq!(fixture.native_target_count(), Some(401));
    }

    #[tokio::test]
    async fn transfer_copy_progress_mixed_failures_keep_absolute_indices_across_batches() {
        let fixture = TransferProgressFixture::new(401, true);
        let native = Connection::open(&fixture.target_path).expect("open seeded target");
        native.execute_batch(
            "INSERT INTO records_copy VALUES ('200','keep-200'),('201','keep-201'),('400','keep-400');"
        ).expect("seed three deliberate conflicts");
        drop(native);
        let (source, target, mut job) = fixture.connections_and_job(DatabaseTransferMode::TableCopy);
        let mut snapshots = Vec::new();
        let result = run_database_transfer(&source, &target, &mut job, &mut |updated| {
            snapshots.push((updated.clone(), fixture.native_target_count()));
        }).await.expect("mixed successful and rejected rows");
        assert_transfer_progress_snapshots(&snapshots, 401, 3);
        assert_eq!(result.copied_row_count, 398);
        assert_eq!(result.failed_row_count, 3);
        assert_eq!(result.row_failures.iter().map(|failure| failure["rowIndex"].as_u64().unwrap()).collect::<Vec<_>>(),
            vec![199, 200, 399]);
        for (saved, _) in &snapshots {
            if let Some(result) = &saved.result {
                let expected = [199usize, 200, 399].into_iter()
                    .filter(|index| *index < result.copied_row_count + result.failed_row_count)
                    .map(|index| index as u64).collect::<Vec<_>>();
                assert_eq!(result.row_failures.iter().map(|failure| failure["rowIndex"].as_u64().unwrap()).collect::<Vec<_>>(), expected);
            }
        }
        let native = Connection::open(&fixture.target_path).expect("verify rejected rows");
        let unchanged: i64 = native.query_row(
            "SELECT COUNT(*) FROM records_copy WHERE (id='200' AND value='keep-200') OR (id='201' AND value='keep-201') OR (id='400' AND value='keep-400')",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(unchanged, 3);
        assert_eq!(fixture.native_target_count(), Some(401));
    }

    #[tokio::test]
    async fn transfer_copy_progress_empty_source_preserves_zero_total() {
        let fixture = TransferProgressFixture::new(0, true);
        let (source, target, mut job) = fixture.connections_and_job(DatabaseTransferMode::TableCopy);
        let mut snapshots = Vec::new();
        let result = run_database_transfer(&source, &target, &mut job, &mut |updated| {
            snapshots.push((updated.clone(), fixture.native_target_count()));
        }).await.expect("empty transfer");
        assert_transfer_progress_snapshots(&snapshots, 0, 0);
        assert!(snapshots.iter().any(|(saved, _)| saved.progress.get("total") == Some(&Value::from(0))));
        assert_eq!(result.copied_row_count, 0);
        assert_eq!(result.failed_row_count, 0);
        assert_eq!(fixture.native_target_count(), Some(0));
    }

    #[tokio::test]
    async fn transfer_copy_progress_schema_only_never_claims_copied_rows() {
        let fixture = TransferProgressFixture::new(3, false);
        let (source, target, mut job) = fixture.connections_and_job(DatabaseTransferMode::SchemaOnly);
        let mut snapshots = Vec::new();
        let result = run_database_transfer(&source, &target, &mut job, &mut |updated| {
            snapshots.push((updated.clone(), fixture.native_target_count()));
        }).await.expect("schema-only transfer");
        assert_transfer_progress_snapshots(&snapshots, 0, 0);
        assert!(snapshots.iter().all(|(saved, _)| saved.progress.get("current").is_none()));
        assert!(result.created_table);
        assert_eq!(result.copied_row_count, 0);
        assert_eq!(result.failed_row_count, 0);
        assert_eq!(fixture.native_target_count(), Some(0));
    }
