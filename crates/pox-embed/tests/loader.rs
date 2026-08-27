use pox_embed::{runtime_target, HttpRequest, PhpRuntime};
use std::process::Command;
use std::sync::Mutex;

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn fake_runtime(target: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("libpox_php.so");
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_runtime.c");
    let target_define = format!("-DPOX_TEST_TARGET=\"{target}\"");
    let status = Command::new("cc")
        .args(["-shared", "-fPIC", "-fvisibility=hidden"])
        .arg(target_define)
        .arg(source)
        .arg("-o")
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success());
    (directory, output)
}

#[test]
fn loads_the_versioned_table_without_php_headers() {
    let _guard = TEST_LOCK.lock().unwrap();
    let (_directory, path) = fake_runtime(runtime_target());
    let php = PhpRuntime::load(path).unwrap();
    assert_eq!(php.version().version, "8.5.9");
    assert_eq!(php.metadata().runtime_revision, "fake");
    assert_eq!(php.metadata().extensions, ["Core"]);
    assert_eq!(php.execute_code("echo 1;", &[] as &[&str]).unwrap(), 0);
}

#[test]
fn owns_web_responses_returned_through_the_abi() {
    let _guard = TEST_LOCK.lock().unwrap();
    let (_directory, path) = fake_runtime(runtime_target());
    let php = PhpRuntime::load(path).unwrap();
    let web = php.web().unwrap();
    let response = web
        .execute(HttpRequest {
            method: "GET".into(),
            uri: "/".into(),
            query_string: String::new(),
            headers: Vec::new(),
            body: Vec::new(),
            document_root: ".".into(),
            script_filename: "index.php".into(),
            server_name: "localhost".into(),
            server_port: 80,
            remote_addr: "127.0.0.1".into(),
            remote_port: 1,
        })
        .unwrap();
    assert_eq!(response.status, 204);
    assert!(response.body.is_empty());
}

#[test]
fn rejects_wrong_targets_and_multiple_active_versions() {
    let _guard = TEST_LOCK.lock().unwrap();
    let (_wrong_directory, wrong_path) = fake_runtime("a-different-target");
    assert!(PhpRuntime::load(wrong_path).is_err());

    let (_first_directory, first_path) = fake_runtime(runtime_target());
    let (_second_directory, second_path) = fake_runtime(runtime_target());
    let first = PhpRuntime::load(first_path).unwrap();
    let error = PhpRuntime::load(second_path).unwrap_err();
    assert!(error.to_string().contains("already active"));
    drop(first);
}
