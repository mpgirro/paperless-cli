use std::io::Write;
use std::process::{Command, Output, Stdio};

use httpmock::Method::POST;
use httpmock::MockServer;
use serde_json::{json, Value};
use tempfile::tempdir;

fn run_bulk(server: Option<&MockServer>, args: &[&str], stdin: Option<&str>) -> Output {
    run_paperless(
        server,
        &[&["--json", "document", "bulk"], args].concat(),
        stdin,
    )
}

fn run_paperless(server: Option<&MockServer>, args: &[&str], stdin: Option<&str>) -> Output {
    let dir = tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_paperless"));
    command
        .env("PAPERLESS_CONFIG_PATH", dir.path().join("config.toml"))
        .env("PAPERLESS_SESSION_PATH", dir.path().join("session.toml"))
        .env_remove("PAPERLESS_URL")
        .env_remove("PAPERLESS_TOKEN")
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

fn ok(then: httpmock::Then) {
    then.status(200)
        .header("content-type", "application/json")
        .body(r#"{"result":"OK"}"#);
}

fn assert_sends_to(path: &str, args: &[&str], body: Value) {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path(path)
            .header("authorization", "Token env-token")
            .json_body(body);
        ok(then);
    });

    let output = run_bulk(Some(&server), args, None);

    assert_success(&output);
    mock.assert();
}

fn assert_sends(args: &[&str], body: Value) {
    assert_sends_to("/api/documents/bulk_edit/", args, body);
}

fn sent_methods(output: &Output) -> Vec<String> {
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    envelope["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["method"].as_str().unwrap().to_string())
        .collect()
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
fn duplicate_document_and_tag_ids_are_sent_once_in_order() {
    assert_sends(
        &[
            "3",
            "1",
            "3",
            "--add-tag",
            "6",
            "--add-tag",
            "5",
            "--add-tag",
            "6",
        ],
        json!({
            "documents": [3, 1],
            "method": "modify_tags",
            "parameters": {"add_tags": [6, 5], "remove_tags": []},
        }),
    );
    assert_sends(
        &["1", "1", "--add-tag", "5", "--add-tag", "5"],
        json!({"documents": [1], "method": "add_tag", "parameters": {"tag": 5}}),
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
fn reprocess_uses_the_reprocess_endpoint() {
    assert_sends_to(
        "/api/documents/reprocess/",
        &["1", "2", "--reprocess"],
        json!({"documents": [1, 2]}),
    );
}

#[test]
fn set_permissions_sends_only_the_given_lists() {
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
                    "change": {"users": [6]},
                },
                "owner": 2,
                "merge": false,
            },
        }),
    );
    assert_sends(
        &[
            "1",
            "--set-permissions",
            "--owner",
            "2",
            "--view-groups",
            "3,4",
        ],
        json!({
            "documents": [1],
            "method": "set_permissions",
            "parameters": {
                "set_permissions": {"view": {"groups": [3, 4]}},
                "owner": 2,
                "merge": false,
            },
        }),
    );
}

#[test]
fn set_permissions_owner_only_and_explicit_clear() {
    assert_sends(
        &["1", "--set-permissions", "--owner", "none"],
        json!({
            "documents": [1],
            "method": "set_permissions",
            "parameters": {"set_permissions": {}, "owner": null, "merge": false},
        }),
    );
    assert_sends(
        &[
            "1",
            "--set-permissions",
            "--owner",
            "2",
            "--change-groups",
            "none",
        ],
        json!({
            "documents": [1],
            "method": "set_permissions",
            "parameters": {
                "set_permissions": {"change": {"groups": []}},
                "owner": 2,
                "merge": false,
            },
        }),
    );
}

#[test]
fn several_operations_are_sent_in_order() {
    let server = MockServer::start();
    let bulk_edit = server.mock(|when, then| {
        when.method(POST).path("/api/documents/bulk_edit/");
        ok(then);
    });
    let reprocess = server.mock(|when, then| {
        when.method(POST).path("/api/documents/reprocess/");
        ok(then);
    });

    let output = run_bulk(
        Some(&server),
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
        ],
        None,
    );

    assert_success(&output);
    bulk_edit.assert_calls(4);
    reprocess.assert();
    assert_eq!(
        sent_methods(&output),
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
fn dry_run_prints_every_request_in_order_without_config() {
    let output = run_bulk(
        None,
        &["1", "--reprocess", "--add-tag", "5", "--dry-run"],
        None,
    );

    assert_success(&output);
    let operations: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        operations,
        json!([
            {
                "method": "add_tag",
                "path": "documents/bulk_edit/",
                "body": {"documents": [1], "method": "add_tag", "parameters": {"tag": 5}},
            },
            {
                "method": "reprocess",
                "path": "documents/reprocess/",
                "body": {"documents": [1]},
            },
        ])
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
    let operations: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(operations[0]["body"]["documents"], json!([1, 2, 3, 9]));
}

#[test]
fn demo_mode_answers_every_operation() {
    let output = run_paperless(
        None,
        &[
            "--demo",
            "--json",
            "document",
            "bulk",
            "301",
            "--add-tag",
            "1",
            "--reprocess",
        ],
        None,
    );

    assert_success(&output);
    assert_eq!(sent_methods(&output), ["add_tag", "reprocess"]);
}

#[test]
fn rejects_invalid_input_without_requests() {
    let server = MockServer::start();
    let any = server.mock(|_, then| {
        then.status(500);
    });
    for (args, stdin, expected) in [
        (vec!["1"], None, "No bulk operation given."),
        (
            vec!["-", "--reprocess"],
            Some("1 x"),
            "Invalid document ID: x",
        ),
        (vec!["-", "--reprocess"], Some(""), "No document IDs given."),
        (vec!["1", "--set-permissions"], None, "--owner <ID|none>"),
        (
            vec!["1", "--view-users", "2", "--reprocess"],
            None,
            "--set-permissions",
        ),
        (
            vec![
                "1",
                "--set-permissions",
                "--owner",
                "2",
                "--view-users",
                "a",
            ],
            None,
            "expected comma-separated IDs or `none`",
        ),
        (
            vec!["1", "--correspondent", "abc"],
            None,
            "expected an ID or `none`",
        ),
    ] {
        let output = run_bulk(Some(&server), &args, stdin);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?} was accepted");
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
    any.assert_calls(0);
}

#[test]
fn failure_reports_the_operations_already_applied() {
    let server = MockServer::start();
    let tags = server.mock(|when, then| {
        when.method(POST)
            .path("/api/documents/bulk_edit/")
            .json_body_includes(r#"{"method":"add_tag"}"#);
        ok(then);
    });
    let reprocess = server.mock(|when, then| {
        when.method(POST).path("/api/documents/reprocess/");
        then.status(400).body(r#"{"documents":["bad"]}"#);
    });

    let output = run_bulk(Some(&server), &["1", "--add-tag", "5", "--reprocess"], None);

    assert!(!output.status.success());
    tags.assert();
    reprocess.assert();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("reprocess failed (already applied: add_tag)"),
        "{stderr}"
    );
}

#[test]
fn a_redirected_bulk_edit_fails_instead_of_turning_into_a_get() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/api/documents/bulk_edit/");
        then.status(302)
            .header("location", "/api/documents/bulk_edit/");
    });
    let get = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/api/documents/bulk_edit/");
        ok(then);
    });

    let output = run_bulk(Some(&server), &["1", "--add-tag", "5"], None);

    assert!(!output.status.success());
    get.assert_calls(0);
}
