use std::io::Write;
use std::process::{Command, Output, Stdio};

use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use serde_json::{json, Value};
use tempfile::tempdir;

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

fn data(output: &Output) -> Value {
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    envelope["data"].clone()
}

fn mock_action(server: &MockServer, body: Value) -> httpmock::Mock<'_> {
    let ids = body["documents"].clone();
    server.mock(|when, then| {
        when.method(POST)
            .path("/api/trash/")
            .header("authorization", "Token env-token")
            .json_body(body);
        then.status(200)
            .header("content-type", "application/json")
            .body(json!({"result": "OK", "doc_ids": ids}).to_string());
    })
}

#[test]
fn list_follows_pagination_and_keeps_id_title_deleted_at() {
    let server = MockServer::start();
    let page_two = server.mock(|when, then| {
        when.method(GET)
            .path("/api/trash/")
            .query_param("page", "2");
        then.status(200).body(
            json!({"next": null, "results": [
                {"id": 2, "title": "B", "deleted_at": "2026-11-02T10:00:00Z", "content": "x"}
            ]})
            .to_string(),
        );
    });
    let next = server.url("/api/trash/?page=2");
    let page_one = server.mock(|when, then| {
        when.method(GET)
            .path("/api/trash/")
            .query_param_missing("page");
        then.status(200).body(
            json!({"next": next, "results": [
                {"id": 1, "title": "A", "deleted_at": "2026-11-01T10:00:00Z", "content": "x"}
            ]})
            .to_string(),
        );
    });

    let output = run_paperless(Some(&server), &["--json", "trash", "list"], None);

    assert_success(&output);
    page_one.assert();
    page_two.assert();
    assert_eq!(
        data(&output),
        json!([
            {"id": 1, "title": "A", "deleted_at": "2026-11-01T10:00:00Z"},
            {"id": 2, "title": "B", "deleted_at": "2026-11-02T10:00:00Z"},
        ])
    );
}

#[test]
fn list_renders_a_table() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/trash/");
        then.status(200).body(
            json!({"next": null, "results": [
                {"id": 7, "title": "Old letter", "deleted_at": "2026-11-01T10:00:00Z"}
            ]})
            .to_string(),
        );
    });

    let output = run_paperless(Some(&server), &["trash", "list"], None);

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("DELETED"), "{stdout}");
    assert!(stdout.contains("Old letter"), "{stdout}");
    assert!(stdout.contains("2026-11-01T10:00:00Z"), "{stdout}");
}

#[test]
fn restore_sends_unique_ids_in_order() {
    let server = MockServer::start();
    let mock = mock_action(&server, json!({"action": "restore", "documents": [3, 1]}));

    let output = run_paperless(
        Some(&server),
        &["--json", "trash", "restore", "3", "1", "3"],
        None,
    );

    assert_success(&output);
    mock.assert();
    assert_eq!(data(&output)["doc_ids"], json!([3, 1]));
}

#[test]
fn restore_reads_ids_from_stdin() {
    let server = MockServer::start();
    let mock = mock_action(&server, json!({"action": "restore", "documents": [4, 5]}));

    let output = run_paperless(
        Some(&server),
        &["--json", "trash", "restore", "-"],
        Some("4\n5 4\n"),
    );

    assert_success(&output);
    mock.assert();
}

#[test]
fn empty_with_yes_sends_the_ids() {
    let server = MockServer::start();
    let mock = mock_action(&server, json!({"action": "empty", "documents": [2, 9]}));

    let output = run_paperless(
        Some(&server),
        &["--json", "trash", "empty", "2", "9", "--yes"],
        None,
    );

    assert_success(&output);
    mock.assert();
}

#[test]
fn empty_without_yes_refuses_when_stdin_is_not_a_terminal() {
    let server = MockServer::start();
    let any = server.mock(|_, then| {
        then.status(500);
    });

    let output = run_paperless(Some(&server), &["trash", "empty", "2"], Some("y\n"));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--yes"), "{stderr}");
    any.assert_calls(0);
}

#[test]
fn rejects_missing_or_invalid_ids_without_requests() {
    let server = MockServer::start();
    let any = server.mock(|_, then| {
        then.status(500);
    });
    for (args, stdin, expected) in [
        (vec!["empty", "--yes"], None, "<IDS>"),
        (vec!["restore"], None, "<IDS>"),
        (
            vec!["empty", "-", "--yes"],
            Some(""),
            "No document IDs given.",
        ),
        (vec!["restore", "-"], Some("  \n"), "No document IDs given."),
        (vec!["restore", "1", "x"], None, "Invalid document ID: x"),
    ] {
        let output = run_paperless(
            Some(&server),
            &[&["trash"], args.as_slice()].concat(),
            stdin,
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?} was accepted");
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
    any.assert_calls(0);
}

#[test]
fn demo_mode_answers_every_subcommand() {
    for args in [
        vec!["list"],
        vec!["restore", "401"],
        vec!["empty", "401", "--yes"],
    ] {
        let output = run_paperless(
            None,
            &[&["--demo", "--json", "trash"], args.as_slice()].concat(),
            None,
        );
        assert_success(&output);
    }
}

#[test]
fn empty_with_stdin_ids_requires_yes() {
    let server = MockServer::start();
    let any = server.mock(|_, then| {
        then.status(500);
    });

    let output = run_paperless(Some(&server), &["trash", "empty", "-"], Some("2\n"));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("IDs from stdin need --yes"), "{stderr}");
    any.assert_calls(0);
}

#[test]
fn a_redirected_write_fails_instead_of_turning_into_a_get() {
    for (action, args) in [
        ("empty", vec!["empty", "5", "--yes"]),
        ("restore", vec!["restore", "5"]),
    ] {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/api/trash/");
            then.status(302).header("location", "/api/trash/");
        });
        let get = server.mock(|when, then| {
            when.method(GET).path("/api/trash/");
            then.status(200).body(r#"{"next":null,"results":[]}"#);
        });

        let output = run_paperless(
            Some(&server),
            &[&["--json", "trash"], args.as_slice()].concat(),
            None,
        );

        assert!(!output.status.success(), "{action} reported success");
        get.assert_calls(0);
    }
}

#[test]
fn pagination_after_a_redirect_never_sends_the_token_to_the_other_host() {
    let other = MockServer::start();
    let leaked = other.mock(|when, then| {
        when.header_exists("authorization");
        then.status(200).body(r#"{"next":null,"results":[]}"#);
    });
    let next = other.url("/api/trash/?page=2");
    other.mock(|when, then| {
        when.method(GET)
            .path("/api/trash/")
            .query_param_missing("page");
        then.status(200)
            .body(json!({"next": next, "results": []}).to_string());
    });
    let server = MockServer::start();
    let target = other.url("/api/trash/");
    server.mock(|when, then| {
        when.method(GET).path("/api/trash/");
        then.status(302).header("location", target.as_str());
    });

    let output = run_paperless(Some(&server), &["--json", "trash", "list"], None);

    assert!(!output.status.success());
    leaked.assert_calls(0);
}
