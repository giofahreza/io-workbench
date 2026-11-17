#[test]
fn validation_summary_separates_recovered_failures_from_latest_gate() {
    let summary = validation_summary(&[
        json!({"passed": false, "commands": [{"exitCode": 1}]}),
        json!({"passed": true, "commands": [{"exitCode": 0}]}),
    ]);
    assert_eq!(summary["runs"], 2);
    assert_eq!(summary["passed"], 1);
    assert_eq!(summary["failed"], 1);
    assert_eq!(summary["failedGates"], 1);
    assert_eq!(summary["latestPassed"], true);
    assert_eq!(summary["latestGatePassed"], true);
    assert_eq!(summary["latestGateOutcome"], "passed");
    assert_eq!(summary["commands"], 2);
}

#[test]
fn validation_summary_keeps_expected_red_visible_after_command_telemetry() {
    let summary = validation_summary(&[
        json!({"taskId": "tests", "passed": false, "gatePassed": true,
            "expectedRedAccepted": true, "commands": [{"exitCode": 1}]}),
        json!({"taskId": "tests", "passed": true, "command": "npm test"}),
    ]);
    assert_eq!(summary["failed"], 1); // Raw history remains available.
    assert_eq!(summary["failedGates"], 0);
    assert_eq!(summary["acceptedExpectedRed"], 1);
    assert_eq!(summary["latestPassed"], true); // Legacy telemetry is unchanged.
    assert_eq!(summary["latestGatePassed"], true);
    assert_eq!(summary["latestGateOutcome"], "expected_red");
}

#[test]
fn validation_summary_does_not_count_copied_command_verdicts_as_separate_gates() {
    let summary = validation_summary(&[
        json!({"taskId": "tests", "passed": false, "commands": [{"exitCode": 1}]}),
        json!({"taskId": "tests", "passed": false, "command": "npm test"}),
        json!({"taskId": "tests", "passed": false, "command": "read report"}),
        json!({"taskId": "implementation", "passed": true, "commands": [{"exitCode": 0}]}),
        json!({"taskId": "implementation", "passed": true, "command": "npm test"}),
    ]);
    assert_eq!(summary["runs"], 5);
    assert_eq!(summary["failed"], 3);
    assert_eq!(summary["failedGates"], 1);
    assert_eq!(summary["passedGates"], 1);
    assert_eq!(summary["latestGateOutcome"], "passed");
}

#[test]
fn validation_summary_reports_remaining_red_and_explicit_gate_failure() {
    let red = json!({"passed": false, "gatePassed": true,
        "remainingRedAccepted": {"baselineTaskId": "tests"}});
    let summary = validation_summary(&[red.clone()]);
    assert_eq!(summary["acceptedRemainingRed"], 1);
    assert_eq!(summary["latestGateOutcome"], "remaining_red");
    let summary = validation_summary(&[
        red,
        json!({"passed": true, "gatePassed": false, "expectedRedAccepted": true}),
    ]);
    assert_eq!(summary["latestGatePassed"], false);
    assert_eq!(summary["latestGateOutcome"], "failed");
    assert_eq!(summary["failedGates"], 1);
    assert_eq!(summary["acceptedExpectedRed"], 0);
}

#[test]
fn validation_summary_unknown_outcomes_are_not_failures_or_successes() {
    let summary = validation_summary(&[
        json!({}),
        json!({"passed": null}),
        json!({"passed": "true"}),
        json!({"passed": true, "gatePassed": null}),
    ]);
    assert_eq!(summary["failed"], 0);
    assert_eq!(summary["unknown"], 3);
    assert_eq!(summary["passedGates"], 0);
    assert_eq!(summary["failedGates"], 0);
    assert_eq!(summary["unknownGates"], 4);
    assert_eq!(summary["latestGatePassed"], Value::Null);
    assert_eq!(summary["latestGateOutcome"], "unknown");
    let empty = validation_summary(&[]);
    assert_eq!(empty["runs"], 0);
    assert_eq!(empty["latestGatePassed"], Value::Null);
    assert_eq!(empty["latestGateOutcome"], Value::Null);
}

#[test]
fn validation_summary_does_not_reuse_another_tasks_gate_or_override_staged_legacy_entries() {
    let summary = validation_summary(&[
        json!({"taskId": "old", "passed": true, "gatePassed": true}),
        json!({"taskId": "new", "command": "npm test"}),
    ]);
    assert_eq!(summary["latestGateOutcome"], "unknown");
    let summary = validation_summary(&[
        json!({"taskId": "same", "passed": true, "commands": ["npm test"]}),
        json!({"taskId": "same", "passed": false, "stage": "final", "command": "npm test"}),
    ]);
    assert_eq!(summary["latestStage"], "final");
    assert_eq!(summary["latestGateOutcome"], "failed");
}
