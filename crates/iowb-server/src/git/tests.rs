    use super::*;

    #[test]
    fn cleans_commit_message_from_markdown() {
        let raw = "Here is a message:\n```text\nfix(auth): handle login\n\nKeep token checks strict.\n```";

        assert_eq!(
            clean_commit_message(raw),
            "fix(auth): handle login\n\nKeep token checks strict."
        );
    }

    #[test]
    fn extracts_anthropic_response_text() {
        let value = serde_json::json!({
            "content": [
                { "type": "text", "text": "feat(ui): add shell" }
            ]
        });

        assert_eq!(extract_response_text(&value), "feat(ui): add shell");
    }

    #[test]
    fn extracts_chat_completion_text() {
        let value = serde_json::json!({
            "choices": [
                { "message": { "content": "chore: update files" } }
            ]
        });

        assert_eq!(extract_response_text(&value), "chore: update files");
    }

    #[test]
    fn selected_hunk_patch_keeps_file_headers_and_requested_hunks() {
        let diff = "diff --git a/file.txt b/file.txt\nindex 111..222 100644\n--- a/file.txt\n+++ b/file.txt\n@@ -1,2 +1,2 @@\n old\n+new\n@@ -8,2 +8,2 @@\n old2\n+new2\n";

        let patch = selected_hunk_patch(diff, &[1]).expect("patch can be selected");

        assert!(patch.contains("diff --git a/file.txt b/file.txt"));
        assert!(!patch.contains("@@ -1,2 +1,2 @@"));
        assert!(patch.contains("@@ -8,2 +8,2 @@"));
        assert!(patch.contains("+new2"));
    }

    #[test]
    fn diff_preview_stripping_preserves_header_like_hunk_content() {
        let diff = "diff --git a/file.txt b/file.txt\nindex 111..222 100644\n--- a/file.txt\n+++ b/file.txt\n@@ -1,4 +1,4 @@\n keep\n---- removed line that looks like a header\n++++ added line that looks like a header\n keep\n";

        assert_eq!(
            strip_diff_headers(diff),
            "@@ -1,4 +1,4 @@\n keep\n---- removed line that looks like a header\n++++ added line that looks like a header\n keep"
        );
    }

    #[test]
    fn detects_unmerged_git_statuses() {
        for status in ["UU", "AA", "DD", "AU", "UD", "UA", "DU", " U", "U "] {
            assert!(is_conflict_status(status), "{status} should be conflicted");
        }

        for status in [" M", "M ", "A ", " D", "??"] {
            assert!(
                !is_conflict_status(status),
                "{status} should not be conflicted"
            );
        }
    }

    #[test]
    fn extracts_conflict_regions_with_base_sections() {
        let content =
            "keep\n<<<<<<< HEAD\nours\n||||||| base\nbase\n=======\ntheirs\n>>>>>>> branch\nkeep\n";

        let regions = extract_conflict_regions(content);

        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].start_line, 2);
        assert_eq!(regions[0].end_line, 8);
        assert_eq!(regions[0].ours, "ours");
        assert_eq!(regions[0].base.as_deref(), Some("base"));
        assert_eq!(regions[0].theirs, "theirs");
    }

    #[test]
    fn parses_stash_and_tag_rows_without_losing_message_text() {
        let stash =
            parse_stash_summary("stash@{0}\u{1f}abc123\u{1f}Gio\u{1f}2026-07-30T12:00:00+07:00\u{1f}WIP: keep | separators")
                .expect("stash row");
        assert_eq!(stash.reference, "stash@{0}");
        assert_eq!(stash.message, "WIP: keep | separators");

        let tag = parse_tag_summary(
            "v1.0.0\u{1f}def456\u{1f}tag\u{1f}2026-07-30T12:00:00+07:00\u{1f}Release 1.0",
        )
        .expect("tag row");
        assert_eq!(tag.name, "v1.0.0");
        assert_eq!(tag.object_type, "tag");
    }

    #[test]
    fn detects_git_versions_that_support_cherry_pick_empty_keep() {
        assert!(!git_supports_cherry_pick_empty_keep("git version 2.44.1"));
        assert!(git_supports_cherry_pick_empty_keep("git version 2.45.0"));
        assert!(git_supports_cherry_pick_empty_keep("git version 3.0.0"));
        assert!(!git_supports_cherry_pick_empty_keep("not a Git version"));
    }

    #[test]
    fn resolves_remote_tracking_names_against_the_longest_configured_remote() {
        let remotes = vec!["team".to_string(), "team/review".to_string()];
        assert_eq!(
            split_configured_remote_tracking_name("team/review/feature/mobile", &remotes),
            Some(("team/review".to_string(), "feature/mobile".to_string())),
        );
        assert_eq!(
            validate_cherry_pick_source_ref("refs/remotes/team/review/feature/mobile", &remotes)
                .expect("slash-named remote is a valid cherry-pick source"),
            "refs/remotes/team/review/feature/mobile",
        );
    }

    #[test]
    fn parses_porcelain_v2_nul_status_without_losing_spaces_or_submodules() {
        let output = concat!(
            "1 .M N... 100644 100644 100644 abc abc src/file with spaces.rs\0",
            "? scratch file.txt\0",
            "u UU N... 100644 100644 100644 100644 abc def ghi path/conflict.txt\0",
        );

        assert_eq!(
            parse_status_entries(output),
            vec![
                (".M".to_string(), "src/file with spaces.rs".to_string()),
                ("??".to_string(), "scratch file.txt".to_string()),
                ("UU".to_string(), "path/conflict.txt".to_string()),
            ],
        );
    }

    #[test]
    fn parses_git_numstat_records_and_ignores_binary_counts() {
        let stats = parse_git_numstat(
            b"12\t4\tsrc/file.rs\0-\t-\tassets/logo.png\0",
        );

        assert_eq!(stats.get("src/file.rs"), Some(&(12, 4)));
        assert_eq!(stats.get("assets/logo.png"), Some(&(0, 0)));
    }

    #[test]
    fn parses_porcelain_v2_rename_destination_and_skips_original_path() {
        let output = "2 R. N... 100644 100644 100644 abc def R100 new name.txt\0old name.txt\0";

        assert_eq!(
            parse_status_entries(output),
            vec![("R.".to_string(), "new name.txt".to_string())],
        );
    }

    #[test]
    fn validates_git_names_without_allowing_option_like_or_malformed_refs() {
        for value in [
            "-bad",
            "foo..bar",
            "foo@{bar}",
            ".hidden",
            "foo.",
            "foo//bar",
            "foo/.bar",
            "foo/bar.lock",
            "foo bar",
            "foo~bar",
        ] {
            assert!(validate_branch_name(value).is_err(), "{value} must be rejected");
            assert!(validate_tag_name(value).is_err(), "{value} must be rejected");
        }

        assert_eq!(
            validate_branch_name("feature/réorganize").unwrap(),
            "feature/réorganize"
        );
        assert_eq!(validate_tag_name("release/2026.08").unwrap(), "release/2026.08");
        assert!(validate_commit_ref("--help").is_err());
        assert!(validate_commit_ref("HEAD with spaces").is_err());
        assert!(validate_remote_name("-origin").is_err());
        assert!(validate_remote_name("origin..backup").is_err());
        assert_eq!(validate_remote_name("team/review").unwrap(), "team/review");
        assert!(validate_remote_name("team//review").is_err());
        assert!(validate_remote_url("--upload-pack=evil").is_err());
        assert!(validate_remote_url("https://example.test/repo\n--upload-pack=evil").is_err());
        assert!(validate_remote_url("https://token@example.test/repo.git").is_err());
        assert!(validate_remote_url("https://example.test/repo.git?access_token=secret").is_err());
        assert!(validate_remote_url("https://example.test/repo.git#secret").is_err());
        assert!(validate_remote_url("ssh://git@example.test/repo.git").is_ok());
    }

    #[test]
    fn preserves_leading_and_trailing_spaces_in_valid_repository_paths() {
        assert_eq!(
            normalize_repo_relative_path("  filename with spaces  "),
            "  filename with spaces  "
        );
    }

    #[tokio::test]
    async fn treats_pathspec_metacharacters_in_file_names_literally() {
        let workspace = std::env::temp_dir().join(format!(
            "iowb-git-literal-pathspec-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        git_test_command(&workspace, ["init", "--initial-branch=main"]);
        configure_test_repository(&workspace);
        std::fs::write(workspace.join("README.md"), "initial").unwrap();
        git_test_command(&workspace, ["add", "README.md"]);
        git_test_command(&workspace, ["commit", "-m", "initial"]);

        let literal = "literal[ab]*?.txt";
        let wildcard_match = "literala123x.txt";
        std::fs::write(workspace.join(literal), "literal").unwrap();
        std::fs::write(workspace.join(wildcard_match), "wildcard").unwrap();
        stage_resolved_path(&workspace, literal)
            .await
            .expect("the literal filename can be staged");

        let staged = git_test_output(&workspace, ["diff", "--cached", "--name-only"]);
        assert!(staged.lines().any(|path| path == literal));
        assert!(!staged.lines().any(|path| path == wildcard_match));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn detects_an_ancestor_repository_before_nested_initialization() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-ancestor-{}",
            uuid::Uuid::new_v4()
        ));
        let project = root.join("workspace");
        std::fs::create_dir_all(&project).unwrap();
        git_test_command(&root, ["init", "--initial-branch=main"]);

        let ancestor = enclosing_git_repository(&project)
            .await
            .expect("the containing repository is detected");
        assert_eq!(ancestor, std::fs::canonicalize(&root).unwrap());

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn discovers_three_sibling_repositories_without_inventing_a_main_repo() {
        let workspace = std::env::temp_dir().join(format!("iowb-git-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        for name in ["one", "two", "three"] {
            let repository = workspace.join(name);
            std::fs::create_dir_all(&repository).unwrap();
            git_test_command(&repository, ["init", "--initial-branch=main"]);
            git_test_command(&repository, ["config", "user.name", "Catalog Test"]);
            git_test_command(&repository, ["config", "user.email", "catalog@example.test"]);
            std::fs::write(repository.join("README.md"), name).unwrap();
            git_test_command(&repository, ["add", "."]);
            git_test_command(&repository, ["commit", "-m", "initial"]);
        }

        let catalog = discover_git_workspace(&workspace).await.unwrap();
        assert_eq!(catalog.repositories.len(), 3);
        assert!(catalog.default_repository_id.is_none());
        assert!(catalog.repositories.iter().all(|repository| {
            repository.initialized
                && matches!(repository.kind, GitRepositoryKind::Nested)
                && repository.id.starts_with("repository:")
        }));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn keeps_root_default_and_discovers_independent_nested_repository() {
        let workspace = std::env::temp_dir().join(format!("iowb-git-root-catalog-{}", uuid::Uuid::new_v4()));
        let nested = workspace.join("tools");
        std::fs::create_dir_all(&nested).unwrap();
        git_test_command(&workspace, ["init", "--initial-branch=main"]);
        git_test_command(&workspace, ["config", "user.name", "Catalog Test"]);
        git_test_command(&workspace, ["config", "user.email", "catalog@example.test"]);
        std::fs::write(workspace.join("README.md"), "root").unwrap();
        git_test_command(&workspace, ["add", "README.md"]);
        git_test_command(&workspace, ["commit", "-m", "initial"]);
        git_test_command(&nested, ["init", "--initial-branch=main"]);
        git_test_command(&nested, ["config", "user.name", "Catalog Test"]);
        git_test_command(&nested, ["config", "user.email", "catalog@example.test"]);
        std::fs::write(nested.join("README.md"), "nested").unwrap();
        git_test_command(&nested, ["add", "."]);
        git_test_command(&nested, ["commit", "-m", "initial"]);

        let catalog = discover_git_workspace(&workspace).await.unwrap();
        assert_eq!(catalog.default_repository_id.as_deref(), Some("root"));
        assert!(catalog
            .repositories
            .iter()
            .any(|repository| repository.id == "repository:tools"));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn initializes_an_uninitialized_submodule_through_its_parent_repository() {
        let workspace = std::env::temp_dir().join(format!("iowb-git-submodule-{}", uuid::Uuid::new_v4()));
        let source = workspace.join("source");
        let checkout = workspace.join("checkout");
        let module = checkout.join("modules/child");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&checkout).unwrap();

        git_test_command(&source, ["init", "--initial-branch=main"]);
        git_test_command(&source, ["config", "user.name", "Catalog Test"]);
        git_test_command(&source, ["config", "user.email", "catalog@example.test"]);
        std::fs::write(source.join("README.md"), "submodule").unwrap();
        git_test_command(&source, ["add", "."]);
        git_test_command(&source, ["commit", "-m", "initial"]);

        git_test_command(&checkout, ["init", "--initial-branch=main"]);
        git_test_command(&checkout, ["config", "user.name", "Catalog Test"]);
        git_test_command(&checkout, ["config", "user.email", "catalog@example.test"]);
        git_test_command_with_file_protocol(
            &checkout,
            ["submodule", "add", source.to_str().unwrap(), "modules/child"],
        );
        git_test_command(&checkout, ["commit", "-am", "add submodule"]);
        git_test_command(&checkout, ["submodule", "deinit", "-f", "--", "modules/child"]);
        assert!(!module.join(".git").exists());

        let catalog = discover_git_workspace(&checkout).await.unwrap();
        let repository = catalog
            .repositories
            .iter()
            .find(|repository| {
                matches!(
                    repository.kind,
                    GitRepositoryKind::Uninitialized
                )
            })
            .cloned()
            .expect("uninitialized submodule is visible");
        initialize_uninitialized_submodule(&catalog, &repository)
            .await
            .expect("submodule initializes");

        assert!(module.join(".git").exists());
        assert_eq!(std::fs::read_to_string(module.join("README.md")).unwrap(), "submodule");
        let refreshed = discover_git_workspace(&checkout).await.unwrap();
        assert!(refreshed.repositories.iter().any(|entry| {
            entry.relative_path == "modules/child"
                && matches!(entry.kind, GitRepositoryKind::Submodule)
                && entry.initialized
        }));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn classifies_linked_worktrees_and_keeps_path_ids_stable() {
        let workspace = std::env::temp_dir().join(format!(
            "iowb-git-worktree-{}",
            uuid::Uuid::new_v4()
        ));
        let main = workspace.join("main");
        let linked = workspace.join("feature worktree");
        std::fs::create_dir_all(&main).unwrap();
        git_test_command(&main, ["init", "--initial-branch=main"]);
        configure_test_repository(&main);
        std::fs::write(main.join("README.md"), "main").unwrap();
        git_test_command(&main, ["add", "README.md"]);
        git_test_command(&main, ["commit", "-m", "initial"]);
        git_test_command(
            &main,
            ["worktree", "add", "-b", "feature", linked.to_str().unwrap()],
        );

        let catalog = discover_git_workspace(&workspace).await.unwrap();
        let main_entry = catalog
            .repositories
            .iter()
            .find(|repository| repository.relative_path == "main")
            .expect("main repository is discovered");
        let linked_entry = catalog
            .repositories
            .iter()
            .find(|repository| repository.relative_path == "feature worktree")
            .expect("linked worktree is discovered");
        assert!(matches!(main_entry.kind, GitRepositoryKind::Nested));
        assert!(matches!(linked_entry.kind, GitRepositoryKind::Worktree));
        assert_eq!(main_entry.id, "repository:main");
        assert_eq!(linked_entry.id, "repository:feature worktree");
        assert!(catalog.default_repository_id.is_none());

        let direct = discover_git_workspace(&linked).await.unwrap();
        let direct_entry = direct
            .repositories
            .first()
            .expect("direct worktree is discovered");
        assert!(matches!(
            direct_entry.kind,
            GitRepositoryKind::Worktree
        ));
        assert_eq!(direct_entry.id, "repository:.");
        assert_eq!(direct.default_repository_id.as_deref(), Some("repository:."));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn rejects_parent_directory_targets_that_contain_nested_repositories() {
        let workspace = std::env::temp_dir().join(format!(
            "iowb-git-boundary-{}",
            uuid::Uuid::new_v4()
        ));
        let nested_parent = workspace.join("packages");
        let nested = nested_parent.join("tools");
        std::fs::create_dir_all(&nested).unwrap();
        git_test_command(&workspace, ["init", "--initial-branch=main"]);
        configure_test_repository(&workspace);
        std::fs::write(workspace.join("README.md"), "root").unwrap();
        git_test_command(&workspace, ["add", "README.md"]);
        git_test_command(&workspace, ["commit", "-m", "initial"]);
        git_test_command(&nested, ["init", "--initial-branch=main"]);
        configure_test_repository(&nested);
        std::fs::write(nested.join("README.md"), "nested").unwrap();
        git_test_command(&nested, ["add", "README.md"]);
        git_test_command(&nested, ["commit", "-m", "initial"]);

        let error = resolve_git_file_target(
            &workspace,
            "packages",
            GitFileTargetPolicy::Stage,
        )
        .await
        .expect_err("parent directory must not cross a nested repository boundary");
        assert!(error.body.error.contains("nested repository"));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn detects_gitlinks_even_without_gitmodules() {
        let workspace = std::env::temp_dir().join(format!(
            "iowb-git-gitlink-{}",
            uuid::Uuid::new_v4()
        ));
        let child = workspace.join("child");
        std::fs::create_dir_all(&workspace).unwrap();
        git_test_command(&workspace, ["init", "--initial-branch=main"]);
        configure_test_repository(&workspace);
        std::fs::write(workspace.join("README.md"), "root").unwrap();
        git_test_command(&workspace, ["add", "README.md"]);
        git_test_command(&workspace, ["commit", "-m", "root"]);

        std::fs::create_dir_all(&child).unwrap();
        git_test_command(&child, ["init", "--initial-branch=main"]);
        configure_test_repository(&child);
        std::fs::write(child.join("README.md"), "child").unwrap();
        git_test_command(&child, ["add", "README.md"]);
        git_test_command(&child, ["commit", "-m", "child"]);
        let child_hash = git_test_output(&child, ["rev-parse", "HEAD"]);
        let cache_info = format!(
            "160000,{},child",
            child_hash.trim()
        );
        git_test_command(
            &workspace,
            ["update-index", "--add", "--cacheinfo", cache_info.as_str()],
        );
        git_test_command(&workspace, ["commit", "-m", "record child gitlink"]);

        let catalog = discover_git_workspace(&workspace).await.unwrap();
        let entry = catalog
            .repositories
            .iter()
            .find(|repository| repository.relative_path == "child")
            .expect("gitlink checkout is discovered");
        assert!(matches!(entry.kind, GitRepositoryKind::Submodule));
        assert!(entry.initialized);
        assert!(!workspace.join(".gitmodules").exists());

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn preserves_dirty_submodule_content_and_pointer_changes_in_status() {
        let workspace = std::env::temp_dir().join(format!(
            "iowb-git-dirty-submodule-{}",
            uuid::Uuid::new_v4()
        ));
        let source = workspace.join("source");
        let checkout = workspace.join("checkout");
        let module = checkout.join("modules/child");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&checkout).unwrap();
        git_test_command(&source, ["init", "--initial-branch=main"]);
        configure_test_repository(&source);
        std::fs::write(source.join("README.md"), "initial").unwrap();
        git_test_command(&source, ["add", "README.md"]);
        git_test_command(&source, ["commit", "-m", "initial"]);
        git_test_command(&checkout, ["init", "--initial-branch=main"]);
        configure_test_repository(&checkout);
        git_test_command_with_file_protocol(
            &checkout,
            ["submodule", "add", source.to_str().unwrap(), "modules/child"],
        );
        git_test_command(&checkout, ["commit", "-am", "add submodule"]);

        std::fs::write(module.join("README.md"), "dirty").unwrap();
        let dirty = git_test_output_bytes(
            &checkout,
            [
                "status",
                "--porcelain=v2",
                "-z",
                "--ignore-submodules=none",
            ],
        );
        let dirty_entries = parse_status_entries_detailed_bytes(&dirty);
        let dirty_entry = dirty_entries
            .iter()
            .find(|(_, path, _)| path == "modules/child")
            .expect("dirty submodule appears in parent status");
        assert!(dirty_entry.1 == "modules/child");
        assert!(dirty_entry.2.as_deref().is_some_and(|value| value.starts_with('S')));

        git_test_command(&module, ["config", "user.name", "Catalog Test"]);
        git_test_command(
            &module,
            ["config", "user.email", "catalog@example.test"],
        );
        git_test_command(&module, ["add", "README.md"]);
        git_test_command(&module, ["commit", "-m", "child update"]);
        let pointer = git_test_output_bytes(
            &checkout,
            [
                "status",
                "--porcelain=v2",
                "-z",
                "--ignore-submodules=none",
            ],
        );
        let pointer_entry = parse_status_entries_detailed_bytes(&pointer)
            .into_iter()
            .find(|(_, path, _)| path == "modules/child")
            .expect("submodule pointer change appears in parent status");
        assert!(pointer_entry.0.contains('M'));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn applies_one_hunk_when_index_and_worktree_are_both_changed() {
        let workspace = std::env::temp_dir().join(format!(
            "iowb-git-mixed-hunks-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        git_test_command(&workspace, ["init", "--initial-branch=main"]);
        configure_test_repository(&workspace);
        let original = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(workspace.join("mixed.txt"), original).unwrap();
        git_test_command(&workspace, ["add", "mixed.txt"]);
        git_test_command(&workspace, ["commit", "-m", "initial"]);
        let mut changed = (1..=20)
            .map(|line| match line {
                2 => "staged change".to_string(),
                19 => "unstaged change".to_string(),
                _ => format!("line {line}"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        changed.push('\n');
        std::fs::write(workspace.join("mixed.txt"), changed).unwrap();
        let diff = git(&workspace, ["diff"]).await.unwrap().stdout;
        let patch = selected_hunk_patch(&diff, &[0]).unwrap();
        apply_patch_to_index(&workspace, &patch, false)
            .await
            .expect("first hunk stages");
        let cached = git(&workspace, ["diff", "--cached"]).await.unwrap().stdout;
        let working = git(&workspace, ["diff"]).await.unwrap().stdout;
        assert!(cached.contains("+staged change"));
        assert!(!cached.contains("+unstaged change"));
        assert!(working.contains("+unstaged change"));
        assert!(!working.contains("+staged change"));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_paths_that_escape_the_repository() {
        use std::os::unix::fs::symlink;

        let workspace = std::env::temp_dir().join(format!(
            "iowb-git-symlink-{}",
            uuid::Uuid::new_v4()
        ));
        let outside = workspace.join("outside");
        let repository = workspace.join("repository");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&repository).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        git_test_command(&repository, ["init"]);
        symlink(&outside, repository.join("escape")).unwrap();

        assert!(safe_repo_child(&repository, "escape/secret.txt").is_err());
        assert!(safe_repo_child(&repository, "../outside/secret.txt").is_err());
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn initial_commit_excludes_independent_nested_repository_content() {
        let workspace = std::env::temp_dir().join(format!(
            "iowb-git-initial-nested-{}",
            uuid::Uuid::new_v4()
        ));
        let nested = workspace.join("tools");
        std::fs::create_dir_all(&nested).unwrap();
        git_test_command(&workspace, ["init", "--initial-branch=main"]);
        configure_test_repository(&workspace);
        std::fs::write(workspace.join("README.md"), "root").unwrap();
        git_test_command(&nested, ["init", "--initial-branch=main"]);
        configure_test_repository(&nested);
        std::fs::write(nested.join("README.md"), "nested").unwrap();
        git_test_command(&nested, ["add", "README.md"]);
        git_test_command(&nested, ["commit", "-m", "nested initial"]);

        let catalog = discover_git_workspace(&workspace).await.unwrap();
        git_with_pathspec_magic(&workspace, initial_commit_add_args(&catalog))
            .await
            .expect("root initial add succeeds");
        git(&workspace, ["commit", "--allow-empty", "-m", "root initial"])
            .await
            .expect("root initial commit succeeds");
        let tree = git(&workspace, ["ls-tree", "--name-only", "HEAD"])
            .await
            .unwrap()
            .stdout;
        assert!(tree.lines().any(|line| line == "README.md"));
        assert!(!tree.lines().any(|line| line == "tools"));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn git_router_mutations_are_scoped_to_the_selected_repository() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-router-{}",
            uuid::Uuid::new_v4()
        ));
        let workspace = root.join("workspace");
        let config_dir = root.join("config");
        let one = workspace.join("one");
        let two = workspace.join("two");
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();
        for repository in [&one, &two] {
            git_test_command(repository, ["init", "--initial-branch=main"]);
            configure_test_repository(repository);
            std::fs::write(repository.join("README.md"), "initial").unwrap();
            git_test_command(repository, ["add", "README.md"]);
            git_test_command(repository, ["commit", "-m", "initial"]);
        }
        std::fs::write(one.join("selected.txt"), "one").unwrap();
        std::fs::write(two.join("untouched.txt"), "two").unwrap();

        let state = iowb_core::AppState::initialize(iowb_core::AppConfig {
            host: "127.0.0.1".parse().unwrap(),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.clone(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 4,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router().with_state(server_state)).await;
        });
        let client = reqwest::Client::new();
        let stage_response = client
            .post(format!("http://{address}/api/git/stage"))
            .json(&serde_json::json!({
                "projectPath": workspace.display().to_string(),
                "repositoryId": "repository:one",
                "file": "selected.txt"
            }))
            .send()
            .await
            .unwrap();
        assert!(stage_response.status().is_success());
        assert!(git_test_output(&one, ["diff", "--cached", "--name-only"])
            .lines()
            .any(|line| line == "selected.txt"));
        assert!(git_test_output(&two, ["diff", "--cached", "--name-only"])
            .trim()
            .is_empty());
        let expected_index_tree = git_test_output(&one, ["write-tree"])
            .trim()
            .to_string();
        let expected_head = git_test_output(&one, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let commit_response = client
            .post(format!("http://{address}/api/git/commit"))
            .json(&serde_json::json!({
                "projectPath": workspace.display().to_string(),
                "repositoryId": "repository:one",
                "message": "selected repository commit",
                "files": ["selected.txt"],
                "expectedIndexTree": expected_index_tree,
                "expectedHead": expected_head,
                "expectedTargetRef": "refs/heads/main"
            }))
            .send()
            .await
            .unwrap();
        assert!(commit_response.status().is_success());
        assert_eq!(
            git_test_output(&one, ["log", "-1", "--format=%s"]).trim(),
            "selected repository commit"
        );
        assert_eq!(
            git_test_output(&two, ["log", "-1", "--format=%s"]).trim(),
            "initial"
        );

        server.abort();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cherry_pick_conflicts_are_recoverable_through_git_handlers() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-cherry-pick-workflow-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("conflict.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "conflict.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("conflict.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "feature change"]);
        let feature_commit = git_test_output(&repository, ["rev-parse", "HEAD"]);
        git_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("conflict.txt"), "main\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "main change"]);
        let main_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let unsafe_no_commit = cherry_pick(
            State(state.clone()),
            Json(CherryPickBody {
                project: git_project_body(&repository),
                commits: vec![feature_commit.trim().to_string()],
                no_commit: true,
                mainline: None,
                expected_head: main_head.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
                source_ref: None,
                expected_source_head: None,
            }),
        )
        .await
        .expect_err("no-commit cherry-pick must be rejected until it has a recoverable workflow");
        assert_eq!(unsafe_no_commit.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(
            git_test_output(&repository, ["status", "--porcelain"])
                .trim()
                .is_empty(),
            "rejecting no-commit must not leave an unmerged index"
        );
        let response = cherry_pick(
            State(state.clone()),
            Json(CherryPickBody {
                project: git_project_body(&repository),
                commits: vec![feature_commit.trim().to_string()],
                no_commit: false,
                mainline: None,
                expected_head: main_head,
                expected_target_ref: "refs/heads/main".to_string(),
                source_ref: None,
                expected_source_head: None,
            }),
        )
        .await
        .expect("cherry-pick conflict is a recoverable response")
        .0;
        let operation = response.operation_state.expect("operation state");
        assert!(response.success);
        assert_eq!(operation.operation, "cherry-pick");
        assert!(operation.in_progress);
        assert!(operation.can_continue);
        assert!(operation.can_abort);
        assert!(operation.can_skip);
        assert!(operation.current_commit.is_some());

        let queried = operation_state(
            State(state.clone()),
            Query(git_project_query(&repository)),
        )
        .await
        .expect("operation state endpoint")
        .0;
        assert_eq!(queried.operation, "cherry-pick");
        let status_during_conflict = status(
            State(state.clone()),
            Query(git_project_query(&repository)),
        )
        .await
        .expect("status includes operation state")
        .0;
        assert_eq!(
            status_during_conflict
                .operation_state
                .expect("status operation state")
                .operation,
            "cherry-pick"
        );

        std::fs::write(repository.join("conflict.txt"), "resolved\n").unwrap();
        git_test_command(&repository, ["add", "conflict.txt"]);
        let continued = continue_operation(
            State(state.clone()),
            Json(git_project_body(&repository)),
        )
        .await
        .expect("continue cherry-pick")
        .0;
        assert!(continued.success);
        assert!(
            !continued
                .operation_state
                .expect("completed operation state")
                .in_progress
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("conflict.txt")).unwrap(),
            "resolved\n"
        );
        assert_eq!(
            git_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "feature change"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_cherry_pick_keeps_redundant_commit_like_lazygit() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-empty-cherry-pick-workflow-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("already.txt"), "same patch\n").unwrap();
        git_test_command(&repository, ["add", "already.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature patch"]);
        let feature_commit = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["checkout", "main"]);
        // This models an equivalent change already introduced through a
        // different commit. LazyGit keeps the redundant selection as an
        // intentionally empty commit instead of stranding users at Skip.
        std::fs::write(repository.join("already.txt"), "same patch\n").unwrap();
        git_test_command(&repository, ["add", "already.txt"]);
        git_test_command(&repository, ["commit", "-m", "already applied another way"]);
        let main_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let response = cherry_pick(
            State(state.clone()),
            Json(CherryPickBody {
                project: git_project_body(&repository),
                commits: vec![feature_commit],
                no_commit: false,
                mainline: None,
                expected_head: main_head,
                expected_target_ref: "refs/heads/main".to_string(),
                source_ref: None,
                expected_source_head: None,
            }),
        )
        .await
        .expect("redundant cherry-pick is retained")
        .0;
        assert!(response.success);
        assert!(
            !response
                .operation_state
                .expect("completed cherry-pick state")
                .in_progress,
            "a redundant pick must not leave recovery state behind"
        );
        assert!(
            git_test_output(&repository, ["status", "--porcelain"])
                .trim()
                .is_empty(),
            "the retained empty pick does not leave user changes to resolve"
        );
        assert!(
            git_test_output(&repository, ["diff", "--quiet", "HEAD^", "HEAD"])
                .trim()
                .is_empty(),
            "the retained commit is intentionally empty"
        );
        assert_eq!(
            git_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "feature patch",
            "LazyGit-compatible preservation keeps the selected commit message"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn source_branch_cherry_pick_preview_pins_both_heads_and_applies_git_order() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-source-cherry-pick-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        // `release` deliberately starts at the same object as `main` so the
        // target-ref guard is tested independently from the target OID guard.
        git_test_command(&repository, ["branch", "release", "main"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("one.txt"), "one\n").unwrap();
        git_test_command(&repository, ["add", "one.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature one"]);
        std::fs::write(repository.join("two.txt"), "two\n").unwrap();
        git_test_command(&repository, ["add", "two.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature two"]);
        git_test_command(&repository, ["checkout", "main"]);
        let main_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let preview = cherry_pick_preview(
            State(state.clone()),
            Query(CherryPickPreviewQuery {
                project: git_project_query(&repository),
                source_ref: "refs/heads/feature".to_string(),
            }),
        )
        .await
        .expect("source branch preview")
        .0;
        assert_eq!(preview.source_ref, "refs/heads/feature");
        assert_eq!(preview.target_ref, "refs/heads/main");
        assert_eq!(preview.target_head, main_head);
        assert_eq!(
            preview
                .commits
                .iter()
                .map(|commit| commit.message.as_str())
                .collect::<Vec<_>>(),
            vec!["feature one", "feature two"],
            "source preview is in Git application order",
        );

        // Switching to a different branch at the same object must still
        // invalidate the confirmation. An expected HEAD alone cannot tell
        // `main` and `release` apart here.
        git_test_command(&repository, ["checkout", "release"]);
        let wrong_target = cherry_pick(
            State(state.clone()),
            Json(CherryPickBody {
                project: git_project_body(&repository),
                commits: preview.commits.iter().map(|commit| commit.hash.clone()).collect(),
                no_commit: false,
                mainline: None,
                expected_head: preview.target_head.clone(),
                expected_target_ref: preview.target_ref.clone(),
                source_ref: Some(preview.source_ref.clone()),
                expected_source_head: Some(preview.source_head.clone()),
            }),
        )
        .await
        .expect_err("a same-OID checkout to a different branch is rejected");
        assert_eq!(wrong_target.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            preview.target_head,
            "target-ref rejection does not mutate the alternate branch",
        );
        git_test_command(&repository, ["checkout", "main"]);

        // Advance the source after the preview. The original confirmation
        // must not silently apply a different branch state.
        git_test_command(&repository, ["checkout", "feature"]);
        std::fs::write(repository.join("three.txt"), "three\n").unwrap();
        git_test_command(&repository, ["add", "three.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature three"]);
        git_test_command(&repository, ["checkout", "main"]);
        let stale = cherry_pick(
            State(state.clone()),
            Json(CherryPickBody {
                project: git_project_body(&repository),
                commits: preview.commits.iter().map(|commit| commit.hash.clone()).collect(),
                no_commit: false,
                mainline: None,
                expected_head: preview.target_head.clone(),
                expected_target_ref: preview.target_ref.clone(),
                source_ref: Some(preview.source_ref.clone()),
                expected_source_head: Some(preview.source_head.clone()),
            }),
        )
        .await
        .expect_err("a changed source branch invalidates the old preview");
        assert_eq!(stale.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            preview.target_head,
            "stale source rejection does not mutate the target branch",
        );

        let refreshed = cherry_pick_preview(
            State(state.clone()),
            Query(CherryPickPreviewQuery {
                project: git_project_query(&repository),
                source_ref: "refs/heads/feature".to_string(),
            }),
        )
        .await
        .expect("refreshed source preview")
        .0;
        let completed = cherry_pick(
            State(state.clone()),
            Json(CherryPickBody {
                project: git_project_body(&repository),
                // Deliberately reverse the visible request order. The server
                // must restore its own source-topology order before calling
                // Git, rather than trusting tap order from the client.
                commits: refreshed
                    .commits
                    .iter()
                    .rev()
                    .map(|commit| commit.hash.clone())
                    .collect(),
                no_commit: false,
                mainline: None,
                expected_head: refreshed.target_head.clone(),
                expected_target_ref: refreshed.target_ref.clone(),
                source_ref: Some(refreshed.source_ref.clone()),
                expected_source_head: Some(refreshed.source_head.clone()),
            }),
        )
        .await
        .expect("contextual cherry-pick")
        .0;
        assert!(completed.success);
        assert_eq!(
            git_test_output(&repository, ["log", "-3", "--format=%s"])
                .lines()
                .collect::<Vec<_>>(),
            vec!["feature three", "feature two", "feature one"],
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn source_branch_cherry_pick_accepts_a_slash_named_remote() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-source-cherry-pick-slash-remote-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        git_test_command(&repository, ["checkout", "-b", "source"]);
        std::fs::write(repository.join("picked.txt"), "from remote tracking ref\n").unwrap();
        git_test_command(&repository, ["add", "picked.txt"]);
        git_test_command(&repository, ["commit", "-m", "pick from slash remote"]);
        let source_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["checkout", "main"]);
        let main_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        // Git permits slashes in remote names. Simulate a fetched tracking ref
        // without depending on a network transport so the source picker must
        // parse `team/review` as the remote rather than `team`.
        git_test_command(
            &repository,
            ["remote", "add", "team/review", "https://example.test/review.git"],
        );
        git_test_command(
            &repository,
            [
                "update-ref",
                "refs/remotes/team/review/feature/mobile",
                source_head.as_str(),
            ],
        );

        let state = git_test_app_state(&root).await;
        let preview = cherry_pick_preview(
            State(state.clone()),
            Query(CherryPickPreviewQuery {
                project: git_project_query(&repository),
                source_ref: "refs/remotes/team/review/feature/mobile".to_string(),
            }),
        )
        .await
        .expect("preview slash-named remote source")
        .0;
        assert_eq!(
            preview.source_ref,
            "refs/remotes/team/review/feature/mobile"
        );
        assert_eq!(preview.target_head, main_head);
        assert_eq!(preview.commits.len(), 1);

        let completed = cherry_pick(
            State(state.clone()),
            Json(CherryPickBody {
                project: git_project_body(&repository),
                commits: preview.commits.iter().map(|commit| commit.hash.clone()).collect(),
                no_commit: false,
                mainline: None,
                expected_head: preview.target_head.clone(),
                expected_target_ref: preview.target_ref.clone(),
                source_ref: Some(preview.source_ref.clone()),
                expected_source_head: Some(preview.source_head.clone()),
            }),
        )
        .await
        .expect("cherry-pick from slash-named remote source")
        .0;
        assert!(completed.success);
        assert_eq!(
            git_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "pick from slash remote"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn source_branch_cherry_pick_preview_keeps_merge_candidates_with_first_parent() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-source-merge-cherry-pick-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("feature.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["add", "feature.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature change"]);
        git_test_command(&repository, ["checkout", "-b", "side"]);
        std::fs::write(repository.join("side.txt"), "side\n").unwrap();
        git_test_command(&repository, ["add", "side.txt"]);
        git_test_command(&repository, ["commit", "-m", "side change"]);
        git_test_command(&repository, ["checkout", "feature"]);
        git_test_command(
            &repository,
            ["merge", "--no-ff", "side", "-m", "merge side into feature"],
        );
        let merge_commit = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        assert_eq!(
            git_test_output(&repository, ["rev-list", "--parents", "-n", "1", "HEAD"])
                .split_whitespace()
                .count(),
            3,
            "fixture must contain a real two-parent merge"
        );
        git_test_command(&repository, ["checkout", "main"]);

        let state = git_test_app_state(&root).await;
        let preview = cherry_pick_preview(
            State(state.clone()),
            Query(CherryPickPreviewQuery {
                project: git_project_query(&repository),
                source_ref: "refs/heads/feature".to_string(),
            }),
        )
        .await
        .expect("source branch preview with merge")
        .0;
        assert!(
            preview.commits.iter().any(|commit| commit.hash == merge_commit),
            "a source merge must be selectable rather than silently filtered"
        );
        assert_eq!(preview.merge_commit_hashes, vec![merge_commit.clone()]);
        assert_eq!(preview.default_mainline, Some(1));

        // Omit mainline deliberately. The handler must mirror LazyGit's
        // `-m1` default for a selected merge and create the parent-1 diff.
        let completed = cherry_pick(
            State(state.clone()),
            Json(CherryPickBody {
                project: git_project_body(&repository),
                commits: vec![merge_commit],
                no_commit: false,
                mainline: None,
                expected_head: preview.target_head.clone(),
                expected_target_ref: preview.target_ref.clone(),
                source_ref: Some(preview.source_ref.clone()),
                expected_source_head: Some(preview.source_head.clone()),
            }),
        )
        .await
        .expect("merge cherry-pick with LazyGit default mainline")
        .0;
        assert!(completed.success);
        assert!(
            !completed
                .operation_state
                .expect("completed merge cherry-pick state")
                .in_progress,
            "the default parent must not leave a recovery workflow"
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("side.txt")).unwrap(),
            "side\n",
            "parent 1 selects the merged side's file change"
        );
        assert_eq!(
            git_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "merge side into feature"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cherry_pick_rejects_an_ignored_local_path_before_it_can_be_overwritten() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-ignored-cherry-pick-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join(".gitignore"), "generated.txt\n").unwrap();
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", ".gitignore", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("generated.txt"), "feature copy\n").unwrap();
        // The feature deliberately starts tracking a file that is ignored on
        // main. A local ignored copy on main would otherwise be vulnerable.
        git_test_command(&repository, ["add", "-f", "generated.txt"]);
        git_test_command(&repository, ["commit", "-m", "track generated output"]);
        git_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("generated.txt"), "local ignored copy\n").unwrap();
        let main_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let preview = cherry_pick_preview(
            State(state.clone()),
            Query(CherryPickPreviewQuery {
                project: git_project_query(&repository),
                source_ref: "refs/heads/feature".to_string(),
            }),
        )
        .await
        .expect("preview can inspect an ignored local path safely")
        .0;
        let rejected = cherry_pick(
            State(state.clone()),
            Json(CherryPickBody {
                project: git_project_body(&repository),
                commits: preview.commits.iter().map(|commit| commit.hash.clone()).collect(),
                no_commit: false,
                mainline: None,
                expected_head: preview.target_head.clone(),
                expected_target_ref: preview.target_ref.clone(),
                source_ref: Some(preview.source_ref.clone()),
                expected_source_head: Some(preview.source_head.clone()),
            }),
        )
        .await
        .expect_err("ignored files make a cherry-pick unsafe");
        assert_eq!(rejected.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            std::fs::read_to_string(repository.join("generated.txt")).unwrap(),
            "local ignored copy\n",
            "the ignored local file is never overwritten",
        );
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            main_head,
            "preflight failure does not move HEAD",
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ignored_local_output_is_not_overwritten_by_checkout_merge_or_stash_branch() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-ignored-checkout-merge-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join(".gitignore"), "generated.txt\n").unwrap();
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", ".gitignore", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let main_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        git_test_command(&repository, ["checkout", "-b", "source"]);
        std::fs::write(repository.join("generated.txt"), "source tracked output\n").unwrap();
        git_test_command(&repository, ["add", "-f", "generated.txt"]);
        git_test_command(&repository, ["commit", "-m", "track generated output"]);
        let source_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("generated.txt"), "local ignored output\n").unwrap();

        let state = git_test_app_state(&root).await;
        let local_checkout = checkout(
            State(state.clone()),
            Json(BranchBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                branch: "source".to_string(),
            }),
        )
        .await
        .expect_err("local checkout must not overwrite ignored output");
        assert!(!local_checkout.body.error.is_empty());
        assert_eq!(current_branch(&repository).await.unwrap(), "main");
        assert_eq!(
            std::fs::read_to_string(repository.join("generated.txt")).unwrap(),
            "local ignored output\n"
        );

        // A remote-tracking branch uses the same checkout primitive, but it
        // must also leave no half-created local tracking branch behind.
        git_test_command(&repository, ["remote", "add", "origin", "."]);
        git_test_command(
            &repository,
            ["update-ref", "refs/remotes/origin/tracked", &source_head],
        );
        let tracking_checkout = checkout_tracking_branch(
            State(state.clone()),
            Json(TrackingCheckoutBody {
                project: git_project_body(&repository),
                remote: "origin".to_string(),
                branch: "tracked".to_string(),
            }),
        )
        .await
        .expect_err("tracking checkout must not overwrite ignored output");
        assert!(!tracking_checkout.body.error.is_empty());
        assert!(git(&repository, ["show-ref", "--verify", "refs/heads/tracked"])
            .await
            .is_err());
        assert_eq!(current_branch(&repository).await.unwrap(), "main");
        assert_eq!(
            std::fs::read_to_string(repository.join("generated.txt")).unwrap(),
            "local ignored output\n"
        );

        let detached_checkout = advanced_detached_checkout(
            State(state.clone()),
            Json(AdvancedDetachedCheckoutBody {
                scope: AdvancedGitProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                reference: source_head.clone(),
                expected_head: main_head.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
                confirm: true,
            }),
        )
        .await
        .expect_err("detached checkout must not overwrite ignored output");
        assert!(!detached_checkout.body.error.is_empty());
        assert_eq!(current_branch(&repository).await.unwrap(), "main");
        assert_eq!(
            std::fs::read_to_string(repository.join("generated.txt")).unwrap(),
            "local ignored output\n"
        );

        let merge_error = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "source".to_string(),
                merge_mode: None,
                no_fast_forward: false,
                commit_squash: false,
                message: None,
                expected_head: main_head.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect_err("merge must not overwrite ignored output");
        assert!(!merge_error.body.error.is_empty());
        assert_eq!(current_branch(&repository).await.unwrap(), "main");
        assert_eq!(
            std::fs::read_to_string(repository.join("generated.txt")).unwrap(),
            "local ignored output\n"
        );

        // Creating a branch from a stash checks out the stash tree. Put the
        // tracked path into a stash from source, then return to main where
        // the same path is ignored and recreate it locally. An ignored-only
        // stash uses an auxiliary parent and is not part of the checkout
        // tree, so this tracked-path setup is the real overwrite case.
        std::fs::remove_file(repository.join("generated.txt")).unwrap();
        git_test_command(&repository, ["checkout", "source"]);
        std::fs::write(repository.join("generated.txt"), "stashed source output\n").unwrap();
        git_test_command(&repository, ["stash", "push", "-m", "saved output"]);
        git_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("generated.txt"), "replacement ignored output\n").unwrap();
        let stash_branch = management_create_branch_from_stash(
            State(state.clone()),
            Json(ManagementStashBranchBody {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                branch: "recover-output".to_string(),
                reference: "stash@{0}".to_string(),
                expected_hash: None,
                confirm: true,
            }),
        )
        .await
        .expect_err("stash branch must not overwrite ignored output");
        assert!(!stash_branch.body.error.is_empty());
        assert!(git(&repository, ["show-ref", "--verify", "refs/heads/recover-output"])
            .await
            .is_err());
        assert_eq!(current_branch(&repository).await.unwrap(), "main");
        assert_eq!(
            std::fs::read_to_string(repository.join("generated.txt")).unwrap(),
            "replacement ignored output\n"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ignored_local_output_is_not_overwritten_by_rebase_hard_reset_or_pull() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-ignored-rewrite-pull-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join(".gitignore"), "generated.txt\n").unwrap();
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", ".gitignore", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("feature.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["add", "feature.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature change"]);
        let feature_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("generated.txt"), "upstream tracked output\n").unwrap();
        git_test_command(&repository, ["add", "-f", "generated.txt"]);
        git_test_command(&repository, ["commit", "-m", "track generated output"]);
        let main_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["checkout", "feature"]);
        std::fs::write(repository.join("generated.txt"), "ignored rebase output\n").unwrap();

        let state = git_test_app_state(&root).await;
        let rebase_error = rebase(
            State(state.clone()),
            Json(RebaseBody {
                project: git_project_body(&repository),
                upstream: "main".to_string(),
                interactive: false,
                expected_head: feature_head.clone(),
                expected_target_ref: "refs/heads/feature".to_string(),
            }),
        )
        .await
        .expect_err("rebase must not overwrite ignored output");
        assert_eq!(rebase_error.status, StatusCode::CONFLICT);
        assert_eq!(current_branch(&repository).await.unwrap(), "feature");
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            feature_head
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("generated.txt")).unwrap(),
            "ignored rebase output\n"
        );

        let reset_error = reset(
            State(state.clone()),
            Json(ResetBody {
                project: git_project_body(&repository),
                target: main_head,
                mode: "hard".to_string(),
                confirm_hard: true,
                expected_head: feature_head.clone(),
                expected_target_ref: "refs/heads/feature".to_string(),
            }),
        )
        .await
        .expect_err("hard reset must not overwrite ignored output");
        assert_eq!(reset_error.status, StatusCode::CONFLICT);
        assert_eq!(current_branch(&repository).await.unwrap(), "feature");
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            feature_head
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("generated.txt")).unwrap(),
            "ignored rebase output\n"
        );

        // A pull fetches the remote object before it integrates it. The
        // guard must examine that fetched tree rather than trusting a stale
        // remote-tracking ref, then leave the local branch untouched.
        let remote = root.join("remote.git");
        let primary = root.join("primary");
        let replica = root.join("replica");
        let remote_path = remote.display().to_string();
        let primary_path = primary.display().to_string();
        let replica_path = replica.display().to_string();
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                remote_path.as_str(),
            ],
        );
        git_test_command(
            &root,
            ["init", "--initial-branch=main", primary_path.as_str()],
        );
        configure_test_repository(&primary);
        std::fs::write(primary.join(".gitignore"), "generated.txt\n").unwrap();
        std::fs::write(primary.join("README.md"), "base\n").unwrap();
        git_test_command(&primary, ["add", ".gitignore", "README.md"]);
        git_test_command(&primary, ["commit", "-m", "base"]);
        git_test_command(&primary, ["remote", "add", "origin", remote_path.as_str()]);
        git_test_command(&primary, ["push", "-u", "origin", "main"]);
        let primary_head = git_test_output(&primary, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        git_test_command(&root, ["clone", remote_path.as_str(), replica_path.as_str()]);
        configure_test_repository(&replica);
        std::fs::write(replica.join("generated.txt"), "remote tracked output\n").unwrap();
        git_test_command(&replica, ["add", "-f", "generated.txt"]);
        git_test_command(&replica, ["commit", "-m", "track generated output"]);
        git_test_command(&replica, ["push"]);

        std::fs::write(primary.join("generated.txt"), "ignored pull output\n").unwrap();
        let pull_error = pull(State(state.clone()), Json(git_project_body(&primary)))
            .await
            .expect_err("pull must not overwrite ignored output");
        assert_eq!(pull_error.status, StatusCode::CONFLICT);
        assert_eq!(current_branch(&primary).await.unwrap(), "main");
        assert_eq!(
            git_test_output(&primary, ["rev-parse", "HEAD"]).trim(),
            primary_head
        );
        assert_eq!(
            std::fs::read_to_string(primary.join("generated.txt")).unwrap(),
            "ignored pull output\n"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn harmless_ignored_build_output_does_not_block_cherry_pick_or_stash_apply() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-harmless-ignored-output-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join(".gitignore"), "build/\n").unwrap();
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", ".gitignore", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("feature.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["add", "feature.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature change"]);
        git_test_command(&repository, ["checkout", "main"]);

        std::fs::create_dir_all(repository.join("build")).unwrap();
        std::fs::write(repository.join("build/cache.txt"), "local build output\n").unwrap();

        let state = git_test_app_state(&root).await;
        let preview = cherry_pick_preview(
            State(state.clone()),
            Query(CherryPickPreviewQuery {
                project: git_project_query(&repository),
                source_ref: "refs/heads/feature".to_string(),
            }),
        )
        .await
        .expect("preview source branch with harmless ignored output")
        .0;
        let picked = cherry_pick(
            State(state.clone()),
            Json(CherryPickBody {
                project: git_project_body(&repository),
                commits: preview.commits.iter().map(|commit| commit.hash.clone()).collect(),
                no_commit: false,
                mainline: None,
                expected_head: preview.target_head.clone(),
                expected_target_ref: preview.target_ref.clone(),
                source_ref: Some(preview.source_ref.clone()),
                expected_source_head: Some(preview.source_head.clone()),
            }),
        )
        .await
        .expect("unrelated ignored build output must not block cherry-pick")
        .0;
        assert!(picked.success);
        assert_eq!(
            std::fs::read_to_string(repository.join("build/cache.txt")).unwrap(),
            "local build output\n"
        );

        std::fs::write(repository.join("tracked.txt"), "stashed\n").unwrap();
        git_test_command(&repository, ["stash", "push", "-m", "safe ignored output"]);
        let applied = apply_stash(
            State(state.clone()),
            Json(StashRefBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                reference: "stash@{0}".to_string(),
                expected_hash: None,
            }),
        )
        .await
        .expect("unrelated ignored build output must not block stash apply")
        .0;
        assert!(applied.success);
        assert_eq!(
            std::fs::read_to_string(repository.join("tracked.txt")).unwrap(),
            "stashed\n"
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("build/cache.txt")).unwrap(),
            "local build output\n"
        );

        git_test_command(&repository, ["reset", "--hard", "HEAD"]);
        let popped = pop_stash(
            State(state.clone()),
            Json(StashRefBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                reference: "stash@{0}".to_string(),
                expected_hash: None,
            }),
        )
        .await
        .expect("unrelated ignored build output must not block stash pop")
        .0;
        assert!(popped.success);
        assert_eq!(
            std::fs::read_to_string(repository.join("tracked.txt")).unwrap(),
            "stashed\n"
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("build/cache.txt")).unwrap(),
            "local build output\n"
        );
        assert!(git_test_output(&repository, ["stash", "list"])
            .trim()
            .is_empty());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stash_apply_rejects_an_ignored_path_stored_in_that_stash() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-ignored-stash-collision-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join(".gitignore"), "build/\n").unwrap();
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", ".gitignore", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        std::fs::create_dir_all(repository.join("build")).unwrap();
        std::fs::write(repository.join("build/cache.txt"), "stashed build output\n").unwrap();
        git_test_command(
            &repository,
            ["stash", "push", "--all", "-m", "ignored build output"],
        );
        std::fs::create_dir_all(repository.join("build")).unwrap();
        std::fs::write(repository.join("build/cache.txt"), "current build output\n").unwrap();

        let state = git_test_app_state(&root).await;
        let rejected = apply_stash(
            State(state.clone()),
            Json(StashRefBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                reference: "stash@{0}".to_string(),
                expected_hash: None,
            }),
        )
        .await
        .expect_err("stash must not overwrite an ignored path it contains");
        assert_eq!(rejected.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            std::fs::read_to_string(repository.join("build/cache.txt")).unwrap(),
            "current build output\n"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merge_and_rebase_conflicts_expose_correct_recovery_actions() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-merge-rebase-workflow-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("conflict.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "conflict.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("conflict.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "feature change"]);
        git_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("conflict.txt"), "main\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "main change"]);

        let state = git_test_app_state(&root).await;
        let merge_response = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "feature".to_string(),
                merge_mode: None,
                no_fast_forward: true,
                commit_squash: false,
                message: None,
                expected_head: git_test_output(&repository, ["rev-parse", "HEAD"])
                    .trim()
                    .to_string(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("merge conflict is recoverable")
        .0;
        let merge_state = merge_response.operation_state.expect("merge state");
        assert_eq!(merge_state.operation, "merge");
        assert!(merge_state.in_progress);
        assert!(!merge_state.can_skip);
        let aborted = abort_operation(
            State(state.clone()),
            Json(git_project_body(&repository)),
        )
        .await
        .expect("abort merge")
        .0;
        assert!(
            !aborted
                .operation_state
                .expect("aborted operation state")
                .in_progress
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("conflict.txt")).unwrap(),
            "main\n"
        );

        git_test_command(&repository, ["checkout", "feature"]);
        let rebase_response = rebase(
            State(state.clone()),
            Json(RebaseBody {
                project: git_project_body(&repository),
                upstream: "main".to_string(),
                interactive: false,
                expected_head: git_test_output(&repository, ["rev-parse", "HEAD"])
                    .trim()
                    .to_string(),
                expected_target_ref: "refs/heads/feature".to_string(),
            }),
        )
        .await
        .expect("rebase conflict is recoverable")
        .0;
        let rebase_state = rebase_response.operation_state.expect("rebase state");
        assert_eq!(rebase_state.operation, "rebase");
        assert!(rebase_state.in_progress);
        assert!(rebase_state.can_continue);
        assert!(rebase_state.can_abort);
        assert!(rebase_state.can_skip);
        assert!(rebase_state.current_step.is_some());
        assert!(rebase_state.total_steps.is_some());
        let skipped = skip_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("skip conflicting rebase commit")
            .0;
        assert!(
            !skipped
                .operation_state
                .expect("completed operation state")
                .in_progress
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("conflict.txt")).unwrap(),
            "main\n"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merge_modes_match_lazygit_and_preserve_legacy_no_fast_forward() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-lazygit-merge-modes-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let base = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        // A lightweight tag must not inherit this setting into a surprise
        // signed/annotated tag. The annotated replacement below disables it
        // because this regression is about tag shape, not signing-agent I/O.
        git_test_command(&repository, ["config", "tag.gpgSign", "true"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("feature.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["add", "feature.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature"]);
        let feature_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["checkout", "main"]);

        let state = git_test_app_state(&root).await;
        let default_merge = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "feature".to_string(),
                merge_mode: Some(MergeMode::Default),
                no_fast_forward: false,
                commit_squash: false,
                message: None,
                expected_head: base.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("default merge")
        .0;
        assert!(default_merge.success);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            feature_head,
            "LazyGit's default choice delegates to Git's ordinary merge behavior"
        );

        git_test_command(&repository, ["reset", "--hard", base.as_str()]);
        let fast_forward = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "feature".to_string(),
                merge_mode: Some(MergeMode::FastForward),
                no_fast_forward: false,
                commit_squash: false,
                message: None,
                expected_head: base.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("fast-forward merge")
        .0;
        assert!(fast_forward.success);
        assert!(!fast_forward
            .operation_state
            .expect("completed fast-forward state")
            .in_progress);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            feature_head,
            "LazyGit's Fast-forward choice must move main to the source object"
        );

        git_test_command(&repository, ["reset", "--hard", base.as_str()]);
        let no_fast_forward = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "feature".to_string(),
                merge_mode: Some(MergeMode::NoFastForward),
                no_fast_forward: false,
                commit_squash: false,
                message: None,
                expected_head: base.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("explicit no-fast-forward merge")
        .0;
        assert!(no_fast_forward.success);
        assert_eq!(
            git_test_output(&repository, ["rev-list", "--parents", "-n", "1", "HEAD"])
                .split_whitespace()
                .count(),
            3,
            "LazyGit's No fast-forward choice must create a two-parent merge"
        );

        // LazyGit disables its Fast-forward item when the selected ref is
        // not a descendant of HEAD. `git merge --ff` would otherwise fall
        // back to an ordinary merge, which is not the operation the user
        // selected.
        git_test_command(&repository, ["reset", "--hard", base.as_str()]);
        std::fs::write(repository.join("main.txt"), "main\n").unwrap();
        git_test_command(&repository, ["add", "main.txt"]);
        git_test_command(&repository, ["commit", "-m", "main divergence"]);
        let divergent_main_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let fast_forward_divergence = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "feature".to_string(),
                merge_mode: Some(MergeMode::FastForward),
                no_fast_forward: false,
                commit_squash: false,
                message: None,
                expected_head: divergent_main_head.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect_err("Fast-forward must reject divergent histories");
        assert_eq!(fast_forward_divergence.status, StatusCode::BAD_REQUEST);
        assert!(fast_forward_divergence
            .body
            .error
            .contains("only available when the current branch is an ancestor"));
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            divergent_main_head,
            "the rejected Fast-forward must not create a hidden ordinary merge"
        );

        git_test_command(&repository, ["reset", "--hard", base.as_str()]);
        let legacy_no_fast_forward = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "feature".to_string(),
                merge_mode: None,
                no_fast_forward: true,
                commit_squash: false,
                message: None,
                expected_head: base.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("legacy noFastForward merge")
        .0;
        assert!(legacy_no_fast_forward.success);
        assert_eq!(
            git_test_output(&repository, ["rev-list", "--parents", "-n", "1", "HEAD"])
                .split_whitespace()
                .count(),
            3,
            "legacy noFastForward must remain compatible with the old client"
        );

        git_test_command(&repository, ["reset", "--hard", base.as_str()]);
        let squash = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "feature".to_string(),
                merge_mode: Some(MergeMode::Squash),
                no_fast_forward: false,
                commit_squash: false,
                message: None,
                expected_head: base.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("uncommitted squash merge")
        .0;
        assert!(squash.success);
        assert!(!squash
            .operation_state
            .expect("completed uncommitted squash state")
            .in_progress);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            base,
            "LazyGit's plain squash leaves HEAD unchanged"
        );
        assert!(git_test_output(&repository, ["diff", "--cached", "--name-only"])
            .lines()
            .any(|path| path == "feature.txt"));
        assert!(
            !squash_merge_recovery_marker_path(&repository)
                .await
                .unwrap()
                .exists(),
            "a successful uncommitted squash is ordinary staged state, not a recovery lock"
        );

        git_test_command(&repository, ["reset", "--hard", base.as_str()]);
        let committed_squash = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "feature".to_string(),
                merge_mode: Some(MergeMode::Squash),
                no_fast_forward: false,
                commit_squash: true,
                message: Some("squashed feature".to_string()),
                expected_head: base.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("explicit squash-and-commit merge")
        .0;
        assert!(committed_squash.success);
        assert!(!committed_squash
            .operation_state
            .expect("completed committed squash state")
            .in_progress);
        assert_eq!(
            git_test_output(&repository, ["rev-list", "--parents", "-n", "1", "HEAD"])
                .split_whitespace()
                .count(),
            2,
            "a committed squash is a normal single-parent commit"
        );
        assert_eq!(
            git_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "squashed feature"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn replacing_a_local_tag_requires_confirmation_and_moves_the_ref() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-force-replace-tag-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let base = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let state = git_test_app_state(&root).await;
        let scope = || ManagementProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };
        let initial = management_create_tag_at_target(
            State(state.clone()),
            Json(ManagementTagCreateBody {
                scope: scope(),
                tag: "v1.0.0".to_string(),
                message: None,
                target: Some(base.clone()),
                force: false,
                confirm: false,
            }),
        )
        .await
        .expect("create initial lightweight tag")
        .0;
        assert!(initial.success);
        let stale_managed_create = management_create_tag_at_target(
            State(state.clone()),
            Json(ManagementTagCreateBody {
                scope: scope(),
                tag: "v1.0.0".to_string(),
                message: None,
                target: Some(base.clone()),
                force: false,
                confirm: false,
            }),
        )
        .await
        .expect_err("an unreviewed existing tag must not be overwritten");
        assert_eq!(stale_managed_create.status, StatusCode::CONFLICT);
        assert!(stale_managed_create
            .body
            .error
            .contains("Refresh the tag list and explicitly confirm replacing it"));
        assert_eq!(
            git_test_output(&repository, ["cat-file", "-t", "refs/tags/v1.0.0"])
                .trim(),
            "commit",
            "a blank tag message must remain a lightweight tag even when tag.gpgSign is enabled"
        );
        let legacy_lightweight = create_tag(
            State(state.clone()),
            Json(TagBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                tag: "legacy-lightweight".to_string(),
                message: None,
                force: false,
                confirm: false,
            }),
        )
        .await
        .expect("create legacy lightweight tag")
        .0;
        assert!(legacy_lightweight.success);
        let stale_legacy_create = create_tag(
            State(state.clone()),
            Json(TagBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                tag: "legacy-lightweight".to_string(),
                message: None,
                force: false,
                confirm: false,
            }),
        )
        .await
        .expect_err("the legacy route must not return Git's raw duplicate-tag error");
        assert_eq!(stale_legacy_create.status, StatusCode::CONFLICT);
        assert!(stale_legacy_create
            .body
            .error
            .contains("Refresh the tag list and explicitly confirm replacing it"));
        assert_eq!(
            git_test_output(&repository, ["cat-file", "-t", "refs/tags/legacy-lightweight"])
                .trim(),
            "commit",
            "the legacy tag endpoint must keep a blank-message tag lightweight too"
        );
        git_test_command(&repository, ["config", "tag.gpgSign", "false"]);
        std::fs::write(repository.join("README.md"), "next\n").unwrap();
        git_test_command(&repository, ["commit", "-a", "-m", "next"]);
        let next = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let unconfirmed = management_create_tag_at_target(
            State(state.clone()),
            Json(ManagementTagCreateBody {
                scope: scope(),
                tag: "v1.0.0".to_string(),
                message: None,
                target: Some(next.clone()),
                force: true,
                confirm: false,
            }),
        )
        .await
        .expect_err("force-replacing a tag must require confirmation");
        assert_eq!(unconfirmed.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "refs/tags/v1.0.0"])
                .trim(),
            base
        );

        let replaced = management_create_tag_at_target(
            State(state.clone()),
            Json(ManagementTagCreateBody {
                scope: scope(),
                tag: "v1.0.0".to_string(),
                message: Some("retagged release".to_string()),
                target: Some(next.clone()),
                force: true,
                confirm: true,
            }),
        )
        .await
        .expect("confirmed tag replacement")
        .0;
        assert!(replaced.success);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "refs/tags/v1.0.0^{}"])
                .trim(),
            next
        );
        assert_eq!(
            git_test_output(&repository, ["cat-file", "-t", "refs/tags/v1.0.0"])
                .trim(),
            "tag",
            "replacement retains the requested annotated-tag form"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn annotated_tag_names_resolve_to_commits_for_tag_ref_workflows() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-annotated-tag-reference-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let base = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let scope = || ManagementProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };
        management_create_tag_at_target(
            State(state.clone()),
            Json(ManagementTagCreateBody {
                scope: scope(),
                tag: "v1.0.0".to_string(),
                message: Some("first release".to_string()),
                target: Some(base.clone()),
                force: false,
                confirm: false,
            }),
        )
        .await
        .expect("create annotated tag");
        assert_eq!(
            git_test_output(&repository, ["cat-file", "-t", "v1.0.0"]).trim(),
            "tag"
        );

        assert_eq!(
            resolve_commit_reference(&repository, "v1.0.0")
                .await
                .expect("peel annotated tag to its commit"),
            base
        );
        let branch = management_create_branch_from_ref(
            State(state.clone()),
            Json(ManagementBranchCreateFromRefBody {
                scope: scope(),
                branch: "release-base".to_string(),
                start_point: "v1.0.0".to_string(),
            }),
        )
        .await
        .expect("branch from annotated tag")
        .0;
        assert!(branch.success);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "release-base"]).trim(),
            base
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn standalone_fetches_preserve_fetch_head_like_lazygit() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-fetch-head-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        let remote = root.join("remote.git");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let remote_path = remote.display().to_string();
        git_test_command(
            &root,
            ["init", "--bare", "--initial-branch=main", remote_path.as_str()],
        );
        git_test_command(&repository, ["remote", "add", "origin", remote_path.as_str()]);
        git_test_command(&repository, ["push", "-u", "origin", "main"]);

        let state = git_test_app_state(&root).await;
        let scope = || ManagementProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };
        let fetch_head = repository.join(".git").join("FETCH_HEAD");
        std::fs::write(&fetch_head, "reviewed fetch input\n").unwrap();
        let _ = management_fetch_remote(
            State(state.clone()),
            Json(ManagementRemoteFetchBody {
                scope: scope(),
                remote: "origin".to_string(),
                prune: false,
                tags: None,
            }),
        )
        .await
        .expect("standalone selected-remote fetch");
        assert_eq!(
            std::fs::read_to_string(&fetch_head).unwrap(),
            "reviewed fetch input\n",
            "the selected-remote Fetch action must not replace FETCH_HEAD"
        );

        std::fs::write(&fetch_head, "legacy reviewed input\n").unwrap();
        let _ = fetch(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("legacy standalone fetch");
        assert_eq!(
            std::fs::read_to_string(&fetch_head).unwrap(),
            "legacy reviewed input\n",
            "the legacy standalone Fetch action must also retain FETCH_HEAD"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fetch_all_remotes_matches_lazygit_and_preserves_fetch_head() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-fetch-all-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        let origin = root.join("origin.git");
        let mirror = root.join("mirror.git");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let base = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let origin_path = origin.display().to_string();
        let mirror_path = mirror.display().to_string();
        git_test_command(
            &root,
            ["init", "--bare", "--initial-branch=main", origin_path.as_str()],
        );
        git_test_command(
            &root,
            ["init", "--bare", "--initial-branch=main", mirror_path.as_str()],
        );
        git_test_command(&repository, ["remote", "add", "origin", origin_path.as_str()]);
        git_test_command(&repository, ["remote", "add", "mirror", mirror_path.as_str()]);
        git_test_command(&repository, ["push", "origin", "main"]);
        git_test_command(&repository, ["push", "mirror", "main"]);

        std::fs::write(repository.join("README.md"), "origin only\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "origin update"]);
        let origin_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["push", "origin", "main"]);

        git_test_command(&repository, ["reset", "--hard", base.as_str()]);
        std::fs::write(repository.join("README.md"), "mirror only\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "mirror update"]);
        let mirror_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["push", "mirror", "main"]);
        git_test_command(&repository, ["reset", "--hard", base.as_str()]);

        // Seed stale tracking refs so the assertion proves the route fetches
        // both configured remotes rather than merely preserving a ref created
        // while publishing this fixture.
        git_test_command(
            &repository,
            ["update-ref", "refs/remotes/origin/main", base.as_str()],
        );
        git_test_command(
            &repository,
            ["update-ref", "refs/remotes/mirror/main", base.as_str()],
        );
        let fetch_head = repository.join(".git").join("FETCH_HEAD");
        std::fs::write(&fetch_head, "reviewed fetch input\n").unwrap();

        let state = git_test_app_state(&root).await;
        let response = management_fetch_all_remotes(
            State(state.clone()),
            Json(ManagementFetchAllRemotesBody {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
            }),
        )
        .await
        .expect("fetch every configured remote")
        .0;

        assert!(response.success);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "refs/remotes/origin/main"])
                .trim(),
            origin_head,
        );
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "refs/remotes/mirror/main"])
                .trim(),
            mirror_head,
        );
        assert_eq!(
            std::fs::read_to_string(&fetch_head).unwrap(),
            "reviewed fetch input\n",
            "Fetch All must not replace FETCH_HEAD used by a reviewed workflow"
        );
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            base,
            "Fetch All updates refs but never integrates either remote"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn squash_merge_conflicts_have_truthful_finish_and_abort_recovery() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-squash-merge-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("conflict.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "conflict.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("conflict.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "feature change"]);
        git_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("conflict.txt"), "main\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "main change"]);
        let main_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let start_squash = |state: AppState| async {
            merge(
                State(state),
                Json(MergeBody {
                    project: git_project_body(&repository),
                    source: "feature".to_string(),
                    merge_mode: Some(MergeMode::Squash),
                    no_fast_forward: false,
                    commit_squash: false,
                    message: None,
                    expected_head: main_head.clone(),
                    expected_target_ref: "refs/heads/main".to_string(),
                }),
            )
            .await
        };

        let conflicted = start_squash(state.clone())
            .await
            .expect("squash conflict is a recoverable response")
            .0;
        let operation = conflicted.operation_state.expect("squash recovery state");
        assert!(conflicted.success);
        assert_eq!(operation.operation, "squash-merge");
        assert!(operation.in_progress);
        assert!(operation.can_continue);
        assert!(operation.can_abort);
        assert!(!operation.can_skip);
        assert!(operation
            .message
            .as_deref()
            .is_some_and(|message| message.contains("staged")));
        assert!(ensure_no_git_workflow_in_progress(&repository)
            .await
            .expect_err("squash recovery locks unrelated mutations")
            .body
            .error
            .contains("squash-merge"));

        std::fs::write(repository.join("conflict.txt"), "resolved\n").unwrap();
        git_test_command(&repository, ["add", "conflict.txt"]);
        let ready_to_finish = operation_state(
            State(state.clone()),
            Query(git_project_query(&repository)),
        )
        .await
        .expect("resolved squash stays visible until finish")
        .0;
        assert!(ready_to_finish.in_progress);
        let finished = continue_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("finish uncommitted squash recovery")
            .0;
        assert!(finished.success);
        assert!(!finished
            .operation_state
            .expect("finished squash state")
            .in_progress);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            main_head,
            "finish must leave an uncommitted squash at the original HEAD"
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("conflict.txt")).unwrap(),
            "resolved\n"
        );
        assert!(git_test_output(&repository, ["diff", "--cached", "--name-only"])
            .lines()
            .any(|path| path == "conflict.txt"));

        git_test_command(&repository, ["reset", "--hard", main_head.as_str()]);
        let conflicted_again = start_squash(state.clone())
            .await
            .expect("second squash conflict")
            .0;
        assert!(conflicted_again
            .operation_state
            .expect("second squash recovery state")
            .in_progress);
        let aborted = abort_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("abort squash recovery")
            .0;
        assert!(aborted.success);
        assert!(!aborted
            .operation_state
            .expect("aborted squash state")
            .in_progress);
        assert_eq!(
            std::fs::read_to_string(repository.join("conflict.txt")).unwrap(),
            "main\n",
            "abort restores the precise pre-squash worktree"
        );
        assert!(git_test_output(&repository, ["status", "--porcelain"])
            .trim()
            .is_empty());

        let committed_conflict = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "feature".to_string(),
                merge_mode: Some(MergeMode::Squash),
                no_fast_forward: false,
                commit_squash: true,
                message: Some("resolved squash merge".to_string()),
                expected_head: main_head.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("committed squash conflict is recoverable")
        .0;
        assert!(committed_conflict
            .operation_state
            .expect("committed squash recovery state")
            .in_progress);
        std::fs::write(repository.join("conflict.txt"), "resolved and committed\n").unwrap();
        git_test_command(&repository, ["add", "conflict.txt"]);
        let committed = continue_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("finish committed squash recovery")
            .0;
        assert!(committed.success);
        assert!(!committed
            .operation_state
            .expect("completed committed squash state")
            .in_progress);
        assert_eq!(
            git_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "resolved squash merge"
        );
        assert_eq!(
            git_test_output(&repository, ["rev-list", "--parents", "-n", "1", "HEAD"])
                .split_whitespace()
                .count(),
            2,
            "finish creates one normal squash commit rather than a merge commit"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_workflows_reject_stale_heads_and_same_oid_branch_swaps() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-workflow-target-snapshot-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("base.txt"), "base\n").unwrap();
        // Keep an ignored-path collision in this fixture. Git checkout
        // defaults to overwriting ignored files, so the guarded branch
        // create/switch route must opt out explicitly.
        std::fs::write(repository.join(".gitignore"), "generated.txt\n").unwrap();
        git_test_command(&repository, ["add", "base.txt", ".gitignore"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let reviewed_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["branch", "release"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("feature.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["add", "feature.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature"]);
        let feature_commit = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["checkout", "release"]);

        let state = git_test_app_state(&root).await;
        let reviewed_main_ref = "refs/heads/main".to_string();

        let merge_error = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "feature".to_string(),
                merge_mode: None,
                no_fast_forward: false,
                commit_squash: false,
                message: None,
                expected_head: reviewed_head.clone(),
                expected_target_ref: reviewed_main_ref.clone(),
            }),
        )
        .await
        .expect_err("merge prepared on main must not run after a same-OID release checkout");
        assert_eq!(merge_error.status, StatusCode::CONFLICT);

        let rebase_error = rebase(
            State(state.clone()),
            Json(RebaseBody {
                project: git_project_body(&repository),
                upstream: "feature".to_string(),
                interactive: false,
                expected_head: reviewed_head.clone(),
                expected_target_ref: reviewed_main_ref.clone(),
            }),
        )
        .await
        .expect_err("rebase prepared on main must not run after a same-OID release checkout");
        assert_eq!(rebase_error.status, StatusCode::CONFLICT);

        let revert_error = revert_commit(
            State(state.clone()),
            Json(RevertBody {
                project: git_project_body(&repository),
                commit: None,
                commits: vec![feature_commit],
                mainline: None,
                expected_head: reviewed_head.clone(),
                expected_target_ref: reviewed_main_ref.clone(),
            }),
        )
        .await
        .expect_err("revert prepared on main must not run after a same-OID release checkout");
        assert_eq!(revert_error.status, StatusCode::CONFLICT);

        let reset_error = reset(
            State(state.clone()),
            Json(ResetBody {
                project: git_project_body(&repository),
                target: "HEAD".to_string(),
                mode: "mixed".to_string(),
                confirm_hard: false,
                expected_head: reviewed_head.clone(),
                expected_target_ref: reviewed_main_ref.clone(),
            }),
        )
        .await
        .expect_err("reset prepared on main must not run after a same-OID release checkout");
        assert_eq!(reset_error.status, StatusCode::CONFLICT);
        assert_eq!(
            git_test_output(&repository, ["symbolic-ref", "--short", "HEAD"]).trim(),
            "release"
        );
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            reviewed_head
        );

        git_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("main.txt"), "newer main\n").unwrap();
        git_test_command(&repository, ["add", "main.txt"]);
        git_test_command(&repository, ["commit", "-m", "newer main"]);
        let stale_head_error = reset(
            State(state.clone()),
            Json(ResetBody {
                project: git_project_body(&repository),
                target: "HEAD~1".to_string(),
                mode: "mixed".to_string(),
                confirm_hard: false,
                expected_head: reviewed_head,
                expected_target_ref: reviewed_main_ref,
            }),
        )
        .await
        .expect_err("a reset form must fail after its reviewed HEAD moves");
        assert_eq!(stale_head_error.status, StatusCode::CONFLICT);
        assert_eq!(
            git_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "newer main"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn detached_checkout_and_bisect_reject_same_oid_branch_swaps() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-detached-bisect-target-snapshot-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("history.txt"), "good\n").unwrap();
        git_test_command(&repository, ["add", "history.txt"]);
        git_test_command(&repository, ["commit", "-m", "known good"]);
        let good = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        std::fs::write(repository.join("history.txt"), "bad\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "known bad"]);
        let bad = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["branch", "release"]);

        let state = git_test_app_state(&root).await;
        let scope = || AdvancedGitProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };

        // `main` and `release` point at the exact same commit. A HEAD-only
        // check would accept these delayed confirmations after the checkout.
        git_test_command(&repository, ["checkout", "release"]);
        let detached = advanced_detached_checkout(
            State(state.clone()),
            Json(AdvancedDetachedCheckoutBody {
                scope: scope(),
                reference: good.clone(),
                expected_head: bad.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
                confirm: true,
            }),
        )
        .await
        .expect_err("detached checkout prepared on main rejects a release checkout at the same OID");
        assert_eq!(detached.status, StatusCode::CONFLICT);
        assert_eq!(current_branch(&repository).await.unwrap(), "release");
        assert_eq!(git_test_output(&repository, ["rev-parse", "HEAD"]).trim(), bad);

        let bisect = advanced_git_bisect_start(
            State(state.clone()),
            Json(AdvancedBisectStartBody {
                scope: scope(),
                bad: bad.clone(),
                good: vec![good],
                expected_head: bad,
                expected_target_ref: "refs/heads/main".to_string(),
                confirm: true,
            }),
        )
        .await
        .expect_err("bisect prepared on main rejects a release checkout at the same OID");
        assert_eq!(bisect.status, StatusCode::CONFLICT);
        assert!(!advanced_bisect_in_progress(&repository).await.unwrap());
        assert_eq!(current_branch(&repository).await.unwrap(), "release");

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn create_and_checkout_branch_pins_the_reviewed_target_and_handles_worktree_safely() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-create-and-checkout-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("base.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "base.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let main_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["branch", "release"]);
        git_test_command(&repository, ["checkout", "-b", "source"]);
        std::fs::write(repository.join("source.txt"), "source\n").unwrap();
        git_test_command(&repository, ["add", "source.txt"]);
        git_test_command(&repository, ["commit", "-m", "source"]);
        std::fs::write(repository.join("generated.txt"), "source tracked output\n").unwrap();
        git_test_command(&repository, ["add", "-f", "generated.txt"]);
        git_test_command(&repository, ["commit", "-m", "track generated output"]);
        git_test_command(&repository, ["checkout", "main"]);

        let state = git_test_app_state(&root).await;
        let request = |branch: &str, start_point: Option<&str>| CreateAndCheckoutBranchBody {
            project: git_project_body(&repository),
            branch: branch.to_string(),
            start_point: start_point.map(str::to_string),
            expected_head: Some(main_head.clone()),
            expected_target_ref: Some("refs/heads/main".to_string()),
            confirm: true,
        };

        // An identical commit object is not enough: a form prepared on main
        // must not create/switch a branch after the user moves to release.
        git_test_command(&repository, ["checkout", "release"]);
        let wrong_target = create_and_checkout_branch(
            State(state.clone()),
            Json(request("wrong-target", None)),
        )
        .await
        .expect_err("same-OID checkout must invalidate the reviewed branch target");
        assert_eq!(wrong_target.status, StatusCode::CONFLICT);
        assert!(git(&repository, ["show-ref", "--verify", "refs/heads/wrong-target"])
            .await
            .is_err());
        git_test_command(&repository, ["checkout", "main"]);

        // Switching to a branch at another ref can rewrite files, so reject
        // it before Git makes a partial checkout when the tree is dirty.
        std::fs::write(repository.join("local-draft.txt"), "draft\n").unwrap();
        let dirty = create_and_checkout_branch(
            State(state.clone()),
            Json(request("from-source", Some("source"))),
        )
        .await
        .expect_err("different-ref checkout needs a clean worktree");
        assert_eq!(dirty.status, StatusCode::CONFLICT);
        assert!(git(&repository, ["show-ref", "--verify", "refs/heads/from-source"])
            .await
            .is_err());
        std::fs::remove_file(repository.join("local-draft.txt")).unwrap();

        // Ignored output is not ordinary Git dirt, but it must still never
        // be overwritten while checking out a source branch that tracks the
        // same path. This is the `--no-overwrite-ignore` guard above.
        std::fs::write(repository.join("generated.txt"), "local ignored output\n").unwrap();
        let _ignored_collision = create_and_checkout_branch(
            State(state.clone()),
            Json(request("from-source", Some("source"))),
        )
        .await
        .expect_err("ignored local output must not be overwritten by branch checkout");
        assert!(git(&repository, ["show-ref", "--verify", "refs/heads/from-source"])
            .await
            .is_err());
        assert_eq!(
            std::fs::read_to_string(repository.join("generated.txt")).unwrap(),
            "local ignored output\n"
        );
        std::fs::remove_file(repository.join("generated.txt")).unwrap();

        let created = create_and_checkout_branch(
            State(state.clone()),
            Json(request("from-source", Some("source"))),
        )
        .await
        .expect("create and switch from the reviewed source ref")
        .0;
        assert_eq!(created.branch.as_deref(), Some("from-source"));
        assert_eq!(
            git_test_output(&repository, ["symbolic-ref", "--short", "HEAD"]).trim(),
            "from-source"
        );
        assert_eq!(std::fs::read_to_string(repository.join("source.txt")).unwrap(), "source\n");

        // The common new-branch path stays at the current commit, so it may
        // retain ordinary drafts exactly as native Git and LazyGit do.
        let source_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        std::fs::write(repository.join("local-draft.txt"), "keep me\n").unwrap();
        let current_ref = CreateAndCheckoutBranchBody {
            project: git_project_body(&repository),
            branch: "topic/current".to_string(),
            start_point: None,
            expected_head: Some(source_head),
            expected_target_ref: Some("refs/heads/from-source".to_string()),
            confirm: true,
        };
        let _ = create_and_checkout_branch(State(state.clone()), Json(current_ref))
            .await
            .expect("create and switch at the current HEAD despite ordinary drafts");
        assert_eq!(
            git_test_output(&repository, ["symbolic-ref", "--short", "HEAD"]).trim(),
            "topic/current"
        );
        assert_eq!(std::fs::read_to_string(repository.join("local-draft.txt")).unwrap(), "keep me\n");

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn active_recovery_blocks_unrelated_mutations_but_keeps_resolution_actions_available() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-recovery-mutation-guards-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("conflict.txt"), "base\n").unwrap();
        std::fs::write(repository.join("support.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "."]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("conflict.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "feature change"]);
        git_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("conflict.txt"), "main\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "main change"]);

        let state = git_test_app_state(&root).await;
        let merge_response = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "feature".to_string(),
                merge_mode: None,
                no_fast_forward: true,
                commit_squash: false,
                message: None,
                expected_head: git_test_output(&repository, ["rev-parse", "HEAD"])
                    .trim()
                    .to_string(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("merge conflict is recoverable")
        .0;
        assert_eq!(
            merge_response
                .operation_state
                .as_ref()
                .expect("merge operation state")
                .operation,
            "merge"
        );

        macro_rules! assert_recovery_blocked {
            ($label:literal, $request:expr) => {{
                let error = ($request).await.expect_err($label);
                assert_eq!(
                    error.status,
                    axum::http::StatusCode::CONFLICT,
                    "{label} must leave the active recovery intact",
                    label = $label,
                );
            }};
        }

        let management_scope = || ManagementProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };
        let advanced_scope = || AdvancedGitProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };

        // Normal branch/tag/history and working-tree actions must not disturb
        // an active merge. They all fail with the same recoverable 409 rather
        // than relying on Git's inconsistent command-specific failures.
        assert_recovery_blocked!(
            "initial commit",
            initial_commit(State(state.clone()), Json(git_project_body(&repository)))
        );
        assert_recovery_blocked!(
            "checkout",
            checkout(
                State(state.clone()),
                Json(BranchBody {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                    branch: "feature".to_string(),
                })
            )
        );
        assert_recovery_blocked!(
            "create branch",
            create_branch(
                State(state.clone()),
                Json(BranchBody {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                    branch: "blocked-topic".to_string(),
                })
            )
        );
        assert_recovery_blocked!(
            "delete branch",
            delete_branch(
                State(state.clone()),
                Json(BranchBody {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                    branch: "feature".to_string(),
                })
            )
        );
        assert_recovery_blocked!(
            "create tag",
            create_tag(
                State(state.clone()),
                Json(TagBody {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                    tag: "blocked-tag".to_string(),
                    message: None,
                    force: false,
                    confirm: false,
                })
            )
        );
        assert_recovery_blocked!(
            "delete tag",
            delete_tag(
                State(state.clone()),
                Json(TagBody {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                    tag: "blocked-tag".to_string(),
                    message: None,
                    force: false,
                    confirm: false,
                })
            )
        );
        assert_recovery_blocked!(
            "discard changes",
            discard(
                State(state.clone()),
                Json(FileBody {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                    file: "conflict.txt".to_string(),
                })
            )
        );
        assert_recovery_blocked!(
            "delete untracked",
            delete_untracked(
                State(state.clone()),
                Json(FileBody {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                    file: "scratch.txt".to_string(),
                })
            )
        );

        // Every management mutation uses the same guard, including operations
        // that only update refs, configuration, submodule state, or linked
        // worktree records.
        assert_recovery_blocked!(
            "rename branch",
            management_rename_branch(
                State(state.clone()),
                Json(ManagementBranchRenameBody {
                    scope: management_scope(),
                    old_branch: "main".to_string(),
                    new_branch: "renamed-main".to_string(),
                })
            )
        );
        assert_recovery_blocked!(
            "create branch from ref",
            management_create_branch_from_ref(
                State(state.clone()),
                Json(ManagementBranchCreateFromRefBody {
                    scope: management_scope(),
                    branch: "blocked-ref".to_string(),
                    start_point: "HEAD".to_string(),
                })
            )
        );
        assert_recovery_blocked!(
            "create tag at ref",
            management_create_tag_at_target(
                State(state.clone()),
                Json(ManagementTagCreateBody {
                    scope: management_scope(),
                    tag: "blocked-managed-tag".to_string(),
                    message: None,
                    target: None,
                    force: false,
                    confirm: false,
                })
            )
        );
        assert_recovery_blocked!(
            "restore file from history",
            management_restore_file(
                State(state.clone()),
                Json(ManagementRestoreFileBody {
                    scope: management_scope(),
                    file: "conflict.txt".to_string(),
                    source: None,
                    stage: false,
                    confirm: true,
                })
            )
        );
        assert_recovery_blocked!(
            "update submodule",
            management_update_submodule(
                State(state.clone()),
                Json(ManagementSubmoduleUpdateBody {
                    scope: management_scope(),
                    path: None,
                    init: true,
                    recursive: false,
                    remote: false,
                    confirm: true,
                })
            )
        );
        assert_recovery_blocked!(
            "sync submodule",
            management_sync_submodule(
                State(state.clone()),
                Json(ManagementSubmoduleSyncBody {
                    scope: management_scope(),
                    path: None,
                    recursive: false,
                })
            )
        );
        let blocked_worktree = root.join("blocked-worktree");
        assert_recovery_blocked!(
            "add worktree",
            management_add_worktree(
                State(state.clone()),
                Json(ManagementWorktreeAddBody {
                    scope: management_scope(),
                    path: blocked_worktree.display().to_string(),
                    branch: Some("blocked-worktree".to_string()),
                    start_point: None,
                    create_branch: true,
                    detach: false,
                    force: false,
                    confirm: true,
                })
            )
        );
        assert_recovery_blocked!(
            "remove worktree",
            management_remove_worktree(
                State(state.clone()),
                Json(ManagementWorktreeRemoveBody {
                    scope: management_scope(),
                    path: blocked_worktree.display().to_string(),
                    force: false,
                    confirm: true,
                })
            )
        );
        assert_recovery_blocked!(
            "prune worktrees",
            management_prune_worktrees(
                State(state.clone()),
                Json(ManagementWorktreePruneBody {
                    scope: management_scope(),
                    confirm: true,
                })
            )
        );
        assert_recovery_blocked!(
            "mark bisect",
            advanced_git_bisect_good(
                State(state.clone()),
                Json(AdvancedBisectMarkBody {
                    scope: advanced_scope(),
                    expected_head: "HEAD".to_string(),
                    confirm: true,
                })
            )
        );
        assert_recovery_blocked!(
            "reset bisect",
            advanced_git_bisect_reset(
                State(state.clone()),
                Json(AdvancedBisectResetBody {
                    scope: advanced_scope(),
                    confirm: true,
                })
            )
        );

        // Resolution is deliberately the exception, but an active operation
        // owns the index. An unrelated file must never be staged, unstaged,
        // or patched into the final merge commit.
        std::fs::write(repository.join("support.txt"), "pending\n").unwrap();
        assert_recovery_blocked!(
            "stage unrelated file",
            stage(
                State(state.clone()),
                Json(FileBody {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                    file: "support.txt".to_string(),
                })
            )
        );
        assert_recovery_blocked!(
            "unstage during recovery",
            unstage(
                State(state.clone()),
                Json(FileBody {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                    file: "conflict.txt".to_string(),
                })
            )
        );
        assert_recovery_blocked!(
            "apply hunks during recovery",
            apply_hunks(
                State(state.clone()),
                Json(HunkBody {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                    file: "support.txt".to_string(),
                    operation: "stage".to_string(),
                    hunk_indexes: vec![0],
                })
            )
        );
        let _ = resolve_conflict(
            State(state.clone()),
            Json(ResolveConflictBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                file: "conflict.txt".to_string(),
                resolution: "manual".to_string(),
                content: Some("resolved\n".to_string()),
                stage: true,
            }),
        )
        .await
        .expect("conflict resolution remains available during recovery");
        assert!(!git_test_output(&repository, ["status", "--porcelain"])
            .lines()
            .any(|line| line.starts_with("UU conflict.txt")));
        let completed = continue_operation(
            State(state.clone()),
            Json(git_project_body(&repository)),
        )
        .await
        .expect("continue the resolved merge")
        .0;
        assert!(
            !completed
                .operation_state
                .expect("completed merge operation state")
                .in_progress
        );
        assert!(
            !git_test_output(&repository, ["show", "--format=", "--name-only", "HEAD"])
                .lines()
                .any(|path| path == "support.txt"),
            "an unrelated recovery-time edit must not enter the merge commit"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn active_bisect_is_visible_and_locks_normal_git_mutations() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-bisect-operation-state-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("history.txt"), "good\n").unwrap();
        git_test_command(&repository, ["add", "history.txt"]);
        git_test_command(&repository, ["commit", "-m", "known good"]);
        let good = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        std::fs::write(repository.join("history.txt"), "unknown\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "unknown revision"]);
        std::fs::write(repository.join("history.txt"), "bad\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "known bad"]);
        let bad = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let scope = AdvancedGitProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };
        let started = advanced_git_bisect_start(
            State(state.clone()),
            Json(AdvancedBisectStartBody {
                scope,
                bad: bad.clone(),
                good: vec![good],
                expected_head: bad,
                expected_target_ref: "refs/heads/main".to_string(),
                confirm: true,
            }),
        )
        .await
        .expect("start bisect")
        .0;
        assert!(started.in_progress);

        let operation = operation_state(
            State(state.clone()),
            Query(git_project_query(&repository)),
        )
        .await
        .expect("read bisect operation state")
        .0;
        assert_eq!(operation.operation, "bisect");
        assert!(operation.in_progress);
        assert!(!operation.can_continue);
        assert!(!operation.can_skip);
        assert!(!operation.can_abort);

        let checkout_error = checkout(
            State(state.clone()),
            Json(BranchBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                branch: "main".to_string(),
            }),
        )
        .await
        .expect_err("checkout must not run while bisect is active");
        assert_eq!(checkout_error.status, axum::http::StatusCode::CONFLICT);

        let reset = advanced_git_bisect_reset(
            State(state.clone()),
            Json(AdvancedBisectResetBody {
                scope: AdvancedGitProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                confirm: true,
            }),
        )
        .await
        .expect("reset active bisect")
        .0;
        assert!(!reset.in_progress);
        let completed = operation_state(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("read completed bisect state")
            .0;
        assert!(!completed.in_progress);

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn divergent_pull_starts_a_recoverable_merge_without_global_git_configuration() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-pull-workflow-{}",
            uuid::Uuid::new_v4()
        ));
        let remote = root.join("remote.git");
        let primary = root.join("primary");
        let replica = root.join("replica");
        let remote_path = remote.display().to_string();
        let primary_path = primary.display().to_string();
        let replica_path = replica.display().to_string();

        std::fs::create_dir_all(&root).unwrap();
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                remote_path.as_str(),
            ],
        );
        git_test_command(
            &root,
            ["init", "--initial-branch=main", primary_path.as_str()],
        );
        configure_test_repository(&primary);
        std::fs::write(primary.join("conflict.txt"), "base\n").unwrap();
        git_test_command(&primary, ["add", "conflict.txt"]);
        git_test_command(&primary, ["commit", "-m", "base"]);
        git_test_command(&primary, ["remote", "add", "origin", remote_path.as_str()]);
        git_test_command(&primary, ["push", "-u", "origin", "main"]);

        git_test_command(&root, ["clone", remote_path.as_str(), replica_path.as_str()]);
        configure_test_repository(&replica);
        std::fs::write(replica.join("conflict.txt"), "remote\n").unwrap();
        git_test_command(&replica, ["commit", "-am", "remote change"]);
        git_test_command(&replica, ["push"]);

        std::fs::write(primary.join("conflict.txt"), "local\n").unwrap();
        git_test_command(&primary, ["commit", "-am", "local change"]);

        let state = git_test_app_state(&root).await;
        let response = pull(State(state.clone()), Json(git_project_body(&primary)))
            .await
            .expect("divergent pull enters a recoverable merge")
            .0;
        assert!(response.success);
        assert_eq!(response.remote_name.as_deref(), Some("origin"));
        assert_eq!(response.remote_branch.as_deref(), Some("main"));
        let operation = response.operation_state.expect("merge operation state");
        assert_eq!(operation.operation, "merge");
        assert!(operation.in_progress);
        assert!(operation.can_continue);
        assert!(operation.can_abort);
        assert!(!operation.can_skip);
        assert!(git_test_output(&primary, ["status", "--porcelain"])
            .lines()
            .any(|line| line.starts_with("UU conflict.txt")));

        let aborted = abort_operation(State(state.clone()), Json(git_project_body(&primary)))
            .await
            .expect("abort pull merge")
            .0;
        assert!(aborted.success);
        assert!(
            !aborted
                .operation_state
                .expect("completed operation state")
                .in_progress
        );
        assert_eq!(
            std::fs::read_to_string(primary.join("conflict.txt")).unwrap(),
            "local\n"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn interactive_pull_matches_lazygit_default_todo_for_request_and_local_config() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-interactive-pull-default-todo-{}",
            uuid::Uuid::new_v4()
        ));
        let remote = root.join("remote.git");
        let primary = root.join("primary");
        let replica = root.join("replica");
        let remote_path = remote.display().to_string();
        let primary_path = primary.display().to_string();
        let replica_path = replica.display().to_string();

        std::fs::create_dir_all(&root).unwrap();
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                remote_path.as_str(),
            ],
        );
        git_test_command(
            &root,
            ["init", "--initial-branch=main", primary_path.as_str()],
        );
        configure_test_repository(&primary);
        std::fs::write(primary.join("README.md"), "base\n").unwrap();
        git_test_command(&primary, ["add", "README.md"]);
        git_test_command(&primary, ["commit", "-m", "base"]);
        git_test_command(&primary, ["remote", "add", "origin", remote_path.as_str()]);
        git_test_command(&primary, ["push", "-u", "origin", "main"]);

        git_test_command(&root, ["clone", remote_path.as_str(), replica_path.as_str()]);
        configure_test_repository(&replica);
        std::fs::write(replica.join("remote.txt"), "first remote commit\n").unwrap();
        git_test_command(&replica, ["add", "remote.txt"]);
        git_test_command(&replica, ["commit", "-m", "remote first"]);
        git_test_command(&replica, ["push"]);
        let remote_first = git_test_output(&replica, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        std::fs::write(primary.join("local.txt"), "first local commit\n").unwrap();
        git_test_command(&primary, ["add", "local.txt"]);
        git_test_command(&primary, ["commit", "-m", "local first"]);

        let state = git_test_app_state(&root).await;
        let scope = || ManagementProjectScope {
            project: Some(primary.display().to_string()),
            project_path: None,
            repository_id: None,
        };
        let direct = management_pull_remote(
            State(state.clone()),
            Json(ManagementRemotePullBody {
                scope: scope(),
                remote: "origin".to_string(),
                remote_branch: "main".to_string(),
                rebase: Some("interactive".to_string()),
            }),
        )
        .await
        .expect("direct interactive pull must use LazyGit's default todo behavior")
        .0;
        assert!(direct.success);
        assert!(direct.message.as_deref().is_some_and(|message| message.contains("interactive rebase (default plan)")));
        assert!(
            git_test_output(&primary, ["merge-base", "--is-ancestor", remote_first.as_str(), "HEAD"])
                .is_empty(),
            "the fetched remote commit must become an ancestor after rebase"
        );
        assert_eq!(
            git_test_output(&primary, ["log", "-1", "--format=%s"]).trim(),
            "local first"
        );

        // Match the persisted `pull.rebase=interactive` case LazyGit reads
        // from Git config rather than a one-shot menu choice.
        git_test_command(&replica, ["pull"]);
        std::fs::write(replica.join("remote-two.txt"), "second remote commit\n").unwrap();
        git_test_command(&replica, ["add", "remote-two.txt"]);
        git_test_command(&replica, ["commit", "-m", "remote second"]);
        git_test_command(&replica, ["push"]);
        let remote_second = git_test_output(&replica, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        std::fs::write(primary.join("local-two.txt"), "second local commit\n").unwrap();
        git_test_command(&primary, ["add", "local-two.txt"]);
        git_test_command(&primary, ["commit", "-m", "local second"]);
        git_test_command(&primary, ["config", "--local", "pull.rebase", "interactive"]);
        let configured = pull(State(state.clone()), Json(git_project_body(&primary)))
        .await
        .expect("configured interactive pull must mirror LazyGit's default todo behavior")
        .0;
        assert!(configured.success);
        assert!(configured.message.as_deref().is_some_and(|message| message.contains("interactive rebase (default plan)")));
        assert!(
            git_test_output(&primary, ["merge-base", "--is-ancestor", remote_second.as_str(), "HEAD"])
                .is_empty(),
            "the newly fetched remote commit must become an ancestor after configured rebase"
        );
        assert_eq!(
            git_test_output(&primary, ["log", "-1", "--format=%s"]).trim(),
            "local second"
        );
        assert!(git_test_output(&primary, ["status", "--porcelain"]).trim().is_empty());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn local_branch_rebase_and_pull_ff_settings_match_lazygit_pull_semantics() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-local-pull-config-{}",
            uuid::Uuid::new_v4()
        ));
        let remote = root.join("remote.git");
        let primary = root.join("primary");
        let replica = root.join("replica");
        let fast_forward_client = root.join("fast-forward-client");
        let ff_only_client = root.join("ff-only-client");
        let remote_path = remote.display().to_string();
        let primary_path = primary.display().to_string();
        let replica_path = replica.display().to_string();
        let fast_forward_client_path = fast_forward_client.display().to_string();
        let ff_only_client_path = ff_only_client.display().to_string();

        std::fs::create_dir_all(&root).unwrap();
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                remote_path.as_str(),
            ],
        );
        git_test_command(
            &root,
            ["init", "--initial-branch=main", primary_path.as_str()],
        );
        configure_test_repository(&primary);
        std::fs::write(primary.join("README.md"), "base\n").unwrap();
        git_test_command(&primary, ["add", "README.md"]);
        git_test_command(&primary, ["commit", "-m", "base"]);
        git_test_command(&primary, ["remote", "add", "origin", remote_path.as_str()]);
        git_test_command(&primary, ["push", "-u", "origin", "main"]);

        git_test_command(&root, ["clone", remote_path.as_str(), replica_path.as_str()]);
        configure_test_repository(&replica);
        std::fs::write(replica.join("remote-one.txt"), "one\n").unwrap();
        git_test_command(&replica, ["add", "remote-one.txt"]);
        git_test_command(&replica, ["commit", "-m", "remote one"]);
        git_test_command(&replica, ["push"]);
        let remote_one = git_test_output(&replica, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        std::fs::write(primary.join("local-one.txt"), "one\n").unwrap();
        git_test_command(&primary, ["add", "local-one.txt"]);
        git_test_command(&primary, ["commit", "-m", "local one"]);
        git_test_command(&primary, ["config", "--local", "pull.rebase", "false"]);
        // Git/LazyGit gives this branch-local preference precedence over the
        // repository-wide pull.rebase value above.
        git_test_command(&primary, ["config", "--local", "branch.main.rebase", "true"]);

        let state = git_test_app_state(&root).await;
        let rebased = pull(State(state.clone()), Json(git_project_body(&primary)))
            .await
            .expect("branch.main.rebase must override pull.rebase")
            .0;
        assert!(rebased.success);
        assert!(rebased.message.as_deref().is_some_and(|message| message.contains("using rebase")));
        assert!(git_test_output(
            &primary,
            ["merge-base", "--is-ancestor", remote_one.as_str(), "HEAD"],
        )
        .is_empty());
        assert_eq!(
            git_test_output(&primary, ["log", "-1", "--format=%s"]).trim(),
            "local one",
            "the local commit was replayed rather than wrapped in a merge commit"
        );

        // pull.ff=false must override Git's normal fast-forward behavior and
        // force a merge commit for an otherwise fast-forwardable update.
        git_test_command(
            &root,
            ["clone", remote_path.as_str(), fast_forward_client_path.as_str()],
        );
        configure_test_repository(&fast_forward_client);
        std::fs::write(replica.join("remote-two.txt"), "two\n").unwrap();
        git_test_command(&replica, ["add", "remote-two.txt"]);
        git_test_command(&replica, ["commit", "-m", "remote two"]);
        git_test_command(&replica, ["push"]);
        let remote_two = git_test_output(&replica, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(
            &fast_forward_client,
            ["config", "--local", "pull.rebase", "false"],
        );
        git_test_command(
            &fast_forward_client,
            ["config", "--local", "pull.ff", "false"],
        );
        let no_fast_forward_pull = pull(
            State(state.clone()),
            Json(git_project_body(&fast_forward_client)),
        )
        .await
        .expect("pull.ff=false must create a merge commit")
        .0;
        assert!(no_fast_forward_pull.success);
        assert_eq!(
            git_test_output(&fast_forward_client, ["rev-list", "--parents", "-n", "1", "HEAD"])
                .split_whitespace()
                .count(),
            3,
            "pull.ff=false overrides the default fast-forward"
        );
        assert!(git_test_output(
            &fast_forward_client,
            ["merge-base", "--is-ancestor", remote_two.as_str(), "HEAD"],
        )
        .is_empty());

        // pull.ff=only must reject a divergent pull without manufacturing an
        // ordinary merge, just as LazyGit's underlying `git pull` does.
        git_test_command(
            &root,
            ["clone", remote_path.as_str(), ff_only_client_path.as_str()],
        );
        configure_test_repository(&ff_only_client);
        std::fs::write(ff_only_client.join("local-only.txt"), "local\n").unwrap();
        git_test_command(&ff_only_client, ["add", "local-only.txt"]);
        git_test_command(&ff_only_client, ["commit", "-m", "local only"]);
        let ff_only_head = git_test_output(&ff_only_client, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        std::fs::write(replica.join("remote-three.txt"), "three\n").unwrap();
        git_test_command(&replica, ["add", "remote-three.txt"]);
        git_test_command(&replica, ["commit", "-m", "remote three"]);
        git_test_command(&replica, ["push"]);
        let remote_three = git_test_output(&replica, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&ff_only_client, ["config", "--local", "pull.rebase", "false"]);
        git_test_command(&ff_only_client, ["config", "--local", "pull.ff", "only"]);
        let ff_only_error = pull(State(state.clone()), Json(git_project_body(&ff_only_client)))
            .await
            .expect_err("pull.ff=only must reject a divergent pull");
        assert_eq!(ff_only_error.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            git_test_output(&ff_only_client, ["rev-parse", "HEAD"]).trim(),
            ff_only_head,
            "the rejected pull.ff=only operation must leave HEAD untouched"
        );
        assert_eq!(
            git_test_output(&ff_only_client, ["rev-parse", "FETCH_HEAD"]).trim(),
            remote_three,
            "the selected remote branch was still fetched before the safe integration rejection"
        );
        assert!(git_test_output(&ff_only_client, ["status", "--porcelain"])
            .trim()
            .is_empty());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn local_tracking_pull_matches_lazygit_dot_remote_semantics() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-local-tracking-pull-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let base_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["checkout", "-b", "source"]);
        std::fs::write(repository.join("source.txt"), "local source\n").unwrap();
        git_test_command(&repository, ["add", "source.txt"]);
        git_test_command(&repository, ["commit", "-m", "source change"]);
        let source_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["checkout", "main"]);
        git_test_command(&repository, ["config", "--local", "branch.main.remote", "."]);
        git_test_command(
            &repository,
            ["config", "--local", "branch.main.merge", "refs/heads/source"],
        );

        let state = git_test_app_state(&root).await;
        let status = remote_status(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("local tracking status")
            .0;
        assert!(!status.has_remote, "a local source is not a network remote");
        assert!(status.has_upstream);
        assert_eq!(status.remote_name.as_deref(), Some("."));
        assert_eq!(status.remote_branch.as_deref(), Some("source"));

        let scope = || ManagementProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };
        let wrong_source = management_pull_remote(
            State(state.clone()),
            Json(ManagementRemotePullBody {
                scope: scope(),
                remote: ".".to_string(),
                remote_branch: "main".to_string(),
                rebase: None,
            }),
        )
        .await
        .expect_err("the local Pull form must stay pinned to its configured upstream");
        assert_eq!(wrong_source.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            base_head,
            "a rejected local source must not move the checked-out branch"
        );

        let pulled = management_pull_remote(
            State(state.clone()),
            Json(ManagementRemotePullBody {
                scope: scope(),
                remote: ".".to_string(),
                remote_branch: "source".to_string(),
                rebase: None,
            }),
        )
        .await
        .expect("local tracking pull should run the same Git pull workflow as LazyGit")
        .0;
        assert!(pulled.success);
        assert_eq!(pulled.remote_name.as_deref(), Some("."));
        assert_eq!(pulled.remote_branch.as_deref(), Some("source"));
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            source_head
        );
        assert!(git_test_output(&repository, ["status", "--porcelain"])
            .trim()
            .is_empty());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pull_without_upstream_requires_an_explicit_destination_like_lazygit() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-pull-without-upstream-{}",
            uuid::Uuid::new_v4()
        ));
        let remote = root.join("remote.git");
        let primary = root.join("primary");
        let replica = root.join("replica");
        let remote_path = remote.display().to_string();
        let primary_path = primary.display().to_string();
        let replica_path = replica.display().to_string();

        std::fs::create_dir_all(&root).unwrap();
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                remote_path.as_str(),
            ],
        );
        git_test_command(
            &root,
            ["init", "--initial-branch=main", primary_path.as_str()],
        );
        configure_test_repository(&primary);
        std::fs::write(primary.join("README.md"), "base\n").unwrap();
        git_test_command(&primary, ["add", "README.md"]);
        git_test_command(&primary, ["commit", "-m", "base"]);
        let primary_head = git_test_output(&primary, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&primary, ["remote", "add", "origin", remote_path.as_str()]);
        // Deliberately omit -u: origin/main exists, but this local branch is
        // not tracking it. The former fallback would have fetched and
        // integrated it despite the missing upstream.
        git_test_command(&primary, ["push", "origin", "main"]);

        git_test_command(&root, ["clone", remote_path.as_str(), replica_path.as_str()]);
        configure_test_repository(&replica);
        std::fs::write(replica.join("remote.txt"), "remote change\n").unwrap();
        git_test_command(&replica, ["add", "remote.txt"]);
        git_test_command(&replica, ["commit", "-m", "remote change"]);
        git_test_command(&replica, ["push"]);

        let state = git_test_app_state(&root).await;
        let error = pull(State(state.clone()), Json(git_project_body(&primary)))
            .await
            .expect_err("Pull without an upstream must not assume origin/current branch");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.body.error.contains("No tracking branch"));
        assert_eq!(
            git_test_output(&primary, ["rev-parse", "HEAD"]).trim(),
            primary_head,
            "rejecting the missing-upstream pull must leave the checked-out branch unchanged"
        );
        assert!(
            !primary.join(".git/FETCH_HEAD").exists(),
            "a missing upstream must be rejected before any implicit origin fetch"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_fetch_then_editable_rebase_plan_pins_the_selected_fetch_head() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-remote-rebase-plan-{}",
            uuid::Uuid::new_v4()
        ));
        let remote = root.join("remote.git");
        let primary = root.join("primary");
        let replica = root.join("replica");
        let remote_path = remote.display().to_string();
        let primary_path = primary.display().to_string();
        let replica_path = replica.display().to_string();

        std::fs::create_dir_all(&root).unwrap();
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                remote_path.as_str(),
            ],
        );
        git_test_command(
            &root,
            ["init", "--initial-branch=main", primary_path.as_str()],
        );
        configure_test_repository(&primary);
        std::fs::write(primary.join("README.md"), "base\n").unwrap();
        git_test_command(&primary, ["add", "README.md"]);
        git_test_command(&primary, ["commit", "-m", "base"]);
        let base_head = git_test_output(&primary, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&primary, ["remote", "add", "origin", remote_path.as_str()]);
        git_test_command(&primary, ["push", "-u", "origin", "main"]);

        git_test_command(&root, ["clone", remote_path.as_str(), replica_path.as_str()]);
        configure_test_repository(&replica);
        git_test_command(&replica, ["checkout", "-b", "release"]);
        std::fs::write(replica.join("release.txt"), "remote release\n").unwrap();
        git_test_command(&replica, ["add", "release.txt"]);
        git_test_command(&replica, ["commit", "-m", "release base"]);
        git_test_command(&replica, ["push", "-u", "origin", "release"]);
        let release_head = git_test_output(&replica, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        git_test_command(&primary, ["checkout", "-b", "feature/mobile"]);
        std::fs::write(primary.join("feature.txt"), "local feature\n").unwrap();
        git_test_command(&primary, ["add", "feature.txt"]);
        git_test_command(&primary, ["commit", "-m", "local feature"]);
        let feature_head = git_test_output(&primary, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let plan = management_remote_rebase_plan(
            State(state.clone()),
            Json(ManagementRemoteRebasePlanBody {
                scope: ManagementProjectScope {
                    project: Some(primary.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                remote: "origin".to_string(),
                remote_branch: "release".to_string(),
            }),
        )
        .await
        .expect("fetch selected remote branch and build a plan")
        .0;
        assert_eq!(plan.upstream, release_head);
        assert_eq!(plan.expected_head, feature_head);
        assert_eq!(plan.expected_target_ref, "refs/heads/feature/mobile");
        assert_eq!(plan.commits.len(), 1);
        assert_eq!(plan.commits[0].commit, feature_head);
        assert_eq!(
            git_test_output(&primary, ["rev-parse", "FETCH_HEAD"]).trim(),
            release_head
        );
        assert_eq!(current_branch(&primary).await.unwrap(), "feature/mobile");
        assert!(git_test_output(&primary, ["status", "--porcelain"]).trim().is_empty());

        // There is no todo to edit when the checked-out branch is simply
        // behind the fetched ref. Do not turn the request into a hidden
        // fast-forward; return guided no-plan state while leaving HEAD alone.
        git_test_command(&primary, ["checkout", "-B", "no-local-commits", base_head.as_str()]);
        let no_plan = management_remote_rebase_plan(
            State(state.clone()),
            Json(ManagementRemoteRebasePlanBody {
                scope: ManagementProjectScope {
                    project: Some(primary.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                remote: "origin".to_string(),
                remote_branch: "release".to_string(),
            }),
        )
        .await
        .expect_err("a branch with no local commits has no editable todo");
        assert_eq!(no_plan.status, StatusCode::BAD_REQUEST);
        assert!(no_plan.body.error.contains("Use direct Pull to fast-forward"));
        assert_eq!(git_test_output(&primary, ["rev-parse", "HEAD"]).trim(), base_head);
        assert_eq!(current_branch(&primary).await.unwrap(), "no-local-commits");

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn selected_remote_branch_operations_do_not_dwim_to_a_same_named_tag() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-selected-branch-not-tag-{}",
            uuid::Uuid::new_v4()
        ));
        let remote = root.join("remote.git");
        let primary = root.join("primary");
        let replica = root.join("replica");
        let remote_path = remote.display().to_string();
        let primary_path = primary.display().to_string();
        let replica_path = replica.display().to_string();

        std::fs::create_dir_all(&root).unwrap();
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                remote_path.as_str(),
            ],
        );
        git_test_command(
            &root,
            ["init", "--initial-branch=main", primary_path.as_str()],
        );
        configure_test_repository(&primary);
        std::fs::write(primary.join("README.md"), "base\n").unwrap();
        git_test_command(&primary, ["add", "README.md"]);
        git_test_command(&primary, ["commit", "-m", "base"]);
        git_test_command(&primary, ["remote", "add", "origin", remote_path.as_str()]);
        git_test_command(&primary, ["push", "-u", "origin", "main"]);

        git_test_command(&root, ["clone", remote_path.as_str(), replica_path.as_str()]);
        configure_test_repository(&replica);
        git_test_command(&replica, ["checkout", "-b", "topic"]);
        std::fs::write(replica.join("branch.txt"), "from branch\n").unwrap();
        git_test_command(&replica, ["add", "branch.txt"]);
        git_test_command(&replica, ["commit", "-m", "topic branch commit"]);
        let branch_head = git_test_output(&replica, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&replica, ["push", "-u", "origin", "topic"]);

        // Give a tag the exact same short name as the selected branch. A
        // bare `git fetch origin topic` DWIMs to this tag, whereas LazyGit
        // explicitly requests `refs/heads/topic` for its selected Pull.
        git_test_command(&replica, ["checkout", "main"]);
        std::fs::write(replica.join("tag.txt"), "from tag\n").unwrap();
        git_test_command(&replica, ["add", "tag.txt"]);
        git_test_command(&replica, ["commit", "-m", "tag target commit"]);
        git_test_command(&replica, ["push", "origin", "main"]);
        git_test_command(
            &replica,
            [
                "-c",
                "tag.gpgSign=false",
                "tag",
                "-a",
                "topic",
                "-m",
                "same named tag",
            ],
        );
        git_test_command(&replica, ["push", "origin", "refs/tags/topic"]);
        let tag_head = git_test_output(&replica, ["rev-parse", "refs/tags/topic^{}"])
            .trim()
            .to_string();
        assert_ne!(branch_head, tag_head);

        git_test_command(&primary, ["checkout", "-b", "feature/mobile"]);
        std::fs::write(primary.join("local.txt"), "local\n").unwrap();
        git_test_command(&primary, ["add", "local.txt"]);
        git_test_command(&primary, ["commit", "-m", "local feature commit"]);

        let state = git_test_app_state(&root).await;
        let scope = || ManagementProjectScope {
            project: Some(primary.display().to_string()),
            project_path: None,
            repository_id: None,
        };

        let plan = management_remote_rebase_plan(
            State(state.clone()),
            Json(ManagementRemoteRebasePlanBody {
                scope: scope(),
                remote: "origin".to_string(),
                remote_branch: "topic".to_string(),
            }),
        )
        .await
        .expect("plan selected remote branch")
        .0;
        assert_eq!(plan.upstream, branch_head);
        assert_eq!(
            git_test_output(&primary, ["rev-parse", "FETCH_HEAD"]).trim(),
            branch_head
        );

        let pulled = management_pull_remote(
            State(state.clone()),
            Json(ManagementRemotePullBody {
                scope: scope(),
                remote: "origin".to_string(),
                remote_branch: "topic".to_string(),
                rebase: Some("false".to_string()),
            }),
        )
        .await
        .expect("pull selected remote branch")
        .0;
        assert!(pulled.success);
        assert_eq!(
            git_test_output(&primary, ["rev-parse", "FETCH_HEAD"]).trim(),
            branch_head
        );
        assert!(git(&primary, ["cat-file", "-e", "HEAD:branch.txt"])
            .await
            .is_ok());
        assert!(git(&primary, ["cat-file", "-e", "HEAD:tag.txt"])
            .await
            .is_err());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn legacy_push_follows_git_simple_and_requires_an_explicit_target_for_a_differently_named_upstream() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-upstream-push-refspec-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        let remote = root.join("remote.git");
        let remote_path = remote.display().to_string();
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                remote_path.as_str(),
            ],
        );
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        git_test_command(&repository, ["remote", "add", "origin", remote_path.as_str()]);
        git_test_command(&repository, ["push", "-u", "origin", "main"]);

        git_test_command(&repository, ["checkout", "-b", "local-topic"]);
        std::fs::write(repository.join("topic.txt"), "first topic commit\n").unwrap();
        git_test_command(&repository, ["add", "topic.txt"]);
        git_test_command(&repository, ["commit", "-m", "topic first"]);
        git_test_command(
            &repository,
            [
                "push",
                "origin",
                "refs/heads/local-topic:refs/heads/review/topic",
            ],
        );
        git_test_command(&repository, ["fetch", "origin"]);
        git_test_command(
            &repository,
            [
                "branch",
                "--set-upstream-to=origin/review/topic",
                "local-topic",
            ],
        );
        std::fs::write(repository.join("topic.txt"), "second topic commit\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "topic second"]);
        let local_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let remote_head_before = git_test_output(&remote, ["rev-parse", "refs/heads/review/topic"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let rejected = push(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect_err("plain Git simple push must refuse a differently named upstream");
        assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
        assert!(rejected.body.error.contains("push.default=simple"));
        assert_eq!(
            git_test_output(&remote, ["rev-parse", "refs/heads/review/topic"]).trim(),
            remote_head_before,
            "an implicit simple push must leave the differently named upstream unchanged"
        );
        assert!(
            git(&remote, ["show-ref", "--verify", "--quiet", "refs/heads/local-topic"])
                .await
                .is_err(),
            "the legacy endpoint must not silently create a same-named remote branch"
        );
        assert_ne!(remote_head_before, local_head);

        git_test_command(&repository, ["checkout", "--detach", "HEAD"]);
        let detached = push(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect_err("legacy push must reject detached HEAD");
        assert_eq!(detached.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(
            git_test_output(&remote, ["rev-parse", "refs/heads/review/topic"]).trim(),
            remote_head_before
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn revert_and_reset_handlers_use_safe_git_semantics() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-revert-reset-workflow-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("value.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "value.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        std::fs::write(repository.join("value.txt"), "changed\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "change value"]);
        let changed_commit = git_test_output(&repository, ["rev-parse", "HEAD"]);

        let state = git_test_app_state(&root).await;
        let reverted = revert_commit(
            State(state.clone()),
            Json(RevertBody {
                project: git_project_body(&repository),
                commit: Some(changed_commit.trim().to_string()),
                commits: Vec::new(),
                mainline: None,
                expected_head: git_test_output(&repository, ["rev-parse", "HEAD"])
                    .trim()
                    .to_string(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("true revert succeeds")
        .0;
        assert!(reverted.success);
        assert_eq!(
            std::fs::read_to_string(repository.join("value.txt")).unwrap(),
            "base\n"
        );
        assert!(
            git_test_output(&repository, ["log", "-1", "--format=%s"])
                .trim()
                .starts_with("Revert \"change value\"")
        );

        std::fs::write(repository.join("value.txt"), "later\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "later value"]);
        let expected_head = git_test_output(&repository, ["rev-parse", "HEAD"]);
        let soft_reset = reset(
            State(state.clone()),
            Json(ResetBody {
                project: git_project_body(&repository),
                target: "HEAD~1".to_string(),
                mode: "soft".to_string(),
                confirm_hard: false,
                expected_head: expected_head.trim().to_string(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("soft reset succeeds")
        .0;
        assert!(soft_reset.success);
        assert!(
            git_test_output(&repository, ["diff", "--cached"])
                .contains("+later")
        );

        std::fs::write(repository.join("value.txt"), "dirty\n").unwrap();
        let denied = reset(
            State(state.clone()),
            Json(ResetBody {
                project: git_project_body(&repository),
                target: "HEAD".to_string(),
                mode: "hard".to_string(),
                confirm_hard: false,
                expected_head: git_test_output(&repository, ["rev-parse", "HEAD"])
                    .trim()
                    .to_string(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect_err("hard reset requires explicit confirmation");
        assert_eq!(denied.status, axum::http::StatusCode::BAD_REQUEST);
        let hard_reset = reset(
            State(state.clone()),
            Json(ResetBody {
                project: git_project_body(&repository),
                target: "HEAD".to_string(),
                mode: "hard".to_string(),
                confirm_hard: true,
                expected_head: git_test_output(&repository, ["rev-parse", "HEAD"])
                    .trim()
                    .to_string(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("confirmed hard reset succeeds")
        .0;
        assert!(hard_reset.success);
        assert_eq!(
            std::fs::read_to_string(repository.join("value.txt")).unwrap(),
            "base\n"
        );
        assert!(git_test_output(&repository, ["status", "--porcelain"])
            .trim()
            .is_empty());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn range_revert_preserves_visible_order_and_requires_a_clean_worktree() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-range-revert-order-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("base.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "base.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        std::fs::write(repository.join("older.txt"), "older\n").unwrap();
        git_test_command(&repository, ["add", "older.txt"]);
        git_test_command(&repository, ["commit", "-m", "older change"]);
        let older = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        std::fs::write(repository.join("newer.txt"), "newer\n").unwrap();
        git_test_command(&repository, ["add", "newer.txt"]);
        git_test_command(&repository, ["commit", "-m", "newer change"]);
        let newer = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let reverted = revert_commit(
            State(state.clone()),
            Json(RevertBody {
                project: git_project_body(&repository),
                commit: None,
                // The Commit tab renders `git log` newest first. Preserve
                // that exact selection order for Git's sequencer.
                commits: vec![newer, older],
                mainline: None,
                expected_head: git_test_output(&repository, ["rev-parse", "HEAD"])
                    .trim()
                    .to_string(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("ordered range revert succeeds")
        .0;
        assert!(reverted.success);
        assert!(!reverted
            .operation_state
            .expect("completed operation state")
            .in_progress);
        let subjects = git_test_output(&repository, ["log", "-2", "--format=%s"]);
        let subjects = subjects.lines().collect::<Vec<_>>();
        assert!(subjects[0].starts_with("Revert \"older change\""));
        assert!(subjects[1].starts_with("Revert \"newer change\""));
        assert!(!repository.join("older.txt").exists());
        assert!(!repository.join("newer.txt").exists());

        std::fs::write(repository.join("third.txt"), "third\n").unwrap();
        git_test_command(&repository, ["add", "third.txt"]);
        git_test_command(&repository, ["commit", "-m", "third change"]);
        let third = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        std::fs::write(repository.join("dirty.txt"), "do not stash me\n").unwrap();

        let denied = revert_commit(
            State(state.clone()),
            Json(RevertBody {
                project: git_project_body(&repository),
                commit: None,
                commits: vec![third],
                mainline: None,
                expected_head: git_test_output(&repository, ["rev-parse", "HEAD"])
                    .trim()
                    .to_string(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect_err("range revert must not hide a dirty worktree in an auto-stash");
        assert_eq!(denied.status, StatusCode::CONFLICT);
        assert_eq!(
            std::fs::read_to_string(repository.join("dirty.txt")).unwrap(),
            "do not stash me\n"
        );
        assert!(git_test_output(&repository, ["stash", "list"]).trim().is_empty());
        assert_eq!(
            git_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "third change"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_range_revert_keeps_a_sequencer_recovery_visible_and_locked() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-empty-range-revert-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("base.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "base.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        std::fs::write(repository.join("revert-me.txt"), "remove me\n").unwrap();
        git_test_command(&repository, ["add", "revert-me.txt"]);
        git_test_command(&repository, ["commit", "-m", "change to revert"]);
        let change = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["commit", "--allow-empty", "-m", "empty change"]);
        let empty = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let started = revert_commit(
            State(state.clone()),
            Json(RevertBody {
                project: git_project_body(&repository),
                commit: None,
                // Git stops on the first empty inverse patch and leaves only
                // `.git/sequencer/todo`; it does not write REVERT_HEAD.
                commits: vec![empty.clone(), change],
                mainline: None,
                expected_head: git_test_output(&repository, ["rev-parse", "HEAD"])
                    .trim()
                    .to_string(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("an empty range entry is a recoverable revert state")
        .0;

        assert!(started.success);
        let operation = started.operation_state.expect("sequencer revert state");
        assert_eq!(operation.operation, "revert");
        assert!(operation.in_progress);
        assert!(operation.can_continue, "Git directs this state to continue");
        assert!(operation.can_abort);
        assert!(operation.can_skip);
        let current_commit = operation.current_commit.as_deref().expect("sequencer commit");
        assert!(
            empty.starts_with(current_commit),
            "Git's sequencer may abbreviate the current commit, but it must identify the empty entry"
        );
        assert!(
            operation
                .message
                .as_deref()
                .is_some_and(|message| message.contains("no changes to apply")),
            "the recovery panel must explain that this is an empty step, not a conflict"
        );

        let blocked_checkout = checkout(
            State(state.clone()),
            Json(BranchBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                branch: "main".to_string(),
            }),
        )
        .await
        .expect_err("ordinary Git mutations stay locked during the sequencer-only state");
        assert_eq!(blocked_checkout.status, StatusCode::CONFLICT);

        // Git's own guidance for an empty entry in a multi-revert is
        // `git revert --continue`: it advances past the empty step and then
        // applies the remaining reviewed inverse commits.
        let continued = continue_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("continue advances the empty revert step")
            .0;
        assert!(continued.success);
        assert!(
            !continued
                .operation_state
                .expect("completed revert state")
                .in_progress
        );
        assert!(!repository.join("revert-me.txt").exists());
        assert!(git_test_output(&repository, ["status", "--porcelain"])
            .trim()
            .is_empty());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn range_revert_defaults_selected_merge_commits_to_the_first_parent() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-range-revert-merge-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("base.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "base.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("feature.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["add", "feature.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature change"]);
        git_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("main.txt"), "main\n").unwrap();
        git_test_command(&repository, ["add", "main.txt"]);
        git_test_command(&repository, ["commit", "-m", "main change"]);
        git_test_command(&repository, ["merge", "--no-ff", "feature", "-m", "merge feature"]);
        let merge_commit = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let reverted = revert_commit(
            State(state.clone()),
            Json(RevertBody {
                project: git_project_body(&repository),
                commit: None,
                commits: vec![merge_commit],
                // No caller-supplied parent: selected merges default to
                // parent 1 just like LazyGit's selected-range command.
                mainline: None,
                expected_head: git_test_output(&repository, ["rev-parse", "HEAD"])
                    .trim()
                    .to_string(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("merge range revert defaults to first parent")
        .0;
        assert!(reverted.success);
        assert!(repository.join("main.txt").exists());
        assert!(!repository.join("feature.txt").exists());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn range_revert_conflict_keeps_the_remaining_selection_recoverable() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-range-revert-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("value.txt"), "").unwrap();
        git_test_command(&repository, ["add", "value.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        std::fs::write(repository.join("other.txt"), "other\n").unwrap();
        git_test_command(&repository, ["add", "other.txt"]);
        git_test_command(&repository, ["commit", "-m", "unrelated change"]);
        let unrelated = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        std::fs::write(repository.join("value.txt"), "first\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "add first line"]);
        let first = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        std::fs::write(repository.join("value.txt"), "first\nsecond\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "add second line"]);

        let state = git_test_app_state(&root).await;
        let started = revert_commit(
            State(state.clone()),
            Json(RevertBody {
                project: git_project_body(&repository),
                commit: None,
                // This is the current-history (newest-to-oldest) range from
                // `add first line` down through `unrelated change`.
                commits: vec![first, unrelated],
                mainline: None,
                expected_head: git_test_output(&repository, ["rev-parse", "HEAD"])
                    .trim()
                    .to_string(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect("a range conflict is a recoverable response")
        .0;
        assert!(started.success);
        let operation = started.operation_state.expect("revert recovery state");
        assert_eq!(operation.operation, "revert");
        assert!(operation.in_progress);
        assert!(operation.can_continue);

        // Match the mobile resolver's outcome: choose a resolved file, stage
        // it, then let `git revert --continue` process the pending entry.
        std::fs::write(repository.join("value.txt"), "second\n").unwrap();
        git_test_command(&repository, ["add", "value.txt"]);
        let continued = continue_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("continue completes the queued inverse commits")
            .0;
        assert!(!continued
            .operation_state
            .expect("completed operation state")
            .in_progress);
        assert_eq!(
            std::fs::read_to_string(repository.join("value.txt")).unwrap(),
            "second\n"
        );
        assert!(!repository.join("other.txt").exists());
        let subjects = git_test_output(&repository, ["log", "-2", "--format=%s"]);
        let subjects = subjects.lines().collect::<Vec<_>>();
        assert!(subjects[0].starts_with("Revert \"unrelated change\""));
        assert!(subjects[1].starts_with("Revert \"add first line\""));

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn undo_latest_local_commit_requires_a_fresh_unshared_head() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-undo-local-commit-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("value.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "value.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let base_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        std::fs::write(repository.join("value.txt"), "local change\n").unwrap();
        git_test_command(&repository, ["add", "value.txt"]);
        git_test_command(&repository, ["commit", "-m", "local change"]);
        let local_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let state = git_test_app_state(&root).await;

        let missing_expected = revert_local_commit(
            State(state.clone()),
            Json(UndoLatestLocalCommitBody {
                project: git_project_body(&repository),
                expected_head: None,
                expected_target_ref: None,
            }),
        )
        .await
        .expect_err("undo must require the displayed immutable HEAD");
        assert_eq!(missing_expected.status, axum::http::StatusCode::BAD_REQUEST);

        let missing_target = revert_local_commit(
            State(state.clone()),
            Json(UndoLatestLocalCommitBody {
                project: git_project_body(&repository),
                expected_head: Some(local_head.clone()),
                expected_target_ref: None,
            }),
        )
        .await
        .expect_err("undo must require the displayed checked-out target");
        assert_eq!(missing_target.status, axum::http::StatusCode::BAD_REQUEST);

        let stale = revert_local_commit(
            State(state.clone()),
            Json(UndoLatestLocalCommitBody {
                project: git_project_body(&repository),
                expected_head: Some(base_head.clone()),
                expected_target_ref: Some("refs/heads/main".to_string()),
            }),
        )
        .await
        .expect_err("undo refuses a stale confirmation");
        assert_eq!(stale.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            local_head
        );

        // `release` intentionally starts at the same object ID as `main`.
        // A HEAD-only guard would accept this confirmation and soft-reset the
        // wrong branch.
        git_test_command(&repository, ["branch", "release"]);
        git_test_command(&repository, ["checkout", "release"]);
        let wrong_target = revert_local_commit(
            State(state.clone()),
            Json(UndoLatestLocalCommitBody {
                project: git_project_body(&repository),
                expected_head: Some(local_head.clone()),
                expected_target_ref: Some("refs/heads/main".to_string()),
            }),
        )
        .await
        .expect_err("undo refuses a same-OID checkout on another branch");
        assert_eq!(wrong_target.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            local_head
        );
        git_test_command(&repository, ["checkout", "main"]);

        // No upstream is configured here. A remote-tracking ref alone still
        // proves the commit was published or fetched and must not be reset.
        git_test_command(
            &repository,
            ["update-ref", "refs/remotes/origin/main", local_head.as_str()],
        );
        let shared = revert_local_commit(
            State(state.clone()),
            Json(UndoLatestLocalCommitBody {
                project: git_project_body(&repository),
                expected_head: Some(local_head.clone()),
                expected_target_ref: Some("refs/heads/main".to_string()),
            }),
        )
        .await
        .expect_err("undo refuses a remote-tracking commit");
        assert_eq!(shared.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            local_head
        );

        git_test_command(&repository, ["update-ref", "-d", "refs/remotes/origin/main"]);
        let undone = revert_local_commit(
            State(state.clone()),
            Json(UndoLatestLocalCommitBody {
                project: git_project_body(&repository),
                expected_head: Some(local_head),
                expected_target_ref: Some("refs/heads/main".to_string()),
            }),
        )
        .await
        .expect("an unshared current commit can be undone")
        .0;
        assert!(undone.success);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            base_head
        );
        assert!(git_test_output(&repository, ["diff", "--cached", "--", "value.txt"])
            .contains("+local change"));

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn commit_handler_commits_the_existing_index_without_restaging_worktree_hunks() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-index-commit-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        let original = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(repository.join("mixed.txt"), original).unwrap();
        git_test_command(&repository, ["add", "mixed.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let changed = (1..=20)
            .map(|line| match line {
                2 => "staged change".to_string(),
                19 => "unstaged change".to_string(),
                _ => format!("line {line}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(repository.join("mixed.txt"), changed).unwrap();
        let diff = git(&repository, ["diff"]).await.expect("working diff").stdout;
        let patch = selected_hunk_patch(&diff, &[0]).expect("first hunk");
        apply_patch_to_index(&repository, &patch, false)
            .await
            .expect("stage only first hunk");
        let context = commit_message_diff_context(
            &repository,
            &repository,
            &["mixed.txt".to_string()],
        )
        .await
        .expect("cached message context");
        assert!(context.contains("staged change"));
        assert!(!context.contains("unstaged change"));
        let reviewed_index_tree = git_test_output(&repository, ["write-tree"])
            .trim()
            .to_string();
        let reviewed_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        commit(
            State(state.clone()),
            Json(CommitBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                message: "commit staged hunk".to_string(),
                files: vec!["mixed.txt".to_string()],
                expected_index_tree: Some(reviewed_index_tree),
                expected_head: Some(reviewed_head),
                expected_target_ref: Some("refs/heads/main".to_string()),
            }),
        )
        .await
        .expect("commit staged index")
        .0;
        let committed = git_test_output(&repository, ["show", "HEAD:mixed.txt"]);
        assert!(committed.contains("staged change"));
        assert!(!committed.contains("unstaged change"));
        assert!(
            git_test_output(&repository, ["diff"])
                .contains("unstaged change")
        );
        let rejected = commit(
            State(state.clone()),
            Json(CommitBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                message: "must not add unstaged content".to_string(),
                files: vec!["mixed.txt".to_string()],
                expected_index_tree: Some(
                    git_test_output(&repository, ["write-tree"])
                        .trim()
                        .to_string(),
                ),
                expected_head: Some(
                    git_test_output(&repository, ["rev-parse", "HEAD"])
                        .trim()
                        .to_string(),
                ),
                expected_target_ref: Some("refs/heads/main".to_string()),
            }),
        )
        .await
        .expect_err("unstaged path must be rejected");
        assert_eq!(rejected.status, axum::http::StatusCode::BAD_REQUEST);

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn commit_handler_rejects_an_index_that_changed_after_status_review() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-index-snapshot-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("reviewed.txt"), "base reviewed\n").unwrap();
        std::fs::write(repository.join("surprise.txt"), "base surprise\n").unwrap();
        git_test_command(&repository, ["add", "reviewed.txt", "surprise.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        std::fs::write(repository.join("reviewed.txt"), "reviewed staged change\n").unwrap();
        git_test_command(&repository, ["add", "reviewed.txt"]);

        let state = git_test_app_state(&root).await;
        let reviewed_status = status(
            State(state.clone()),
            Query(git_project_query(&repository)),
        )
        .await
        .expect("status snapshot")
        .0;
        let reviewed_index_tree = reviewed_status
            .index_tree
            .expect("a mergeable index has a tree snapshot");
        let reviewed_head = reviewed_status
            .head
            .expect("a repository with commits has a HEAD snapshot");
        let reviewed_target_ref = reviewed_status
            .target_ref
            .expect("a branch checkout has a target-ref snapshot");
        assert!(
            matches!(reviewed_index_tree.len(), 40 | 64)
                && reviewed_index_tree.chars().all(|character| character.is_ascii_hexdigit()),
            "status must expose the full index tree object ID",
        );

        let missing_snapshot = commit(
            State(state.clone()),
            Json(CommitBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                message: "must require the reviewed index snapshot".to_string(),
                files: vec!["reviewed.txt".to_string()],
                expected_index_tree: None,
                expected_head: Some(reviewed_head.clone()),
                expected_target_ref: Some(reviewed_target_ref.clone()),
            }),
        )
        .await
        .expect_err("commit requests must include the reviewed index tree");
        assert_eq!(missing_snapshot.status, StatusCode::BAD_REQUEST);

        let missing_head = commit(
            State(state.clone()),
            Json(CommitBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                message: "must require the reviewed HEAD snapshot".to_string(),
                files: vec!["reviewed.txt".to_string()],
                expected_index_tree: Some(reviewed_index_tree.clone()),
                expected_head: None,
                expected_target_ref: Some(reviewed_target_ref.clone()),
            }),
        )
        .await
        .expect_err("commit requests must include the reviewed HEAD");
        assert_eq!(missing_head.status, StatusCode::BAD_REQUEST);
        assert!(missing_head.body.error.contains("expectedHead is required"));

        let missing_target = commit(
            State(state.clone()),
            Json(CommitBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                message: "must require the reviewed checkout target".to_string(),
                files: vec!["reviewed.txt".to_string()],
                expected_index_tree: Some(reviewed_index_tree.clone()),
                expected_head: Some(reviewed_head.clone()),
                expected_target_ref: None,
            }),
        )
        .await
        .expect_err("commit requests must include the reviewed checkout target");
        assert_eq!(missing_target.status, StatusCode::BAD_REQUEST);
        assert!(missing_target
            .body
            .error
            .contains("expectedTargetRef is required"));

        // A full HEAD object ID is not enough: `release` currently points at
        // the same commit as `main`, but this commit review belongs to main.
        // The target-ref guard must win before the index snapshot is checked.
        git_test_command(&repository, ["branch", "release"]);
        git_test_command(&repository, ["checkout", "release"]);
        let wrong_target = commit(
            State(state.clone()),
            Json(CommitBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                message: "must not commit to an equal-OID branch".to_string(),
                files: vec!["reviewed.txt".to_string()],
                expected_index_tree: Some(reviewed_index_tree.clone()),
                expected_head: Some(reviewed_head.clone()),
                expected_target_ref: Some(reviewed_target_ref.clone()),
            }),
        )
        .await
        .expect_err("a same-OID branch switch invalidates the commit review");
        assert_eq!(wrong_target.status, StatusCode::CONFLICT);
        assert!(wrong_target
            .body
            .error
            .contains("checked-out branch changed since this commit was prepared"));
        assert_eq!(
            git_test_output(&repository, ["symbolic-ref", "--short", "HEAD"]).trim(),
            "release"
        );
        git_test_command(&repository, ["checkout", "main"]);

        // Another tool stages a path after the user reviewed Git status. Git
        // commit would otherwise include it because commits operate on the
        // entire index rather than the selected list in this request.
        std::fs::write(repository.join("surprise.txt"), "unexpected staged change\n").unwrap();
        git_test_command(&repository, ["add", "surprise.txt"]);
        let head_before = git_test_output(&repository, ["rev-parse", "HEAD"]);

        let rejected = commit(
            State(state.clone()),
            Json(CommitBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                message: "must not commit an unreviewed index path".to_string(),
                files: vec!["reviewed.txt".to_string()],
                expected_index_tree: Some(reviewed_index_tree.clone()),
                expected_head: Some(reviewed_head.clone()),
                expected_target_ref: Some(reviewed_target_ref.clone()),
            }),
        )
        .await
        .expect_err("a changed index requires a fresh review");
        assert_eq!(rejected.status, StatusCode::CONFLICT);
        assert!(rejected
            .body
            .error
            .contains("staged index changed after review"));
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]),
            head_before
        );
        let staged = git_test_output(&repository, ["diff", "--cached", "--name-only"]);
        assert!(staged.lines().any(|path| path == "reviewed.txt"));
        assert!(staged.lines().any(|path| path == "surprise.txt"));

        // A separate Git client can advance the checked-out branch after the
        // index was reviewed. The commit route must report this stale target
        // rather than falling through to the now-different index snapshot.
        git_test_command(&repository, ["commit", "-m", "external staged change"]);
        let stale_head = commit(
            State(state.clone()),
            Json(CommitBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                message: "must not commit after the reviewed HEAD moves".to_string(),
                files: vec!["reviewed.txt".to_string()],
                expected_index_tree: Some(reviewed_index_tree),
                expected_head: Some(reviewed_head),
                expected_target_ref: Some(reviewed_target_ref),
            }),
        )
        .await
        .expect_err("a moved HEAD invalidates the commit review");
        assert_eq!(stale_head.status, StatusCode::CONFLICT);
        assert!(stale_head
            .body
            .error
            .contains("current HEAD changed since this commit was prepared"));

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn normal_rebase_rejects_noneditable_interactive_mode_and_hidden_autostash() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-rebase-safety-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("base.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "base.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("feature.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["add", "feature.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature"]);
        git_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("main.txt"), "main\n").unwrap();
        git_test_command(&repository, ["add", "main.txt"]);
        git_test_command(&repository, ["commit", "-m", "main"]);
        git_test_command(&repository, ["checkout", "feature"]);

        let state = git_test_app_state(&root).await;
        let expected_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let interactive_error = rebase(
            State(state.clone()),
            Json(RebaseBody {
                project: git_project_body(&repository),
                upstream: "main".to_string(),
                interactive: true,
                expected_head: expected_head.clone(),
                expected_target_ref: "refs/heads/feature".to_string(),
            }),
        )
        .await
        .expect_err("noneditable interactive rebase must be rejected");
        assert_eq!(interactive_error.status, StatusCode::BAD_REQUEST);
        assert!(interactive_error.body.error.contains("Interactive rebase"));
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            expected_head
        );

        // Git's rebase.autoStash would otherwise create a temporary stash
        // without a native UI surface to review it. A dirty tree must be
        // rejected before Git starts, even when the upstream is valid.
        git_test_command(&repository, ["config", "rebase.autoStash", "true"]);
        std::fs::write(repository.join("feature.txt"), "dirty local edit\n").unwrap();
        let dirty_error = rebase(
            State(state.clone()),
            Json(RebaseBody {
                project: git_project_body(&repository),
                upstream: "main".to_string(),
                interactive: false,
                expected_head: expected_head.clone(),
                expected_target_ref: "refs/heads/feature".to_string(),
            }),
        )
        .await
        .expect_err("dirty normal rebase must not trigger an autostash");
        assert_eq!(dirty_error.status, StatusCode::CONFLICT);
        assert!(dirty_error.body.error.contains("working tree must be clean"));
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            expected_head
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("feature.txt")).unwrap(),
            "dirty local edit\n"
        );
        assert!(git_test_output(&repository, ["stash", "list"]).trim().is_empty());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn normal_rebase_preserves_merge_topology_like_lazygit() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-rebase-merges-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("base.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "base.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("feature-one.txt"), "one\n").unwrap();
        git_test_command(&repository, ["add", "feature-one.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature one"]);
        git_test_command(&repository, ["checkout", "-b", "side"]);
        std::fs::write(repository.join("side.txt"), "side\n").unwrap();
        git_test_command(&repository, ["add", "side.txt"]);
        git_test_command(&repository, ["commit", "-m", "side change"]);

        git_test_command(&repository, ["checkout", "feature"]);
        std::fs::write(repository.join("feature-two.txt"), "two\n").unwrap();
        git_test_command(&repository, ["add", "feature-two.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature two"]);
        git_test_command(&repository, ["merge", "--no-ff", "side", "-m", "merge side"]);
        let feature_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        git_test_command(&repository, ["checkout", "main"]);
        std::fs::write(repository.join("upstream.txt"), "upstream\n").unwrap();
        git_test_command(&repository, ["add", "upstream.txt"]);
        git_test_command(&repository, ["commit", "-m", "upstream"]);
        let main_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["checkout", "feature"]);

        let state = git_test_app_state(&root).await;
        let response = rebase(
            State(state.clone()),
            Json(RebaseBody {
                project: git_project_body(&repository),
                upstream: "main".to_string(),
                interactive: false,
                expected_head: feature_head,
                expected_target_ref: "refs/heads/feature".to_string(),
            }),
        )
        .await
        .expect("merge-preserving rebase")
        .0;
        assert!(response.success);
        assert!(
            !response
                .operation_state
                .expect("completed rebase state")
                .in_progress
        );
        assert_eq!(
            git_test_output(&repository, ["symbolic-ref", "--short", "HEAD"]).trim(),
            "feature"
        );
        assert!(
            git_test_output(&repository, ["merge-base", "--is-ancestor", main_head.as_str(), "HEAD"])
                .is_empty(),
            "the rebased branch must be based on the selected upstream"
        );
        let parents = git_test_output(&repository, ["rev-list", "--parents", "-n", "1", "HEAD"]);
        assert_eq!(
            parents.split_whitespace().count(),
            3,
            "the recreated top commit must remain a two-parent merge"
        );
        assert_eq!(
            git_test_output(&repository, ["log", "-1", "--format=%s"]).trim(),
            "merge side"
        );
        assert!(git_test_output(&repository, ["status", "--porcelain"])
            .trim()
            .is_empty());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merge_explicitly_disables_configured_autostash() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-merge-no-autostash-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("shared.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "shared.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        git_test_command(&repository, ["checkout", "-b", "feature"]);
        std::fs::write(repository.join("shared.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "feature change"]);
        git_test_command(&repository, ["checkout", "main"]);
        let expected_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["config", "merge.autoStash", "true"]);
        std::fs::write(repository.join("shared.txt"), "dirty local edit\n").unwrap();

        let state = git_test_app_state(&root).await;
        let error = merge(
            State(state.clone()),
            Json(MergeBody {
                project: git_project_body(&repository),
                source: "feature".to_string(),
                merge_mode: None,
                no_fast_forward: true,
                commit_squash: false,
                message: None,
                expected_head: expected_head.clone(),
                expected_target_ref: "refs/heads/main".to_string(),
            }),
        )
        .await
        .expect_err("a conflicting dirty merge must not create a hidden autostash");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(
            error
                .body
                .details
                .as_deref()
                .is_some_and(|details| details.contains("--no-autostash")),
            "the command must override merge.autoStash explicitly",
        );
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            expected_head
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("shared.txt")).unwrap(),
            "dirty local edit\n"
        );
        assert!(git_test_output(&repository, ["stash", "list"]).trim().is_empty());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pull_merge_explicitly_disables_configured_autostash() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-pull-no-autostash-{}",
            uuid::Uuid::new_v4()
        ));
        let remote = root.join("remote.git");
        let primary = root.join("primary");
        let replica = root.join("replica");
        let remote_path = remote.display().to_string();
        let primary_path = primary.display().to_string();
        let replica_path = replica.display().to_string();
        std::fs::create_dir_all(&root).unwrap();
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                remote_path.as_str(),
            ],
        );
        git_test_command(
            &root,
            ["init", "--initial-branch=main", primary_path.as_str()],
        );
        configure_test_repository(&primary);
        std::fs::write(primary.join("shared.txt"), "base\n").unwrap();
        git_test_command(&primary, ["add", "shared.txt"]);
        git_test_command(&primary, ["commit", "-m", "base"]);
        git_test_command(&primary, ["remote", "add", "origin", remote_path.as_str()]);
        git_test_command(&primary, ["push", "-u", "origin", "main"]);

        git_test_command(&root, ["clone", remote_path.as_str(), replica_path.as_str()]);
        configure_test_repository(&replica);
        std::fs::write(replica.join("shared.txt"), "remote update\n").unwrap();
        git_test_command(&replica, ["commit", "-am", "remote update"]);
        git_test_command(&replica, ["push"]);

        git_test_command(&primary, ["config", "merge.autoStash", "true"]);
        std::fs::write(primary.join("shared.txt"), "dirty local edit\n").unwrap();
        let expected_head = git_test_output(&primary, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let state = git_test_app_state(&root).await;
        let error = pull(State(state.clone()), Json(git_project_body(&primary)))
            .await
            .expect_err("a dirty merge-style pull must not create a hidden autostash");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(
            error
                .body
                .details
                .as_deref()
                .is_some_and(|details| details.contains("--no-autostash")),
            "the pull command must override merge.autoStash explicitly",
        );
        assert_eq!(
            git_test_output(&primary, ["rev-parse", "HEAD"]).trim(),
            expected_head
        );
        assert_eq!(
            std::fs::read_to_string(primary.join("shared.txt")).unwrap(),
            "dirty local edit\n"
        );
        assert!(git_test_output(&primary, ["stash", "list"]).trim().is_empty());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn discard_worktree_changes_preserves_staged_index_for_mm_and_am_paths() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-discard-index-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("mixed.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "mixed.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        // `MM`: index contains the reviewed staged version while the
        // worktree has a further edit. The default discard must restore the
        // worktree to the index, not reset the index to HEAD.
        std::fs::write(repository.join("mixed.txt"), "staged version\n").unwrap();
        git_test_command(&repository, ["add", "mixed.txt"]);
        std::fs::write(repository.join("mixed.txt"), "unstaged version\n").unwrap();
        assert!(git_test_output(&repository, ["status", "--short"])
            .lines()
            .any(|line| line.starts_with("MM mixed.txt")));

        let state = git_test_app_state(&root).await;
        let _ = discard(
            State(state.clone()),
            Json(FileBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                file: "mixed.txt".to_string(),
            }),
        )
        .await
        .expect("discard only the working-tree edit from an MM path");
        assert_eq!(
            std::fs::read_to_string(repository.join("mixed.txt")).unwrap(),
            "staged version\n"
        );
        assert_eq!(
            git_test_output(&repository, ["show", ":mixed.txt"]),
            "staged version\n"
        );
        assert!(git_test_output(&repository, ["diff", "--", "mixed.txt"])
            .trim()
            .is_empty());
        assert!(git_test_output(&repository, ["diff", "--cached", "--", "mixed.txt"])
            .contains("+staged version"));

        // `AM` has no HEAD version for the path. It still must retain the
        // staged addition while throwing away only the later worktree edit.
        std::fs::write(repository.join("added.txt"), "staged addition\n").unwrap();
        git_test_command(&repository, ["add", "added.txt"]);
        std::fs::write(repository.join("added.txt"), "unstaged addition\n").unwrap();
        assert!(git_test_output(&repository, ["status", "--short"])
            .lines()
            .any(|line| line.starts_with("AM added.txt")));
        let _ = discard(
            State(state.clone()),
            Json(FileBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                file: "added.txt".to_string(),
            }),
        )
        .await
        .expect("discard only the working-tree edit from an AM path");
        assert_eq!(
            std::fs::read_to_string(repository.join("added.txt")).unwrap(),
            "staged addition\n"
        );
        assert_eq!(
            git_test_output(&repository, ["show", ":added.txt"]),
            "staged addition\n"
        );
        assert!(git_test_output(&repository, ["diff", "--", "added.txt"])
            .trim()
            .is_empty());
        assert!(git_test_output(&repository, ["diff", "--cached", "--", "added.txt"])
            .contains("+staged addition"));

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn discard_all_changes_requires_confirmation_and_resets_mm_and_am_paths() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-discard-all-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("mixed.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "mixed.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        std::fs::write(repository.join("mixed.txt"), "staged version\n").unwrap();
        git_test_command(&repository, ["add", "mixed.txt"]);
        std::fs::write(repository.join("mixed.txt"), "unstaged version\n").unwrap();
        let state = git_test_app_state(&root).await;
        let missing_confirmation = discard_all(
            State(state.clone()),
            Json(GitDiscardAllRequest {
                project: repository.display().to_string(),
                file: "mixed.txt".to_string(),
                confirm: false,
                repository_id: None,
            }),
        )
        .await
        .expect_err("full discard must require explicit confirmation");
        assert_eq!(missing_confirmation.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            std::fs::read_to_string(repository.join("mixed.txt")).unwrap(),
            "unstaged version\n"
        );

        let _ = discard_all(
            State(state.clone()),
            Json(GitDiscardAllRequest {
                project: repository.display().to_string(),
                file: "mixed.txt".to_string(),
                confirm: true,
                repository_id: None,
            }),
        )
        .await
        .expect("confirmed full discard resets MM to HEAD");
        assert_eq!(
            std::fs::read_to_string(repository.join("mixed.txt")).unwrap(),
            "base\n"
        );
        assert_eq!(git_test_output(&repository, ["show", ":mixed.txt"]), "base\n");
        assert!(git_test_output(&repository, ["diff", "--", "mixed.txt"])
            .trim()
            .is_empty());
        assert!(git_test_output(&repository, ["diff", "--cached", "--", "mixed.txt"])
            .trim()
            .is_empty());

        std::fs::write(repository.join("added.txt"), "staged addition\n").unwrap();
        git_test_command(&repository, ["add", "added.txt"]);
        std::fs::write(repository.join("added.txt"), "unstaged addition\n").unwrap();
        assert!(git_test_output(&repository, ["status", "--short"])
            .lines()
            .any(|line| line.starts_with("AM added.txt")));
        let _ = discard_all(
            State(state.clone()),
            Json(GitDiscardAllRequest {
                project: repository.display().to_string(),
                file: "added.txt".to_string(),
                confirm: true,
                repository_id: None,
            }),
        )
        .await
        .expect("confirmed full discard removes an AM path absent from HEAD");
        assert!(!repository.join("added.txt").exists());
        assert!(git_test_output(&repository, ["status", "--short"])
            .lines()
            .all(|line| !line.ends_with("added.txt")));

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn selective_stash_restore_lists_previews_and_restores_real_stash_content() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-selective-stash-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        let base = (1..=24)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(repository.join("tracked.txt"), &base).unwrap();
        std::fs::write(repository.join("removed.txt"), "remove me\n").unwrap();
        git_test_command(&repository, ["add", "."]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        let stashed = (1..=24)
            .map(|line| match line {
                2 => "first stashed hunk".to_string(),
                22 => "second stashed hunk".to_string(),
                _ => format!("line {line}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(repository.join("tracked.txt"), stashed).unwrap();
        std::fs::remove_file(repository.join("removed.txt")).unwrap();
        std::fs::write(repository.join("untracked.txt"), "untracked stash content\n").unwrap();
        git_test_command(
            &repository,
            ["stash", "push", "--include-untracked", "-m", "selective restore"],
        );

        let state = git_test_app_state(&root).await;
        let scope = ManagementProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };
        let inventory = management_stash_files(
            State(state.clone()),
            Query(ManagementStashFilesQuery {
                scope,
                reference: "stash@{0}".to_string(),
            }),
        )
        .await
        .expect("list selective stash files")
        .0;
        assert!(inventory.files.iter().any(|file| {
            file.path == "tracked.txt"
                && file.source == "tracked"
                && file.status == "modified"
        }));
        assert!(inventory.files.iter().any(|file| {
            file.path == "removed.txt"
                && file.source == "tracked"
                && file.status == "deleted"
        }));
        assert!(inventory.files.iter().any(|file| {
            file.path == "untracked.txt" && file.source == "untracked"
        }));

        let preview = management_stash_file_diff(
            State(state.clone()),
            Query(ManagementStashFileDiffQuery {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                reference: "stash@{0}".to_string(),
                file: "tracked.txt".to_string(),
            }),
        )
        .await
        .expect("preview tracked stash file")
        .0;
        assert!(preview.can_restore_hunks);
        assert!(preview.diff.contains("first stashed hunk"));
        assert!(preview.diff.contains("second stashed hunk"));

        let _ = management_restore_stash_hunks(
            State(state.clone()),
            Json(ManagementStashRestoreHunksBody {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                reference: "stash@{0}".to_string(),
                expected_hash: None,
                file: "tracked.txt".to_string(),
                hunk_indexes: vec![0],
                confirm: true,
            }),
        )
        .await
        .expect("restore first stash hunk");
        let restored_hunk = std::fs::read_to_string(repository.join("tracked.txt")).unwrap();
        assert!(restored_hunk.contains("first stashed hunk"));
        assert!(!restored_hunk.contains("second stashed hunk"));
        assert!(git_test_output(&repository, ["diff", "--cached"]).trim().is_empty());

        let _ = management_restore_stash_files(
            State(state.clone()),
            Json(ManagementStashRestoreFilesBody {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                reference: "stash@{0}".to_string(),
                expected_hash: None,
                files: vec!["removed.txt".to_string(), "untracked.txt".to_string()],
                confirm: true,
            }),
        )
        .await
        .expect("restore deleted and untracked stash files");
        assert!(!repository.join("removed.txt").exists());
        assert_eq!(
            std::fs::read_to_string(repository.join("untracked.txt")).unwrap(),
            "untracked stash content\n"
        );
        assert!(git_test_output(&repository, ["diff", "--cached"]).trim().is_empty());

        // Exercise the public routes too: these handlers were intentionally
        // kept separate from the original stash API, so a direct call alone
        // would not catch a missing router registration.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router().with_state(server_state)).await;
        });
        let response = reqwest::Client::new()
            .get(format!("http://{address}/api/git/stash/files"))
            .query(&[
                ("projectPath", repository.display().to_string()),
                ("reference", "stash@{0}".to_string()),
            ])
            .send()
            .await
            .expect("request selective stash inventory");
        assert!(response.status().is_success());
        let response = response.json::<serde_json::Value>().await.unwrap();
        assert!(response["files"].is_array());
        server.abort();

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn advanced_stash_including_untracked_removes_the_captured_file() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-advanced-stash-untracked-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        std::fs::write(repository.join("tracked.txt"), "modified\n").unwrap();
        std::fs::write(repository.join("staged.txt"), "staged\n").unwrap();
        git_test_command(&repository, ["add", "staged.txt"]);
        std::fs::write(repository.join("untracked.txt"), "captured untracked\n").unwrap();

        let state = git_test_app_state(&root).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router().with_state(server_state)).await;
        });
        let response = reqwest::Client::new()
            .post(format!("http://{address}/api/git/stash/create-advanced"))
            .json(&serde_json::json!({
                "projectPath": repository.display().to_string(),
                "message": "capture all work",
                "includeUntracked": true,
                "confirm": true,
            }))
            .send()
            .await
            .expect("create advanced stash request");
        assert!(response.status().is_success());
        assert!(response
            .json::<serde_json::Value>()
            .await
            .expect("advanced stash response")["success"]
            .as_bool()
            .unwrap_or(false));

        assert!(
            git_test_output(&repository, ["status", "--porcelain"])
                .trim()
                .is_empty(),
            "an advanced stash including untracked files must leave a clean worktree"
        );
        assert!(
            !repository.join("untracked.txt").exists(),
            "the captured untracked file must be removed from the worktree"
        );
        let stashed_files = git_test_output(
            &repository,
            ["stash", "show", "--include-untracked", "--name-status", "stash@{0}"],
        );
        assert!(stashed_files.contains("M\ttracked.txt"));
        assert!(stashed_files.contains("A\tstaged.txt"));
        assert!(stashed_files.contains("A\tuntracked.txt"));

        server.abort();
        let _ = server.await;
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ordinary_stash_keeps_untracked_files_out_of_the_stash() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-ordinary-stash-untracked-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        std::fs::write(repository.join("tracked.txt"), "modified\n").unwrap();
        std::fs::write(repository.join("untracked.txt"), "captured untracked\n").unwrap();

        let state = git_test_app_state(&root).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router().with_state(server_state)).await;
        });
        let response = reqwest::Client::new()
            .post(format!("http://{address}/api/git/stash"))
            .json(&serde_json::json!({
                "projectPath": repository.display().to_string(),
                "message": "ordinary stash capture",
            }))
            .send()
            .await
            .expect("ordinary stash request");
        assert!(response.status().is_success());

        assert!(
            repository.join("untracked.txt").exists(),
            "ordinary stash must leave untracked files alone unless the user opts into advanced stash"
        );
        assert_eq!(
            git_test_output(&repository, ["status", "--porcelain"]),
            "?? untracked.txt\n",
            "only the untracked path should remain after a normal stash"
        );
        let stashed_files = git_test_output(
            &repository,
            ["stash", "show", "--include-untracked", "--name-status", "stash@{0}"],
        );
        assert!(stashed_files.contains("M\ttracked.txt"));
        assert!(!stashed_files.contains("A\tuntracked.txt"));

        server.abort();
        let _ = server.await;
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lazygit_stash_unstaged_preserves_the_index_and_saves_only_the_worktree_delta() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-stash-unstaged-mixed-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let original_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        // One file deliberately has an index delta and a distinct worktree
        // delta. `stash --keep-index` saves base -> mixed here; LazyGit saves
        // staged -> mixed via its temporary-commit sequence.
        std::fs::write(repository.join("tracked.txt"), "staged\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        std::fs::write(repository.join("tracked.txt"), "mixed\n").unwrap();
        std::fs::write(repository.join("untracked.txt"), "leave me alone\n").unwrap();

        let state = git_test_app_state(&root).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router().with_state(server_state)).await;
        });
        let response = reqwest::Client::new()
            .post(format!("http://{address}/api/git/stash/create-advanced"))
            .json(&serde_json::json!({
                "projectPath": repository.display().to_string(),
                "message": "unstaged only",
                "includeUntracked": false,
                "unstagedOnly": true,
                "confirm": true,
            }))
            .send()
            .await
            .expect("create LazyGit-style stash-unstaged request");
        assert!(response.status().is_success());
        assert!(response
            .json::<serde_json::Value>()
            .await
            .expect("stash-unstaged response")["success"]
            .as_bool()
            .unwrap_or(false));

        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            original_head,
            "the temporary commit must not remain on the checked-out branch"
        );
        let status = git_test_output(&repository, ["status", "--short"]);
        assert!(status.contains("M  tracked.txt"), "{status}");
        assert!(status.contains("?? untracked.txt"), "{status}");
        let cached = git_test_output(&repository, ["diff", "--cached", "--", "tracked.txt"]);
        assert!(cached.contains("-base"), "{cached}");
        assert!(cached.contains("+staged"), "{cached}");
        assert!(
            git_test_output(&repository, ["diff", "--", "tracked.txt"])
                .trim()
                .is_empty(),
            "the unstaged delta must leave the worktree"
        );
        let stash_delta = git_test_output(
            &repository,
            ["diff", "stash@{0}^", "stash@{0}", "--", "tracked.txt"],
        );
        assert!(stash_delta.contains("-staged"), "{stash_delta}");
        assert!(stash_delta.contains("+mixed"), "{stash_delta}");
        assert!(
            !stash_delta.contains("-base"),
            "the stash must not include the already-staged base delta: {stash_delta}"
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("untracked.txt")).unwrap(),
            "leave me alone\n"
        );
        assert!(
            !management_stash_unstaged_recovery_marker_path(&repository)
                .await
                .unwrap()
                .exists(),
            "a completed transaction must unlock the repository"
        );

        server.abort();
        let _ = server.await;
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stash_unstaged_recovery_reconciles_the_post_commit_marker_crash_window() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-stash-unstaged-post-commit-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let original_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        // Reproduce exactly the process-death boundary: the durable marker
        // was written before the synthetic commit, then Git advanced HEAD,
        // but no code had yet recorded that new object ID in the marker.
        std::fs::write(repository.join("tracked.txt"), "staged\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        std::fs::write(repository.join("tracked.txt"), "mixed\n").unwrap();
        let marker = ManagementStashUnstagedRecoveryMarker {
            version: MANAGEMENT_STASH_UNSTAGED_RECOVERY_VERSION,
            original_head: original_head.clone(),
            target_ref: "refs/heads/main".to_string(),
            transaction_id: Some(uuid::Uuid::new_v4().to_string()),
            temporary_head: None,
            stash_hash: None,
            stash_stored: false,
            message: Some("reconcile post-commit crash".to_string()),
        };
        management_write_stash_unstaged_recovery_marker(&repository, &marker)
            .await
            .unwrap();
        let temporary_message = management_stash_unstaged_synthetic_commit_message(&marker);
        git_test_command(
            &repository,
            [
                "commit",
                "--no-verify",
                "--no-gpg-sign",
                "-m",
                temporary_message.as_str(),
            ],
        );
        let temporary_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        assert_ne!(temporary_head, original_head);

        let state = git_test_app_state(&root).await;
        let response = continue_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("reconcile and finish the interrupted stash-unstaged transaction")
            .0;
        assert!(response.success);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            original_head,
            "the synthetic commit must not remain on the branch after recovery"
        );
        assert_eq!(
            git_test_output(&repository, ["status", "--short"]),
            "M  tracked.txt\n"
        );
        let stash_delta = git_test_output(
            &repository,
            ["diff", "stash@{0}^", "stash@{0}", "--", "tracked.txt"],
        );
        assert!(stash_delta.contains("-staged") && stash_delta.contains("+mixed"), "{stash_delta}");
        assert!(
            !management_stash_unstaged_recovery_marker_path(&repository)
                .await
                .unwrap()
                .exists(),
            "successful reconciliation must unlock the repository"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stash_unstaged_abort_reconciles_the_post_commit_marker_crash_window() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-stash-unstaged-post-commit-abort-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let original_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        std::fs::write(repository.join("tracked.txt"), "staged\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        std::fs::write(repository.join("tracked.txt"), "mixed\n").unwrap();
        let marker = ManagementStashUnstagedRecoveryMarker {
            version: MANAGEMENT_STASH_UNSTAGED_RECOVERY_VERSION,
            original_head: original_head.clone(),
            target_ref: "refs/heads/main".to_string(),
            transaction_id: Some(uuid::Uuid::new_v4().to_string()),
            temporary_head: None,
            stash_hash: None,
            stash_stored: false,
            message: None,
        };
        management_write_stash_unstaged_recovery_marker(&repository, &marker)
            .await
            .unwrap();
        let temporary_message = management_stash_unstaged_synthetic_commit_message(&marker);
        git_test_command(
            &repository,
            [
                "commit",
                "--no-verify",
                "--no-gpg-sign",
                "-m",
                temporary_message.as_str(),
            ],
        );

        let state = git_test_app_state(&root).await;
        let response = abort_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("reconcile and abort the interrupted stash-unstaged transaction")
            .0;
        assert!(response.success);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            original_head
        );
        assert_eq!(
            git_test_output(&repository, ["status", "--short"]),
            "MM tracked.txt\n"
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("tracked.txt")).unwrap(),
            "mixed\n"
        );
        assert!(
            !management_stash_unstaged_recovery_marker_path(&repository)
                .await
                .unwrap()
                .exists(),
            "abort must unlock the repository after restoring its original mixed state"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stash_unstaged_recovery_refuses_an_unverified_post_commit_head() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-stash-unstaged-unverified-head-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let original_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let marker = ManagementStashUnstagedRecoveryMarker {
            version: MANAGEMENT_STASH_UNSTAGED_RECOVERY_VERSION,
            original_head,
            target_ref: "refs/heads/main".to_string(),
            transaction_id: Some(uuid::Uuid::new_v4().to_string()),
            temporary_head: None,
            stash_hash: None,
            stash_stored: false,
            message: None,
        };
        management_write_stash_unstaged_recovery_marker(&repository, &marker)
            .await
            .unwrap();
        std::fs::write(repository.join("tracked.txt"), "unrelated\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "--no-verify", "-m", "unrelated external commit"]);
        let unrelated_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let response = continue_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("an unverified recovery remains visible to the client")
            .0;
        assert!(response.success);
        assert!(
            response
                .output
                .as_deref()
                .is_some_and(|message| message.contains("not the validated temporary commit")),
            "the client must be told that native recovery refused the unknown HEAD"
        );
        assert!(
            response
                .operation_state
                .as_ref()
                .is_some_and(|operation| operation.in_progress),
            "the recovery lock must remain visible rather than allowing another mutation"
        );
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            unrelated_head,
            "recovery must not mutate an unverified user commit"
        );
        assert!(
            management_stash_unstaged_recovery_marker_path(&repository)
                .await
                .unwrap()
                .exists(),
            "the unresolved marker must keep ordinary Git mutations locked"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lazygit_stash_unstaged_falls_back_to_an_ordinary_stash_without_an_index() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-stash-unstaged-fallback-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let original_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        std::fs::write(repository.join("tracked.txt"), "unstaged\n").unwrap();
        std::fs::write(repository.join("untracked.txt"), "keep\n").unwrap();

        let state = git_test_app_state(&root).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router().with_state(server_state)).await;
        });
        // Omit includeUntracked deliberately. The endpoint must use
        // LazyGit's tracked-only fallback for this action, rather than the
        // advanced-stash default of capturing untracked paths.
        let response = reqwest::Client::new()
            .post(format!("http://{address}/api/git/stash/create-advanced"))
            .json(&serde_json::json!({
                "projectPath": repository.display().to_string(),
                "unstagedOnly": true,
                "confirm": true,
            }))
            .send()
            .await
            .expect("create no-index stash-unstaged request");
        assert!(response.status().is_success());
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            original_head
        );
        assert_eq!(
            git_test_output(&repository, ["status", "--short"]),
            "?? untracked.txt\n"
        );
        assert_eq!(
            git_test_output(&repository, ["stash", "list", "--format=%H"])
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count(),
            1
        );
        assert!(
            !management_stash_unstaged_recovery_marker_path(&repository)
                .await
                .unwrap()
                .exists()
        );

        server.abort();
        let _ = server.await;
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stash_unstaged_recovery_finishes_a_stash_store_crash_without_duplicating_the_entry() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-stash-unstaged-store-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let original_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        std::fs::write(repository.join("tracked.txt"), "staged\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        std::fs::write(repository.join("tracked.txt"), "mixed\n").unwrap();
        git_test_command(&repository, ["commit", "--no-verify", "-m", "temporary"]);
        let temporary_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let stash_hash = git_test_output(&repository, ["stash", "create"])
            .trim()
            .to_string();
        git_test_command(
            &repository,
            ["stash", "store", "-m", "already stored", &stash_hash],
        );
        // This is the crash window: the object ID was persisted before store,
        // but the post-store marker bit was not yet written.
        management_write_stash_unstaged_recovery_marker(
            &repository,
            &ManagementStashUnstagedRecoveryMarker {
                version: MANAGEMENT_STASH_UNSTAGED_RECOVERY_VERSION,
                original_head: original_head.clone(),
                target_ref: "refs/heads/main".to_string(),
                transaction_id: None,
                temporary_head: Some(temporary_head),
                stash_hash: Some(stash_hash.clone()),
                stash_stored: false,
                message: Some("already stored".to_string()),
            },
        )
        .await
        .unwrap();

        let state = git_test_app_state(&root).await;
        let response = continue_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("finish interrupted stash-unstaged recovery")
            .0;
        assert!(response.success);
        assert_eq!(
            git_test_output(&repository, ["stash", "list", "--format=%H"])
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count(),
            1,
            "resume must recognize the existing immutable stash instead of storing it twice"
        );
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            original_head
        );
        assert!(
            !management_stash_unstaged_recovery_marker_path(&repository)
                .await
                .unwrap()
                .exists()
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stash_unstaged_abort_after_final_soft_reset_restores_the_original_mixed_change() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-stash-unstaged-abort-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        let original_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        std::fs::write(repository.join("tracked.txt"), "staged\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        std::fs::write(repository.join("tracked.txt"), "mixed\n").unwrap();
        std::fs::write(repository.join("untracked.txt"), "keep\n").unwrap();
        git_test_command(&repository, ["commit", "--no-verify", "-m", "temporary"]);
        let temporary_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let stash_hash = git_test_output(&repository, ["stash", "create"])
            .trim()
            .to_string();
        git_test_command(
            &repository,
            ["stash", "store", "-m", "recover me", &stash_hash],
        );
        git_test_command(&repository, ["reset", "--hard", &temporary_head]);
        git_test_command(&repository, ["reset", "--soft", &original_head]);
        management_write_stash_unstaged_recovery_marker(
            &repository,
            &ManagementStashUnstagedRecoveryMarker {
                version: MANAGEMENT_STASH_UNSTAGED_RECOVERY_VERSION,
                original_head: original_head.clone(),
                target_ref: "refs/heads/main".to_string(),
                transaction_id: None,
                temporary_head: Some(temporary_head),
                stash_hash: Some(stash_hash),
                stash_stored: true,
                message: Some("recover me".to_string()),
            },
        )
        .await
        .unwrap();

        let state = git_test_app_state(&root).await;
        let response = abort_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("restore interrupted stash-unstaged changes")
            .0;
        assert!(response.success);
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            original_head
        );
        assert_eq!(
            git_test_output(&repository, ["status", "--short"]),
            "MM tracked.txt\n?? untracked.txt\n"
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("tracked.txt")).unwrap(),
            "mixed\n"
        );
        let cached = git_test_output(&repository, ["diff", "--cached", "--", "tracked.txt"]);
        assert!(cached.contains("-base") && cached.contains("+staged"), "{cached}");
        let unstaged = git_test_output(&repository, ["diff", "--", "tracked.txt"]);
        assert!(unstaged.contains("-staged") && unstaged.contains("+mixed"), "{unstaged}");
        assert!(
            !management_stash_unstaged_recovery_marker_path(&repository)
                .await
                .unwrap()
                .exists()
        );
        assert!(git_test_output(&repository, ["stash", "list"]).contains("recover me"));

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn management_stash_rename_matches_lazygit_and_rejects_stale_rows() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-stash-rename-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        std::fs::write(repository.join("tracked.txt"), "older stash\n").unwrap();
        git_test_command(&repository, ["stash", "push", "-m", "older"]);
        std::fs::write(repository.join("tracked.txt"), "newer stash\n").unwrap();
        git_test_command(&repository, ["stash", "push", "-m", "newer"]);

        let state = git_test_app_state(&root).await;
        let before = stashes(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("list stashes before rename")
            .0
            .stashes;
        assert_eq!(before.len(), 2);
        let older = before[1].clone();

        // Exercise the public route: a LazyGit rename re-stores the selected
        // stash, so it appears at the top with a new reflog subject while its
        // immutable stash commit remains the same.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router().with_state(server_state)).await;
        });
        let response = reqwest::Client::new()
            .post(format!("http://{address}/api/git/stash/rename"))
            .json(&serde_json::json!({
                "projectPath": repository.display().to_string(),
                "reference": older.reference.clone(),
                "expectedHash": older.hash.clone(),
                "message": "renamed older",
            }))
            .send()
            .await
            .expect("request stash rename");
        assert!(response.status().is_success());
        assert!(response
            .json::<serde_json::Value>()
            .await
            .expect("decode stash rename response")["success"]
            .as_bool()
            .unwrap_or(false));
        server.abort();
        let _ = server.await;

        let after_older = stashes(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("list stashes after older rename")
            .0
            .stashes;
        assert_eq!(after_older.len(), 2);
        assert_eq!(after_older[0].hash, older.hash);
        assert!(after_older[0].message.contains("renamed older"));
        assert_eq!(after_older[1].hash, before[0].hash);

        // Git ignores `stash store` when the selected object is already the
        // tip of refs/stash. The handler's protected drop/store path covers
        // that LazyGit case without losing the newest entry.
        let top = after_older[0].clone();
        let _ = management_rename_stash(
            State(state.clone()),
            Json(ManagementStashRenameBody {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                reference: top.reference,
                expected_hash: top.hash.clone(),
                message: "renamed top".to_string(),
            }),
        )
        .await
        .expect("rename newest stash safely");
        let after_top = stashes(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("list stashes after top rename")
            .0
            .stashes;
        assert_eq!(after_top.len(), 2);
        assert_eq!(after_top[0].hash, top.hash);
        assert!(after_top[0].message.contains("renamed top"));
        assert!(git_test_output(
            &repository,
            ["for-each-ref", "--format=%(refname)", "refs/iowb/stash-rename"],
        )
        .trim()
        .is_empty());

        // A new stash changes positional selectors. The immutable hash
        // included by the mobile row must stop this stale rename before it
        // affects a different stash at the same `stash@{N}` position.
        let stale = after_top[1].clone();
        std::fs::write(repository.join("tracked.txt"), "latest stash\n").unwrap();
        git_test_command(&repository, ["stash", "push", "-m", "latest"]);
        let stale_error = management_rename_stash(
            State(state.clone()),
            Json(ManagementStashRenameBody {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                reference: stale.reference,
                expected_hash: stale.hash,
                message: "must not rename another stash".to_string(),
            }),
        )
        .await
        .expect_err("stale stash row must be rejected");
        assert_eq!(stale_error.status, axum::http::StatusCode::CONFLICT);
        let after_stale = stashes(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("list stashes after stale rename rejection")
            .0
            .stashes;
        assert!(after_stale
            .iter()
            .all(|stash| !stash.message.contains("must not rename")));
        assert_eq!(after_stale.len(), 3);

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn management_stash_branch_requires_a_pristine_worktree_and_retains_stash_like_lazygit() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-stash-branch-pristine-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        std::fs::write(repository.join("tracked.txt"), "from stash\n").unwrap();
        git_test_command(&repository, ["stash", "push", "-m", "branch source"]);
        std::fs::write(repository.join("tracked.txt"), "local dirty\n").unwrap();
        std::fs::write(repository.join("untracked.txt"), "local untracked\n").unwrap();

        let state = git_test_app_state(&root).await;
        let error = management_create_branch_from_stash(
            State(state.clone()),
            Json(ManagementStashBranchBody {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                branch: "recover-stash".to_string(),
                reference: "stash@{0}".to_string(),
                expected_hash: None,
                confirm: true,
            }),
        )
        .await
        .expect_err("dirty worktree must block stash branch before checkout");
        assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(current_branch(&repository).await.unwrap(), "main");
        assert!(git(&repository, ["show-ref", "--verify", "refs/heads/recover-stash"])
            .await
            .is_err());
        assert!(git_test_output(&repository, ["stash", "list"]).contains("branch source"));
        assert_eq!(
            std::fs::read_to_string(repository.join("tracked.txt")).unwrap(),
            "local dirty\n"
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("untracked.txt")).unwrap(),
            "local untracked\n"
        );

        // After the user clears their own edits, branch directly at the
        // stash ref. This is LazyGit's actual action (checkout -b), not
        // `git stash branch`: the branch points at the stash commit and the
        // selected stash remains available for later inspection or apply.
        git_test_command(&repository, ["reset", "--hard", "HEAD"]);
        std::fs::remove_file(repository.join("untracked.txt")).unwrap();
        let stash_hash = git_test_output(&repository, ["rev-parse", "stash@{0}"])
            .trim()
            .to_string();
        let response = management_create_branch_from_stash(
            State(state.clone()),
            Json(ManagementStashBranchBody {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                branch: "recover-stash".to_string(),
                reference: "stash@{0}".to_string(),
                expected_hash: None,
                confirm: true,
            }),
        )
        .await
        .expect("create a branch at the stash")
        .0;
        assert_eq!(response.branch.as_deref(), Some("recover-stash"));
        assert_eq!(current_branch(&repository).await.unwrap(), "recover-stash");
        assert_eq!(
            git_test_output(&repository, ["rev-parse", "HEAD"]).trim(),
            stash_hash
        );
        assert!(git_test_output(&repository, ["stash", "list"]).contains("branch source"));
        assert_eq!(
            std::fs::read_to_string(repository.join("tracked.txt")).unwrap(),
            "from stash\n"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stale_stash_hash_blocks_every_mutating_stash_action() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-stale-stash-action-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "tracked.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        std::fs::write(repository.join("tracked.txt"), "original saved change\n").unwrap();
        git_test_command(&repository, ["stash", "push", "-m", "original selection"]);
        let original_hash = git_test_output(&repository, ["rev-parse", "stash@{0}"])
            .trim()
            .to_string();

        // Simulate another client creating a stash while the original row's
        // confirmation sheet remains open. The old visible selector now
        // resolves to a different immutable object.
        std::fs::write(repository.join("tracked.txt"), "newer saved change\n").unwrap();
        git_test_command(&repository, ["stash", "push", "-m", "newer selection"]);
        let current_hash = git_test_output(&repository, ["rev-parse", "stash@{0}"])
            .trim()
            .to_string();
        assert_ne!(original_hash, current_hash);

        let state = git_test_app_state(&root).await;
        let stale_reference = "stash@{0}".to_string();
        let stale_hash = Some(original_hash.clone());
        let scope = || ManagementProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };

        let legacy_apply = apply_stash(
            State(state.clone()),
            Json(StashRefBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                reference: stale_reference.clone(),
                expected_hash: stale_hash.clone(),
            }),
        )
        .await
        .expect_err("a stale stash apply must not target the new top entry");
        assert_eq!(legacy_apply.status, axum::http::StatusCode::CONFLICT);

        let legacy_pop = pop_stash(
            State(state.clone()),
            Json(StashRefBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                reference: stale_reference.clone(),
                expected_hash: stale_hash.clone(),
            }),
        )
        .await
        .expect_err("a stale stash pop must not target the new top entry");
        assert_eq!(legacy_pop.status, axum::http::StatusCode::CONFLICT);

        let legacy_drop = drop_stash(
            State(state.clone()),
            Json(StashRefBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                reference: stale_reference.clone(),
                expected_hash: stale_hash.clone(),
            }),
        )
        .await
        .expect_err("a stale stash drop must not delete the new top entry");
        assert_eq!(legacy_drop.status, axum::http::StatusCode::CONFLICT);

        let advanced_apply = management_apply_stash(
            State(state.clone()),
            Json(ManagementStashApplyBody {
                scope: scope(),
                reference: stale_reference.clone(),
                expected_hash: stale_hash.clone(),
                index: false,
                pop: false,
                confirm: false,
            }),
        )
        .await
        .expect_err("advanced apply must reject a stale stash selection");
        assert_eq!(advanced_apply.status, axum::http::StatusCode::CONFLICT);

        let selective_files = management_restore_stash_files(
            State(state.clone()),
            Json(ManagementStashRestoreFilesBody {
                scope: scope(),
                reference: stale_reference.clone(),
                expected_hash: stale_hash.clone(),
                files: vec!["tracked.txt".to_string()],
                confirm: true,
            }),
        )
        .await
        .expect_err("selective file restore must reject a stale stash selection");
        assert_eq!(selective_files.status, axum::http::StatusCode::CONFLICT);

        let selective_hunks = management_restore_stash_hunks(
            State(state.clone()),
            Json(ManagementStashRestoreHunksBody {
                scope: scope(),
                reference: stale_reference.clone(),
                expected_hash: stale_hash.clone(),
                file: "tracked.txt".to_string(),
                hunk_indexes: vec![0],
                confirm: true,
            }),
        )
        .await
        .expect_err("selective hunk restore must reject a stale stash selection");
        assert_eq!(selective_hunks.status, axum::http::StatusCode::CONFLICT);

        let branch = management_create_branch_from_stash(
            State(state.clone()),
            Json(ManagementStashBranchBody {
                scope: scope(),
                branch: "must-not-exist".to_string(),
                reference: stale_reference,
                expected_hash: stale_hash,
                confirm: true,
            }),
        )
        .await
        .expect_err("stash branch must reject a stale stash selection");
        assert_eq!(branch.status, axum::http::StatusCode::CONFLICT);

        assert_eq!(
            git_test_output(&repository, ["rev-parse", "stash@{0}"]).trim(),
            current_hash
        );
        assert!(git_test_output(&repository, ["stash", "list"]).contains("original selection"));
        assert!(git_test_output(&repository, ["stash", "list"]).contains("newer selection"));
        assert!(git_test_output(&repository, ["status", "--porcelain"])
            .trim()
            .is_empty());
        assert!(git(&repository, ["show-ref", "--verify", "refs/heads/must-not-exist"])
            .await
            .is_err());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn legacy_stash_apply_and_pop_conflicts_have_safe_recovery() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-stash-conflict-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("conflict.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "conflict.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        std::fs::write(repository.join("conflict.txt"), "stashed change\n").unwrap();
        std::fs::write(repository.join("stash-untracked.txt"), "from stash\n").unwrap();
        git_test_command(
            &repository,
            ["stash", "push", "--include-untracked", "-m", "recovery"],
        );
        std::fs::write(repository.join("conflict.txt"), "current branch change\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "current branch change"]);

        let state = git_test_app_state(&root).await;
        // Exercise the ordinary mobile-facing route, not only the shared
        // helper. A conflicting apply must return recoverable state instead
        // of an opaque 400 while Git leaves unmerged index entries behind.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_state = state.clone();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router().with_state(server_state)).await;
        });
        let apply_response = reqwest::Client::new()
            .post(format!("http://{address}/api/git/stash/apply"))
            .json(&serde_json::json!({
                "projectPath": repository.display().to_string(),
                "reference": "stash@{0}",
            }))
            .send()
            .await
            .expect("request legacy stash apply");
        assert!(apply_response.status().is_success());
        let apply_response = apply_response
            .json::<serde_json::Value>()
            .await
            .expect("decode legacy stash apply");
        assert_eq!(apply_response["operationState"]["operation"], "stash");
        assert_eq!(apply_response["operationState"]["inProgress"], true);
        assert_eq!(apply_response["operationState"]["canContinue"], true);
        assert_eq!(apply_response["operationState"]["canAbort"], true);
        assert_eq!(apply_response["operationState"]["canSkip"], false);
        server.abort();

        let during_apply = operation_state(
            State(state.clone()),
            Query(git_project_query(&repository)),
        )
        .await
        .expect("read stash operation state")
        .0;
        assert_eq!(during_apply.operation, "stash");
        assert!(during_apply.in_progress);
        assert!(repository.join("stash-untracked.txt").exists());
        assert!(git_test_output(&repository, ["status", "--porcelain"])
            .lines()
            .any(|line| line.starts_with("UU conflict.txt")));

        // A remote mutation must not run during any recoverable operation.
        let blocked_push = push(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect_err("push is blocked during stash recovery");
        assert_eq!(blocked_push.status, axum::http::StatusCode::CONFLICT);
        let blocked_remote_pull = management_pull_remote(
            State(state.clone()),
            Json(ManagementRemotePullBody {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                remote: "origin".to_string(),
                remote_branch: "main".to_string(),
                rebase: None,
            }),
        )
        .await
        .expect_err("selected-remote pull is blocked during stash recovery");
        assert_eq!(blocked_remote_pull.status, axum::http::StatusCode::CONFLICT);
        let blocked_remote_push = management_push_remote(
            State(state.clone()),
            Json(ManagementRemotePushBody {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                remote: "origin".to_string(),
                branch: None,
                remote_branch: None,
                set_upstream: false,
                force_with_lease: false,
                force: false,
                confirm: false,
            }),
        )
        .await
        .expect_err("selected-remote push is blocked during stash recovery");
        assert_eq!(blocked_remote_push.status, axum::http::StatusCode::CONFLICT);

        std::fs::write(repository.join("conflict.txt"), "resolved apply\n").unwrap();
        git_test_command(&repository, ["add", "conflict.txt"]);
        let finished = continue_operation(
            State(state.clone()),
            Json(git_project_body(&repository)),
        )
        .await
        .expect("finish legacy stash apply recovery")
        .0;
        assert!(finished.success);
        assert!(
            !finished
                .operation_state
                .expect("finished stash state")
                .in_progress
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("conflict.txt")).unwrap(),
            "resolved apply\n"
        );
        assert!(git_test_output(&repository, ["stash", "list"])
            .contains("recovery"));

        // Return to the clean pre-apply state and cover the legacy Pop path.
        git_test_command(&repository, ["reset", "--hard", "HEAD"]);
        std::fs::remove_file(repository.join("stash-untracked.txt")).unwrap();
        let popped = pop_stash(
            State(state.clone()),
            Json(StashRefBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                reference: "stash@{0}".to_string(),
                expected_hash: None,
            }),
        )
        .await
        .expect("legacy stash pop conflict is recoverable")
        .0;
        assert_eq!(
            popped
                .operation_state
                .as_ref()
                .expect("stash pop operation state")
                .operation,
            "stash"
        );
        assert!(repository.join("stash-untracked.txt").exists());

        let abandoned = abort_operation(
            State(state.clone()),
            Json(git_project_body(&repository)),
        )
        .await
        .expect("abandon legacy stash pop recovery")
        .0;
        assert!(abandoned.success);
        assert!(
            !abandoned
                .operation_state
                .expect("abandoned stash state")
                .in_progress
        );
        assert_eq!(
            std::fs::read_to_string(repository.join("conflict.txt")).unwrap(),
            "current branch change\n"
        );
        assert!(!repository.join("stash-untracked.txt").exists());
        // Git itself retains a conflicted pop entry. The recovery endpoint
        // intentionally preserves that safety property.
        assert!(git_test_output(&repository, ["stash", "list"])
            .contains("recovery"));

        // The richer stash dialog uses the management endpoint. Cover both
        // apply and pop there too, so the legacy and advanced controls cannot
        // silently diverge in their conflict lifecycle.
        let advanced_apply = management_apply_stash(
            State(state.clone()),
            Json(ManagementStashApplyBody {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                reference: "stash@{0}".to_string(),
                expected_hash: None,
                index: false,
                pop: false,
                confirm: false,
            }),
        )
        .await
        .expect("advanced stash apply conflict is recoverable")
        .0;
        assert_eq!(
            advanced_apply
                .operation_state
                .as_ref()
                .expect("advanced apply state")
                .operation,
            "stash"
        );
        let _ = abort_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("abandon advanced stash apply recovery");
        assert!(!repository.join("stash-untracked.txt").exists());

        let advanced_pop = management_apply_stash(
            State(state.clone()),
            Json(ManagementStashApplyBody {
                scope: ManagementProjectScope {
                    project: Some(repository.display().to_string()),
                    project_path: None,
                    repository_id: None,
                },
                reference: "stash@{0}".to_string(),
                expected_hash: None,
                index: false,
                pop: true,
                confirm: true,
            }),
        )
        .await
        .expect("advanced stash pop conflict is recoverable")
        .0;
        assert_eq!(
            advanced_pop
                .operation_state
                .as_ref()
                .expect("advanced pop state")
                .operation,
            "stash"
        );
        std::fs::write(repository.join("conflict.txt"), "resolved advanced pop\n").unwrap();
        git_test_command(&repository, ["add", "conflict.txt"]);
        let _ = continue_operation(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("finish advanced stash pop recovery");
        assert!(git_test_output(&repository, ["stash", "list"])
            .contains("recovery"));
        git_test_command(&repository, ["reset", "--hard", "HEAD"]);
        std::fs::remove_file(repository.join("stash-untracked.txt")).unwrap();

        // The clean-worktree precondition avoids Git's partial stash apply:
        // it otherwise can restore untracked files before rejecting an
        // overlapping tracked path.
        std::fs::write(repository.join("conflict.txt"), "dirty local\n").unwrap();
        let dirty_apply = apply_stash(
            State(state.clone()),
            Json(StashRefBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                reference: "stash@{0}".to_string(),
                expected_hash: None,
            }),
        )
        .await
        .expect_err("dirty worktree blocks stash apply before partial mutation");
        assert_eq!(dirty_apply.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            std::fs::read_to_string(repository.join("conflict.txt")).unwrap(),
            "dirty local\n"
        );
        assert!(!repository.join("stash-untracked.txt").exists());

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn noninteractive_git_helper_accepts_interactive_rebase_without_a_terminal_editor() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-interactive-editor-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        git_test_command(&root, ["init", "--initial-branch=main"]);
        configure_test_repository(&root);
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git_test_command(&root, ["add", "base.txt"]);
        git_test_command(&root, ["commit", "-m", "base"]);
        std::fs::write(root.join("topic.txt"), "topic\n").unwrap();
        git_test_command(&root, ["add", "topic.txt"]);
        git_test_command(&root, ["commit", "-m", "topic"]);

        git_with_noninteractive_editor(&root, ["rebase", "--interactive", "HEAD~1"])
            .await
            .expect("interactive rebase uses the no-op sequence editor");
        assert_eq!(
            git_test_output(&root, ["log", "-1", "--format=%s"]).trim(),
            "topic",
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn role_aware_remote_url_updates_preserve_other_urls_and_redact_existing_credentials() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-remote-url-roles-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);

        git_test_command(
            &repository,
            [
                "remote",
                "add",
                "team/review",
                "https://token@example.test/owner/fetch-primary.git",
            ],
        );
        git_test_command(
            &repository,
            [
                "remote",
                "set-url",
                "--add",
                "team/review",
                "https://example.test/owner/fetch-secondary.git",
            ],
        );
        git_test_command(
            &repository,
            [
                "remote",
                "set-url",
                "--add",
                "--push",
                "team/review",
                "https://token@example.test/owner/push-primary.git",
            ],
        );
        git_test_command(
            &repository,
            [
                "remote",
                "set-url",
                "--add",
                "--push",
                "team/review",
                "ssh://git@example.test/owner/push-secondary.git",
            ],
        );
        git_test_command(
            &repository,
            [
                "remote",
                "add",
                "fallback",
                "https://example.test/owner/fallback.git",
            ],
        );

        let state = git_test_app_state(&root).await;
        let scope = || ManagementProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };

        let remotes = management_remotes(
            State(state.clone()),
            Query(ManagementRemotesQuery { scope: scope() }),
        )
        .await
        .expect("list remote URL roles")
        .0;
        let review = remotes
            .remotes
            .iter()
            .find(|remote| remote.name == "team/review")
            .expect("slash-named remote");
        assert!(review.has_explicit_push_urls);
        assert_eq!(review.fetch_urls.len(), 2);
        assert_eq!(review.push_urls.len(), 2);
        assert!(review.fetch_urls[0].contains("<redacted>@"));
        assert!(review.push_urls[0].contains("<redacted>@"));
        assert!(!review.fetch_urls.join("\n").contains("token@example"));
        let fallback = remotes
            .remotes
            .iter()
            .find(|remote| remote.name == "fallback")
            .expect("fallback remote");
        assert!(!fallback.has_explicit_push_urls);
        assert_eq!(fallback.push_urls, fallback.fetch_urls);

        let fetch_update = management_update_remote_url(
            State(state.clone()),
            Json(ManagementRemoteUrlUpdateBody {
                scope: scope(),
                remote: "team/review".to_string(),
                role: ManagementRemoteUrlRole::Fetch,
                index: Some(1),
                add: false,
                url: "https://example.test/owner/fetch-updated.git".to_string(),
            }),
        )
        .await
        .expect("replace only second fetch URL")
        .0;
        assert_eq!(fetch_update.remote_name.as_deref(), Some("team/review"));
        assert_eq!(
            git_test_output(
                &repository,
                ["config", "--local", "--get-all", "remote.team/review.url"],
            )
            .lines()
            .collect::<Vec<_>>(),
            vec![
                "https://token@example.test/owner/fetch-primary.git",
                "https://example.test/owner/fetch-updated.git",
            ],
        );
        assert_eq!(
            git_test_output(
                &repository,
                ["config", "--local", "--get-all", "remote.team/review.pushurl"],
            )
            .lines()
            .collect::<Vec<_>>(),
            vec![
                "https://token@example.test/owner/push-primary.git",
                "ssh://git@example.test/owner/push-secondary.git",
            ],
        );

        let push_update = management_update_remote_url(
            State(state.clone()),
            Json(ManagementRemoteUrlUpdateBody {
                scope: scope(),
                remote: "team/review".to_string(),
                role: ManagementRemoteUrlRole::Push,
                index: Some(1),
                add: false,
                url: "ssh://git@example.test/owner/push-updated.git".to_string(),
            }),
        )
        .await
        .expect("replace only second explicit push URL")
        .0;
        assert_eq!(push_update.remote_name.as_deref(), Some("team/review"));
        assert_eq!(
            git_test_output(
                &repository,
                ["config", "--local", "--get-all", "remote.team/review.url"],
            )
            .lines()
            .collect::<Vec<_>>(),
            vec![
                "https://token@example.test/owner/fetch-primary.git",
                "https://example.test/owner/fetch-updated.git",
            ],
        );
        assert_eq!(
            git_test_output(
                &repository,
                ["config", "--local", "--get-all", "remote.team/review.pushurl"],
            )
            .lines()
            .collect::<Vec<_>>(),
            vec![
                "https://token@example.test/owner/push-primary.git",
                "ssh://git@example.test/owner/push-updated.git",
            ],
        );

        let explicit_push = management_update_remote_url(
            State(state.clone()),
            Json(ManagementRemoteUrlUpdateBody {
                scope: scope(),
                remote: "fallback".to_string(),
                role: ManagementRemoteUrlRole::Push,
                index: None,
                add: true,
                url: "ssh://git@example.test/owner/fallback-push.git".to_string(),
            }),
        )
        .await
        .expect("add explicit push URL")
        .0;
        assert_eq!(explicit_push.remote_name.as_deref(), Some("fallback"));
        assert_eq!(
            git_test_output(
                &repository,
                ["config", "--local", "--get-all", "remote.fallback.url"],
            )
            .trim(),
            "https://example.test/owner/fallback.git",
        );
        assert_eq!(
            git_test_output(
                &repository,
                ["config", "--local", "--get-all", "remote.fallback.pushurl"],
            )
            .trim(),
            "ssh://git@example.test/owner/fallback-push.git",
        );
        let stale_index = management_update_remote_url(
            State(state.clone()),
            Json(ManagementRemoteUrlUpdateBody {
                scope: scope(),
                remote: "team/review".to_string(),
                role: ManagementRemoteUrlRole::Fetch,
                index: Some(9),
                add: false,
                url: "https://example.test/owner/never-written.git".to_string(),
            }),
        )
        .await
        .expect_err("stale URL index is rejected");
        assert_eq!(stale_index.status, axum::http::StatusCode::BAD_REQUEST);

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn management_plain_force_push_requires_confirmation_and_replaces_the_selected_remote_ref() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-management-plain-force-push-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        let remote = root.join("remote.git");
        let replica = root.join("replica");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("base.txt"), "base\n").unwrap();
        git_test_command(&repository, ["add", "base.txt"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        let remote_path = remote.display().to_string();
        let replica_path = replica.display().to_string();
        git_test_command(
            &root,
            ["init", "--bare", "--initial-branch=main", remote_path.as_str()],
        );
        git_test_command(&repository, ["remote", "add", "origin", remote_path.as_str()]);
        git_test_command(&repository, ["push", "-u", "origin", "main"]);

        // Advance the remote from a separate clone so the local branch now
        // needs an explicit history replacement rather than a fast-forward.
        git_test_command(&root, ["clone", remote_path.as_str(), replica_path.as_str()]);
        configure_test_repository(&replica);
        std::fs::write(replica.join("remote.txt"), "remote advance\n").unwrap();
        git_test_command(&replica, ["add", "remote.txt"]);
        git_test_command(&replica, ["commit", "-m", "remote advance"]);
        git_test_command(&replica, ["push", "origin", "main"]);
        let remote_head_before_force = git_test_output(&remote, ["rev-parse", "refs/heads/main"])
            .trim()
            .to_string();

        // Rewrite locally from the common base, creating the normal
        // non-fast-forward case that `git push --force` is intended to
        // resolve.  Keep the local remote-tracking ref stale on purpose.
        std::fs::write(repository.join("local.txt"), "local replacement\n").unwrap();
        git_test_command(&repository, ["add", "local.txt"]);
        git_test_command(&repository, ["commit", "-m", "local replacement"]);
        let local_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let state = git_test_app_state(&root).await;
        let scope = || ManagementProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };
        let body = |force_with_lease, force, confirm| ManagementRemotePushBody {
            scope: scope(),
            remote: "origin".to_string(),
            branch: Some("main".to_string()),
            remote_branch: Some("main".to_string()),
            set_upstream: false,
            force_with_lease,
            force,
            confirm,
        };

        let unconfirmed = management_push_remote(
            State(state.clone()),
            Json(body(false, true, false)),
        )
        .await
        .expect_err("plain force push must require explicit confirmation");
        assert_eq!(unconfirmed.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(
            git_test_output(&remote, ["rev-parse", "refs/heads/main"])
                .trim(),
            remote_head_before_force,
            "a rejected force push must not alter the remote ref",
        );

        let incompatible = management_push_remote(
            State(state.clone()),
            Json(body(true, true, true)),
        )
        .await
        .expect_err("plain force and force-with-lease cannot be combined");
        assert_eq!(incompatible.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(
            git_test_output(&remote, ["rev-parse", "refs/heads/main"])
                .trim(),
            remote_head_before_force,
            "an invalid force policy must not alter the remote ref",
        );

        let pushed = management_push_remote(
            State(state.clone()),
            Json(body(false, true, true)),
        )
        .await
        .expect("confirmed plain force push")
        .0;
        assert!(pushed.success);
        assert_eq!(pushed.remote_name.as_deref(), Some("origin"));
        assert_eq!(pushed.remote_branch.as_deref(), Some("main"));
        assert_eq!(
            git_test_output(&remote, ["rev-parse", "refs/heads/main"])
                .trim(),
            local_head,
            "plain force push replaces exactly the requested remote branch",
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn management_branch_upstream_can_be_set_then_unset_without_leaving_tracking_config() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-unset-upstream-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        let remote = root.join("remote.git");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "initial"]);
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                remote.display().to_string().as_str(),
            ],
        );
        git_test_command(
            &repository,
            ["remote", "add", "origin", remote.display().to_string().as_str()],
        );
        // Fetch creates the remote-tracking ref required by Git's explicit
        // `branch --set-upstream-to origin/main` command without itself
        // configuring an upstream for main.
        git_test_command(&repository, ["push", "origin", "main"]);
        git_test_command(&repository, ["fetch", "origin"]);

        let state = git_test_app_state(&root).await;
        let scope = || ManagementProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };
        let set = management_set_branch_upstream(
            State(state.clone()),
            Json(ManagementBranchUpstreamBody {
                scope: scope(),
                branch: "main".to_string(),
                remote: "origin".to_string(),
                remote_branch: "main".to_string(),
            }),
        )
        .await
        .expect("set upstream")
        .0;
        assert!(set.success);
        assert_eq!(
            git_test_output(&repository, ["config", "--local", "--get", "branch.main.remote"])
                .trim(),
            "origin"
        );
        assert_eq!(
            git_test_output(&repository, ["config", "--local", "--get", "branch.main.merge"])
                .trim(),
            "refs/heads/main"
        );

        let unset = management_unset_branch_upstream(
            State(state.clone()),
            Json(ManagementBranchUnsetUpstreamBody {
                scope: scope(),
                branch: "main".to_string(),
            }),
        )
        .await
        .expect("unset upstream")
        .0;
        assert!(unset.success);
        assert_eq!(unset.branch.as_deref(), Some("main"));
        assert!(
            git(&repository, ["config", "--local", "--get", "branch.main.remote"])
                .await
                .is_err(),
            "unset upstream must remove branch.<name>.remote"
        );
        assert!(
            git(&repository, ["config", "--local", "--get", "branch.main.merge"])
                .await
                .is_err(),
            "unset upstream must remove branch.<name>.merge"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configured_push_targets_follow_git_push_remote_and_upstream_without_same_name_fallback() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-configured-push-target-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        let upstream = root.join("upstream.git");
        let publish_remote = root.join("publish.git");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);

        let upstream_path = upstream.display().to_string();
        let publish_path = publish_remote.display().to_string();
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                upstream_path.as_str(),
            ],
        );
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                publish_path.as_str(),
            ],
        );
        git_test_command(&repository, ["remote", "add", "origin", upstream_path.as_str()]);
        git_test_command(
            &repository,
            ["remote", "add", "publish", publish_path.as_str()],
        );
        git_test_command(&repository, ["push", "-u", "origin", "main"]);
        // Ensure the configured differently named tracking branch exists so
        // Git can resolve @{upstream} and status can report normal ahead /
        // behind state too.
        git_test_command(&repository, ["push", "origin", "main:review/mobile"]);
        git_test_command(&repository, ["fetch", "origin"]);
        git_test_command(&repository, ["checkout", "-b", "feature/mobile"]);
        std::fs::write(repository.join("feature.txt"), "feature\n").unwrap();
        git_test_command(&repository, ["add", "feature.txt"]);
        git_test_command(&repository, ["commit", "-m", "feature"]);
        let feature_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(
            &repository,
            ["config", "--local", "branch.feature/mobile.remote", "origin"],
        );
        git_test_command(
            &repository,
            [
                "config",
                "--local",
                "branch.feature/mobile.merge",
                "refs/heads/review/mobile",
            ],
        );
        git_test_command(&repository, ["config", "--local", "push.default", "upstream"]);

        let state = git_test_app_state(&root).await;
        let scope = || ManagementProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };
        let push_body = |remote: &str| ManagementRemotePushBody {
            scope: scope(),
            remote: remote.to_string(),
            branch: None,
            remote_branch: None,
            set_upstream: false,
            force_with_lease: false,
            force: false,
            confirm: false,
        };

        let upstream_status = remote_status(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("read configured upstream push target")
            .0;
        assert_eq!(upstream_status.remote_name.as_deref(), Some("origin"));
        assert_eq!(upstream_status.remote_branch.as_deref(), Some("origin/review/mobile"));
        assert_eq!(upstream_status.push_remote_name.as_deref(), Some("origin"));
        assert_eq!(upstream_status.push_remote_branch.as_deref(), Some("review/mobile"));
        assert_eq!(upstream_status.push_mode.as_deref(), Some("upstream"));

        let pushed_upstream = management_push_remote(State(state.clone()), Json(push_body("origin")))
            .await
            .expect("ordinary push uses the configured differently named upstream")
            .0;
        assert_eq!(pushed_upstream.remote_name.as_deref(), Some("origin"));
        assert_eq!(pushed_upstream.remote_branch.as_deref(), Some("review/mobile"));
        assert_eq!(
            git_test_output(&upstream, ["rev-parse", "refs/heads/review/mobile"])
                .trim(),
            feature_head,
        );
        assert!(
            git(&upstream, ["show-ref", "--verify", "--quiet", "refs/heads/feature/mobile"])
                .await
                .is_err(),
            "the endpoint must not silently create a same-named origin branch"
        );

        // A branch-level pushRemote takes priority over remote.pushDefault.
        // With simple mode, Git deliberately treats the dedicated push remote
        // as a current-branch destination instead of treating the fetch
        // upstream as the target.
        git_test_command(
            &repository,
            ["config", "--local", "branch.feature/mobile.pushRemote", "publish"],
        );
        git_test_command(&repository, ["config", "--local", "remote.pushDefault", "origin"]);
        git_test_command(&repository, ["config", "--local", "push.default", "simple"]);
        let push_remote_status = remote_status(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("read branch pushRemote target")
            .0;
        assert_eq!(push_remote_status.push_remote_name.as_deref(), Some("publish"));
        assert_eq!(push_remote_status.push_remote_branch.as_deref(), Some("feature/mobile"));
        assert_eq!(push_remote_status.push_mode.as_deref(), Some("simple"));

        let wrong_remote = management_push_remote(State(state.clone()), Json(push_body("origin")))
            .await
            .expect_err("a blank target cannot silently redirect an explicit different remote");
        assert_eq!(wrong_remote.status, StatusCode::BAD_REQUEST);
        assert!(wrong_remote.body.error.contains("configured push target"));
        let pushed_publish = management_push_remote(State(state.clone()), Json(push_body("publish")))
            .await
            .expect("blank target resolves to the configured pushRemote destination")
            .0;
        assert_eq!(pushed_publish.remote_name.as_deref(), Some("publish"));
        assert_eq!(pushed_publish.remote_branch.as_deref(), Some("feature/mobile"));
        assert_eq!(
            git_test_output(&publish_remote, ["rev-parse", "refs/heads/feature/mobile"])
                .trim(),
            feature_head,
        );

        // The branch setting is absent on this new branch. remote.pushDefault
        // remains an explicit destination even with several configured
        // remotes, so the legacy Publish endpoint may safely honor it.
        git_test_command(&repository, ["checkout", "-b", "publish-default"]);
        std::fs::write(repository.join("publish.txt"), "publish\n").unwrap();
        git_test_command(&repository, ["add", "publish.txt"]);
        git_test_command(&repository, ["commit", "-m", "publish default"]);
        let publish_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["config", "--local", "push.default", "current"]);
        let publish_status = remote_status(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("read remote.pushDefault target")
            .0;
        assert_eq!(publish_status.push_remote_name.as_deref(), Some("origin"));
        assert_eq!(publish_status.push_remote_branch.as_deref(), Some("publish-default"));

        // Change the explicit global/default target after the branch-specific
        // precedence check above. This must win over the ambiguous pair of
        // configured remotes without picking origin.
        git_test_command(&repository, ["config", "--local", "remote.pushDefault", "publish"]);
        let default_status = remote_status(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("refresh remote.pushDefault target")
            .0;
        assert_eq!(default_status.push_remote_name.as_deref(), Some("publish"));
        assert_eq!(default_status.push_remote_branch.as_deref(), Some("publish-default"));
        let published = publish(
            State(state.clone()),
            Json(BranchBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                branch: "publish-default".to_string(),
            }),
        )
        .await
        .expect("publish honors remote.pushDefault with multiple remotes")
        .0;
        assert_eq!(published.remote_name.as_deref(), Some("publish"));
        assert_eq!(published.remote_branch.as_deref(), Some("publish-default"));
        assert_eq!(
            git_test_output(&publish_remote, ["rev-parse", "refs/heads/publish-default"])
                .trim(),
            publish_head,
        );
        assert!(
            git(&upstream, ["show-ref", "--verify", "--quiet", "refs/heads/publish-default"])
                .await
                .is_err(),
            "legacy Publish must not silently choose origin when remote.pushDefault names publish"
        );

        // `push.autoSetupRemote=true` is a Git/LazyGit ordinary-push
        // behavior, not the dedicated Publish workflow. With no upstream or
        // explicit push remote, Git selects origin, pushes the current branch,
        // then records that destination as the new tracking upstream.
        git_test_command(&repository, ["config", "--local", "--unset", "remote.pushDefault"]);
        git_test_command(&repository, ["checkout", "-b", "auto-setup"]);
        std::fs::write(repository.join("auto-setup.txt"), "auto setup\n").unwrap();
        git_test_command(&repository, ["add", "auto-setup.txt"]);
        git_test_command(&repository, ["commit", "-m", "auto setup push"]);
        let auto_setup_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["config", "--local", "push.default", "simple"]);
        git_test_command(
            &repository,
            ["config", "--local", "push.autoSetupRemote", "true"],
        );
        let auto_setup_status = remote_status(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("read auto-setup ordinary push target")
            .0;
        assert_eq!(auto_setup_status.push_remote_name.as_deref(), Some("origin"));
        assert_eq!(auto_setup_status.push_remote_branch.as_deref(), Some("auto-setup"));
        assert_eq!(auto_setup_status.push_mode.as_deref(), Some("simple"));
        assert!(auto_setup_status.push_sets_upstream);
        let auto_setup_push = push(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("ordinary push honors push.autoSetupRemote")
            .0;
        assert!(auto_setup_push.success);
        assert_eq!(
            git_test_output(&upstream, ["rev-parse", "refs/heads/auto-setup"])
                .trim(),
            auto_setup_head,
        );
        assert_eq!(
            git_test_output(&repository, ["config", "--local", "--get", "branch.auto-setup.remote"])
                .trim(),
            "origin",
        );
        assert_eq!(
            git_test_output(&repository, ["config", "--local", "--get", "branch.auto-setup.merge"])
                .trim(),
            "refs/heads/auto-setup",
        );

        // The same auto-setup side effect applies when a dedicated push
        // default selects a fork/review remote. It must not fall back to the
        // fetch upstream or leave the branch untracked after the first push.
        git_test_command(&repository, ["checkout", "-b", "auto-push-default"]);
        std::fs::write(repository.join("auto-push-default.txt"), "push default\n").unwrap();
        git_test_command(&repository, ["add", "auto-push-default.txt"]);
        git_test_command(&repository, ["commit", "-m", "auto push default"]);
        let auto_push_default_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["config", "--local", "remote.pushDefault", "publish"]);
        let auto_push_default_status = remote_status(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("read auto-setup push-default target")
            .0;
        assert_eq!(auto_push_default_status.push_remote_name.as_deref(), Some("publish"));
        assert_eq!(
            auto_push_default_status.push_remote_branch.as_deref(),
            Some("auto-push-default"),
        );
        assert!(auto_push_default_status.push_sets_upstream);
        let auto_push_default = push(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("ordinary push tracks the configured push default when auto setup is enabled")
            .0;
        assert!(auto_push_default.success);
        assert_eq!(
            git_test_output(&publish_remote, ["rev-parse", "refs/heads/auto-push-default"])
                .trim(),
            auto_push_default_head,
        );
        assert_eq!(
            git_test_output(
                &repository,
                ["config", "--local", "--get", "branch.auto-push-default.remote"],
            )
            .trim(),
            "publish",
        );

        // Git also honors push.autoSetupRemote for upstream mode.  The
        // configured push default selects the network destination first;
        // because this fresh branch has no tracking config yet, the ordinary
        // push must then record that selected destination as its upstream.
        git_test_command(&repository, ["checkout", "-b", "auto-upstream-default"]);
        std::fs::write(
            repository.join("auto-upstream-default.txt"),
            "upstream auto setup\n",
        )
        .unwrap();
        git_test_command(&repository, ["add", "auto-upstream-default.txt"]);
        git_test_command(&repository, ["commit", "-m", "auto upstream push"]);
        let auto_upstream_default_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["config", "--local", "push.default", "upstream"]);
        let auto_upstream_default_status = remote_status(
            State(state.clone()),
            Query(git_project_query(&repository)),
        )
        .await
        .expect("read upstream-mode auto-setup target")
        .0;
        assert_eq!(
            auto_upstream_default_status.push_remote_name.as_deref(),
            Some("publish"),
        );
        assert_eq!(
            auto_upstream_default_status.push_remote_branch.as_deref(),
            Some("auto-upstream-default"),
        );
        assert_eq!(auto_upstream_default_status.push_mode.as_deref(), Some("upstream"));
        assert!(auto_upstream_default_status.push_sets_upstream);
        let auto_upstream_default = push(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("ordinary upstream-mode push tracks the configured push default")
            .0;
        assert!(auto_upstream_default.success);
        assert_eq!(
            git_test_output(
                &publish_remote,
                ["rev-parse", "refs/heads/auto-upstream-default"],
            )
            .trim(),
            auto_upstream_default_head,
        );
        assert_eq!(
            git_test_output(
                &repository,
                [
                    "config",
                    "--local",
                    "--get",
                    "branch.auto-upstream-default.remote",
                ],
            )
            .trim(),
            "publish",
        );
        git_test_command(&repository, ["config", "--local", "--unset", "remote.pushDefault"]);

        // A remote-specific push refspec overrides push.default. Preserve
        // its one explicit current-branch mapping rather than collapsing it
        // to a same-named branch; autoSetupRemote must not create tracking
        // when Git is following the refspec.
        git_test_command(&repository, ["checkout", "-b", "refspec-topic"]);
        std::fs::write(repository.join("refspec.txt"), "configured refspec\n").unwrap();
        git_test_command(&repository, ["add", "refspec.txt"]);
        git_test_command(&repository, ["commit", "-m", "configured refspec"]);
        let refspec_head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(&repository, ["config", "--local", "push.default", "nothing"]);
        git_test_command(
            &repository,
            [
                "config",
                "--local",
                "remote.origin.push",
                "refs/heads/refspec-topic:refs/heads/review/refspec-topic",
            ],
        );
        let refspec_status = remote_status(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("read configured refspec push target")
            .0;
        assert_eq!(refspec_status.push_remote_name.as_deref(), Some("origin"));
        assert_eq!(
            refspec_status.push_remote_branch.as_deref(),
            Some("review/refspec-topic"),
        );
        assert_eq!(refspec_status.push_mode.as_deref(), Some("remote.<name>.push"));
        assert!(!refspec_status.push_sets_upstream);
        let refspec_push = push(State(state.clone()), Json(git_project_body(&repository)))
            .await
            .expect("ordinary push honors one configured current-branch refspec")
            .0;
        assert!(refspec_push.success);
        assert_eq!(
            git_test_output(&upstream, ["rev-parse", "refs/heads/review/refspec-topic"])
                .trim(),
            refspec_head,
        );
        assert!(
            git(&repository, ["config", "--local", "--get", "branch.refspec-topic.remote"])
                .await
                .is_err(),
            "a configured remote refspec must not invent an upstream"
        );
        git_test_command(
            &repository,
            [
                "config",
                "--local",
                "--add",
                "remote.origin.push",
                "refs/heads/main:refs/heads/also-main",
            ],
        );
        let multi_refspec_status = remote_status(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("multi-refspec configuration remains inspectable")
            .0;
        assert!(multi_refspec_status.push_remote_name.is_none());
        assert!(
            multi_refspec_status
                .push_target_message
                .as_deref()
                .is_some_and(|message| message.contains("2 configured push refspecs")),
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_status_stays_usable_when_saved_push_tracking_config_is_malformed() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-push-status-config-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("README.md"), "base\n").unwrap();
        git_test_command(&repository, ["add", "README.md"]);
        git_test_command(&repository, ["commit", "-m", "base"]);
        git_test_command(
            &repository,
            ["remote", "add", "origin", root.join("origin.git").display().to_string().as_str()],
        );
        // A stale hand-edited merge setting should not blank the whole Git
        // workspace just because optional Push guidance cannot resolve it.
        git_test_command(&repository, ["config", "--local", "branch.main.remote", "origin"]);
        git_test_command(
            &repository,
            ["config", "--local", "branch.main.merge", "refs/tags/not-a-branch"],
        );

        let state = git_test_app_state(&root).await;
        let status = remote_status(State(state.clone()), Query(git_project_query(&repository)))
            .await
            .expect("Git remote status remains available for malformed optional push config")
            .0;
        assert!(status.has_remote);
        assert!(!status.has_upstream);
        assert!(status.push_remote_name.is_none());
        assert_eq!(status.push_mode.as_deref(), Some("unavailable"));
        assert!(
            status
                .push_target_message
                .as_deref()
                .is_some_and(|message| message.contains("Saved push configuration needs attention")),
        );

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn management_branch_remote_tag_history_submodule_and_worktree_handlers_use_real_git() {
        let root = std::env::temp_dir().join(format!(
            "iowb-git-management-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = root.join("repository");
        let remote = root.join("remote.git");
        let release_remote = root.join("release-remote.git");
        let source = root.join("submodule-source");
        std::fs::create_dir_all(&repository).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        git_test_command(&repository, ["init", "--initial-branch=main"]);
        configure_test_repository(&repository);
        std::fs::write(repository.join("history.txt"), "first\n").unwrap();
        git_test_command(&repository, ["add", "history.txt"]);
        git_test_command(&repository, ["commit", "-m", "first history"]);
        let base = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                remote.display().to_string().as_str(),
            ],
        );
        git_test_command(
            &root,
            [
                "init",
                "--bare",
                "--initial-branch=main",
                release_remote.display().to_string().as_str(),
            ],
        );
        git_test_command(
            &repository,
            ["remote", "add", "origin", remote.display().to_string().as_str()],
        );
        git_test_command(
            &repository,
            [
                "remote",
                "add",
                "release",
                release_remote.display().to_string().as_str(),
            ],
        );
        git_test_command(&repository, ["push", "-u", "origin", "main"]);

        git_test_command(&source, ["init", "--initial-branch=main"]);
        configure_test_repository(&source);
        std::fs::write(source.join("README.md"), "submodule\n").unwrap();
        git_test_command(&source, ["add", "README.md"]);
        git_test_command(&source, ["commit", "-m", "submodule source"]);

        let state = git_test_app_state(&root).await;
        let scope = || ManagementProjectScope {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        };

        let remotes = management_remotes(
            State(state.clone()),
            Query(ManagementRemotesQuery { scope: scope() }),
        )
        .await
        .expect("list remotes")
        .0;
        assert!(remotes.remotes.iter().any(|remote| remote.name == "origin"));

        let _ = management_rename_branch(
            State(state.clone()),
            Json(ManagementBranchRenameBody {
                scope: scope(),
                old_branch: "main".to_string(),
                new_branch: "trunk".to_string(),
            }),
        )
        .await
        .expect("rename current branch");
        assert_eq!(current_branch(&repository).await.unwrap(), "trunk");
        let ambiguous_publish = publish(
            State(state.clone()),
            Json(BranchBody {
                project: Some(repository.display().to_string()),
                project_path: None,
                repository_id: None,
                branch: "trunk".to_string(),
            }),
        )
        .await
        .expect_err("legacy publish rejects an ambiguous remote destination");
        assert_eq!(ambiguous_publish.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(ambiguous_publish
            .body
            .error
            .contains("Choose an explicit remote"));
        let _ = management_push_remote(
            State(state.clone()),
            Json(ManagementRemotePushBody {
                scope: scope(),
                remote: "origin".to_string(),
                branch: Some("trunk".to_string()),
                remote_branch: None,
                set_upstream: true,
                force_with_lease: false,
                force: false,
                confirm: false,
            }),
        )
        .await
        .expect("publish renamed branch");
        let _ = management_set_branch_upstream(
            State(state.clone()),
            Json(ManagementBranchUpstreamBody {
                scope: scope(),
                branch: "trunk".to_string(),
                remote: "origin".to_string(),
                remote_branch: "trunk".to_string(),
            }),
        )
        .await
        .expect("set explicit branch upstream");
        let _ = management_create_branch_from_ref(
            State(state.clone()),
            Json(ManagementBranchCreateFromRefBody {
                scope: scope(),
                branch: "review".to_string(),
                start_point: base.clone(),
            }),
        )
        .await
        .expect("create branch from historical commit");
        assert!(git(&repository, ["show-ref", "--verify", "refs/heads/review"])
            .await
            .is_ok());
        let _ = management_fetch_remote(
            State(state.clone()),
            Json(ManagementRemoteFetchBody {
                scope: scope(),
                remote: "origin".to_string(),
                prune: true,
                tags: Some(true),
            }),
        )
        .await
        .expect("fetch selected remote");
        let pulled = management_pull_remote(
            State(state.clone()),
            Json(ManagementRemotePullBody {
                scope: scope(),
                remote: "origin".to_string(),
                remote_branch: "trunk".to_string(),
                rebase: Some("false".to_string()),
            }),
        )
        .await
        .expect("pull selected remote branch")
        .0;
        assert!(pulled.success);
        assert_eq!(pulled.remote_name.as_deref(), Some("origin"));
        assert_eq!(pulled.remote_branch.as_deref(), Some("trunk"));

        let _ = management_create_tag_at_target(
            State(state.clone()),
            Json(ManagementTagCreateBody {
                scope: scope(),
                tag: "v1.0.0".to_string(),
                message: Some("release one".to_string()),
                target: Some(base.clone()),
                force: false,
                confirm: false,
            }),
        )
        .await
        .expect("create annotated tag at historical commit");
        let pushed_tag = management_push_remote_tag(
            State(state.clone()),
            Json(ManagementRemoteTagPushBody {
                scope: scope(),
                // Verify this never falls back to origin when several
                // destinations are configured: releases can intentionally
                // publish somewhere different from branch collaboration.
                remote: "release".to_string(),
                tag: "v1.0.0".to_string(),
            }),
        )
        .await
        .expect("push tag to selected remote")
        .0;
        assert_eq!(pushed_tag.remote_name.as_deref(), Some("release"));
        assert!(git(&release_remote, ["show-ref", "--verify", "refs/tags/v1.0.0"])
            .await
            .is_ok());
        assert!(git(&remote, ["show-ref", "--verify", "refs/tags/v1.0.0"])
            .await
            .is_err());
        let _ = management_delete_remote_tag(
            State(state.clone()),
            Json(ManagementRemoteTagDeleteBody {
                scope: scope(),
                remote: "release".to_string(),
                tag: "v1.0.0".to_string(),
                confirm: true,
            }),
        )
        .await
        .expect("delete remote tag");
        assert!(git(&release_remote, ["show-ref", "--verify", "refs/tags/v1.0.0"])
            .await
            .is_err());

        std::fs::write(repository.join("history.txt"), "second\n").unwrap();
        git_test_command(&repository, ["commit", "-am", "second history"]);
        let head = git_test_output(&repository, ["rev-parse", "HEAD"])
            .trim()
            .to_string();
        let history = management_file_log(
            State(state.clone()),
            Query(ManagementFileLogQuery {
                scope: scope(),
                file: "history.txt".to_string(),
                reference: None,
                limit: Some(10),
            }),
        )
        .await
        .expect("load file history")
        .0;
        assert!(history.commits.len() >= 2);
        let blame = management_blame(
            State(state.clone()),
            Query(ManagementBlameQuery {
                scope: scope(),
                file: "history.txt".to_string(),
                reference: None,
                start_line: Some(1),
                end_line: Some(1),
            }),
        )
        .await
        .expect("blame history file")
        .0;
        assert_eq!(blame.lines.len(), 1);
        let reflog = management_reflog(
            State(state.clone()),
            Query(ManagementReflogQuery {
                scope: scope(),
                reference: None,
                limit: Some(10),
            }),
        )
        .await
        .expect("read reflog")
        .0;
        assert!(!reflog.entries.is_empty());
        let compared = management_compare_refs(
            State(state.clone()),
            Json(ManagementCompareRefsBody {
                scope: scope(),
                base: base.clone(),
                head: head.clone(),
                file: Some("history.txt".to_string()),
            }),
        )
        .await
        .expect("compare historical commits")
        .0;
        assert!(compared.diff.contains("+second"));
        let _ = management_restore_file(
            State(state.clone()),
            Json(ManagementRestoreFileBody {
                scope: scope(),
                file: "history.txt".to_string(),
                source: Some(base),
                stage: false,
                confirm: true,
            }),
        )
        .await
        .expect("restore historical file to working tree");
        assert_eq!(std::fs::read_to_string(repository.join("history.txt")).unwrap(), "first\n");
        git_test_command(&repository, ["restore", "--worktree", "history.txt"]);

        // Add with a file-protocol-only test helper, then exercise server-side
        // sync/update without granting the server any special test protocol.
        git_test_command_with_file_protocol(
            &repository,
            [
                "submodule",
                "add",
                source.display().to_string().as_str(),
                "modules/source",
            ],
        );
        git_test_command(&repository, ["commit", "-am", "add managed submodule"]);
        let _ = management_sync_submodule(
            State(state.clone()),
            Json(ManagementSubmoduleSyncBody {
                scope: scope(),
                path: Some("modules/source".to_string()),
                recursive: false,
            }),
        )
        .await
        .expect("sync configured submodule");
        let _ = management_update_submodule(
            State(state.clone()),
            Json(ManagementSubmoduleUpdateBody {
                scope: scope(),
                path: Some("modules/source".to_string()),
                init: false,
                recursive: false,
                remote: false,
                confirm: true,
            }),
        )
        .await
        .expect("update configured submodule");

        let linked = root.join("linked-worktree");
        let _ = management_add_worktree(
            State(state.clone()),
            Json(ManagementWorktreeAddBody {
                scope: scope(),
                path: linked.display().to_string(),
                branch: Some("linked-topic".to_string()),
                start_point: Some(head),
                create_branch: true,
                detach: false,
                force: false,
                confirm: true,
            }),
        )
        .await
        .expect("create linked worktree");
        let worktrees = management_worktrees(
            State(state.clone()),
            Query(ManagementWorktreesQuery { scope: scope() }),
        )
        .await
        .expect("list linked worktrees")
        .0;
        assert!(worktrees
            .worktrees
            .iter()
            .any(|worktree| worktree.path == linked.display().to_string()));
        let _ = management_remove_worktree(
            State(state.clone()),
            Json(ManagementWorktreeRemoveBody {
                scope: scope(),
                path: linked.display().to_string(),
                force: false,
                confirm: true,
            }),
        )
        .await
        .expect("remove linked worktree");
        assert!(!linked.exists());
        let _ = management_prune_worktrees(
            State(state.clone()),
            Json(ManagementWorktreePruneBody {
                scope: scope(),
                confirm: true,
            }),
        )
        .await
        .expect("prune stale worktree records");

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    async fn git_test_app_state(root: &std::path::Path) -> iowb_core::AppState {
        let config_dir = root.join("config");
        iowb_core::AppState::initialize(iowb_core::AppConfig {
            host: "127.0.0.1".parse().unwrap(),
            port: 0,
            config_dir: config_dir.clone(),
            database_path: config_dir.join("test.db"),
            workspace_root: root.to_path_buf(),
            auth_required: false,
            local_token: None,
            otp_secret: None,
            max_sessions: 4,
            max_scan_depth: 2,
            max_file_read_bytes: 1024 * 1024,
        })
        .await
        .unwrap()
    }

    fn git_project_body(repository: &std::path::Path) -> ProjectBody {
        ProjectBody {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
            allow_workspace_init: false,
        }
    }

    fn git_project_query(repository: &std::path::Path) -> ProjectQuery {
        ProjectQuery {
            project: Some(repository.display().to_string()),
            project_path: None,
            repository_id: None,
        }
    }

    fn configure_test_repository(repository: &std::path::Path) {
        git_test_command(repository, ["config", "user.name", "Catalog Test"]);
        git_test_command(
            repository,
            ["config", "user.email", "catalog@example.test"],
        );
        // Keep Git behavior in these real-command tests independent from
        // developer/global signing defaults. Tests that exercise signing set
        // their repository-local value explicitly.
        git_test_command(repository, ["config", "commit.gpgSign", "false"]);
        git_test_command(repository, ["config", "tag.gpgSign", "false"]);
    }

    fn git_test_output<I, S>(cwd: &std::path::Path, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        String::from_utf8(git_test_output_bytes(cwd, args)).unwrap()
    }

    fn git_test_output_bytes<I, S>(cwd: &std::path::Path, args: I) -> Vec<u8>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git test command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn git_test_command<I, S>(cwd: &std::path::Path, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git test command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_test_command_with_file_protocol<I, S>(cwd: &std::path::Path, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = std::process::Command::new("git")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git test command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn redacts_remote_credentials_from_git_diagnostics() {
        let diagnostic = redact_git_diagnostic(
            "Command failed: git remote add origin https://gio:secret-token@example.test/repo.git\n\
             fatal: Authentication failed for 'https://another-secret@example.test/repo.git/?access_token=query-secret#fragment-secret'",
        );

        assert!(!diagnostic.contains("secret-token"));
        assert!(!diagnostic.contains("another-secret"));
        assert!(!diagnostic.contains("query-secret"));
        assert!(!diagnostic.contains("fragment-secret"));
        assert!(diagnostic.contains("https://<redacted>@example.test/repo.git"));
        assert!(diagnostic.contains("https://<redacted>@example.test/repo.git/'"));
    }
