#!/usr/bin/env python3
"""Verify Pox behind Caddy TLS on loopback, without modifying system trust.

Requires an installed Caddy, OpenSSL and curl with HTTP/2 support. Uses a temporary
certificate, configuration and application; all child processes are cleaned up.
"""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import resource
import signal
import socket
import ssl
import subprocess
import tempfile
import time
import uuid


def port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def wait_for(check, message, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if check():
                return
        except (OSError, http.client.HTTPException):
            pass
        time.sleep(0.02)
    raise AssertionError(message)


def no_core():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


class SystemdBackend:
    """Own one temporary user unit rendered from the deployment service."""
    def __init__(self, command, root, runtime):
        self.unit = "pox-deployment-" + uuid.uuid4().hex + ".service"
        directory = Path(os.environ["XDG_RUNTIME_DIR"]) / "systemd/user"
        directory.mkdir(parents=True, exist_ok=True)
        self.path = directory / self.unit
        template = Path(__file__).resolve().parent.parent / "examples/deployment/pox.service"
        def quote(value, command=False):
            # systemd command quoting; disable specifier/environment expansion.
            value = str(value).replace('$', '$$') if command else str(value)
            return '"' + value.replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%') + '"'
        text = template.read_text().replace("User=pox\n", "").replace("Group=pox\n", "")
        text = text.replace("WorkingDirectory=/srv/pox/app", "WorkingDirectory=" + str(root).replace("%", "%%"))
        text = text.replace("Environment=POX_PHP_RUNTIME=/opt/pox/runtime/libpox_php.so",
                            "Environment=" + quote("POX_PHP_RUNTIME=" + str(runtime)))
        text = text.replace("ExecStart=/usr/local/bin/pox server", "ExecStart=" + " ".join(quote(part, command=True) for part in command))
        self.path.write_text(text)
        try:
            self.control("daemon-reload")
            self.control("start", self.unit)
        except BaseException:
            self.path.unlink(missing_ok=True)
            self.control("daemon-reload")
            raise

    def control(self, *arguments):
        return subprocess.check_output(["systemctl", "--user", *arguments], text=True, timeout=45)

    def properties(self):
        return dict(line.split("=", 1) for line in self.control("show", self.unit,
                    "--property=MainPID,ExecMainCode,ExecMainStatus,NRestarts,ActiveState,SubState,Result").splitlines())

    def logs(self):
        return subprocess.check_output(["journalctl", "--user", "--unit=" + self.unit,
                                        "--output=cat", "--no-pager"], text=True, timeout=10)

    def send_signal(self, sig):
        if sig == signal.SIGTERM:
            self.control("--no-block", "stop", self.unit)
        else:
            self.control("kill", "--kill-whom=main", "--signal=" + str(int(sig)), self.unit)

    def wait(self, timeout=5):
        state = {}
        def exited():
            state.update(self.properties())
            return state["MainPID"] == "0"
        wait_for(exited, "systemd backend did not exit", timeout)
        status = int(state["ExecMainStatus"])
        return -status if state["ExecMainCode"] in ("2", "3") else status

    def close(self):
        try:
            self.control("stop", self.unit)
            subprocess.run(["systemctl", "--user", "reset-failed", self.unit],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
        finally:
            self.path.unlink(missing_ok=True)
            self.control("daemon-reload")


def run_mode(args, worker):
    with tempfile.TemporaryDirectory(prefix="pox-deployment-") as temporary:
        root = Path(temporary)
        public = root / "public"
        public.mkdir()
        backend, management, frontend = port(), port(), port()
        callback = """
if ($_SERVER['REQUEST_URI'] === '/crash') { file_put_contents(__DIR__.'/crash-entered', 'yes'); while (true) {} }
if ($_SERVER['REQUEST_URI'] === '/stream') {
    header('Content-Type: text/plain'); echo 'first'; flush();
    while (!file_exists(__DIR__.'/stream-release')) { usleep(1000); clearstatcache(); }
    echo 'last'; return;
}
if ($_SERVER['REQUEST_URI'] === '/slow') {
    file_put_contents(__DIR__.'/entered', 'yes');
    while (!file_exists(__DIR__.'/release')) { usleep(1000); clearstatcache(); }
}
header('Content-Type: application/json');
echo json_encode([
    'https' => $_SERVER['HTTPS'] ?? null,
    'scheme' => $_SERVER['REQUEST_SCHEME'],
    'host' => $_SERVER['HTTP_HOST'],
    'server_name' => $_SERVER['SERVER_NAME'],
    'server_port' => (int) $_SERVER['SERVER_PORT'],
    'remote' => $_SERVER['REMOTE_ADDR'],
    'protocol' => $_SERVER['SERVER_PROTOCOL'],
    'forwarded_port' => $_SERVER['HTTP_X_FORWARDED_PORT'] ?? null,
    'uri' => $_SERVER['REQUEST_URI'],
    'body' => file_get_contents('php://input'),
]);
"""
        if args.fault_extension:
            callback = callback.replace("=== '/crash'", "=== '/crash' || $_SERVER['REQUEST_URI'] === '/crash-stream'")
            callback = callback.replace("while (true) {}", """
if ($_SERVER['REQUEST_URI'] === '/crash-stream') { echo 'first'; flush(); }
while (!file_exists(__DIR__.'/crash-release')) { usleep(1000); clearstatcache(); }
pox_test_native_fault();
""")
            callback = """
if ($_SERVER['REQUEST_URI'] === '/fault-extension') {
    echo function_exists('pox_test_native_fault') ? 'loaded' : 'missing'; return;
}
""" + callback
        script = public / "index.php"
        script.write_text("<?php " + (
            f"while (pox_handle_request(function () {{ {callback} }})) {{}}"
            if worker else callback
        ))
        (public / "asset.txt").write_text("static-content")
        (root / "pox.toml").write_text(
            f"[server]\ntrusted_proxies = ['127.0.0.1/32']\n"
            f"admin_address = '127.0.0.1:{management}'\n"
            "[server.limits]\nrequest_timeout_ms = 5000\nshutdown_timeout_ms = 3000\n"
        )
        if args.fault_extension:
            with (root / "pox.toml").open("a") as config:
                config.write("[php.ini]\nextension = " + json.dumps(str(args.fault_extension)) + "\n")
        cert, key = root / "cert.pem", root / "key.pem"
        subprocess.run([
            "openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256",
            "-nodes", "-days", "1", "-subj", "/CN=localhost", "-addext", "subjectAltName=DNS:localhost",
            "-keyout", str(key), "-out", str(cert),
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        template = Path(__file__).resolve().parent.parent / "examples/deployment/Caddyfile"
        config = "{\n admin off\n auto_https disable_redirects\n}\n" + template.read_text()
        config = config.replace("{$POX_SITE} {", f"https://localhost:{frontend} {{\n bind 127.0.0.1\n tls {cert} {key}")
        (root / "Caddyfile").write_text(config)
        environment = dict(os.environ, POX_PHP_RUNTIME=str(args.runtime),
                           POX_UPSTREAM=f"127.0.0.1:{backend}",
                           XDG_CONFIG_HOME=str(root / "config"), XDG_DATA_HOME=str(root / "data"))
        command = [str(args.binary), "server", "--host", "127.0.0.1", "--port", str(backend),
                   "--document-root", str(public), "--workers", "2"]
        if worker:
            command.extend(["--worker", str(script)])
        processes = []
        supervised = None
        supervisor_evidence = None
        context = ssl.create_default_context(cafile=str(cert))

        def https(path="/", headers=None, body=None):
            connection = http.client.HTTPSConnection("localhost", frontend, context=context, timeout=5)
            try:
                connection.request("POST" if body is not None else "GET", path, body=body, headers=headers or {})
                response = connection.getresponse()
                return response.status, response.read()
            finally:
                connection.close()

        def ready():
            connection = http.client.HTTPConnection("127.0.0.1", management, timeout=1)
            try:
                connection.request("GET", "/ready")
                response = connection.getresponse()
                response.read()
                return response.status == 200
            finally:
                connection.close()

        with (root / "pox.log").open("wb") as pox_log, (root / "caddy.log").open("wb") as caddy_log:
            def start_backend():
                child = subprocess.Popen(command, cwd=root, env=environment, stdout=pox_log,
                                         stderr=pox_log, preexec_fn=no_core)
                processes.append(child)
                wait_for(ready, "Pox did not become ready")
                assert child.poll() is None
                return child

            try:
                if args.systemd:
                    supervised = SystemdBackend(command, root, args.runtime)
                    backend_process = supervised
                    wait_for(ready, "supervised Pox did not become ready")
                    original_pid = supervised.properties()["MainPID"]
                else:
                    backend_process = start_backend()
                subprocess.run([str(args.caddy), "validate", "--config", str(root / "Caddyfile")],
                               env=environment, check=True, stdout=caddy_log, stderr=caddy_log)
                caddy = subprocess.Popen([str(args.caddy), "run", "--config", str(root / "Caddyfile")],
                                         env=environment, stdout=caddy_log, stderr=caddy_log)
                processes.append(caddy)
                wait_for(lambda: https("/asset.txt") == (200, b"static-content"), "TLS proxy did not start")
                if args.fault_extension:
                    assert https("/fault-extension") == (200, b"loaded"), "test extension did not load"
                spoof = {"X-Forwarded-For": "198.51.100.99", "X-Forwarded-Proto": "http",
                         "X-Forwarded-Host": "attacker.invalid", "X-Forwarded-Port": "1",
                         "Forwarded": "for=198.51.100.99;proto=http;host=attacker.invalid"}
                status, data = https("/identity?raw=a%2Fb", spoof, b"binary\x00payload")
                identity = json.loads(data)
                assert status == 200, (status, data)
                assert identity == {
                    "https": "on", "scheme": "https", "host": f"localhost:{frontend}",
                    "server_name": "localhost", "server_port": frontend, "remote": "127.0.0.1",
                    "protocol": "HTTP/1.1", "forwarded_port": None,
                    "uri": "/identity?raw=a%2Fb", "body": "binary\x00payload",
                }, identity
                # HTTP/2 terminates at Caddy; the independently validated Pox hop remains HTTP/1.1.
                h2 = subprocess.run(["curl", "--silent", "--show-error", "--noproxy", "*", "--http2",
                                     "--cacert", str(cert), f"https://localhost:{frontend}/h2",
                                     "--output", str(root / "h2.json"), "--write-out", "%{http_version} %{http_code}"],
                                    check=True, capture_output=True, text=True)
                assert h2.stdout == "2 200", h2.stdout
                assert json.loads((root / "h2.json").read_text())["protocol"] == "HTTP/1.1"
                # The management listener is never selected as a public upstream.
                status, data = https("/metrics")
                assert status == 200 and json.loads(data)["uri"] == "/metrics"
                streaming = http.client.HTTPSConnection("localhost", frontend, context=context, timeout=5)
                try:
                    streaming.request("GET", "/stream")
                    response = streaming.getresponse()
                    assert response.status == 200 and response.read(5) == b"first"
                    (public / "stream-release").write_text("yes")
                    assert response.read() == b"last"
                finally:
                    streaming.close()
                # Loss of the PHP process must be visible, and the proxy must recover after restart.
                crashing = http.client.HTTPSConnection("localhost", frontend, context=context, timeout=5)
                try:
                    crashing.request("GET", "/crash")
                    wait_for(lambda: (public / "crash-entered").exists(), "crash request did not enter PHP")
                    if args.fault_extension:
                        (public / "crash-release").write_text("yes")
                    else:
                        backend_process.send_signal(signal.SIGKILL)
                    expected_signal = signal.SIGSEGV if args.fault_extension else signal.SIGKILL
                    assert backend_process.wait(timeout=5) == -expected_signal
                    response = crashing.getresponse()
                    assert response.status == 502, response.status
                    response.read()
                finally:
                    crashing.close()
                assert caddy.poll() is None
                if supervised:
                    wait_for(lambda: supervised.properties()["MainPID"] not in ("0", original_pid) and ready(),
                             "systemd did not automatically replace Pox")
                    restarted = supervised.properties()
                    assert int(restarted["NRestarts"]) == 1
                    supervisor_evidence = {"original_pid": int(original_pid),
                                           "replacement_pid": int(restarted["MainPID"]),
                                           "automatic_restarts": int(restarted["NRestarts"]),
                                           "termination_signal": expected_signal.name}
                else:
                    backend_process = start_backend()
                wait_for(lambda: https("/asset.txt") == (200, b"static-content"), "proxy did not recover")
                if args.fault_extension:
                    assert https("/fault-extension") == (200, b"loaded")
                    (public / "crash-release").unlink()
                    (public / "crash-entered").unlink()
                    previous_pid = supervised.properties()["MainPID"] if supervised else backend_process.pid
                    broken_stream = http.client.HTTPSConnection("localhost", frontend, context=context, timeout=5)
                    try:
                        broken_stream.request("GET", "/crash-stream")
                        response = broken_stream.getresponse()
                        assert response.status == 200 and response.read(5) == b"first"
                        (public / "crash-release").write_text("yes")
                        assert backend_process.wait(timeout=5) == -signal.SIGSEGV
                        try:
                            response.read()
                        except http.client.IncompleteRead:
                            pass
                        else:
                            raise AssertionError("native crash incorrectly completed the streamed body")
                    finally:
                        broken_stream.close()
                    if supervised:
                        wait_for(lambda: supervised.properties()["MainPID"] not in ("0", previous_pid) and ready(),
                                 "systemd did not recover after streamed native crash")
                        restarted = supervised.properties()
                        assert int(restarted["NRestarts"]) == 2
                        supervisor_evidence["streamed_fault"] = {
                            "original_pid": int(previous_pid), "replacement_pid": int(restarted["MainPID"]),
                            "automatic_restarts": int(restarted["NRestarts"]), "termination_signal": "SIGSEGV"}
                    else:
                        backend_process = start_backend()
                    wait_for(lambda: https("/fault-extension") == (200, b"loaded"),
                             "PHP did not recover after streamed native crash")
                # Drain an active TLS request before shutting down its PHP process.
                connection = http.client.HTTPSConnection("localhost", frontend, context=context, timeout=5)
                try:
                    connection.request("GET", "/slow")
                    wait_for(lambda: (public / "entered").exists(), "request did not reach PHP")
                    backend_process.send_signal(signal.SIGTERM)
                    (public / "release").write_text("yes")
                    response = connection.getresponse()
                    assert response.status == 200 and json.loads(response.read())["uri"] == "/slow"
                finally:
                    connection.close()
                assert backend_process.wait(timeout=5) == 0
                if supervised:
                    time.sleep(2.2)
                    state = supervised.properties()
                    assert state["ActiveState"] == "inactive" and state["MainPID"] == "0", state
                    wait_for(lambda: '{"event":"shutdown_complete"}' in supervised.logs(),
                             "supervised Pox did not report completed shutdown")
                    supervisor_evidence["after_stop"] = state
                    supervisor_evidence["shutdown_complete_logged"] = True
                    if args.crash_loop:
                        # Reset while active: stopped successful units may already be
                        # unloaded. The current process precedes the fresh rate window;
                        # exactly five subsequent starts are allowed in that window.
                        supervised.control("start", supervised.unit)
                        wait_for(ready, "Pox did not start for crash-loop validation")
                        supervised.control("reset-failed", supervised.unit)
                        crash_pids = []
                        for attempt in range(6):
                            current_pid = supervised.properties()["MainPID"]
                            assert current_pid != "0"
                            crash_pids.append(int(current_pid))
                            supervised.send_signal(signal.SIGKILL)
                            assert supervised.wait(timeout=5) == -signal.SIGKILL
                            if attempt < 5:
                                wait_for(lambda: supervised.properties()["MainPID"] not in ("0", current_pid) and ready(),
                                         "restart failed before the configured start limit")
                        wait_for(lambda: supervised.properties()["Result"] == "start-limit-hit",
                                 "systemd did not enforce the configured crash-loop limit")
                        limited = supervised.properties()
                        assert limited["ActiveState"] == "failed" and limited["MainPID"] == "0", limited
                        assert https("/asset.txt")[0] == 502
                        refused = subprocess.run(["systemctl", "--user", "start", supervised.unit],
                                                 capture_output=True, text=True, timeout=10)
                        assert refused.returncode != 0, "rate-limited unit unexpectedly restarted"
                        supervised.control("reset-failed", supervised.unit)
                        supervised.control("start", supervised.unit)
                        wait_for(ready, "operator reset did not restore PHP readiness")
                        wait_for(lambda: https("/identity")[0] == 200, "TLS did not recover after operator reset")
                        supervisor_evidence["crash_loop"] = {
                            "killed_pids": crash_pids, "limited_state": limited,
                            "start_refused_before_reset": True,
                            "recovered_pid": int(supervised.properties()["MainPID"])}
                        supervised.send_signal(signal.SIGTERM)
                        assert supervised.wait(timeout=5) == 0
                return {"supervisor": "systemd-user" if supervised else "harness",
                        "supervisor_evidence": supervisor_evidence,
                        "mode": "worker" if worker else "standard", "passed": True,
                        "checks": ["verified TLS", "forwarding spoof rejection", "binary request body",
                                   "HTTP/2 frontend to HTTP/1.1 backend", "private management listener",
                                   "forced process loss produces 502", "proxy recovery after backend restart", "graceful TLS drain", "early PHP flush through TLS"] +
                                  (["automatic systemd restart", "systemd stop drains without restart"] if supervised else []) +
                                  (["native extension memory fault produces SIGSEGV", "native crash aborts committed TLS response"] if args.fault_extension else []) +
                                  (["systemd crash-loop limit", "operator reset restores service"] if args.crash_loop else [])}
            except Exception:
                if supervised:
                    print(supervised.logs())
                print((root / "pox.log").read_text(errors="replace"))
                print((root / "caddy.log").read_text(errors="replace"))
                raise
            finally:
                for child in reversed(processes):
                    if child.poll() is None:
                        child.terminate()
                        try:
                            child.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            child.kill()
                            child.wait()
                if supervised:
                    supervised.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--caddy", type=Path, required=True)
    parser.add_argument("--systemd", action="store_true",
                        help="Linux: test automatic recovery using an isolated systemd user unit")
    parser.add_argument("--fault-extension", type=Path,
                        help="test-only native fault extension built against the matching PHP headers")
    parser.add_argument("--crash-loop", action="store_true",
                        help="With --systemd, verify crash-loop limiting and operator reset")
    args = parser.parse_args()
    if args.crash_loop and not args.systemd:
        parser.error("--crash-loop requires --systemd")
    for name in ("binary", "runtime", "caddy"):
        setattr(args, name, getattr(args, name).resolve(strict=True))
    if args.fault_extension:
        args.fault_extension = args.fault_extension.resolve(strict=True)
    provenance = {name: {"path": str(getattr(args, name)),
                         "sha256": hashlib.sha256(getattr(args, name).read_bytes()).hexdigest()}
                  for name in ("binary", "runtime", "caddy")}
    provenance["caddy_version"] = subprocess.check_output([str(args.caddy), "version"], text=True).strip()
    if args.fault_extension:
        provenance["fault_extension_sha256"] = hashlib.sha256(args.fault_extension.read_bytes()).hexdigest()
    if args.systemd:
        provenance["systemd_version"] = subprocess.check_output(["systemctl", "--version"], text=True).splitlines()[0]
        template = Path(__file__).resolve().parent.parent / "examples/deployment/pox.service"
        provenance["service_template_sha256"] = hashlib.sha256(template.read_bytes()).hexdigest()
    print(json.dumps({"provenance": provenance, "results": [run_mode(args, worker) for worker in (False, True)]}))


if __name__ == "__main__":
    main()
