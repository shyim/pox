//! Real-socket checks, enabled by the same runtime feature as `mise run test:runtime`.
#![cfg(feature = "runtime-integration")]

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn request_paths_are_confined_in_both_modes() {
    for worker in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("public");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(directory.path().join("secret.txt"), "outside-secret").unwrap();
        std::fs::write(root.join(".env"), "private-config").unwrap();
        std::fs::write(root.join("index.php"), "<?php echo 'front-controller';").unwrap();
        std::fs::write(root.join("upper.PHP"), "<?php echo 'executed';").unwrap();
        std::fs::write(root.join("hello world.txt"), "static-content").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                directory.path().join("secret.txt"),
                root.join("escape.txt"),
            )
            .unwrap();
            std::os::unix::fs::symlink(root.join("upper.PHP"), root.join("source.txt")).unwrap();
            std::os::unix::fs::symlink(root.join(".env"), root.join("config.txt")).unwrap();
        }
        let worker_path = directory.path().join("worker.php");
        std::fs::write(
            &worker_path,
            "<?php while (pox_handle_request(function () { echo 'worker-response'; })) {}",
        )
        .unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut command = Command::new(env!("CARGO_BIN_EXE_pox"));
        command
            .current_dir(directory.path())
            .args(["server", "--port", &port.to_string(), "--document-root"])
            .arg(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if worker {
            command
                .arg("--worker")
                .arg(&worker_path)
                .args(["--workers", "2"]);
        }
        let mut server = Server(command.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            assert!(
                server.0.try_wait().unwrap().is_none(),
                "server exited during startup"
            );
            assert!(Instant::now() < deadline, "server startup timeout");
            std::thread::sleep(Duration::from_millis(20));
        }
        for (path, status) in [
            ("/../secret.txt", 403),
            ("/%2e%2e/secret.txt", 403),
            ("/.env", 403),
            ("/%00", 400),
            ("/%zz", 400),
            ("/hello%20world.txt", 200),
            ("/route", 200),
            ("/upper.PHP", 200),
        ] {
            let response = exchange(port, "GET", path);
            assert!(
                response.starts_with(&format!("HTTP/1.1 {status} ")),
                "{worker} {path}: {response}"
            );
            assert!(
                !response.contains("outside-secret")
                    && !response.contains("private-config")
                    && !response.contains("<?php"),
                "source disclosure: {response}"
            );
            if path == "/hello%20world.txt" {
                assert!(response.ends_with("static-content"));
            }
        }
        #[cfg(unix)]
        for path in ["/escape.txt", "/source.txt", "/config.txt"] {
            let response = exchange(port, "GET", path);
            assert!(
                !response.contains("outside-secret")
                    && !response.contains("private-config")
                    && !response.contains("<?php"),
                "{path}: {response}"
            );
        }
        let response = exchange(port, "HEAD", "/hello%20world.txt");
        assert!(
            response.ends_with("\r\n\r\n"),
            "HEAD sent a body: {response}"
        );
    }
}

fn exchange(port: u16, method: &str, path: &str) -> String {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        socket,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    response
}

fn admin_server(worker: bool, limits: &str, callback: &str, bootstrap: &str) -> (TestServer, u16) {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let server = TestServer::start_with_proxy_config(
        worker,
        limits,
        callback,
        false,
        bootstrap,
        1,
        &format!("admin_address = '127.0.0.1:{port}'"),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < deadline,
            "admin startup: {}",
            server.logs()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    (server, port)
}

fn blocking_native_callback() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let callback = format!("if (!file_exists(__DIR__.'/release')) {{ $socket = stream_socket_client('tcp://127.0.0.1:{}'); file_put_contents(__DIR__.'/entered', 'yes'); fread($socket, 1); fclose($socket); }} echo 'done';", listener.local_addr().unwrap().port());
    (listener, callback)
}

#[test]
fn admin_checks_remain_responsive_when_php_exceeds_its_deadline() {
    for worker in [false, true] {
        let (native, callback) = blocking_native_callback();
        let (server, port) = admin_server(
            worker,
            "request_timeout_ms = 400\nmax_inflight_requests = 1",
            &callback,
            "",
        );
        assert_status(&exchange(port, "GET", "/ready"), 200);
        assert_status(&server.get("/asset.txt"), 200);
        let mut request = server.socket();
        request
            .write_all(b"GET /index.php HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !server.directory.path().join("public/entered").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        // Busy within the deadline is healthy; public admission is independent.
        assert_status(&exchange(port, "GET", "/ready"), 200);
        assert_status(&server.get("/asset.txt"), 503);
        let mut result = String::new();
        request.read_to_string(&mut result).unwrap();
        assert_status(&result, 504);
        assert_status(&exchange(port, "GET", "/live"), 200);
        assert_status(&exchange(port, "GET", "/ready"), 503);
        let metrics = exchange(port, "GET", "/metrics");
        assert!(metrics.contains("pox_php_expired_jobs 1\n"), "{metrics}");
        assert!(
            metrics.contains("pox_http_responses_total{class=\"5xx\"} 2\n"),
            "{metrics}"
        );
        assert!(
            metrics.contains("pox_http_admitted_requests 1\n"),
            "{metrics}"
        );
        assert!(exchange(port, "HEAD", "/metrics").ends_with("\r\n\r\n"));
        assert_status(&exchange(port, "POST", "/ready"), 405);
        assert_status(&exchange(port, "GET", "/unknown"), 404);
        std::fs::write(server.directory.path().join("public/release"), "yes").unwrap();
        native.accept().unwrap().0.write_all(b"x").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !exchange(port, "GET", "/ready").starts_with("HTTP/1.1 200 ") {
            assert!(
                Instant::now() < deadline,
                "readiness did not recover: {}",
                server.logs()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_status(&server.get("/index.php"), 200);
    }
}

#[test]
fn admin_readiness_tracks_failed_worker_replacements() {
    let (server, port) = admin_server(
        true,
        "request_timeout_ms = 2000",
        "file_put_contents(__DIR__.'/bad-bootstrap', 'yes'); exit;",
        "if (file_exists(__DIR__.'/bad-bootstrap')) { exit; }",
    );
    assert_status(&exchange(port, "GET", "/ready"), 200);
    assert_status(&server.get("/index.php"), 502);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !exchange(port, "GET", "/ready").starts_with("HTTP/1.1 503 ") {
        assert!(Instant::now() < deadline, "failed workers remained ready");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_status(&exchange(port, "GET", "/live"), 200);
    std::fs::remove_file(server.directory.path().join("public/bad-bootstrap")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !exchange(port, "GET", "/ready").starts_with("HTTP/1.1 200 ") {
        assert!(
            Instant::now() < deadline,
            "replacement did not become ready: {}",
            server.logs()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn admin_connections_expire_and_disconnected_php_stays_tracked() {
    for worker in [false, true] {
        let (native, callback) = blocking_native_callback();
        let (server, port) = admin_server(worker, "request_timeout_ms = 300", &callback, "");
        let mut slow_admin = TcpStream::connect(("127.0.0.1", port)).unwrap();
        slow_admin
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        slow_admin
            .write_all(b"GET /live HTTP/1.1\r\nHost:")
            .unwrap();
        let mut request = server.socket();
        request
            .write_all(b"GET /index.php HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !server.directory.path().join("public/entered").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        request.shutdown(std::net::Shutdown::Both).unwrap();
        drop(request);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !exchange(port, "GET", "/ready").starts_with("HTTP/1.1 503 ") {
            assert!(Instant::now() < deadline, "disconnected execution was lost");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_status(&exchange(port, "GET", "/live"), 200);
        let mut bytes = Vec::new();
        match slow_admin.read_to_end(&mut bytes) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(error) => panic!("admin connection did not expire: {error}"),
        }
        std::fs::write(server.directory.path().join("public/release"), "yes").unwrap();
        native.accept().unwrap().0.write_all(b"x").unwrap();
    }
}

#[test]
#[cfg(unix)]
fn admin_stays_live_but_unready_during_graceful_shutdown() {
    for worker in [false, true] {
        let (mut server, port) = admin_server(worker, "shutdown_timeout_ms = 3000\nrequest_timeout_ms = 2500",
            "file_put_contents(__DIR__.'/entered', 'yes'); while (!file_exists(__DIR__.'/release')) { usleep(10000); clearstatcache(); } echo 'done';", "");
        let mut request = server.socket();
        request
            .write_all(b"GET /index.php HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !server.directory.path().join("public/entered").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        terminate(&server);
        let deadline = Instant::now() + Duration::from_secs(1);
        while !exchange(port, "GET", "/ready").starts_with("HTTP/1.1 503 ") {
            assert!(Instant::now() < deadline, "draining server stayed ready");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_status(&exchange(port, "GET", "/live"), 200);
        std::fs::write(server.directory.path().join("public/release"), "yes").unwrap();
        let mut result = String::new();
        request.read_to_string(&mut result).unwrap();
        assert_status(&result, 200);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = server.process.0.try_wait().unwrap() {
                assert!(status.success(), "{}", server.logs());
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    }
}

struct TestServer {
    process: Server,
    directory: tempfile::TempDir,
    port: u16,
}

impl TestServer {
    fn start(worker: bool, limits: &str, callback: &str) -> Self {
        Self::start_with_watch(worker, limits, callback, false)
    }

    fn start_with_watch(worker: bool, limits: &str, callback: &str, watch: bool) -> Self {
        Self::start_with_bootstrap(worker, limits, callback, watch, "")
    }

    fn start_with_bootstrap(
        worker: bool,
        limits: &str,
        callback: &str,
        watch: bool,
        bootstrap: &str,
    ) -> Self {
        Self::start_with_concurrency(
            worker,
            limits,
            callback,
            watch,
            bootstrap,
            if worker { 2 } else { 1 },
        )
    }

    fn start_with_concurrency(
        worker: bool,
        limits: &str,
        callback: &str,
        watch: bool,
        bootstrap: &str,
        threads: usize,
    ) -> Self {
        Self::start_with_proxy_config(worker, limits, callback, watch, bootstrap, threads, "")
    }

    fn start_with_proxy_config(
        worker: bool,
        limits: &str,
        callback: &str,
        watch: bool,
        bootstrap: &str,
        threads: usize,
        proxy: &str,
    ) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("public");
        std::fs::create_dir(&root).unwrap();
        let script = if worker {
            format!(
                "<?php {bootstrap} while (pox_handle_request(function () {{ {callback} }})) {{}}"
            )
        } else {
            format!("<?php {callback}")
        };
        std::fs::write(root.join("index.php"), script).unwrap();
        std::fs::write(root.join("asset.txt"), "static-content").unwrap();
        std::fs::write(directory.path().join("pox.toml"), format!("[server]\nhost = '192.0.2.1'\nport = 1\ndocument_root = 'missing'\n{proxy}\n[server.limits]\n{limits}")).unwrap();
        // Deliberately contradict config: explicit CLI values must win.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let logs = std::fs::File::create(directory.path().join("server.log")).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_pox"));
        command
            .current_dir(directory.path())
            .args([
                "server",
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--document-root",
            ])
            .arg(&root)
            .stdout(Stdio::null())
            .stderr(logs);
        command.args(["--workers", &threads.to_string()]);
        if worker {
            command.arg("--worker").arg(root.join("index.php"));
        }
        if watch {
            command.args(["--watch", "**/*.php"]);
        }
        let mut server = Self {
            process: Server(command.spawn().unwrap()),
            directory,
            port,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            assert!(
                server.process.0.try_wait().unwrap().is_none(),
                "server exited: {}",
                server.logs()
            );
            assert!(
                Instant::now() < deadline,
                "startup timeout: {}",
                server.logs()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        server
    }

    fn logs(&self) -> String {
        std::fs::read_to_string(self.directory.path().join("server.log")).unwrap()
    }
    fn socket(&self) -> TcpStream {
        let socket = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
    }
    fn raw(&self, request: &str) -> String {
        let mut socket = self.socket();
        socket.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        match socket.read_to_string(&mut response) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(error) => panic!(
                "read failed: {error}; response: {response}; log: {}",
                self.logs()
            ),
        }
        response
    }
    fn get(&self, path: &str) -> String {
        self.raw(&format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        ))
    }
}

fn assert_status(response: &str, status: u16) {
    assert!(
        response.starts_with(&format!("HTTP/1.1 {status} ")),
        "expected {status}: {response}"
    );
}

#[test]
fn transport_bounds_and_validates_bodies_before_php_in_both_modes() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "max_body_bytes = 2048\nmax_header_bytes = 8192\nmax_headers = 16\nbody_timeout_ms = 150\nheader_timeout_ms = 150\nidle_timeout_ms = 250", "file_put_contents(__DIR__.'/calls', 'x', FILE_APPEND); echo file_get_contents('php://input');");
        for request in [
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2049\r\n\r\n".to_string(),
            format!("POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n801\r\n{}\r\n0\r\n\r\n", "x".repeat(2049)),
        ] { assert_status(&server.raw(&request), 413); }
        for request in [
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab",
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: +2\r\n\r\nab",
            "GET / HTTP/1.1\r\nHost: localhost\r\nHost: evil.test\r\n\r\n",
            "GET / HTTP/1.1\r\nConnection: close\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: localhost:invalid\r\n\r\n",
            "POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: gzip\r\n\r\n",
            "POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\nbad-chunk\r\n",
        ] { assert_status(&server.raw(request), 400); }
        assert_status(
            &server.raw("POST / HTTP/1.1\r\nHost: localhost\r\nExpect: something-else\r\n\r\n"),
            417,
        );
        assert_status(
            &server.raw(&format!(
                "GET / HTTP/1.1\r\nHost: localhost\r\nX-Large: {}\r\n\r\n",
                "a".repeat(9000)
            )),
            431,
        );
        assert_status(
            &server.raw("POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10\r\n\r\npartial"),
            408,
        );
        let response = server.raw("GET / HTTP/1.1\r\nHost: local");
        assert!(
            response.is_empty() || response.starts_with("HTTP/1.1 408 "),
            "{response}"
        );
        assert!(
            !server.directory.path().join("public/calls").exists(),
            "invalid body reached PHP"
        );
        assert!(server.raw("POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc").ends_with("abc"));
        assert!(server.raw("POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n3\r\ndef\r\n0\r\n\r\n").ends_with("def"));
        let response = server.raw("POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nghi\r\n0\r\n\r\nGET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(
            response.ends_with("ghi") && response.matches("HTTP/1.1 200").count() == 1,
            "{response}"
        );
        assert_eq!(
            std::fs::read(server.directory.path().join("public/calls")).unwrap(),
            b"xxx"
        );
    }
}

#[test]
fn malformed_framing_closes_without_executing_a_pipelined_request() {
    let cases = [
        (
            "conflicting comma lengths",
            "Content-Length: 1, 2\r\n",
            "ab",
        ),
        ("negative length", "Content-Length: -1\r\n", ""),
        (
            "overflow length",
            "Content-Length: 18446744073709551616\r\n",
            "",
        ),
        ("space before colon", "Content-Length : 0\r\n", ""),
        ("tab before colon", "Content-Length\t: 0\r\n", ""),
        (
            "duplicate transfer coding",
            "Transfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n",
            "0\r\n\r\n",
        ),
        (
            "coding chain",
            "Transfer-Encoding: gzip, chunked\r\n",
            "0\r\n\r\n",
        ),
        (
            "repeated coding",
            "Transfer-Encoding: chunked, chunked\r\n",
            "0\r\n\r\n",
        ),
        (
            "hex prefix",
            "Transfer-Encoding: chunked\r\n",
            "0x1\r\na\r\n0\r\n\r\n",
        ),
        (
            "negative chunk",
            "Transfer-Encoding: chunked\r\n",
            "-1\r\na\r\n0\r\n\r\n",
        ),
        (
            "overflow chunk",
            "Transfer-Encoding: chunked\r\n",
            "10000000000000000\r\n",
        ),
        (
            "invalid chunk terminator",
            "Transfer-Encoding: chunked\r\n",
            "1\r\naX\n0\r\n\r\n",
        ),
        (
            "invalid trailer",
            "Transfer-Encoding: chunked\r\n",
            "0\r\nBad Trailer: value\r\n\r\n",
        ),
    ];
    for worker in [false, true] {
        let server = TestServer::start(worker, "body_timeout_ms = 500", "file_put_contents(__DIR__.'/calls', $_SERVER['REQUEST_URI'].PHP_EOL, FILE_APPEND); echo 'executed';");
        for (name, headers, body) in cases {
            let response = server.raw(&format!(
                "POST /malformed HTTP/1.1\r\nHost: localhost\r\n{headers}\r\n{body}GET /followup HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            ));
            assert!(
                response.starts_with("HTTP/1.1 400 "),
                "worker={worker}, {name}: {response}"
            );
            assert_eq!(
                response.matches("HTTP/1.1 ").count(),
                1,
                "worker={worker}, {name}: {response}"
            );
            assert!(
                !server.directory.path().join("public/calls").exists(),
                "worker={worker}, {name} reached PHP"
            );
        }
        assert!(server.get("/healthy").ends_with("executed"));
        assert_eq!(
            std::fs::read_to_string(server.directory.path().join("public/calls")).unwrap(),
            "/healthy\n"
        );
    }
}

#[test]
fn php_dispatch_is_parallel_and_overload_is_bounded() {
    for worker in [false, true] {
        let server = TestServer::start_with_concurrency(worker, "max_inflight_requests = 2\nqueue_timeout_ms = 100\nrequest_timeout_ms = 2000", "file_put_contents(__DIR__.'/started-'.trim($_SERVER['REQUEST_URI'], '/'), 'x'); while (!file_exists(__DIR__.'/release')) { usleep(1000); } echo $_SERVER['REQUEST_URI'];", false, "", 2);
        std::thread::scope(|scope| {
            let first = scope.spawn(|| server.get("/one"));
            let second = scope.spawn(|| server.get("/two"));
            let deadline = Instant::now() + Duration::from_secs(1);
            while !server.directory.path().join("public/started-one").exists()
                || !server.directory.path().join("public/started-two").exists()
            {
                assert!(
                    Instant::now() < deadline,
                    "both workers must start before either finishes: {}",
                    server.logs()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_status(&server.get("/three"), 503);
            std::fs::write(server.directory.path().join("public/release"), "go").unwrap();
            assert!(first.join().unwrap().ends_with("/one"));
            assert!(second.join().unwrap().ends_with("/two"));
        });
    }
}

#[test]
fn timed_out_php_keeps_its_capacity_until_execution_finishes() {
    let server = TestServer::start(
        false,
        "max_inflight_requests = 1\nrequest_timeout_ms = 100\nqueue_timeout_ms = 50",
        "usleep(500000); echo 'done';",
    );
    assert_status(&server.get("/"), 504);
    assert_status(&server.get("/"), 503);
    // Static requests share the memory/admission budget and recover after PHP exits.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if server.get("/asset.txt").starts_with("HTTP/1.1 200 ") {
            break;
        }
        assert!(Instant::now() < deadline, "execution capacity was leaked");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read_through(socket: &mut TcpStream, bytes: &mut Vec<u8>, needle: &[u8]) {
    let mut buffer = [0; 1024];
    while !bytes.windows(needle.len()).any(|part| part == needle) {
        let count = socket.read(&mut buffer).unwrap();
        assert_ne!(
            count,
            0,
            "stream ended early: {}",
            String::from_utf8_lossy(bytes)
        );
        bytes.extend_from_slice(&buffer[..count]);
    }
}

fn read_wire_response(reader: &mut impl std::io::BufRead) -> (String, Vec<u8>) {
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        assert_ne!(
            reader.read_line(&mut line).unwrap(),
            0,
            "missing response headers"
        );
        headers.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let mut body = Vec::new();
    if headers
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked\r\n")
    {
        loop {
            let mut line = String::new();
            assert_ne!(
                reader.read_line(&mut line).unwrap(),
                0,
                "missing chunk terminator"
            );
            let size = usize::from_str_radix(line.trim().split(';').next().unwrap(), 16).unwrap();
            assert!(size <= 1024 * 1024, "unbounded transport chunk");
            if size > 0 {
                let start = body.len();
                body.resize(start + size, 0);
                reader.read_exact(&mut body[start..]).unwrap();
            }
            let mut delimiter = [0; 2];
            reader.read_exact(&mut delimiter).unwrap();
            assert_eq!(&delimiter, b"\r\n");
            if size == 0 {
                break;
            }
        }
    } else if let Some(length) = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().unwrap())
    }) {
        body.resize(length, 0);
        reader.read_exact(&mut body).unwrap();
    } else {
        reader.read_to_end(&mut body).unwrap();
    }
    (headers, body)
}

#[test]
fn streamed_php_preserves_keepalive_framing_and_closes_http10() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "",
            "header('Content-Length: 999'); if ($_SERVER['REQUEST_URI'] === '/flush') { echo 'first'; flush(); echo 'last'; } else { echo '0'; echo substr(str_repeat('0123456789', 15000), 1); }");
        for iteration in 0..40 {
            let path = if iteration < 4 { "/large" } else { "/flush" };
            let mut socket = server.socket();
            write!(socket, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\nGET /asset.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
            let mut reader = std::io::BufReader::new(socket);
            let (headers, body) = read_wire_response(&mut reader);
            assert_status(&headers, 200);
            assert!(!headers.contains("content-length:"), "{headers}");
            assert!(headers.contains("transfer-encoding: chunked"), "{headers}");
            assert_eq!(
                body,
                if path == "/large" {
                    b"0123456789".repeat(15000)
                } else {
                    b"firstlast".to_vec()
                }
            );
            let (headers, body) = read_wire_response(&mut reader);
            assert_status(&headers, 200);
            assert_eq!(body, b"static-content");
            let mut trailing = Vec::new();
            reader.read_to_end(&mut trailing).unwrap();
            assert!(trailing.is_empty());
        }
        let mut socket = server.socket();
        socket
            .write_all(b"GET /large HTTP/1.0\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
            .unwrap();
        let (headers, body) = read_wire_response(&mut std::io::BufReader::new(socket));
        assert!(headers.starts_with("HTTP/1.0 200 "), "{headers}");
        assert!(!headers.contains("transfer-encoding:"));
        assert!(!headers.contains("content-length:"));
        assert_eq!(body, b"0123456789".repeat(15000));
    }
}

#[test]
fn php_flush_delivers_a_chunk_before_execution_finishes() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "request_timeout_ms = 3000\nidle_timeout_ms = 50",
            "echo 'first'; flush(); while (!file_exists(__DIR__.'/release')) { usleep(1000); clearstatcache(); } echo 'last';");
        let mut socket = server.socket();
        socket
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut bytes = Vec::new();
        read_through(&mut socket, &mut bytes, b"5\r\nfirst\r\n");
        let initial = String::from_utf8_lossy(&bytes);
        assert!(initial.starts_with("HTTP/1.1 200 "));
        assert!(initial.contains("transfer-encoding: chunked\r\n"));
        assert!(!initial.contains("content-length:"));
        assert!(!initial.contains("last"));
        std::thread::sleep(Duration::from_millis(150)); // An active body is not an idle keepalive connection.
        std::fs::write(server.directory.path().join("public/release"), "yes").unwrap();
        socket.read_to_end(&mut bytes).unwrap();
        let response = String::from_utf8_lossy(&bytes);
        assert!(response.contains("4\r\nlast\r\n"), "{response}");
        assert!(response.ends_with("0\r\n\r\n"), "{response}");
    }
}

#[test]
fn streamed_php_disconnect_and_stalled_writes_release_native_capacity() {
    for worker in [false, true] {
        for stalled in [false, true] {
            let callback = if stalled {
                "if ($_SERVER['REQUEST_URI'] === '/stream') { file_put_contents(__DIR__.'/entered', 'yes'); while (true) { echo str_repeat('x', 16384); } } echo 'healthy';"
            } else {
                "if ($_SERVER['REQUEST_URI'] === '/stream') { echo 'first'; flush(); while (true) {} } echo 'healthy';"
            };
            let server = TestServer::start_with_concurrency(
                worker,
                "request_timeout_ms = 5000\nwrite_timeout_ms = 100\nmax_inflight_requests = 1",
                callback,
                false,
                "",
                1,
            );
            let mut socket = server.socket();
            socket
                .write_all(b"GET /stream HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .unwrap();
            if stalled {
                let deadline = Instant::now() + Duration::from_secs(2);
                while !server.directory.path().join("public/entered").exists() {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(5));
                }
                // Keep the client open without reading: the send buffer and
                // bounded body queue must fill, then the write deadline cancels PHP.
            } else {
                read_through(&mut socket, &mut Vec::new(), b"5\r\nfirst\r\n");
                socket.shutdown(std::net::Shutdown::Both).unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let response = server.get("/healthy");
                if response.starts_with("HTTP/1.1 200 ") {
                    assert!(response.ends_with("healthy"));
                    break;
                }
                assert_status(&response, 503);
                assert!(
                    Instant::now() < deadline,
                    "stream retained capacity: {}",
                    server.logs()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[test]
fn php_failure_after_flush_aborts_instead_of_finishing_the_chunked_body() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "request_timeout_ms = 3000",
            "if ($_SERVER['REQUEST_URI'] === '/healthy') { echo 'healthy'; return; } echo 'first'; flush(); while (!file_exists(__DIR__.'/release')) { usleep(1000); clearstatcache(); } pox_missing_function();");
        let mut socket = server.socket();
        socket
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut bytes = Vec::new();
        read_through(&mut socket, &mut bytes, b"5\r\nfirst\r\n");
        std::fs::write(server.directory.path().join("public/release"), "yes").unwrap();
        match socket.read_to_end(&mut bytes) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(error) => panic!("stream did not abort: {error}"),
        }
        assert!(
            !bytes.ends_with(b"0\r\n\r\n"),
            "failed PHP emitted a success terminator"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let response = server.get("/healthy");
            if response.starts_with("HTTP/1.1 200 ") {
                assert!(response.ends_with("healthy"));
                break;
            }
            assert_status(&response, 503);
            assert!(Instant::now() < deadline, "{}", server.logs());
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn streamed_php_deadline_and_output_overflow_abort_committed_responses() {
    for worker in [false, true] {
        for path in ["/timeout", "/overflow"] {
            let server = TestServer::start_with_concurrency(worker,
                "request_timeout_ms = 300\nmax_response_bytes = 64\nqueue_timeout_ms = 100",
                "if ($_SERVER['REQUEST_URI'] === '/healthy') { echo 'healthy'; return; } echo 'first'; flush(); if ($_SERVER['REQUEST_URI'] === '/timeout') { while (true) {} } while (!file_exists(__DIR__.'/release')) { usleep(1000); clearstatcache(); } echo str_repeat('x', 100);",
                false, "", 1);
            let mut socket = server.socket();
            write!(
                socket,
                "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut bytes = Vec::new();
            read_through(&mut socket, &mut bytes, b"5\r\nfirst\r\n");
            if path == "/overflow" {
                std::fs::write(server.directory.path().join("public/release"), "yes").unwrap();
            }
            match socket.read_to_end(&mut bytes) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                Err(error) => panic!("failed stream did not close: {error}"),
            }
            assert!(!bytes.ends_with(b"0\r\n\r\n"));
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let response = server.get("/healthy");
                if response.starts_with("HTTP/1.1 200 ") {
                    assert!(response.ends_with("healthy"));
                    break;
                }
                assert_status(&response, 503);
                assert!(Instant::now() < deadline, "{}", server.logs());
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[test]
fn shutdown_callback_failure_does_not_complete_a_streamed_response() {
    let server = TestServer::start(false, "request_timeout_ms = 3000",
        "register_shutdown_function(function () { pox_missing_shutdown_function(); }); echo 'first'; flush(); while (!file_exists(__DIR__.'/release')) { usleep(1000); clearstatcache(); }");
    let mut socket = server.socket();
    socket
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut bytes = Vec::new();
    read_through(&mut socket, &mut bytes, b"5\r\nfirst\r\n");
    std::fs::write(server.directory.path().join("public/release"), "yes").unwrap();
    match socket.read_to_end(&mut bytes) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(error) => panic!("shutdown failure did not abort stream: {error}"),
    }
    assert!(!bytes.ends_with(b"0\r\n\r\n"));
}

#[test]
fn http_deadlines_cancel_busy_php_and_shutdown_callbacks() {
    for worker in [false, true] {
        let server = TestServer::start_with_concurrency(worker,
            "request_timeout_ms = 200\nqueue_timeout_ms = 100\nmax_inflight_requests = 2",
            "if ($_SERVER['REQUEST_URI'] === '/shutdown') { register_shutdown_function(function () { file_put_contents(__DIR__.'/cleanup', 'yes'); while (true) {} }); while (true) {} } if ($_SERVER['REQUEST_URI'] === '/loop') { while (true) {} } echo 'healthy';",
            false, "", 1);
        for route in ["/loop", "/shutdown", "/loop"] {
            let started = Instant::now();
            assert_status(&server.get(route), 504);
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let response = server.get("/healthy");
                if response.starts_with("HTTP/1.1 200 ") {
                    assert!(response.ends_with("healthy"));
                    break;
                }
                assert_status(&response, 503);
                assert!(
                    Instant::now() < deadline,
                    "PHP did not recover: {}",
                    server.logs()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(started.elapsed() < Duration::from_secs(3));
        }
        assert!(
            server.directory.path().join("public/cleanup").exists(),
            "shutdown callback was not exercised"
        );
    }
}

#[test]
fn disconnect_cancels_executing_php_before_its_request_deadline() {
    for worker in [false, true] {
        let server = TestServer::start_with_concurrency(worker,
            "request_timeout_ms = 5000\nqueue_timeout_ms = 100",
            "if ($_SERVER['REQUEST_URI'] === '/loop') { file_put_contents(__DIR__.'/entered', 'yes'); while (true) {} } echo 'healthy';",
            false, "", 1);
        let mut socket = server.socket();
        socket
            .write_all(b"GET /loop HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !server.directory.path().join("public/entered").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        socket.shutdown(std::net::Shutdown::Both).unwrap();
        drop(socket);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let response = server.get("/healthy");
            if response.starts_with("HTTP/1.1 200 ") {
                assert!(response.ends_with("healthy"));
                break;
            }
            assert_status(&response, 503);
            assert!(
                Instant::now() < deadline,
                "disconnected PHP kept running: {}",
                server.logs()
            );
        }
    }
}

#[test]
fn keepalive_static_conditions_and_php_response_headers() {
    let server = TestServer::start(false, "", "header('Set-Cookie: a=1', false); header('Set-Cookie: b=2', false); header('Connection: X-Private'); header('X-Private: secret'); header('Content-Length: 999'); echo 'body';");
    let response = server.get("/");
    assert_status(&response, 200);
    assert!(response.ends_with("body"));
    assert_eq!(response.matches("set-cookie:").count(), 2, "{response}");
    assert!(
        !response.contains("x-private") && !response.contains("999"),
        "{response}"
    );
    let static_response = server.get("/asset.txt");
    let etag = static_response
        .lines()
        .find_map(|line| line.strip_prefix("etag: "))
        .unwrap();
    assert_status(&server.raw(&format!("GET /asset.txt HTTP/1.1\r\nHost: localhost\r\nIf-None-Match: {etag}\r\nConnection: close\r\n\r\n")), 304);
    assert_status(
        &server.raw("POST /asset.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
        405,
    );
    let response = server.raw("HEAD /asset.txt HTTP/1.1\r\nHost: localhost\r\n\r\nGET /asset.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    assert_eq!(response.matches("HTTP/1.1 200 ").count(), 2, "{response}");
    assert_eq!(response.matches("static-content").count(), 1, "{response}");
}

#[test]
fn short_php_status_lines_use_the_parsed_status_without_reading_past_the_string() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "",
            "header(match ($_SERVER['REQUEST_URI']) { '/short' => 'HTTP/', '/partial' => 'HTTP/1', default => 'HTTP/1.1 404 Missing' }); echo 'safe';");
        for (path, expected) in [("/short", 200), ("/partial", 200), ("/normal", 404)] {
            let response = server.get(path);
            assert_status(&response, expected);
            assert!(response.ends_with("safe"));
        }
    }
}

#[cfg(unix)]
fn terminate(server: &TestServer) {
    assert!(Command::new("kill")
        .args(["-TERM", &server.process.0.id().to_string()])
        .status()
        .unwrap()
        .success());
}

#[cfg(unix)]
#[test]
fn shutdown_drains_an_active_request_and_exits_cleanly() {
    for worker in [false, true] {
        let mut server = TestServer::start(
            worker,
            "shutdown_timeout_ms = 2000",
            "file_put_contents(__DIR__.'/started', 'x'); usleep(250000); echo 'drained';",
        );
        let mut socket = server.socket();
        socket
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !server.directory.path().join("public/started").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        terminate(&server);
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        assert!(response.ends_with("drained"), "{response}");
        while server.process.0.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "shutdown hung: {}",
                server.logs()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            server.process.0.wait().unwrap().success(),
            "{}",
            server.logs()
        );
        assert!(server.logs().contains("shutdown_complete"));
    }
}

#[cfg(unix)]
#[test]
fn shutdown_deadline_terminates_hung_php_without_unloading_it() {
    for worker in [false, true] {
        let mut server = TestServer::start(
            worker,
            "shutdown_timeout_ms = 150\nrequest_timeout_ms = 2000",
            "file_put_contents(__DIR__.'/started', 'x'); sleep(60);",
        );
        let mut socket = server.socket();
        socket
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !server.directory.path().join("public/started").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        terminate(&server);
        while server.process.0.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "hung PHP prevented process shutdown"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(server.process.0.wait().unwrap().code(), Some(1));
        assert!(server.logs().contains("shutdown_deadline_exceeded"));
    }
}

#[test]
fn connection_limit_and_idle_deadline_release_socket_capacity() {
    let server = TestServer::start(
        false,
        "max_connections = 2\nidle_timeout_ms = 200\nheader_timeout_ms = 2000",
        "usleep(300000); echo 'done';",
    );
    std::thread::sleep(Duration::from_millis(30));
    let mut first = server.socket();
    let mut second = server.socket();
    first.write_all(b"GET / HTTP/1.1\r\n").unwrap();
    second.write_all(b"GET / HTTP/1.1\r\n").unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let mut excess = server.socket();
    let mut byte = [0];
    match excess.read(&mut byte) {
        Ok(0) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        result => panic!("connection limit did not reject excess client: {result:?}"),
    }
    // Incomplete headers also have the shorter connection idle bound.
    assert_eq!(first.read(&mut byte).unwrap(), 0);
    assert_eq!(second.read(&mut byte).unwrap(), 0);
    // Idle timers must not cancel PHP while the service is executing.
    assert!(server.get("/").ends_with("done"));
    let mut idle = server.socket();
    assert_eq!(idle.read(&mut byte).unwrap(), 0);
}

#[test]
fn blocked_static_writer_is_evicted() {
    let server = TestServer::start(
        false,
        "max_connections = 1\nwrite_timeout_ms = 100\nidle_timeout_ms = 2000",
        "echo 'alive';",
    );
    let large = std::fs::File::create(server.directory.path().join("public/large.bin")).unwrap();
    large.set_len(128 * 1024 * 1024).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let mut blocked = server.socket();
    blocked
        .write_all(b"GET /large.bin HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    // Leave the receiving socket open without consuming data. The kernel send
    // buffer fills; the server must release its only connection permit.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        assert!(
            Instant::now() < deadline,
            "stalled writer retained connection permit: {}",
            server.logs()
        );
        std::thread::sleep(Duration::from_millis(30));
        let mut socket = server.socket();
        if socket
            .write_all(b"GET /asset.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .is_err()
        {
            continue;
        }
        let mut response = String::new();
        if socket.read_to_string(&mut response).is_ok() && response.ends_with("static-content") {
            break;
        }
    }
}

#[test]
fn queue_wait_deadline_rejects_before_execution() {
    let server = TestServer::start(
        false,
        "max_inflight_requests = 4\nqueue_timeout_ms = 50\nrequest_timeout_ms = 2000",
        "file_put_contents(__DIR__.'/started', 'x'); usleep(400000); echo 'done';",
    );
    std::thread::scope(|scope| {
        let active = scope.spawn(|| server.get("/"));
        let deadline = Instant::now() + Duration::from_secs(1);
        while !server.directory.path().join("public/started").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let start = Instant::now();
        assert_status(&server.get("/queued"), 503);
        assert!(start.elapsed() < Duration::from_millis(300));
        assert!(active.join().unwrap().ends_with("done"));
    });
}

#[test]
fn static_ranges_and_directory_redirects_preserve_http_semantics() {
    let server = TestServer::start(false, "", "echo 'index';");
    std::fs::create_dir(server.directory.path().join("public/sub")).unwrap();
    std::fs::write(
        server.directory.path().join("public/sub/index.php"),
        "<?php echo 'sub';",
    )
    .unwrap();
    let response = server.get("/sub?q=%2f");
    assert_status(&response, 308);
    assert!(response.contains("location: /sub/?q=%2f"), "{response}");
    for (range, status, content) in [
        ("bytes=0-5", 206, "static"),
        ("bytes=-7", 206, "content"),
        ("bytes=7-", 206, "content"),
        ("bytes=0-999", 206, "static-content"),
        ("bytes=999-", 416, ""),
        ("bytes=-0", 416, ""),
        ("bytes=10-2", 200, "static-content"),
        ("bytes=0-1,3-4", 200, "static-content"),
    ] {
        let response = server.raw(&format!("GET /asset.txt HTTP/1.1\r\nHost: localhost\r\nRange: {range}\r\nConnection: close\r\n\r\n"));
        assert_status(&response, status);
        assert_eq!(response.split_once("\r\n\r\n").unwrap().1, content);
    }
    let response = server.raw("GET /asset.txt HTTP/1.1\r\nHost: localhost\r\nRange: bytes=0-5\r\nIf-Range: W/\"stale\"\r\nConnection: close\r\n\r\n");
    assert_status(&response, 200);
    assert!(response.ends_with("static-content"));
    assert_status(&server.raw("GET /asset.txt HTTP/1.1\r\nHost: localhost\r\nIf-Match: \"stale\"\r\nConnection: close\r\n\r\n"), 412);
}

#[test]
fn disconnected_queued_request_does_not_execute_php() {
    let server = TestServer::start(false, "queue_timeout_ms = 2000\nrequest_timeout_ms = 2000", "file_put_contents(__DIR__.'/calls', $_SERVER['REQUEST_URI'].\"\\n\", FILE_APPEND); while (!file_exists(__DIR__.'/release')) { usleep(1000); } echo 'done';");
    std::thread::scope(|scope| {
        let first = scope.spawn(|| server.get("/active"));
        let deadline = Instant::now() + Duration::from_secs(1);
        while !server.directory.path().join("public/calls").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut abandoned = server.socket();
        abandoned
            .write_all(b"GET /abandoned HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        drop(abandoned);
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(server.directory.path().join("public/release"), "go").unwrap();
        assert!(first.join().unwrap().ends_with("done"));
    });
    assert!(server.get("/after").ends_with("done"));
    assert_eq!(
        std::fs::read_to_string(server.directory.path().join("public/calls")).unwrap(),
        "/active\n/after\n"
    );
}

#[test]
fn request_headers_preserve_cookies_without_trusting_proxy_identity() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "", "echo json_encode(['name' => $_SERVER['SERVER_NAME'], 'remote' => $_SERVER['REMOTE_ADDR'], 'forwarded' => $_SERVER['HTTP_X_FORWARDED_FOR'] ?? null, 'hop' => $_SERVER['HTTP_X_HOP'] ?? null, 'cookies' => $_COOKIE, 'length' => $_SERVER['CONTENT_LENGTH'], 'body' => file_get_contents('php://input')]);");
        let response = server.raw("POST / HTTP/1.1\r\nHost: [::1]:8080\r\nCookie: a=1\r\nCookie: b=2\r\nX-Forwarded-For: 192.0.2.1\r\nConnection: close, X-Hop\r\nX-Hop: secret\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n");
        assert_status(&response, 200);
        let body: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"name":"::1", "remote":"127.0.0.1", "forwarded":null, "hop":null, "cookies":{"a":"1", "b":"2"}, "length":"3", "body":"abc"})
        );
    }
}

#[test]
fn watcher_reloads_the_persistent_worker_script() {
    let server =
        TestServer::start_with_watch(true, "request_timeout_ms = 2000", "echo 'old';", true);
    assert!(server.get("/").ends_with("old"));
    std::fs::write(
        server.directory.path().join("public/index.php"),
        "<?php while (pox_handle_request(function () { echo 'new'; })) {}",
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if server.get("/").ends_with("new") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "worker reload failed: {}",
            server.logs()
        );
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(server.logs().contains("php_worker_reload"));
}

#[test]
fn native_body_and_header_overflow_return_errors_and_recover() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "max_response_bytes = 1024\nmax_header_bytes = 8192", "if ($_SERVER['REQUEST_URI'] === '/headers') { header('X-Large: '.str_repeat('h', 9000)); } echo str_repeat('x', $_SERVER['REQUEST_URI'] === '/overflow' ? 1025 : 1024);");
        assert_status(&server.get("/overflow"), 502);
        assert_status(&server.get("/headers"), 502);
        let response = server.get("/normal");
        assert_status(&response, 200);
        assert_eq!(response.split_once("\r\n\r\n").unwrap().1.len(), 1024);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn native_output_memory_stays_bounded_while_php_generates_large_output() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "max_response_bytes = 1024", "if ($_SERVER['REQUEST_URI'] === '/large') { for ($i=0; $i<8192; $i++) { echo str_repeat('x', 65536); } } else { echo 'warm'; }");
        let peak_kib = || {
            let status =
                std::fs::read_to_string(format!("/proc/{}/status", server.process.0.id())).unwrap();
            status
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .parse::<u64>()
                .unwrap()
        };
        assert!(server.get("/").ends_with("warm"));
        let before = peak_kib();
        assert_status(&server.get("/large"), 502);
        let after = peak_kib();
        assert!(
            after < before + 16 * 1024,
            "512 MiB of output grew RSS excessively: {before} -> {after} KiB"
        );
        assert!(server.get("/").ends_with("warm"));
    }
}

#[test]
fn cgi_script_metadata_distinguishes_entrypoint_path_info_and_raw_uri() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "", "echo json_encode(['script' => $_SERVER['SCRIPT_NAME'], 'self' => $_SERVER['PHP_SELF'], 'info' => $_SERVER['PATH_INFO'] ?? null, 'translated' => $_SERVER['PATH_TRANSLATED'] ?? null, 'uri' => $_SERVER['REQUEST_URI'], 'query' => $_SERVER['QUERY_STRING'], 'filename' => $_SERVER['SCRIPT_FILENAME']]);");
        let root = server
            .directory
            .path()
            .join("public")
            .canonicalize()
            .unwrap();
        for (uri, query, info) in [
            ("/pretty?x=1", "x=1", None),
            ("/index.php/info%20part?x=%2f", "x=%2f", Some("/info part")),
            ("/%69ndex.php/a+b", "", Some("/a+b")),
            ("/index.php/", "", Some("/")),
            ("/pretty-again", "", None),
        ] {
            let response = server.get(uri);
            assert_status(&response, 200);
            let body: serde_json::Value =
                serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(
                body,
                serde_json::json!({
                    "script":"/index.php", "self":format!("/index.php{}", info.unwrap_or("")),
                    "info":info, "translated":info.map(|info| format!("{}{info}",root.display())),
                    "uri":uri, "query":query, "filename":root.join("index.php"),
                }),
                "worker={worker} uri={uri}"
            );
        }
    }
}

#[test]
fn path_info_only_selects_existing_confined_php_files() {
    let server = TestServer::start(false, "", "echo 'front';");
    let root = server.directory.path().join("public");
    std::fs::create_dir(root.join("directory.php")).unwrap();
    std::fs::write(
        root.join("directory.php/test.PHP"),
        "<?php echo $_SERVER['SCRIPT_NAME'].':'.$_SERVER['PATH_INFO'];",
    )
    .unwrap();
    assert!(server
        .get("/directory.php/test.PHP/more")
        .ends_with("/directory.php/test.PHP:/more"));
    assert!(server.get("/missing.php/more").ends_with("front"));
    assert_status(&server.get("/index.php/%2e%2e/secret"), 403);
    #[cfg(unix)]
    {
        std::fs::write(
            server.directory.path().join("outside.php"),
            "<?php echo 'private';",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            server.directory.path().join("outside.php"),
            root.join("escape.php"),
        )
        .unwrap();
        assert_status(&server.get("/escape.php/more"), 403);
    }
}

#[test]
fn workers_recover_after_exit_and_fatal_without_replaying_requests() {
    let server = TestServer::start(true, "", "if ($_SERVER['REQUEST_URI'] !== '/ok') { file_put_contents(__DIR__.'/attempts', 'x', FILE_APPEND); if ($_SERVER['REQUEST_URI'] === '/exit') { exit(17); } throw new RuntimeException('test failure'); } echo 'healthy';");
    for path in ["/exit", "/fatal", "/exit", "/fatal"] {
        let before = server.logs().matches("php_worker_replaced").count();
        assert_status(&server.get(path), 502);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if server.logs().matches("php_worker_replaced").count() > before {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "worker not replaced: {}",
                server.logs()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(server.get("/ok").ends_with("healthy"));
    }
    assert_eq!(
        std::fs::read(server.directory.path().join("public/attempts")).unwrap(),
        b"xxxx"
    );
}

#[test]
fn worker_replacement_does_not_wait_for_another_active_worker() {
    let server = TestServer::start(true, "request_timeout_ms = 4000", "if ($_SERVER['REQUEST_URI'] === '/hold') { file_put_contents(__DIR__.'/held', 'x'); while (!file_exists(__DIR__.'/release')) { usleep(1000); } } if ($_SERVER['REQUEST_URI'] === '/crash') { exit(1); } echo 'healthy';");
    std::thread::scope(|scope| {
        let held = scope.spawn(|| server.get("/hold"));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !server.directory.path().join("public/held").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_status(&server.get("/crash"), 502);
        while !server.logs().contains("php_worker_replaced") {
            assert!(
                Instant::now() < deadline,
                "recovery waited for unrelated active worker"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(server.get("/ok").ends_with("healthy"));
        std::fs::write(server.directory.path().join("public/release"), "go").unwrap();
        assert!(held.join().unwrap().ends_with("healthy"));
    });
}

#[test]
fn worker_recycling_restarts_php_state_after_request_budget() {
    let server = TestServer::start(true, "worker_max_requests = 2", "$GLOBALS['id'] ??= bin2hex(random_bytes(12)); $GLOBALS['n'] = ($GLOBALS['n'] ?? 0) + 1; echo $GLOBALS['id'].':'.$GLOBALS['n'];");
    let mut incarnations = std::collections::HashMap::<String, usize>::new();
    for _ in 0..16 {
        let response = server.get("/");
        assert_status(&response, 200);
        let body = response.split_once("\r\n\r\n").unwrap().1;
        let (id, count) = body.split_once(':').unwrap();
        let seen = incarnations.entry(id.to_owned()).or_default();
        *seen += 1;
        assert_eq!(count.parse::<usize>().unwrap(), *seen);
        assert!(*seen <= 2, "worker exceeded its request budget");
    }
    assert!(incarnations.len() >= 8);
    assert!(server.logs().contains("php_worker_replaced"));
}

#[test]
fn invalid_or_stuck_worker_startup_fails_before_listening() {
    for script in [
        "<?php syntax error !",
        "<?php return;",
        "<?php while (true) { usleep(1000); }",
    ] {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("worker.php"), script).unwrap();
        std::fs::write(
            directory.path().join("pox.toml"),
            "[server.limits]\nrequest_timeout_ms = 200\n",
        )
        .unwrap();
        let logs = std::fs::File::create(directory.path().join("output")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_pox"))
            .current_dir(directory.path())
            .args([
                "server",
                "--port",
                "0",
                "--worker",
                "worker.php",
                "--workers",
                "1",
            ])
            .stdout(logs.try_clone().unwrap())
            .stderr(logs)
            .spawn()
            .unwrap();
        let mut process = Server(child);
        let deadline = Instant::now() + Duration::from_secs(2);
        while process.0.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "startup did not honor its deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!process.0.wait().unwrap().success());
        let output = std::fs::read_to_string(directory.path().join("output")).unwrap();
        assert!(
            !output.contains("HTTP server listening"),
            "unready worker accepted traffic: {output}"
        );
    }
}

#[test]
fn repeated_worker_startup_failures_use_backoff_and_leave_static_service_available() {
    let server = TestServer::start(true, "", "exit(1);");
    std::fs::write(
        server.directory.path().join("public/index.php"),
        "<?php syntax error !",
    )
    .unwrap();
    assert_status(&server.get("/"), 502);
    assert_status(&server.get("/"), 502);
    std::thread::sleep(Duration::from_millis(1200));
    let replacements = server.logs().matches("php_worker_replaced").count();
    assert!(
        (2..=8).contains(&replacements),
        "unexpected retry rate {replacements}: {}",
        server.logs()
    );
    assert!(server.get("/asset.txt").ends_with("static-content"));
    assert_status(&server.get("/"), 503);
    std::fs::write(
        server.directory.path().join("public/index.php"),
        "<?php while (pox_handle_request(function () { echo 'recovered'; })) {}",
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if server.get("/").ends_with("recovered") {
            break;
        }
        assert!(Instant::now() < deadline, "repaired script was not retried");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn worker_reload_drains_active_callback_before_replacing_script() {
    let server = TestServer::start_with_watch(true, "request_timeout_ms = 4000", "file_put_contents(__DIR__.'/held', 'x'); while (!file_exists(__DIR__.'/release')) { usleep(1000); } echo 'old';", true);
    std::thread::scope(|scope| {
        let active = scope.spawn(|| server.get("/"));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !server.directory.path().join("public/held").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        std::fs::write(
            server.directory.path().join("public/index.php"),
            "<?php while (pox_handle_request(function () { echo 'new'; })) {}",
        )
        .unwrap();
        while !server.logs().contains("php_worker_reload") {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::write(server.directory.path().join("public/release"), "go").unwrap();
        assert!(active.join().unwrap().ends_with("old"));
        assert!(server.get("/").ends_with("new"));
    });
}

#[cfg(unix)]
#[test]
fn repeated_worker_reloads_drain_both_callbacks_and_then_shutdown() {
    let callback = |generation| {
        format!(
        "$uri = $_SERVER['REQUEST_URI']; if ($uri !== '/probe') {{ file_put_contents(__DIR__.'/held-{generation}-'.basename($uri), 'x'); while (!file_exists(__DIR__.'/release-{generation}')) {{ usleep(1000); clearstatcache(); }} }} echo '{generation}:'.$uri;"
    )
    };
    let mut server = TestServer::start_with_watch(
        true,
        "request_timeout_ms = 5000\nshutdown_timeout_ms = 2000",
        &callback(0),
        true,
    );
    let root = server.directory.path().join("public");
    for generation in 0..8 {
        std::thread::scope(|scope| {
            let first = scope.spawn(|| server.get("/first"));
            let second = scope.spawn(|| server.get("/second"));
            let deadline = Instant::now() + Duration::from_secs(4);
            for name in ["first", "second"] {
                while !root.join(format!("held-{generation}-{name}")).exists() {
                    assert!(
                        Instant::now() < deadline,
                        "both workers did not start: {}",
                        server.logs()
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            let reloads = server.logs().matches("php_worker_reload").count();
            std::fs::write(
                root.join("next.tmp"),
                format!(
                    "<?php while (pox_handle_request(function () {{ {} }})) {{}}",
                    callback(generation + 1)
                ),
            )
            .unwrap();
            std::fs::rename(root.join("next.tmp"), root.join("index.php")).unwrap();
            while server.logs().matches("php_worker_reload").count() == reloads {
                assert!(
                    Instant::now() < deadline,
                    "reload did not start: {}",
                    server.logs()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            std::fs::write(root.join(format!("release-{generation}")), "go").unwrap();
            for (response, name) in [
                (first.join().unwrap(), "first"),
                (second.join().unwrap(), "second"),
            ] {
                assert_status(&response, 200);
                assert!(
                    response.ends_with(&format!("{generation}:/{name}")),
                    "{response}"
                );
            }
            let response = server.get("/probe");
            assert_status(&response, 200);
            assert!(
                response.ends_with(&format!("{}:/probe", generation + 1)),
                "{response}"
            );
        });
    }
    terminate(&server);
    let deadline = Instant::now() + Duration::from_secs(3);
    while server.process.0.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "shutdown after reloads hung: {}",
            server.logs()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        server.process.0.wait().unwrap().success(),
        "{}",
        server.logs()
    );
    assert!(server.logs().contains("shutdown_complete"));
}

#[cfg(unix)]
#[test]
fn shutdown_during_worker_reload_drains_or_enforces_its_deadline() {
    for release in [true, false] {
        let mut server = TestServer::start_with_watch(
            true,
            "request_timeout_ms = 5000\nshutdown_timeout_ms = 1500",
            "file_put_contents(__DIR__.'/held', 'x'); while (!file_exists(__DIR__.'/release')) { usleep(1000); clearstatcache(); } echo 'original';",
            true,
        );
        let root = server.directory.path().join("public");
        let mut socket = server.socket();
        socket
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        while !root.join("held").exists() {
            assert!(Instant::now() < deadline, "callback did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        std::fs::write(
            root.join("next.tmp"),
            "<?php while (pox_handle_request(function () { echo 'replacement'; })) {}",
        )
        .unwrap();
        std::fs::rename(root.join("next.tmp"), root.join("index.php")).unwrap();
        while !server.logs().contains("php_worker_reload") {
            assert!(
                Instant::now() < deadline,
                "reload did not start: {}",
                server.logs()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        terminate(&server);
        let shutdown_start = Instant::now();
        // Observe that shutdown closed the listener before unblocking PHP.
        while TcpStream::connect(("127.0.0.1", server.port)).is_ok() {
            assert!(
                shutdown_start.elapsed() < Duration::from_secs(1),
                "listener remained open"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        if release {
            std::fs::write(root.join("release"), "go").unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).unwrap();
            assert_status(&response, 200);
            assert!(response.ends_with("original"), "{response}");
        }
        while server.process.0.try_wait().unwrap().is_none() {
            assert!(
                shutdown_start.elapsed() < Duration::from_secs(3),
                "reload prevented shutdown: {}",
                server.logs()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = server.process.0.wait().unwrap();
        if release {
            assert!(status.success(), "{}", server.logs());
            assert!(server.logs().contains("shutdown_complete"));
        } else {
            assert_eq!(status.code(), Some(1), "{}", server.logs());
            assert!(server.logs().contains("shutdown_deadline_exceeded"));
            assert!(!server.logs().contains("shutdown_complete"));
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn failed_worker_requests_release_native_request_bodies() {
    let server = TestServer::start(
        true,
        "",
        "if ($_SERVER['REQUEST_METHOD'] === 'POST') { exit(1); } echo 'healthy';",
    );
    let body = "x".repeat(4 * 1024 * 1024);
    let request = format!("POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let crash = || {
        // A successful probe on each incarnation distinguishes this cleanup test
        // from the separate test of consecutive startup-failure backoff.
        assert!(server.get("/").ends_with("healthy"));
        assert!(server.get("/").ends_with("healthy"));
        let before = server.logs().matches("php_worker_replaced").count();
        assert_status(&server.raw(&request), 502);
        let deadline = Instant::now() + Duration::from_secs(3);
        while server.logs().matches("php_worker_replaced").count() == before {
            assert!(
                Instant::now() < deadline,
                "replacement did not become available: {}",
                server.logs()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let rss_kib = || {
        let status =
            std::fs::read_to_string(format!("/proc/{}/status", server.process.0.id())).unwrap();
        status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<u64>()
            .unwrap()
    };
    crash();
    crash();
    let before = rss_kib();
    for _ in 0..32 {
        crash();
    }
    let after = rss_kib();
    assert!(
        after < before + 64 * 1024,
        "native bodies leaked across 128 MiB of failed requests: {before} -> {after} KiB"
    );
}

#[test]
fn worker_request_globals_and_filter_input_do_not_leak_between_clients() {
    let server = TestServer::start(true, "", "echo json_encode(['get'=>$_GET, 'post'=>$_POST, 'request'=>$_REQUEST, 'cookie'=>$_COOKIE, 'filter'=>filter_input(INPUT_POST, 'field')]);");
    for _ in 0..2 {
        let response = server.raw("POST /?who=first HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 9\r\nCookie: private=one\r\nConnection: close\r\n\r\nfield=old");
        assert_status(&response, 200);
        let data: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(data["request"]["field"], "old");
        assert_eq!(data["filter"], "old");
    }
    for _ in 0..2 {
        let response = server.get("/?who=second");
        let data: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(data["get"], serde_json::json!({"who":"second"}));
        assert_eq!(data["request"], serde_json::json!({"who":"second"}));
        assert_eq!(data["post"], serde_json::json!([]));
        assert_eq!(data["cookie"], serde_json::json!([]));
        assert!(data["filter"].is_null());
    }
}

#[test]
fn worker_sessions_are_saved_and_isolated_between_clients() {
    let server = TestServer::start(true, "", "session_save_path(__DIR__.'/sessions'); session_start(); $prior = $_SESSION['secret'] ?? null; if (isset($_GET['value'])) { $_SESSION['secret'] = $_GET['value']; } echo json_encode(['id'=>session_id(), 'prior'=>$prior]);");
    std::fs::create_dir(server.directory.path().join("public/sessions")).unwrap();
    let mut ids = Vec::new();
    for path in ["/?value=alpha", "/?value=beta", "/", "/"] {
        let response = server.get(path);
        assert_status(&response, 200);
        let data: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert!(data["prior"].is_null(), "session leaked: {data}");
        let id = data["id"].as_str().unwrap().to_owned();
        assert!(!ids.contains(&id), "new client inherited an old session ID");
        ids.push(id);
    }
    for (id, expected) in ids.iter().take(2).zip(["alpha", "beta"]) {
        let response = server.raw(&format!("GET / HTTP/1.1\r\nHost: localhost\r\nCookie: PHPSESSID={id}\r\nConnection: close\r\n\r\n"));
        let data: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(data["id"], *id);
        assert_eq!(
            data["prior"], expected,
            "session was not flushed before response completion"
        );
    }
}

#[test]
fn worker_uploads_are_processed_then_removed_without_stale_file_globals() {
    let server = TestServer::start(true, "", "if (isset($_FILES['upload'])) { $f=$_FILES['upload']; echo json_encode(['tmp'=>$f['tmp_name'], 'name'=>$f['name'], 'valid'=>is_uploaded_file($f['tmp_name']), 'data'=>file_get_contents($f['tmp_name']), 'post'=>$_POST]); } else { echo json_encode(['files'=>$_FILES, 'post'=>$_POST]); }");
    let body = "--pox-boundary\r\nContent-Disposition: form-data; name=\"field\"\r\n\r\nvalue\r\n--pox-boundary\r\nContent-Disposition: form-data; name=\"upload\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\nfile-content\r\n--pox-boundary--\r\n";
    for _ in 0..2 {
        let response = server.raw(&format!("POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: multipart/form-data; boundary=pox-boundary\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()));
        assert_status(&response, 200);
        let data: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(data["valid"], true);
        assert_eq!(data["name"], "test.txt");
        assert_eq!(data["data"], "file-content");
        assert_eq!(data["post"]["field"], "value");
        assert!(
            !std::path::Path::new(data["tmp"].as_str().unwrap()).exists(),
            "upload survived request cleanup"
        );
    }
    for _ in 0..2 {
        let response = server.get("/");
        let data: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(data, serde_json::json!({"files":[],"post":[]}));
    }
}

#[test]
fn worker_cleanup_preserves_application_owned_streams() {
    let server = TestServer::start(true, "", "if (!isset($GLOBALS['stream'])) { $GLOBALS['stream']=fopen('php://temp', 'w+'); fwrite($GLOBALS['stream'], 'persistent'); } rewind($GLOBALS['stream']); echo stream_get_contents($GLOBALS['stream']);");
    for _ in 0..6 {
        assert!(server.get("/").ends_with("persistent"));
    }
}

#[test]
fn worker_preserves_bootstrap_session_handlers() {
    let server = TestServer::start_with_bootstrap(true, "", "session_start(); $prior = $_SESSION['secret'] ?? null; $_SESSION['secret'] = 'saved'; echo json_encode(['id'=>session_id(), 'prior'=>$prior]);", false, r#"
        session_set_save_handler(
            fn($path, $name) => true,
            fn() => true,
            fn($id) => @file_get_contents(__DIR__.'/sessions/'.$id) ?: '',
            fn($id, $data) => file_put_contents(__DIR__.'/sessions/'.$id, $data) !== false,
            fn($id) => true,
            fn($age) => 0
        );
    "#);
    std::fs::create_dir(server.directory.path().join("public/sessions")).unwrap();
    for _ in 0..6 {
        let response = server.raw("GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        let data: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert!(data["prior"].is_null());
        let id = data["id"].as_str().unwrap();
        assert!(!id.is_empty());
        assert!(server
            .directory
            .path()
            .join("public/sessions")
            .join(id)
            .exists());
        let response = server.raw(&format!("GET / HTTP/1.1\r\nHost: localhost\r\nCookie: PHPSESSID={id}\r\nConnection: close\r\n\r\n"));
        let data: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(data["prior"], "saved");
    }
}

#[test]
fn standard_parallel_requests_reset_php_state_and_recover_after_fatal() {
    let server = TestServer::start_with_concurrency(false, "", "class PerRequestClass {} $prior = $GLOBALS['client'] ?? null; $GLOBALS['client'] = $_GET['client']; if (isset($_GET['fatal'])) { throw new Exception('failed request'); } usleep(10000); echo json_encode(['prior'=>$prior, 'client'=>$GLOBALS['client'], 'query'=>$_GET['client'], 'errors'=>ini_get('display_errors')]);", false, "", 2);
    for batch in 0..10 {
        std::thread::scope(|scope| {
            let calls: Vec<_> = (0..2)
                .map(|slot| {
                    let server = &server;
                    scope.spawn(move || {
                        let client = format!("{batch}-{slot}");
                        let response = server.get(&format!("/?client={client}"));
                        assert_status(&response, 200);
                        let data: serde_json::Value =
                            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1)
                                .unwrap();
                        assert!(data["prior"].is_null());
                        assert_eq!(data["client"], client);
                        assert_eq!(data["query"], client);
                        assert_eq!(data["errors"], "0");
                    })
                })
                .collect();
            for call in calls {
                call.join().unwrap();
            }
        });
        assert_status(&server.get("/?client=failed&fatal=1"), 500);
    }
}

#[test]
fn php_receives_actual_protocol_without_stale_worker_metadata() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "", "echo $_SERVER['SERVER_PROTOCOL'];");
        for version in [
            "HTTP/1.0", "HTTP/1.0", "HTTP/1.1", "HTTP/1.1", "HTTP/1.0", "HTTP/1.1",
        ] {
            let response = server.raw(&format!("GET / {version}\r\nHost: localhost\r\nServer-Protocol: attacker\r\nConnection: close\r\n\r\n"));
            assert!(
                response.starts_with(&format!("{version} 200 ")),
                "{response}"
            );
            assert_eq!(response.split_once("\r\n\r\n").unwrap().1, version);
        }
        let response = server.raw("GET / HTTP/1.0\r\nConnection: close\r\n\r\n");
        assert!(response.ends_with("HTTP/1.0"), "{response}");
    }
}

#[test]
fn php_redirect_status_uses_actual_sapi_protocol() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "", "header('Location: /next');");
        for (version, code) in [
            ("HTTP/1.0", 302),
            ("HTTP/1.1", 303),
            ("HTTP/1.0", 302),
            ("HTTP/1.1", 303),
        ] {
            let response = server.raw(&format!("POST / {version}\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"));
            assert!(
                response.starts_with(&format!("{version} {code} ")),
                "{response}"
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn worker_php_execution_timeout_recovers_and_resets_each_request() {
    let server = TestServer::start_with_concurrency(true, "request_timeout_ms = 5000", "if ($_SERVER['REQUEST_URI'] === '/loop') { while (true) {} } $start = hrtime(true); while (hrtime(true) - $start < 150000000) {} echo 'healthy';", false, "ini_set('max_execution_time', '1');", 1);
    for _ in 0..2 {
        let failed = server.get("/loop");
        assert_status(&failed, 502);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let response = server.get("/");
            if response.ends_with("healthy") {
                break;
            }
            assert_status(&response, 503);
            assert!(
                Instant::now() < deadline,
                "worker did not recover: {}",
                server.logs()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    // Total CPU work exceeds one second; every request must get a fresh budget.
    for _ in 0..10 {
        let response = server.get("/");
        assert_status(&response, 200);
        assert!(response.ends_with("healthy"));
    }
    assert!(
        server.logs().contains("Maximum execution time"),
        "{}",
        server.logs()
    );
}

#[test]
fn php_error_logs_are_bounded_json_and_do_not_leak_to_responses() {
    for worker in [false, true] {
        let server = TestServer::start(
            worker,
            "",
            r#"error_log("quoted \"value\"\nnew line\\end"); error_log('Grüße 世界'); error_log("invalid \xff"); error_log(str_repeat('x', 10000)); trigger_error('private-warning', E_USER_WARNING); echo 'public';"#,
        );
        let response = server.get("/");
        assert_status(&response, 200);
        assert!(response.ends_with("public"));
        assert!(!response.contains("private-warning"));
        let logs = server.logs();
        let errors: Vec<serde_json::Value> = logs
            .lines()
            .map(|line| {
                assert!(line.len() < 4096);
                serde_json::from_str(line).unwrap()
            })
            .filter(|record: &serde_json::Value| record["event"] == "php_error")
            .collect();
        assert!(errors
            .iter()
            .any(|record| record["message"] == "quoted \"value\"\nnew line\\end"));
        assert!(errors
            .iter()
            .any(|record| record["message"] == "Grüße 世界"));
        assert!(errors.iter().any(|record| record["truncated"] == true));
        assert!(errors.iter().any(|record| record["message"]
            .as_str()
            .unwrap()
            .contains("private-warning")));
    }
}

#[test]
fn php_authentication_metadata_is_parsed_and_isolated_between_clients() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "", "echo json_encode(['user'=>$_SERVER['PHP_AUTH_USER'] ?? null, 'password'=>$_SERVER['PHP_AUTH_PW'] ?? null, 'digest'=>$_SERVER['PHP_AUTH_DIGEST'] ?? null, 'type'=>$_SERVER['AUTH_TYPE'] ?? null, 'raw'=>$_SERVER['HTTP_AUTHORIZATION'] ?? null, 'software'=>$_SERVER['SERVER_SOFTWARE']]);");
        for (authorization, user, password, digest, kind) in [
            (
                Some("Basic YWxpY2U6c2VjcmV0"),
                Some("alice"),
                Some("secret"),
                None,
                Some("Basic"),
            ),
            (
                Some("bAsIc Ym9iOm90aGVy"),
                Some("bob"),
                Some("other"),
                None,
                Some("Basic"),
            ),
            (None, None, None, None, None),
            (
                Some("Digest username=\"digest-user\""),
                None,
                None,
                Some("username=\"digest-user\""),
                Some("Digest"),
            ),
            (Some("Basic !!!"), None, None, None, None),
            (Some("Bearer opaque-token"), None, None, None, None),
            (None, None, None, None, None),
        ] {
            for _ in 0..2 {
                let auth = authorization
                    .map(|value| format!("Authorization: {value}\r\n"))
                    .unwrap_or_default();
                let response = server.raw(&format!(
                    "GET / HTTP/1.1\r\nHost: localhost\r\n{auth}Connection: close\r\n\r\n"
                ));
                assert_status(&response, 200);
                let data: serde_json::Value =
                    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
                assert_eq!(
                    data,
                    serde_json::json!({"user":user,"password":password,"digest":digest,"type":kind,"raw":authorization,"software":"pox"})
                );
            }
        }
    }
}

#[test]
fn ambiguous_authentication_and_content_type_are_rejected_before_php() {
    for worker in [false, true] {
        let server = TestServer::start(
            worker,
            "",
            "file_put_contents(__DIR__.'/executed', 'x'); echo 'unexpected';",
        );
        for headers in [
            "Authorization: Basic YWxpY2U6c2VjcmV0\r\nAuthorization: Bearer token\r\n",
            "Content-Type: application/json\r\nContent-Type: application/x-www-form-urlencoded\r\n",
        ] {
            let response = server.raw(&format!("POST / HTTP/1.1\r\nHost: localhost\r\n{headers}Content-Length: 0\r\nConnection: close\r\n\r\n"));
            assert_status(&response, 400);
            assert!(!server.directory.path().join("public/executed").exists());
        }
    }
}

#[test]
fn trusted_proxy_metadata_sets_client_identity_and_resets_https() {
    for worker in [false, true] {
        let server = TestServer::start_with_proxy_config(worker, "", "echo json_encode(['ip'=>$_SERVER['REMOTE_ADDR'], 'remote_port'=>$_SERVER['REMOTE_PORT'], 'host'=>$_SERVER['HTTP_HOST'], 'name'=>$_SERVER['SERVER_NAME'], 'port'=>$_SERVER['SERVER_PORT'], 'https'=>$_SERVER['HTTPS'] ?? null, 'scheme'=>$_SERVER['REQUEST_SCHEME'], 'forwarded'=>$_SERVER['HTTP_X_FORWARDED_FOR'] ?? null]);", false, "", 2, "trusted_proxies = ['127.0.0.1/32', '10.0.0.0/8']");
        for secure in [true, true, false, false, true, false] {
            let (scheme, port) = if secure {
                ("https", 8443)
            } else {
                ("http", 8080)
            };
            let response = server.raw(&format!("GET / HTTP/1.1\r\nHost: internal\r\nX-Forwarded-For: 192.0.2.99, 198.51.100.7, 10.0.0.2\r\nX-Forwarded-Proto: {scheme}\r\nX-Forwarded-Host: public.example:{port}\r\nX-Forwarded-Port: {port}\r\nConnection: close\r\n\r\n"));
            assert_status(&response, 200);
            let data: serde_json::Value =
                serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(
                data,
                serde_json::json!({"ip":"198.51.100.7", "remote_port":"0", "host":format!("public.example:{port}"), "name":"public.example", "port":port.to_string(), "https":if secure {Some("on")} else {None}, "scheme":scheme, "forwarded":null})
            );
        }
        let response = server.get("/");
        let data: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(data["ip"], "127.0.0.1");
        assert_eq!(data["scheme"], "http");
        assert!(data["https"].is_null());
    }
}

#[test]
fn trusted_proxy_rejects_ambiguous_or_invalid_forwarding_metadata() {
    let server = TestServer::start_with_proxy_config(
        false,
        "",
        "file_put_contents(__DIR__.'/executed', 'x');",
        false,
        "",
        1,
        "trusted_proxies = ['127.0.0.1/32']",
    );
    for headers in [
        "X-Forwarded-For: unknown\r\n",
        "X-Forwarded-For: 192.0.2.1,\r\n",
        "X-Forwarded-Proto: https, http\r\n",
        "X-Forwarded-Proto: https\r\nX-Forwarded-Proto: http\r\n",
        "X-Forwarded-Host: user@example.com\r\n",
        "X-Forwarded-Host: a.example,b.example\r\n",
        "X-Forwarded-Port: 0\r\n",
        "X-Forwarded-Port: 65536\r\n",
        "X-Forwarded-Host: public.example:443\r\nX-Forwarded-Port: 8080\r\n",
    ] {
        let response = server.raw(&format!(
            "GET / HTTP/1.1\r\nHost: local\r\n{headers}Connection: close\r\n\r\n"
        ));
        assert_status(&response, 400);
        assert!(!server.directory.path().join("public/executed").exists());
    }
}

#[test]
fn untrusted_proxy_headers_cannot_set_https_or_public_authority() {
    for trusted in ["", "trusted_proxies = ['10.0.0.0/8']"] {
        let server = TestServer::start_with_proxy_config(false, "", "echo json_encode(['ip'=>$_SERVER['REMOTE_ADDR'], 'host'=>$_SERVER['HTTP_HOST'], 'name'=>$_SERVER['SERVER_NAME'], 'https'=>$_SERVER['HTTPS'] ?? null, 'scheme'=>$_SERVER['REQUEST_SCHEME']]);", false, "", 1, trusted);
        let response = server.raw("GET / HTTP/1.1\r\nHost: actual.example\r\nX-Forwarded-For: unknown\r\nX-Forwarded-Proto: https\r\nX-Forwarded-Host: spoof.example\r\nX-Forwarded-Port: 443\r\nForwarded: for=192.0.2.1;proto=https\r\nConnection: close\r\n\r\n");
        assert_status(&response, 200);
        let data: serde_json::Value =
            serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            data,
            serde_json::json!({"ip":"127.0.0.1", "host":"actual.example", "name":"actual.example", "https":null, "scheme":"http"})
        );
    }
}

#[test]
fn recycling_queue_deadline_and_disconnect_do_not_execute_abandoned_requests() {
    for disconnect in [false, true] {
        let limits = if disconnect {
            "worker_max_requests = 1\nqueue_timeout_ms = 1000"
        } else {
            "worker_max_requests = 1\nqueue_timeout_ms = 100"
        };
        let server = TestServer::start_with_concurrency(true, limits, "file_put_contents(__DIR__.'/calls', $_SERVER['REQUEST_URI'].\"\\n\", FILE_APPEND); echo 'done';", false, "if (file_exists(__DIR__.'/hold-bootstrap')) { file_put_contents(__DIR__.'/booting', 'x'); while (!file_exists(__DIR__.'/release')) { usleep(1000); } }", 1);
        let root = server.directory.path().join("public");
        std::fs::write(root.join("hold-bootstrap"), "x").unwrap();
        assert_status(&server.get("/first"), 200);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !root.join("booting").exists() {
            assert!(Instant::now() < deadline, "replacement did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        if disconnect {
            let mut socket = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
            socket
                .write_all(b"GET /abandoned HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .unwrap();
            std::thread::sleep(Duration::from_millis(50));
            drop(socket);
            std::thread::sleep(Duration::from_millis(50));
        } else {
            assert_status(&server.get("/abandoned"), 503);
        }
        std::fs::remove_file(root.join("hold-bootstrap")).unwrap();
        std::fs::write(root.join("release"), "go").unwrap();
        assert_status(&server.get("/after"), 200);
        assert_eq!(
            std::fs::read_to_string(root.join("calls")).unwrap(),
            "/first\n/after\n"
        );
    }
}

#[test]
fn php_bodyless_responses_preserve_metadata_without_poisoning_keepalive() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "", "http_response_code((int) ($_GET['status'] ?? 200)); if (!isset($_GET['omit'])) { header('Content-Length: 8'); } if ($_SERVER['REQUEST_METHOD'] !== 'HEAD') { echo '12345678'; }");
        for (method, status, length) in [
            ("HEAD", 200, Some("8")),
            ("GET", 204, None),
            ("GET", 205, Some("0")),
            ("GET", 304, None),
            ("HEAD", 204, None),
            ("HEAD", 205, Some("0")),
            ("HEAD", 304, None),
        ] {
            let response = server.raw(&format!("{method} /?status={status} HTTP/1.1\r\nHost: localhost\r\n\r\nGET /asset.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"));
            assert_status(&response, status);
            let (headers, rest) = response.split_once("\r\n\r\n").unwrap();
            let actual = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "));
            assert_eq!(actual, length, "{response}");
            assert!(
                rest.starts_with("HTTP/1.1 200 "),
                "unexpected payload before next response: {response}"
            );
            assert!(rest.ends_with("static-content"));
        }
        let response =
            server.raw("HEAD /?omit=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        assert_status(&response, 200);
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(!headers.contains("content-length:"), "{response}");
        assert!(body.is_empty());
    }
}

#[test]
fn invalid_php_response_metadata_and_final_status_return_bad_gateway() {
    for worker in [false, true] {
        let server = TestServer::start(worker, "", "if ($_SERVER['REQUEST_URI'] === '/duplicate') { header('Content-Length: 8', false); header('Content-Length: 9', false); } elseif ($_SERVER['REQUEST_URI'] === '/invalid') { header('Content-Length: invalid'); } else { http_response_code((int) trim($_SERVER['REQUEST_URI'], '/')); } echo 'body';");
        for path in ["/duplicate", "/invalid"] {
            assert_status(
                &server.raw(&format!(
                    "HEAD {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                )),
                502,
            );
        }
        for status in [100, 103, 199, 600, 700] {
            assert_status(&server.get(&format!("/{status}")), 502);
        }
    }
}

#[test]
fn body_metrics_distinguish_stream_failures_from_header_status() {
    for worker in [false, true] {
        for fail in [false, true] {
            let callback = format!("echo 'first'; flush(); while (!file_exists(__DIR__.'/release')) {{ usleep(1000); clearstatcache(); }} {}", if fail { "pox_missing_function();" } else { "echo 'last';" });
            let (server, admin) = admin_server(worker, "request_timeout_ms = 3000", &callback, "");
            let mut socket = server.socket();
            write!(
                socket,
                "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut first = Vec::new();
            read_through(&mut socket, &mut first, b"first\r\n");
            let metrics = exchange(admin, "GET", "/metrics");
            for outcome in ["complete", "error", "dropped"] {
                assert!(
                    metrics.contains(&format!(
                        "pox_http_response_bodies_total{{outcome=\"{outcome}\"}} 0\n"
                    )),
                    "{metrics}"
                );
            }
            assert!(
                metrics.contains("pox_http_responses_total{class=\"2xx\"} 1\n"),
                "{metrics}"
            );
            std::fs::write(server.directory.path().join("public/release"), "yes").unwrap();
            let mut rest = Vec::new();
            let _ = socket.read_to_end(&mut rest);
            let outcome = if fail { "error" } else { "complete" };
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let metrics = exchange(admin, "GET", "/metrics");
                if metrics.contains(&format!(
                    "pox_http_response_bodies_total{{outcome=\"{outcome}\"}} 1\n"
                )) {
                    break;
                }
                assert!(Instant::now() < deadline, "{metrics}");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(server.logs().contains("\"phase\":\"response_headers\""));
            // Hyper can stop polling a static body at Content-Length without EOF.
            std::fs::write(
                server.directory.path().join("public/asset.txt"),
                vec![b'x'; 100_000],
            )
            .unwrap();
            assert!(exchange(server.port, "GET", "/asset.txt").ends_with(&"x".repeat(100_000)));
            assert!(exchange(server.port, "HEAD", "/asset.txt").ends_with("\r\n\r\n"));
            let expected = if fail { 2 } else { 3 };
            let metrics = exchange(admin, "GET", "/metrics");
            assert!(
                metrics.contains(&format!(
                    "pox_http_response_bodies_total{{outcome=\"complete\"}} {expected}\n"
                )),
                "{metrics}"
            );
            assert!(
                metrics.contains("pox_http_response_bodies_total{outcome=\"dropped\"} 0\n"),
                "{metrics}"
            );
        }
    }
}

#[test]
fn body_metrics_count_a_disconnected_pending_stream_once() {
    for worker in [false, true] {
        let (server, admin) = admin_server(
            worker,
            "request_timeout_ms = 3000",
            "echo 'first'; flush(); while (true) { usleep(1000); }",
            "",
        );
        let mut socket = server.socket();
        write!(socket, "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        read_through(&mut socket, &mut Vec::new(), b"first\r\n");
        socket.shutdown(std::net::Shutdown::Both).unwrap();
        drop(socket);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let metrics = exchange(admin, "GET", "/metrics");
            if metrics.contains("pox_http_response_bodies_total{outcome=\"dropped\"} 1\n") {
                assert!(
                    metrics.contains("pox_http_response_bodies_total{outcome=\"complete\"} 0\n"),
                    "{metrics}"
                );
                assert!(
                    metrics.contains("pox_http_response_bodies_total{outcome=\"error\"} 0\n"),
                    "{metrics}"
                );
                break;
            }
            assert!(Instant::now() < deadline, "{metrics}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn body_metrics_reject_premature_static_eof() {
    let (server, admin) = admin_server(false, "write_timeout_ms = 3000", "echo 'unused';", "");
    let path = server.directory.path().join("public/truncated.bin");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(64 * 1024 * 1024).unwrap();
    let mut socket = server.socket();
    write!(
        socket,
        "GET /truncated.bin HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut headers = Vec::new();
    read_through(&mut socket, &mut headers, b"\r\n\r\n");
    assert!(String::from_utf8_lossy(&headers).contains("content-length: 67108864"));
    // The file is larger than the socket buffers, so headers arrive before EOF.
    file.set_len(0).unwrap();
    let mut body = Vec::new();
    let _ = socket.read_to_end(&mut body);
    assert!(body.len() < 64 * 1024 * 1024);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let metrics = exchange(admin, "GET", "/metrics");
        if metrics.contains("pox_http_response_bodies_total{outcome=\"error\"} 1\n") {
            assert!(
                metrics.contains("pox_http_response_bodies_total{outcome=\"complete\"} 0\n"),
                "{metrics}"
            );
            break;
        }
        assert!(Instant::now() < deadline, "{metrics}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
