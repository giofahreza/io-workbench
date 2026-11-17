// Keep the regression bounded even against the old nonterminating redactor.
// The child is this isolated test executable, never the live server, and all
// values are synthetic. Only this created child is terminated on timeout.
#[test]
fn transcript_redactor_finishes_bounded_synthetic_contract() {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "agentic_board::tests::transcript_redactor_synthetic_child_contract",
            "--nocapture",
        ])
        .env("IOWB_TEST_REDACTOR_CHILD", "synthetic-contract")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let timed_out = loop {
        if child.try_wait().unwrap().is_some() {
            break false;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            break true;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let output = child.wait_with_output().unwrap();
    assert!(
        !timed_out,
        "synthetic transcript redaction exceeded three seconds"
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn transcript_redactor_synthetic_child_contract() {
    if std::env::var("IOWB_TEST_REDACTOR_CHILD").as_deref() != Ok("synthetic-contract") {
        return;
    }
    for (input, expected) in [
        (
            "authorization: synthetic-value",
            "authorization: [REDACTED]",
        ),
        ("authorization: [REDACTED]", "authorization: [REDACTED]"),
        (
            "api_key=synthetic-one api_key=synthetic-two",
            "api_key=[REDACTED] api_key=[REDACTED]",
        ),
        (
            "PASSWORD=synthetic-one secret: synthetic-two",
            "PASSWORD=[REDACTED] secret: [REDACTED]",
        ),
        (
            "🔒 password=synthetic-value résumé",
            "🔒 password=[REDACTED] résumé",
        ),
        ("password", "password"),
        ("password:   ", "password:   "),
        ("No additional authorization", "No additional authorization"),
        (
            "password=, password=synthetic-value",
            "password=, password=[REDACTED]",
        ),
    ] {
        let actual = redact_transcript_text(input);
        assert_eq!(actual, expected, "synthetic input: {input}");
        assert_eq!(
            redact_transcript_text(&actual),
            actual,
            "redaction must be idempotent"
        );
    }
    // Preserve the existing conservative text policy; ordinary marker prose
    // must terminate and retain the surrounding scope constraint.
    let prose = "This is a technical file allowance, not scope authorization: an unqualified user ban on packages/dependencies also forbids test/dev packages and npm/npx downloads.";
    let redacted = redact_transcript_text(prose);
    assert!(redacted.starts_with("This is a technical file allowance, not scope authorization:"));
    assert!(redacted.ends_with("unqualified user ban on packages/dependencies also forbids test/dev packages and npm/npx downloads."));
    assert_eq!(redact_transcript_text(&redacted), redacted);
    assert_eq!(
        redact_secret_after_marker("synthetic text", ""),
        "synthetic text"
    );
    let structured = json!({"api_key": "synthetic-only", "content": "password=synthetic-only"});
    let result = redact_transcript_value(&structured);
    assert_eq!(
        result,
        json!({"api_key": "[REDACTED]", "content": "password=[REDACTED]"})
    );
    assert_eq!(redact_transcript_value(&result), result);
}
