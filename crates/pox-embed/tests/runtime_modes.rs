#![cfg(feature = "runtime-integration")]

use pox_embed::{HttpRequest, PhpRuntime};
use std::fs;

fn runtime() -> PhpRuntime {
    let path = std::env::var_os("POX_PHP_RUNTIME")
        .expect("POX_PHP_RUNTIME must point to a test libpox_php.so");
    PhpRuntime::load(path).expect("load test PHP runtime")
}

fn request(root: &std::path::Path, script: &std::path::Path, uri: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".to_string(),
        uri: uri.to_string(),
        query_string: "name=pox".to_string(),
        headers: vec![("Content-Type".to_string(), "text/plain".to_string())],
        body: b"request-body".to_vec(),
        document_root: root.to_string_lossy().into_owned(),
        script_filename: script.to_string_lossy().into_owned(),
        server_name: "localhost".to_string(),
        server_port: 8000,
        remote_addr: "127.0.0.1".to_string(),
        remote_port: 12345,
    }
}

#[test]
fn web_runtime_owns_php_request_layouts() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("index.php");
    fs::write(
        &script,
        "<?php header('X-Pox: web'); http_response_code(201); echo $_SERVER['REQUEST_URI'] . ':' . file_get_contents('php://input');",
    )
    .unwrap();
    let php = runtime();
    let web = php.web().unwrap();
    let response = web
        .execute(request(directory.path(), &script, "/hello?name=pox"))
        .unwrap();
    assert_eq!(response.status, 201);
    assert!(response
        .headers
        .iter()
        .any(|(name, value)| name == "X-Pox" && value == "web"));
    assert_eq!(response.body, b"/hello?name=pox:request-body");
}

#[test]
fn worker_callbacks_use_only_the_stable_host_table() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("worker.php");
    fs::write(
        &script,
        "<?php while (pox_handle_request(function () { header('X-Pox: worker'); echo $_SERVER['REQUEST_URI']; })) {}",
    )
    .unwrap();
    let php = runtime();
    let workers = php
        .workers(
            script.to_string_lossy().as_ref(),
            directory.path().to_string_lossy().as_ref(),
            1,
        )
        .unwrap();
    let response = workers
        .handle_request(request(directory.path(), &script, "/worker"))
        .unwrap();
    assert_eq!(response.status, 200);
    assert!(response
        .headers
        .iter()
        .any(|(name, value)| name == "X-Pox" && value == "worker"));
    assert_eq!(response.body, b"/worker");
}
