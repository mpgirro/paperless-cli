use std::fs;
use std::process::{Command, Output};

use httpmock::Method::{DELETE, GET, POST};
use httpmock::MockServer;
use serde_json::{json, Value};
use tempfile::tempdir;

const TOKEN: &str = "secret-test-token";

fn run_api(server: &MockServer, args: &[&str]) -> Output {
    let dir = tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_paperless"))
        .env("PAPERLESS_CONFIG_PATH", dir.path().join("config.toml"))
        .env("PAPERLESS_SESSION_PATH", dir.path().join("session.toml"))
        .env("PAPERLESS_URL", server.base_url())
        .env("PAPERLESS_TOKEN", TOKEN)
        .arg("api")
        .args(args)
        .output()
        .unwrap()
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn get_prints_the_response_json_with_stored_auth() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/api/status/")
            .header("authorization", format!("Token {TOKEN}"));
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"pngx_version":"2.0.0"}"#);
    });

    let output = run_api(&server, &["GET", "status/"]);

    mock.assert();
    assert!(output.status.success());
    assert_eq!(stdout_json(&output), json!({"pngx_version": "2.0.0"}));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(TOKEN));
}

#[test]
fn method_is_case_insensitive_and_query_pairs_are_sent() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/api/documents/")
            .query_param("title__icontains", "a=b")
            .query_param("page", "2");
        then.status(200).body(r#"{"results":[]}"#);
    });

    let output = run_api(
        &server,
        &[
            "get",
            "/documents/",
            "--query",
            "title__icontains=a=b",
            "--query",
            "page=2",
        ],
    );

    mock.assert();
    assert!(output.status.success());
}

#[test]
fn post_sends_inline_json_data() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/api/documents/bulk_edit/")
            .json_body(json!({"documents": [1], "method": "delete"}));
        then.status(200).body(r#"{"result":"OK"}"#);
    });

    let output = run_api(
        &server,
        &[
            "POST",
            "documents/bulk_edit/",
            "--data",
            r#"{"documents":[1],"method":"delete"}"#,
        ],
    );

    mock.assert();
    assert!(output.status.success());
    assert_eq!(stdout_json(&output), json!({"result": "OK"}));
}

#[test]
fn post_reads_json_data_from_a_file() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/api/tags/")
            .json_body(json!({"name": "x"}));
        then.status(201).body(r#"{"id":5}"#);
    });
    let dir = tempdir().unwrap();
    let body = dir.path().join("body.json");
    fs::write(&body, r#"{"name":"x"}"#).unwrap();

    let output = run_api(
        &server,
        &["POST", "tags/", "--data", &format!("@{}", body.display())],
    );

    mock.assert();
    assert!(output.status.success());
    assert_eq!(stdout_json(&output), json!({"id": 5}));
}

#[test]
fn invalid_json_data_fails_without_a_request() {
    let server = MockServer::start();
    let mock = server.mock(|_when, then| {
        then.status(200);
    });

    let output = run_api(&server, &["POST", "tags/", "--data", "{not json"]);

    mock.assert_calls(0);
    assert!(!output.status.success());
}

#[test]
fn non_2xx_exits_non_zero_with_the_body_on_stderr() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/documents/9/");
        then.status(404)
            .body(r#"{"detail":"No Document matches the given query."}"#);
    });

    let output = run_api(&server, &["GET", "documents/9/"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(stderr.contains("404"));
    assert!(stderr.contains("No Document matches the given query."));
    assert!(!stderr.contains(TOKEN));
}

#[test]
fn all_follows_next_links_and_merges_results() {
    let server = MockServer::start();
    let page_two = server.mock(|when, then| {
        when.method(GET)
            .path("/api/trash/")
            .query_param("page", "2");
        then.status(200)
            .body(r#"{"next":null,"results":[{"id":2}]}"#);
    });
    let next = server.url("/api/trash/?page=2");
    let page_one = server.mock(|when, then| {
        when.method(GET)
            .path("/api/trash/")
            .query_param_missing("page");
        then.status(200)
            .body(json!({"next": next, "results": [{"id": 1}]}).to_string());
    });

    let output = run_api(&server, &["GET", "trash/", "--all"]);

    page_one.assert();
    page_two.assert();
    assert!(output.status.success());
    assert_eq!(stdout_json(&output), json!([{"id": 1}, {"id": 2}]));
}

#[test]
fn all_is_rejected_for_non_get_methods() {
    let server = MockServer::start();
    let mock = server.mock(|_when, then| {
        then.status(200);
    });

    let output = run_api(&server, &["POST", "trash/", "--all"]);

    mock.assert_calls(0);
    assert!(!output.status.success());
}

#[test]
fn a_missing_trailing_slash_is_added_before_the_query() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/api/documents/1/notes/")
            .query_param("x", "1");
        then.status(200).body(r#"{"id":1}"#);
    });

    let output = run_api(
        &server,
        &["POST", "documents/1/notes?x=1", "--data", r#"{"note":"n"}"#],
    );

    mock.assert();
    assert!(output.status.success());
}

#[test]
fn a_redirected_write_fails_instead_of_turning_into_a_get() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/api/old/");
        then.status(301).header("location", "/api/new/");
    });
    let get = server.mock(|when, then| {
        when.method(GET).path("/api/new/");
        then.status(200).body("[]");
    });

    let output = run_api(&server, &["POST", "old/", "--data", "{}"]);

    get.assert_calls(0);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("301"));
}

#[test]
fn a_non_json_success_body_is_printed_raw() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/documents/1/download/");
        then.status(200).body("%PDF-1.4 raw");
    });

    let output = run_api(&server, &["GET", "documents/1/download/"]);

    assert!(output.status.success());
    assert_eq!(output.stdout, b"%PDF-1.4 raw");
}

#[test]
fn an_empty_204_prints_an_empty_object() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(DELETE).path("/api/tags/3/");
        then.status(204);
    });

    let output = run_api(&server, &["DELETE", "tags/3/"]);

    assert!(output.status.success());
    assert_eq!(stdout_json(&output), json!({}));
}

#[test]
fn all_puts_the_raw_error_body_on_stderr() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/trash/");
        then.status(403).body(r#"{"detail":"custom denial"}"#);
    });

    let output = run_api(&server, &["GET", "trash/", "--all"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("custom denial"));
}

#[test]
fn all_fails_on_a_response_without_results() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/documents/1/");
        then.status(200).body(r#"{"id":1}"#);
    });

    let output = run_api(&server, &["GET", "documents/1/", "--all"]);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn all_keeps_a_page_size_given_in_the_path() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/api/trash/")
            .query_param("page_size", "5")
            .query_param_count("page_size", ".*", 1);
        then.status(200).body(r#"{"next":null,"results":[]}"#);
    });

    let output = run_api(&server, &["GET", "trash/?page_size=5", "--all"]);

    mock.assert();
    assert!(output.status.success());
}

#[test]
fn all_refuses_a_cross_host_next_link() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/trash/");
        then.status(200)
            .body(r#"{"next":"http://other.invalid/api/trash/?page=2","results":[{"id":1}]}"#);
    });

    let output = run_api(&server, &["GET", "trash/", "--all"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cross-origin"));
}
