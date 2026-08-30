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
