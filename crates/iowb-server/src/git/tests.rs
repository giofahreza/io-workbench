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
        assert!(validate_remote_url("--upload-pack=evil").is_err());
        assert!(validate_remote_url("https://example.test/repo\n--upload-pack=evil").is_err());
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

        let commit_response = client
            .post(format!("http://{address}/api/git/commit"))
            .json(&serde_json::json!({
                "projectPath": workspace.display().to_string(),
                "repositoryId": "repository:one",
                "message": "selected repository commit",
                "files": ["selected.txt"]
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

    fn configure_test_repository(repository: &std::path::Path) {
        git_test_command(repository, ["config", "user.name", "Catalog Test"]);
        git_test_command(
            repository,
            ["config", "user.email", "catalog@example.test"],
        );
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
             fatal: Authentication failed for 'https://another-secret@example.test/repo.git/'",
        );

        assert!(!diagnostic.contains("secret-token"));
        assert!(!diagnostic.contains("another-secret"));
        assert!(diagnostic.contains("https://<redacted>@example.test/repo.git"));
        assert!(diagnostic.contains("https://<redacted>@example.test/repo.git/'"));
    }
