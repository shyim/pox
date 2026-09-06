#!/usr/bin/env python3
"""Verify module startup failure and recovery in fresh HTTP server processes."""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time


def unused_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def stop(child):
    if child.poll() is None:
        child.kill()
    child.wait(timeout=5)


def run_mode(args, worker):
    with tempfile.TemporaryDirectory(prefix="pox-startup-failure-") as temporary:
        root = Path(temporary)
        script = root / "index.php"
        callback = "echo 'healthy';"
        script.write_text("<?php " + (
            "while (pox_handle_request(function () { " + callback + " })) {}"
            if worker else callback))
        config = root / "pox.toml"
        config.write_text("[php.ini]\nextension = " + json.dumps(str(args.extension)) + "\n")
        port = unused_port()
        command = [str(args.binary), "server", "--host", "127.0.0.1", "--port",
                   str(port), "--document-root", str(root)]
        if worker:
            command.extend(["--worker", str(script)])
        environment = dict(os.environ, POX_PHP_RUNTIME=str(args.runtime))
        with (root / "failure.log").open("w+b") as log:
            started = time.monotonic()
            child = subprocess.Popen(command, cwd=root, env=environment,
                                     stdout=log, stderr=log)
            exposed = False
            try:
                while child.poll() is None and time.monotonic() - started < 10:
                    try:
                        with socket.create_connection(("127.0.0.1", port), timeout=0.02):
                            exposed = True
                    except OSError:
                        pass
                    time.sleep(0.005)
                assert child.poll() is not None, "startup failure did not terminate"
                elapsed = time.monotonic() - started
                log.seek(0)
                output = log.read().decode(errors="replace")
                assert child.returncode > 0, (child.returncode, output)
                assert "Unable to start pox_test_startup_failure module" in output, output
                assert not exposed, "failed startup exposed an HTTP listener"
                failed_exit = child.returncode
            finally:
                stop(child)

        config.unlink()
        with (root / "recovery.log").open("w+b") as log:
            child = subprocess.Popen(command, cwd=root, env=environment,
                                     stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 10
                while True:
                    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=1)
                    try:
                        connection.request("GET", "/")
                        response = connection.getresponse()
                        assert response.status == 200 and response.read() == b"healthy"
                        break
                    except ConnectionRefusedError:
                        assert child.poll() is None, "recovery process exited"
                        assert time.monotonic() < deadline, "recovery startup timed out"
                        time.sleep(0.02)
                    finally:
                        connection.close()
                child.terminate()
                child.wait(timeout=5)
                log.seek(0)
                recovery_output = log.read().decode(errors="replace")
                assert child.returncode == 0, recovery_output
                assert "shutdown_complete" in recovery_output, recovery_output
            finally:
                stop(child)
        return {"mode": "worker" if worker else "standard", "passed": True,
                "startup_exit": failed_exit, "failure_seconds": round(elapsed, 3),
                "listener_observed": exposed, "failure_output": output,
                "fresh_process_recovered": True, "recovery_shutdown_exit": 0}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "runtime", "extension"):
        parser.add_argument("--" + name, required=True, type=lambda p: Path(p).resolve(strict=True))
    args = parser.parse_args()
    provenance = {name: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                  for name, path in vars(args).items()}
    provenance["harness_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    print(json.dumps({"provenance": provenance,
                      "results": [run_mode(args, worker) for worker in (False, True)]}))


if __name__ == "__main__":
    main()
