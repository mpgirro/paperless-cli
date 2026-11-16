use std::io::Write;
use std::process::{Command, Output, Stdio};

use httpmock::Method::POST;
use httpmock::MockServer;
use serde_json::{json, Value};
use tempfile::tempdir;

fn run_bulk(server: Option<&MockServer>, args: &[&str], stdin: Option<&str>) -> Output {
    let dir = tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_paperless"));
    command
        .env("PAPERLESS_CONFIG_PATH", dir.path().join("config.toml"))
        .env("PAPERLESS_SESSION_PATH", dir.path().join("session.toml"))
        .env_remove("PAPERLESS_URL")
        .env_remove("PAPERLESS_TOKEN")
        .args(["--json", "document", "bulk"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(server) = server {
        command
            .env("PAPERLESS_URL", server.base_url())
            .env("PAPERLESS_TOKEN", "env-token");
    }
    let mut child = command.spawn().unwrap();
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    drop(child.stdin.take());
    child.wait_with_output().unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_sends(args: &[&str], body: Value) {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/api/documents/bulk_edit/")
            .header("authorization", "Token env-token")
            .json_body(body);
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"result":"OK"}"#);
    });

    let output = run_bulk(Some(&server), args, None);

    assert_success(&output);
    mock.assert();
}

#[test]
fn single_add_tag_uses_add_tag() {
    assert_sends(
        &["1", "2", "--add-tag", "5"],
        json!({"documents": [1, 2], "method": "add_tag", "parameters": {"tag": 5}}),
    );
}

#[test]
fn single_remove_tag_uses_remove_tag() {
    assert_sends(
        &["1", "--remove-tag", "7"],
        json!({"documents": [1], "method": "remove_tag", "parameters": {"tag": 7}}),
    );
}

#[test]
fn add_and_remove_tags_use_one_modify_tags_call() {
    assert_sends(
        &["1", "2", "--add-tag", "5", "--remove-tag", "7"],
        json!({
            "documents": [1, 2],
            "method": "modify_tags",
            "parameters": {"add_tags": [5], "remove_tags": [7]},
        }),
    );
}

#[test]
fn several_added_tags_use_modify_tags() {
    assert_sends(
        &["1", "--add-tag", "5", "--add-tag", "6"],
        json!({
            "documents": [1],
            "method": "modify_tags",
            "parameters": {"add_tags": [5, 6], "remove_tags": []},
        }),
    );
}

#[test]
fn correspondent_sets_and_clears() {
    assert_sends(
        &["1", "--correspondent", "3"],
        json!({"documents": [1], "method": "set_correspondent", "parameters": {"correspondent": 3}}),
    );
    assert_sends(
        &["1", "--correspondent", "none"],
        json!({"documents": [1], "method": "set_correspondent", "parameters": {"correspondent": null}}),
    );
}

#[test]
fn document_type_sets_and_clears() {
    assert_sends(
        &["1", "--type", "4"],
        json!({"documents": [1], "method": "set_document_type", "parameters": {"document_type": 4}}),
    );
    assert_sends(
        &["1", "--type", "none"],
        json!({"documents": [1], "method": "set_document_type", "parameters": {"document_type": null}}),
    );
}

#[test]
fn reprocess_sends_reprocess() {
    assert_sends(
        &["1", "2", "--reprocess"],
        json!({"documents": [1, 2], "method": "reprocess", "parameters": {}}),
    );
}

#[test]
fn set_permissions_sends_owner_and_both_actions() {
    assert_sends(
        &[
            "1",
            "--set-permissions",
            "--owner",
            "2",
            "--view-users",
            "3,4",
            "--view-groups",
            "5",
            "--change-users",
            "6",
        ],
        json!({
            "documents": [1],
            "method": "set_permissions",
            "parameters": {
                "set_permissions": {
                    "view": {"users": [3, 4], "groups": [5]},
                    "change": {"users": [6], "groups": []},
                },
                "owner": 2,
                "merge": false,
            },
        }),
    );
    assert_sends(
        &["1", "--set-permissions", "--owner", "none"],
        json!({
            "documents": [1],
            "method": "set_permissions",
            "parameters": {
                "set_permissions": {
                    "view": {"users": [], "groups": []},
                    "change": {"users": [], "groups": []},
                },
                "owner": null,
                "merge": false,
            },
        }),
    );
}

#[test]
fn dry_run_prints_every_body_in_order_without_config() {
    let output = run_bulk(
        None,
        &[
            "1",
            "--reprocess",
            "--type",
            "4",
            "--correspondent",
            "none",
            "--set-permissions",
            "--owner",
            "2",
            "--remove-tag",
            "7",
            "--dry-run",
        ],
        None,
    );

    assert_success(&output);
    let methods: Vec<String> = serde_json::from_slice::<Value>(&output.stdout)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|body| body["method"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        methods,
        [
            "remove_tag",
            "set_correspondent",
            "set_document_type",
            "set_permissions",
            "reprocess"
        ]
    );
}

#[test]
fn dash_reads_ids_from_stdin() {
    let output = run_bulk(
        None,
        &["-", "9", "--reprocess", "--dry-run"],
        Some("1\n2 3\n"),
    );

    assert_success(&output);
    let bodies: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(bodies[0]["documents"], json!([1, 2, 3, 9]));
}

#[test]
fn rejects_invalid_input_without_requests() {
    let server = MockServer::start();
    let any = server.mock(|_, then| {
        then.status(500);
    });
    for (args, stdin) in [
        (vec!["1"], None),
        (vec!["-", "--reprocess"], Some("1 x")),
        (vec!["-", "--reprocess"], Some("")),
        (vec!["1", "--set-permissions"], None),
        (vec!["1", "--view-users", "2", "--reprocess"], None),
        (vec!["1", "--correspondent", "abc"], None),
    ] {
        let output = run_bulk(Some(&server), &args, stdin);
        assert!(!output.status.success(), "{args:?} was accepted");
    }
    any.assert_calls(0);
}

#[test]
fn failure_reports_the_methods_already_applied() {
    let server = MockServer::start();
    let tags = server.mock(|when, then| {
        when.method(POST)
            .path("/api/documents/bulk_edit/")
            .json_body_includes(r#"{"method":"add_tag"}"#);
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"result":"OK"}"#);
    });
    let reprocess = server.mock(|when, then| {
        when.method(POST)
            .path("/api/documents/bulk_edit/")
            .json_body_includes(r#"{"method":"reprocess"}"#);
        then.status(400).body(r#"{"documents":["bad"]}"#);
    });

    let output = run_bulk(Some(&server), &["1", "--add-tag", "5", "--reprocess"], None);

    assert!(!output.status.success());
    tags.assert();
    reprocess.assert();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("reprocess"), "{stderr}");
    assert!(stderr.contains("add_tag"), "{stderr}");
}
