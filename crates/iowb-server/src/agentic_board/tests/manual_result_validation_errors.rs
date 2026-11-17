const OBSERVED_VALIDATION_RESULT: &str = "pass — all 6 acceptance criteria satisfied: Low/Medium/High each render a visible row with the matching badge text and data-priority; empty and whitespace-only submissions are rejected with the same visible error hint and never create a row; the title field is trimmed before storage (the leading/trailing-space probe '   Trimmed me   ' stored exactly 'Trimmed me').";
const OBSERVED_LITERAL_HTML_RESULT: &str = "pass — all 7 acceptance criteria satisfied: counts update on every add/toggle/delete mutation; counters reflect the full task set, never the filtered view; full page reload restores every task (ids, titles, priorities, done flags, order) and counters; localStorage is keyed under the namespaced versioned `pocket-tasks:v1` and contains valid JSON; both<script>alert(1)</script> and <img src=x onerror=alert(1)> titles render as literal text via textContent without executing scripts or loading images; six distinct corrupt localStorage payloads all reload to a non-crashing empty state (0/0/0).";

#[test]
fn manual_result_validation_error_accepts_actual_observed_negative_path() {
    assert!(manual_test_result_is_successful(OBSERVED_VALIDATION_RESULT));
}

#[test]
fn manual_result_validation_error_requires_observation_and_explicit_success() {
    for result in [
        "Pass: the expected validation error was shown for an empty title.",
        "All checks passed; the expected input-validation error appeared.",
        "Pass: invalid input was rejected with a visible error message.",
        "PASSED: empty submissions were rejected with the same VISIBLE ERROR HINT.",
    ] {
        assert!(manual_test_result_is_successful(result), "{result}");
    }
    for result in [
        "The expected validation error was shown for an empty title.",
        "Empty submissions were rejected with the same visible error hint.",
        "Pass: the expected validation error.",
        "Pass: the expected validation error should be visible.",
        "Pass: the expected validation error will be shown.",
        "Pass: invalid input should be rejected with a visible error hint.",
        "Pass: an error message was shown.",
        "Pass: the unexpected validation error appeared.",
    ] {
        assert!(!manual_test_result_is_successful(result), "{result}");
    }
}

#[test]
fn manual_result_validation_error_does_not_mask_other_errors() {
    for problem in [
        "unexpected runtime error on save",
        "an error occurred on save",
        "save threw TypeError",
        "one step failed",
        "one step is blocked",
        "one step is pending",
        "the save flow could not be verified",
        "SUMMARY:FAIL",
    ] {
        let result = format!("{OBSERVED_VALIDATION_RESULT}; {problem}");
        assert!(!manual_test_result_is_successful(&result), "{result}");
    }
}

#[test]
fn manual_result_validation_error_rejects_missing_expected_feedback() {
    for result in [
        "Pass: the expected validation error was not visible.",
        "Pass: the expected validation error was not observed.",
        "Pass: the expected validation error wasn't shown.",
        "Pass: the expected validation error wasn’t shown.",
        "Pass: empty input was not rejected with a visible error hint.",
        "Pass: the expected validation error was missing but previously shown.",
        "Pass: the expected validation error did not appear; another message was visible.",
        "Pass: empty input was rejected with a visible error hint that was incorrect.",
        "Pass: empty input was rejected but the visible error hint was absent.",
        "Pass: empty input was rejected; the visible error hint was missing.",
        "Pass: an unexpected visible error hint appeared after invalid input was rejected.",
    ] {
        assert!(!manual_test_result_is_successful(result), "{result}");
    }
}

#[test]
fn manual_result_validation_error_negation_is_local_to_each_problem() {
    for result in [
        "Pass: no errors on add; error on save.",
        "Pass: without errors on add; an error on save.",
        "Pass: no failures on add; one check failed on save.",
        "Pass: without failures on add; failure on save.",
        "Pass: no; error on save.",
        "Pass: without. Error on save.",
    ] {
        assert!(!manual_test_result_is_successful(result), "{result}");
    }
    for result in [
        "Verified successfully with no failures.",
        "Pass: no errors on add; no errors on save.",
        "All checks passed without errors and without failures.",
    ] {
        assert!(manual_test_result_is_successful(result), "{result}");
    }
}

#[test]
fn manual_result_validation_error_accepts_actual_inert_html_observation() {
    assert!(manual_test_result_is_successful(
        OBSERVED_LITERAL_HTML_RESULT
    ));
}

#[test]
fn manual_result_validation_error_distinguishes_identifiers_from_exception_reports() {
    for result in [
        "Pass: <img src=x onerror=alert(1)> titles render as literal text.",
        "Pass: onerror is rendered as textContent.",
        "Pass: errorHint is visible for invalid input.",
    ] {
        assert!(manual_test_result_is_successful(result), "{result}");
    }
    for exception in [
        "Error",
        "errors",
        "TypeError",
        "AssertionError",
        "ReferenceError",
        "SyntaxError",
        "RangeError",
        "URIError",
        "EvalError",
        "AggregateError",
        "InternalError",
        "NetworkError",
        "TimeoutError",
        "RuntimeError",
        "ValueError",
        "AbortError",
        "SecurityError",
        "NotAllowedError",
    ] {
        let result = format!("Pass: save produced {exception}.");
        assert!(!manual_test_result_is_successful(&result), "{result}");
    }
    let result = format!("{OBSERVED_LITERAL_HTML_RESULT}; error on reload.");
    assert!(!manual_test_result_is_successful(&result));
    assert!(!manual_test_result_is_successful(
        "Pass: 保存—error on reload."
    ));
}

#[test]
fn manual_result_validation_error_preserves_assertion_red_baseline_rejection() {
    assert!(!manual_test_result_is_successful(
        "pass — full red baseline captured; all behaviors represented by failing tests, each failing via a real AssertionError"
    ));
}
