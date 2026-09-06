#!/usr/bin/env python3
"""Seeded malformed-framing checks against a real Pox HTTP process."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import socket
import subprocess
import tempfile
import time


def exchange(port, request):
    with socket.create_connection(("127.0.0.1", port), timeout=2) as client:
        client.settimeout(2)
        client.sendall(request)
        response = bytearray()
        while True:
            try:
                data = client.recv(4096)
            except ConnectionResetError:
                break
            if not data:
                return bytes(response)
            response.extend(data)
            if len(response) > 65536:
                raise AssertionError("unbounded rejection response")
        return bytes(response)


def corpus(seed, count):
    rng = random.Random(seed)
    invalid = b"ghijklmnopqrstuvwxyz!@$^_~"
    for index in range(count):
        digits = str(rng.randrange(10**rng.randrange(1, 70))).encode()
        bad = bytes([rng.choice(invalid)])
        kind = index % 4
        if kind == 0:
            headers, body = b"Content-Length: " + bad + digits + b"\r\n", b""
        elif kind == 1:
            headers, body = b"Content-Length: " + digits + bad + b"\r\n", b""
        elif kind == 2:
            headers = b"Transfer-Encoding: chunked\r\n"
            body = bad + digits + b"\r\nx\r\n0\r\n\r\n"
        else:
            value = rng.randrange(100000)
            headers = (f"Content-Length: {value}\r\nContent-Length: {value + 1}\r\n").encode()
            body = b""
        yield (b"POST /malformed HTTP/1.1\r\nHost: localhost\r\n" + headers +
               b"\r\n" + body + b"GET /pipelined HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")


def run(args, worker):
    with tempfile.TemporaryDirectory(prefix="pox-framing-fuzz-") as temporary:
        root = Path(temporary)
        callback = "file_put_contents(__DIR__.'/calls', $_SERVER['REQUEST_URI'].PHP_EOL, FILE_APPEND); echo 'healthy';"
        script = root / "index.php"
        script.write_text("<?php " + ("while (pox_handle_request(function () { " + callback + " })) {}" if worker else callback))
        (root / "pox.toml").write_text("[server.limits]\nbody_timeout_ms = 500\nheader_timeout_ms = 500\n")
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        command = [str(args.binary), "server", "--host", "127.0.0.1", "--port", str(port), "--document-root", str(root), "--workers", "2"]
        if worker:
            command.extend(["--worker", str(script)])
        environment = dict(os.environ, POX_PHP_RUNTIME=str(args.runtime))
        with (root / "server.log").open("w+b") as log:
            child = subprocess.Popen(command, cwd=root, env=environment, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 10
                while True:
                    assert child.poll() is None, "server exited during startup"
                    try:
                        with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                            break
                    except OSError:
                        assert time.monotonic() < deadline, "startup deadline"
                        time.sleep(0.01)
                digest = hashlib.sha256()
                started = time.monotonic()
                for index, request in enumerate(corpus(args.seed, args.cases)):
                    digest.update(len(request).to_bytes(8, "big"))
                    digest.update(request)
                    response = exchange(port, request)
                    assert response.startswith(b"HTTP/1.1 400 "), (index, request, response)
                    assert response.count(b"HTTP/1.1 ") == 1, (index, response)
                    assert not (root / "calls").exists(), (index, "invalid or pipelined request reached PHP")
                    healthy = exchange(port, b"GET /healthy HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    assert healthy.startswith(b"HTTP/1.1 200 ") and healthy.endswith(b"healthy"), (index, healthy)
                    assert (root / "calls").read_text() == "/healthy\n", index
                    (root / "calls").unlink()
                child.terminate()
                assert child.wait(timeout=5) == 0, "unclean shutdown"
                return dict(mode="worker" if worker else "standard", cases=args.cases,
                            seed=args.seed, corpus_sha256=digest.hexdigest(),
                            elapsed_seconds=round(time.monotonic() - started, 3), passed=True)
            except Exception:
                log.flush()
                log.seek(0)
                print(log.read()[-8192:].decode(errors="replace"))
                raise
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--seed", type=int, default=20260906)
    parser.add_argument("--cases", type=int, default=1024)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.runtime = args.runtime.resolve(strict=True)
    if args.cases < 4:
        parser.error("--cases must be at least 4")
    result = dict(binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                  runtime_sha256=hashlib.sha256(args.runtime.read_bytes()).hexdigest(),
                  harness_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  scope="Seeded invalid Content-Length, conflicting lengths and malformed chunk sizes; not exhaustive protocol fuzzing.",
                  reports=[run(args, worker) for worker in [False, True]])
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
