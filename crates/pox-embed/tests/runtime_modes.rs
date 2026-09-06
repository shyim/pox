#![cfg(feature = "runtime-integration")]

use pox_embed::{HttpRequest, PhpRuntime};
use std::fs;

fn runtime() -> PhpRuntime {
    let path = std::env::var_os("POX_PHP_RUNTIME")
        .expect("POX_PHP_RUNTIME must point to a test Pox PHP runtime library");
    PhpRuntime::load(path).expect("load test PHP runtime")
}

fn request(root: &std::path::Path, script: &std::path::Path, uri: &str) -> HttpRequest {
    HttpRequest {
        output: None,
        cancellation: None,
        secure: false,
        protocol: pox_embed::HttpProtocol::Http11,
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

enum OutputEvent {
    Start(u16, Vec<(String, String)>),
    Chunk(Vec<u8>),
    Flush,
}

struct ChannelOutput(std::sync::mpsc::SyncSender<OutputEvent>);
impl pox_embed::HttpOutput for ChannelOutput {
    fn start(&self, status: u16, headers: Vec<(String, String)>) -> bool {
        self.0.send(OutputEvent::Start(status, headers)).is_ok()
    }
    fn write(&self, chunk: &[u8]) -> bool {
        assert!(chunk.len() <= 16384);
        self.0.send(OutputEvent::Chunk(chunk.to_vec())).is_ok()
    }
    fn flush(&self) -> bool {
        self.0.send(OutputEvent::Flush).is_ok()
    }
}

#[test]
fn native_output_streams_before_completion_and_bounds_chunks_in_both_modes() {
    for worker in [false, true] {
        let php = runtime();
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("index.php");
        let callback = "http_response_code(202); header('X-Stream: yes'); echo 'first'; flush(); file_put_contents(__DIR__.'/after-flush', 'yes'); echo str_repeat('x', 50000);";
        fs::write(
            &script,
            if worker {
                format!("<?php while (pox_handle_request(function () {{ {callback} }})) {{}}")
            } else {
                format!("<?php {callback}")
            },
        )
        .unwrap();
        let mut pool = worker.then(|| {
            php.workers(
                script.to_str().unwrap(),
                directory.path().to_str().unwrap(),
                1,
            )
            .unwrap()
        });
        if let Some(pool) = &mut pool {
            pool.wait_ready(std::time::Duration::from_secs(2)).unwrap();
        }
        let web = (!worker).then(|| php.web().unwrap());
        let (sender, receiver) = std::sync::mpsc::sync_channel(0);
        let mut input = request(directory.path(), &script, "/");
        input.output = Some(pox_embed::ResponseOutput::new(ChannelOutput(sender)));
        let marker = directory.path().join("after-flush");
        std::thread::scope(|scope| {
            let consumer = scope.spawn(move || {
                let next = || {
                    receiver
                        .recv_timeout(std::time::Duration::from_secs(3))
                        .unwrap()
                };
                let OutputEvent::Start(status, headers) = next() else {
                    panic!("missing headers")
                };
                assert_eq!(status, 202);
                assert!(headers
                    .iter()
                    .any(|(name, value)| name == "X-Stream" && value == "yes"));
                let OutputEvent::Chunk(first) = next() else {
                    panic!("missing first chunk")
                };
                assert_eq!(first, b"first");
                // A zero-capacity sink blocks flush until the consumer accepts
                // it. PHP must not have reached the statement after flush.
                assert!(!marker.exists(), "output arrived only after PHP finished");
                assert!(matches!(next(), OutputEvent::Flush));
                let mut bytes = 0;
                while let Ok(event) = receiver.recv_timeout(std::time::Duration::from_secs(3)) {
                    match event {
                        OutputEvent::Chunk(chunk) => {
                            assert!(chunk.iter().all(|byte| *byte == b'x'));
                            bytes += chunk.len();
                        }
                        OutputEvent::Flush => {}
                        OutputEvent::Start(_, _) => panic!("duplicate headers"),
                    }
                }
                assert_eq!(bytes, 50000);
            });
            let response = if let Some(pool) = &pool {
                pool.handle_request(input)
            } else {
                web.as_ref().unwrap().execute(input)
            }
            .unwrap();
            assert_eq!(response.status, 202);
            assert!(
                response.body.is_empty(),
                "native runtime retained streamed body"
            );
            consumer.join().unwrap();
        });
    }
}

struct RejectOutput;
impl pox_embed::HttpOutput for RejectOutput {
    fn start(&self, _: u16, _: Vec<(String, String)>) -> bool {
        true
    }
    fn write(&self, _: &[u8]) -> bool {
        false
    }
    fn flush(&self) -> bool {
        true
    }
}

struct RejectHeaders(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl pox_embed::HttpOutput for RejectHeaders {
    fn start(&self, _: u16, _: Vec<(String, String)>) -> bool {
        false
    }
    fn write(&self, _: &[u8]) -> bool {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
        false
    }
    fn flush(&self) -> bool {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
        false
    }
}

#[test]
fn native_output_header_rejection_on_flush_aborts_before_the_next_statement() {
    let php = runtime();
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("index.php");
    fs::write(
        &script,
        "<?php flush(); file_put_contents(__DIR__.'/unexpected', 'yes');",
    )
    .unwrap();
    let web = php.web().unwrap();
    let mut input = request(directory.path(), &script, "/");
    let unexpected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    input.output = Some(pox_embed::ResponseOutput::new(RejectHeaders(
        unexpected.clone(),
    )));
    assert!(matches!(
        web.execute(input),
        Err(pox_embed::PhpError::ResponseOutputFailed)
    ));
    assert!(!directory.path().join("unexpected").exists());
    assert!(!unexpected.load(std::sync::atomic::Ordering::Relaxed));
}

struct CountOutput(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl pox_embed::HttpOutput for CountOutput {
    fn start(&self, _: u16, _: Vec<(String, String)>) -> bool {
        true
    }
    fn write(&self, chunk: &[u8]) -> bool {
        self.0
            .fetch_add(chunk.len(), std::sync::atomic::Ordering::Relaxed);
        true
    }
    fn flush(&self) -> bool {
        true
    }
}

#[test]
fn native_output_reports_fatal_errors_after_delivering_output() {
    for worker in [false, true] {
        let php = runtime();
        php.set_ini_entries(Some("display_errors=0\nlog_errors=1"))
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("index.php");
        let callback = "echo 'before'; flush(); pox_test_missing_function();";
        fs::write(
            &script,
            if worker {
                format!("<?php while (pox_handle_request(function () {{ {callback} }})) {{}}")
            } else {
                format!("<?php {callback}")
            },
        )
        .unwrap();
        let mut pool = worker.then(|| {
            php.workers(
                script.to_str().unwrap(),
                directory.path().to_str().unwrap(),
                1,
            )
            .unwrap()
        });
        if let Some(pool) = &mut pool {
            pool.wait_ready(std::time::Duration::from_secs(2)).unwrap();
        }
        let web = (!worker).then(|| php.web().unwrap());
        let bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut input = request(directory.path(), &script, "/");
        input.output = Some(pox_embed::ResponseOutput::new(CountOutput(bytes.clone())));
        let result = if let Some(pool) = &pool {
            pool.handle_request(input)
        } else {
            web.as_ref().unwrap().execute(input)
        };
        assert_eq!(bytes.load(std::sync::atomic::Ordering::Relaxed), 6);
        assert!(
            matches!(
                result,
                Err(pox_embed::PhpError::ResponseOutputFailed | pox_embed::PhpError::WorkerStopped)
            ),
            "{result:?}"
        );
    }
}

#[test]
fn worker_output_disconnect_does_not_poison_the_next_client_when_abort_is_ignored() {
    let php = runtime();
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("worker.php");
    fs::write(&script, "<?php ignore_user_abort(true); while (pox_handle_request(function () { echo connection_status(); })) {}").unwrap();
    let mut pool = php
        .workers(
            script.to_str().unwrap(),
            directory.path().to_str().unwrap(),
            1,
        )
        .unwrap();
    pool.wait_ready(std::time::Duration::from_secs(2)).unwrap();
    let mut input = request(directory.path(), &script, "/reject");
    input.output = Some(pox_embed::ResponseOutput::new(RejectOutput));
    assert!(matches!(
        pool.handle_request(input),
        Err(pox_embed::PhpError::ResponseOutputFailed)
    ));
    assert_eq!(
        pool.ready_count(),
        1,
        "ignore_user_abort should keep the incarnation alive"
    );
    assert_eq!(
        pool.handle_request(request(directory.path(), &script, "/next"))
            .unwrap()
            .body,
        b"0"
    );
}

#[test]
fn native_output_sink_failure_and_body_limit_are_not_successful_responses() {
    for worker in [false, true] {
        let php = runtime();
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("index.php");
        let callback = "echo 'first'; echo 'second';";
        fs::write(
            &script,
            if worker {
                format!("<?php while (pox_handle_request(function () {{ {callback} }})) {{}}")
            } else {
                format!("<?php {callback}")
            },
        )
        .unwrap();
        let mut pool = worker.then(|| {
            php.workers(
                script.to_str().unwrap(),
                directory.path().to_str().unwrap(),
                1,
            )
            .unwrap()
        });
        if let Some(pool) = &mut pool {
            pool.wait_ready(std::time::Duration::from_secs(2)).unwrap();
        }
        let web = (!worker).then(|| php.web().unwrap());
        let mut input = request(directory.path(), &script, "/");
        input.output = Some(pox_embed::ResponseOutput::new(RejectOutput));
        let result = if let Some(pool) = &pool {
            pool.handle_request(input)
        } else {
            web.as_ref().unwrap().execute(input)
        };
        assert!(
            matches!(result, Err(pox_embed::PhpError::ResponseOutputFailed)),
            "{result:?}"
        );
        if let Some(pool) = &mut pool {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while pool.ready_count() == 0 {
                pool.repair_stopped();
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        // Limit failure happens before any sink callback, even if that sink
        // would have rejected a write. Zero means no output, not unlimited.
        let mut input = request(directory.path(), &script, "/");
        input.output = Some(pox_embed::ResponseOutput::new(RejectOutput));
        let limits = pox_embed::ResponseLimits {
            body_bytes: 0,
            header_bytes: 32768,
        };
        let result = if let Some(pool) = &pool {
            pool.handle_request_with_limits(input, limits)
        } else {
            web.as_ref().unwrap().execute_with_limits(input, limits)
        };
        assert!(
            matches!(result, Err(pox_embed::PhpError::ResponseBufferFailed)),
            "{result:?}"
        );
        let healthy = request(directory.path(), &script, "/");
        let response = if let Some(pool) = &pool {
            pool.handle_request(healthy)
        } else {
            web.as_ref().unwrap().execute(healthy)
        }
        .unwrap();
        assert_eq!(response.body, b"firstsecond");
    }
}

fn cancel_when_entered(
    control: pox_embed::RequestCancellation,
    marker: std::path::PathBuf,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !marker.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "PHP did not enter the cancellable request"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        control.cancel();
    })
}

#[test]
fn native_cancellation_is_request_scoped_in_both_modes() {
    for worker in [false, true] {
        let php = runtime();
        // Native timeout is a test fallback; host cancellation must finish first.
        php.set_ini_entries(Some("max_execution_time=3\ndisplay_errors=0\nlog_errors=1"))
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("index.php");
        let callback = "if ($_SERVER['REQUEST_URI'] === '/cancel') { file_put_contents(__DIR__.'/entered', 'yes'); echo str_repeat('x', 10000); while (true) {} } else { file_put_contents(__DIR__.'/next', 'yes'); for ($i=0; $i<20; $i++) usleep(1000); echo 'healthy'; }";
        fs::write(
            &script,
            if worker {
                format!("<?php while (pox_handle_request(function () {{ {callback} }})) {{}}")
            } else {
                format!("<?php {callback}")
            },
        )
        .unwrap();
        let mut pool = worker.then(|| {
            php.workers(
                script.to_str().unwrap(),
                directory.path().to_str().unwrap(),
                1,
            )
            .unwrap()
        });
        if let Some(pool) = &mut pool {
            pool.wait_ready(std::time::Duration::from_secs(2)).unwrap();
        }
        let web = (!worker).then(|| php.web().unwrap());
        for _ in 0..8 {
            let control = php.cancellation().unwrap();
            let mut input = request(directory.path(), &script, "/cancel");
            input.cancellation = Some(control.clone());
            let cancel = cancel_when_entered(control.clone(), directory.path().join("entered"));
            let started = std::time::Instant::now();
            let result = if let Some(pool) = &pool {
                pool.handle_request(input.clone())
            } else {
                web.as_ref().unwrap().execute(input.clone())
            };
            cancel.join().unwrap();
            assert!(
                matches!(result, Err(pox_embed::PhpError::RequestCancelled)),
                "{worker}: {result:?}"
            );
            assert!(
                started.elapsed() < std::time::Duration::from_secs(2),
                "only the PHP fallback timer stopped execution"
            );
            let reused = if let Some(pool) = &pool {
                pool.handle_request(input)
            } else {
                web.as_ref().unwrap().execute(input)
            };
            assert!(matches!(reused, Err(pox_embed::PhpError::CancellationUsed)));
            if let Some(pool) = &mut pool {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while pool.ready_count() == 0 {
                    pool.repair_stopped();
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            // Late cancellation after completion/TSRM teardown must not poison
            // another request, even if the allocator reuses the same globals.
            let late = cancel_when_entered(control, directory.path().join("next"));
            let input = request(directory.path(), &script, "/healthy");
            let result = if let Some(pool) = &pool {
                pool.handle_request(input)
            } else {
                web.as_ref().unwrap().execute(input)
            };
            late.join().unwrap();
            assert_eq!(result.unwrap().body, b"healthy");
            fs::remove_file(directory.path().join("entered")).unwrap();
            fs::remove_file(directory.path().join("next")).unwrap();
        }
        let control = php.cancellation().unwrap();
        control.cancel();
        let mut input = request(directory.path(), &script, "/cancel");
        input.cancellation = Some(control);
        let result = if let Some(pool) = &pool {
            pool.handle_request(input)
        } else {
            web.as_ref().unwrap().execute(input)
        };
        assert!(matches!(result, Err(pox_embed::PhpError::RequestCancelled)));
        assert!(!directory.path().join("entered").exists());
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

#[test]
fn concurrent_worker_requests_keep_their_own_responses() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("worker.php");
    fs::write(&script, "<?php while (pox_handle_request(function () { usleep(1000); echo $_SERVER['REQUEST_URI']; })) {}").unwrap();
    let php = runtime();
    let workers = php
        .workers(
            script.to_str().unwrap(),
            directory.path().to_str().unwrap(),
            2,
        )
        .unwrap();
    std::thread::scope(|scope| {
        for caller in 0..8 {
            let workers = &workers;
            let root = directory.path();
            let script = &script;
            scope.spawn(move || {
                for iteration in 0..20 {
                    let uri = format!("/{caller}/{iteration}");
                    let response = workers.handle_request(request(root, script, &uri)).unwrap();
                    assert_eq!(response.body, uri.as_bytes());
                }
            });
        }
    });
}

#[test]
fn native_response_limits_reject_overflow_without_poisoning_later_requests() {
    use pox_embed::{PhpError, ResponseLimits};
    for worker_mode in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("index.php");
        let callback = "if ($_SERVER['REQUEST_URI'] === '/header') { header('X-Large: '.str_repeat('h', 5000)); } echo str_repeat('x', (int) $_SERVER['REQUEST_URI']);";
        let code = if worker_mode {
            format!("<?php while (pox_handle_request(function () {{ {callback} }})) {{}}")
        } else {
            format!("<?php {callback}")
        };
        fs::write(&script, code).unwrap();
        let php = runtime();
        assert!(php.supports_response_limits());
        let web = if !worker_mode {
            Some(php.web().unwrap())
        } else {
            None
        };
        let workers = if worker_mode {
            Some(
                php.workers(
                    script.to_str().unwrap(),
                    directory.path().to_str().unwrap(),
                    1,
                )
                .unwrap(),
            )
        } else {
            None
        };
        let execute = |uri: &str, limit: u32| {
            let request = request(directory.path(), &script, uri);
            let limits = ResponseLimits {
                body_bytes: limit,
                header_bytes: 1024,
            };
            if let Some(web) = &web {
                web.execute_with_limits(request, limits)
            } else {
                workers
                    .as_ref()
                    .unwrap()
                    .handle_request_with_limits(request, limits)
            }
        };
        assert_eq!(execute("32", 32).unwrap().body, vec![b'x'; 32]);
        assert!(matches!(
            execute("33", 32),
            Err(PhpError::ResponseBufferFailed)
        ));
        assert!(matches!(
            execute("/header", 32),
            Err(PhpError::ResponseBufferFailed)
        ));
        assert_eq!(execute("0", 0).unwrap().body.len(), 0);
        assert!(matches!(
            execute("1", 0),
            Err(PhpError::ResponseBufferFailed)
        ));
        assert_eq!(execute("16", 32).unwrap().body, vec![b'x'; 16]);
    }
}

#[test]
fn execution_modes_exclude_each_other_and_release_ownership_after_shutdown() {
    use pox_embed::PhpError;
    let php = runtime();
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("index.php");
    fs::write(&script, "<?php echo 'web-ok';").unwrap();
    let worker_script = directory.path().join("worker.php");
    fs::write(
        &worker_script,
        "<?php while (pox_handle_request(function () { echo 'worker-ok'; })) {}",
    )
    .unwrap();
    let web = php.web().unwrap();
    let independently_loaded = runtime();
    std::thread::scope(|scope| {
        for runtime in [&php, &independently_loaded] {
            let worker_script = &worker_script;
            let root = directory.path();
            scope.spawn(move || {
                assert!(matches!(runtime.web(), Err(PhpError::RuntimeBusy)));
                assert!(matches!(
                    runtime.workers(worker_script.to_str().unwrap(), root.to_str().unwrap(), 1),
                    Err(PhpError::RuntimeBusy)
                ));
                assert!(matches!(
                    runtime.execute_code("echo 'wrong';", &[] as &[&str]),
                    Err(PhpError::RuntimeBusy)
                ));
                assert!(matches!(
                    runtime.set_ini_entries(Some("display_errors=1")),
                    Err(PhpError::RuntimeBusy)
                ));
            });
        }
    });
    assert_eq!(
        web.execute(request(directory.path(), &script, "/"))
            .unwrap()
            .body,
        b"web-ok"
    );
    drop(web);
    let workers = php
        .workers(
            worker_script.to_str().unwrap(),
            directory.path().to_str().unwrap(),
            1,
        )
        .unwrap();
    assert!(matches!(php.web(), Err(PhpError::RuntimeBusy)));
    assert!(matches!(
        php.set_ini_entries(None),
        Err(PhpError::RuntimeBusy)
    ));
    assert_eq!(
        workers
            .handle_request(request(directory.path(), &worker_script, "/"))
            .unwrap()
            .body,
        b"worker-ok"
    );
    drop(workers);
    php.set_ini_entries(None).unwrap();
    assert_eq!(php.execute_code("exit(0);", &[] as &[&str]).unwrap(), 0);
    let web = php.web().unwrap();
    assert_eq!(
        web.execute(request(directory.path(), &script, "/"))
            .unwrap()
            .body,
        b"web-ok"
    );
}

#[test]
fn reusable_web_threads_keep_request_state_isolated_and_can_reattach() {
    use pox_embed::ResponseLimits;
    let php = runtime();
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("index.php");
    fs::write(&script, "<?php class PerRequest {} $GLOBALS['count'] = ($GLOBALS['count'] ?? 0) + 1; echo $GLOBALS['count'].':'.$_SERVER['REQUEST_URI'];").unwrap();
    let web = php.web().unwrap();
    let executor = web.parallel_executor().unwrap();
    // Attaching the PHP module owner as a dispatch thread is invalid.
    assert!(executor.attach().is_err());
    std::thread::scope(|scope| {
        for caller in 0..4 {
            let executor = &executor;
            let root = directory.path();
            let script = &script;
            scope.spawn(move || {
                for generation in 0..2 {
                    let thread = executor.attach().unwrap();
                    assert!(executor.attach().is_err());
                    for index in 0..20 {
                        let uri = format!("/{caller}/{generation}/{index}");
                        let response = thread
                            .execute_with_limits(
                                request(root, script, &uri),
                                ResponseLimits {
                                    body_bytes: 1024,
                                    header_bytes: 1024,
                                },
                            )
                            .unwrap();
                        assert_eq!(response.body, format!("1:{uri}").as_bytes());
                    }
                }
            });
        }
    });
    assert_eq!(
        web.execute(request(directory.path(), &script, "/owner"))
            .unwrap()
            .body,
        b"1:/owner"
    );
}
