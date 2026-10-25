mod support;

use serde_json::{json, Value};
use std::process::{Command, Output};
use support::SessionsDirectory;

fn run(directory: &SessionsDirectory, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codex-quota"))
        .args(["--sessions-dir", directory.path().to_str().unwrap()])
        .args(arguments)
        .env("TZ", "Asia/Kolkata")
        .output()
        .unwrap()
}

fn event(limits: Value) -> String {
    json!({"timestamp":"2026-10-24T09:06:39.567Z", "type":"event_msg",
        "payload":{"type":"token_count","rate_limits":limits}})
    .to_string()
}

#[test]
fn reports_threshold_decision_in_text_and_json_with_local_reset_time() {
    let directory = SessionsDirectory::new();
    directory.write(
        "2026/10/24/rollout-cli.jsonl",
        &event(json!({
            "primary":{"used_percent":99, "window_minutes":10080, "resets_at":4102444800_i64},
            "credits":{"has_credits":true,"unlimited":false,"balance":"62500.0000000000"}
        })),
    );
    let output = run(&directory, &["--json"]);
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "blocked");
    assert_eq!(report["primary"]["used_percent"], 99.0);
    assert_eq!(report["primary"]["window_length"], "1w");
    assert_eq!(report["primary"]["resets_at"], "2100-01-01T05:30:00+05:30");
    assert_eq!(report["timestamp_source"], "event");
    assert!(report["age_seconds"].as_u64().is_some());
    let text = run(&directory, &[]);
    assert_eq!(text.status.code(), Some(1));
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("99%"));
    assert!(text.contains("62500.0000000000"));
    assert!(text.contains("+05:30"));
    assert_eq!(
        run(&directory, &["--threshold", "100"]).status.code(),
        Some(0)
    );
}

#[test]
fn stale_usage_is_unknown_warns_and_allows_both_output_formats() {
    let directory = SessionsDirectory::new();
    directory.write(
        "2026/10/24/rollout-stale.jsonl",
        &event(json!({
            "primary":{"used_percent":100,"resets_at":1},
            "secondary":{"used_percent":100,"resets_at":2},
            "rate_limit_reached_type":"primary"
        })),
    );
    let json = run(&directory, &["--json"]);
    assert_eq!(json.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(report["primary"]["used_percent"], Value::Null);
    assert_eq!(report["primary"]["measured_used_percent"], 100.0);
    assert_eq!(report["secondary"]["stale"], true);
    assert_eq!(report["warnings"].as_array().unwrap().len(), 2);
    let text = run(&directory, &[]);
    assert_eq!(text.status.code(), Some(0));
    assert!(String::from_utf8(text.stdout)
        .unwrap()
        .contains("usage unknown"));
    assert!(String::from_utf8(text.stderr).unwrap().contains("STALE"));
}

#[test]
fn missing_and_empty_measurements_use_distinct_no_data_exit_code() {
    let directory = SessionsDirectory::new();
    let output = run(&directory, &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("No quota data"));
    let json = run(&directory, &["--json"]);
    assert_eq!(json.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(report["status"], "no_data");
    directory.write(
        "2026/10/24/rollout-empty.jsonl",
        &event(json!({"primary":null,"credits":{}})),
    );
    let output = run(&directory, &["--json"]);
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "no_data");
}

#[test]
fn explicit_reached_type_blocks_without_window_usage() {
    let directory = SessionsDirectory::new();
    directory.write(
        "2026/10/24/rollout-reached.jsonl",
        &event(json!({"rate_limit_reached_type":"credits"})),
    );
    for arguments in [&[][..], &["--json"][..]] {
        assert_eq!(run(&directory, arguments).status.code(), Some(1));
    }
}

#[test]
fn invalid_arguments_cannot_silently_allow_work() {
    let directory = SessionsDirectory::new();
    for arguments in [
        vec!["--threshold", "NaN"],
        vec!["--threshold", "inf"],
        vec!["--threshold", "-1"],
        vec!["--threshold", "101"],
        vec!["--threshold"],
        vec!["--unknown"],
    ] {
        let mut arguments = arguments;
        arguments.insert(0, "--json");
        let output = run(&directory, &arguments);
        assert_eq!(output.status.code(), Some(3), "{arguments:?}");
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["status"], "error");
    }
}

#[test]
fn invalid_input_and_io_errors_have_error_exit_code_in_json() {
    let directory = SessionsDirectory::new();
    let path = directory.write("not-a-directory", "");
    let output = run(
        &directory,
        &["--json", "--sessions-dir", path.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(3));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains("cannot list sessions"));
    directory.write(
        "2026/10/24/rollout-invalid.jsonl",
        &event(json!({"primary":{"used_percent":"99"}})),
    );
    let output = run(&directory, &["--json"]);
    assert_eq!(output.status.code(), Some(3));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains("invalid rate_limits"));
}

#[test]
fn codex_home_is_honored_and_explicit_directory_has_precedence() {
    let home = SessionsDirectory::new();
    home.write(
        "sessions/2026/10/24/rollout-env.jsonl",
        &event(json!({"primary":{"used_percent":100}})),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_codex-quota"))
        .env("CODEX_HOME", home.path())
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let empty = SessionsDirectory::new();
    let output = Command::new(env!("CARGO_BIN_EXE_codex-quota"))
        .env("CODEX_HOME", home.path())
        .arg("--sessions-dir")
        .arg(empty.path())
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let output = Command::new(env!("CARGO_BIN_EXE_codex-quota"))
        .env("CODEX_HOME", "")
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn help_and_version_work_without_a_sessions_directory() {
    for argument in ["--help", "--version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_codex-quota"))
            .env("CODEX_HOME", "")
            .arg(argument)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(String::from_utf8(output.stdout)
            .unwrap()
            .contains("codex-quota"));
    }
}
