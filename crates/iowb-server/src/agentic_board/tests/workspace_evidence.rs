fn filesystem_evidence_fixture() -> PathBuf {
    let root = std::env::temp_dir().join(format!("iowb-filesystem-evidence-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn non_git_created_file_is_deterministic_implementation_evidence() {
    let root = filesystem_evidence_fixture();
    let project = root.to_str().unwrap();
    let before = capture_workspace_snapshot(project);
    assert_eq!(before["provider"], "filesystem sha256");
    assert_eq!(before["complete"], true);
    fs::write(
        root.join("index.html"),
        "<!doctype html><title>Pocket Tasks</title>",
    )
    .unwrap();
    let mut run = board_fixture(json!({"command": "Create static shell", "projectPath": project}));
    run.tasks[0].hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
    run.tasks[0].hierarchy.executable = true;
    let delta = record_task_workspace_changes(&mut run, "task-1", before);
    assert_eq!(change_summary_paths(&delta), vec!["index.html"]);
    assert_eq!(run.tasks[0].changed_files, vec!["index.html"]);
    assert_eq!(delta["isGit"], false);
    assert_eq!(delta["attributableChangedFileCount"], 1);
    let result = apply_completion_evidence_gate(
        &run,
        "task-1",
        json!({
            "status": "done",
            "summary": "Created static shell",
            "qaResult": "pass",
            "changedFiles": [root.join("index.html").display().to_string()]
        }),
        &delta,
    );
    assert_eq!(result["evidenceGate"]["passed"], true);
    assert_eq!(result["status"], "done");
    assert!(!root.join(".git").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn non_git_snapshot_detects_content_modification_deletion_and_unchanged_files() {
    let root = filesystem_evidence_fixture();
    fs::create_dir(root.join("nested folder")).unwrap();
    fs::write(root.join("nested folder/app.js"), "let done=false;").unwrap();
    fs::write(root.join("obsolete.css"), "old").unwrap();
    fs::write(root.join("README.md"), "unchanged").unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    fs::write(root.join("nested folder/app.js"), "let done=true;").unwrap();
    fs::remove_file(root.join("obsolete.css")).unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &before, &after);
    assert_eq!(
        change_summary_paths(&delta),
        vec!["nested folder/app.js", "obsolete.css"]
    );
    assert_eq!(delta["unknownChangeCount"], 0);
    assert_eq!(delta["preExistingChangeCount"], 0);
    assert_eq!(delta["changedBySubtask"][1]["afterHash"], Value::Null);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_file_claims_do_not_replace_non_git_snapshot_evidence() {
    let root = filesystem_evidence_fixture();
    fs::write(root.join("index.html"), "preexisting and unchanged").unwrap();
    let snapshot = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &snapshot, &snapshot);
    let mut run =
        board_fixture(json!({"command": "Create shell", "projectPath": root.to_str().unwrap()}));
    run.tasks[0].hierarchy.level = TASK_LEVEL_SUBTASK.to_string();
    run.tasks[0].hierarchy.executable = true;
    let result = apply_completion_evidence_gate(
        &run,
        "task-1",
        json!({
            "status": "done",
            "summary": "Claimed implementation",
            "qaResult": "pass",
            "changedFiles": ["invented.js", "index.html", "/outside/project.js"],
            "evidence": ["Provider claimed these files were created"]
        }),
        &delta,
    );
    assert!(change_summary_paths(&delta).is_empty());
    assert_eq!(result["evidenceGate"]["passed"], false);
    assert_eq!(result["status"], "needs_followup");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn partial_non_git_snapshots_cannot_attribute_unseen_files() {
    let root = filesystem_evidence_fixture();
    fs::write(root.join("a.txt"), "a").unwrap();
    fs::write(root.join("b.txt"), "b").unwrap();
    let partial = capture_filesystem_workspace_snapshot(root.to_str().unwrap(), "not git", 1);
    let complete = capture_filesystem_workspace_snapshot(root.to_str().unwrap(), "not git", 10);
    assert_eq!(partial["complete"], false);
    assert_eq!(partial["truncated"], true);
    for (before, after) in [(&partial, &complete), (&complete, &partial)] {
        let delta = summarize_workspace_delta("task-1", before, after);
        assert!(change_summary_paths(&delta).is_empty());
        assert_eq!(delta["unknownChangeCount"], 1);
        assert_eq!(delta["snapshotComplete"], false);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unavailable_or_mixed_snapshot_baselines_do_not_prove_non_git_changes() {
    let root = filesystem_evidence_fixture();
    fs::write(root.join("index.html"), "shell").unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    for before in [
        json!({"isGit": false, "filesByPath": {}, "error": "legacy unavailable snapshot"}),
        json!({"isGit": true, "filesByPath": {}}),
        capture_filesystem_workspace_snapshot(
            root.join("missing").to_str().unwrap(),
            "not git",
            10,
        ),
    ] {
        let delta = summarize_workspace_delta("task-1", &before, &after);
        assert!(change_summary_paths(&delta).is_empty());
        assert_eq!(delta["unknownChangeCount"], 1);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn non_git_oversized_file_is_incomplete_evidence_not_an_empty_hash() {
    let root = filesystem_evidence_fixture();
    fs::File::create(root.join("large.bin"))
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    let snapshot = capture_workspace_snapshot(root.to_str().unwrap());
    assert_eq!(snapshot["complete"], false);
    assert_eq!(snapshot["filesByPath"]["large.bin"]["hash"], Value::Null);
    assert!(
        snapshot["error"]
            .as_str()
            .unwrap()
            .contains("snapshot limit")
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn non_git_snapshot_skips_generated_directories_and_symlink_targets() {
    let root = filesystem_evidence_fixture();
    let outside = filesystem_evidence_fixture();
    fs::create_dir(root.join("node_modules")).unwrap();
    fs::write(root.join("node_modules/ignored.js"), "dependency").unwrap();
    fs::write(outside.join("outside.js"), "outside").unwrap();
    std::os::unix::fs::symlink(outside.join("outside.js"), root.join("linked.js")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("linked-dir")).unwrap();
    let before = capture_workspace_snapshot(root.to_str().unwrap());
    fs::write(outside.join("outside.js"), "outside modified").unwrap();
    fs::write(root.join("node_modules/ignored.js"), "dependency modified").unwrap();
    let after = capture_workspace_snapshot(root.to_str().unwrap());
    let delta = summarize_workspace_delta("task-1", &before, &after);
    assert_eq!(before["complete"], true);
    assert_eq!(after["files"], json!([]));
    assert!(change_summary_paths(&delta).is_empty());
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn broken_git_metadata_does_not_fall_back_to_clean_filesystem_evidence() {
    let root = filesystem_evidence_fixture();
    fs::create_dir(root.join(".git")).unwrap();
    fs::write(root.join("index.html"), "shell").unwrap();
    let snapshot = capture_workspace_snapshot(root.to_str().unwrap());
    assert_eq!(snapshot["provider"], "git status --short");
    assert_eq!(snapshot["files"], json!([]));
    assert!(snapshot["error"].as_str().is_some());
    fs::remove_dir_all(root).unwrap();
}
