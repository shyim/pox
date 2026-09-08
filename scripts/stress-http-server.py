#!/usr/bin/env python3
"""Exercise real PHP HTTP requests and report Linux process resource use as JSON.

Uses only Python's standard library. Each request has unique body/URI/cookie data;
non-200 responses, incorrect output, or unclean shutdown make the run fail.
"""
import argparse
import collections
import concurrent.futures
import hashlib
import http.client
import json
import math
import os
import itertools
import platform
import sys
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time


def resources(pid):
    status = Path(f"/proc/{pid}/status").read_text()
    rss = next(int(line.split()[1]) * 1024 for line in status.splitlines() if line.startswith("VmRSS:"))
    return {"rss_bytes": rss, "fds": len(list(Path(f"/proc/{pid}/fd").iterdir()))}


def run(args, mode):
    with tempfile.TemporaryDirectory(prefix="pox-http-stress-") as directory:
        root = Path(directory)
        body = "$counter++; echo json_encode(['uri'=>$_SERVER['REQUEST_URI'], 'cookie'=>$_COOKIE['client'] ?? null, 'hash'=>hash('sha256', file_get_contents('php://input')), 'counter'=>$counter]);"
        if args.flush:
            body += " flush();"
        script = "<?php $counter = 0; " + (f"while (pox_handle_request(function () use (&$counter) {{ {body} }})) {{}}" if mode == "worker" else body)
        (root / "index.php").write_text(script)
        (root / "pox.toml").write_text(f"[server.limits]\nmax_inflight_requests = {max(32, args.clients * 2)}\nqueue_timeout_ms = 5000\nrequest_timeout_ms = 10000\nshutdown_timeout_ms = 10000\nworker_max_requests = {args.recycle}\n")
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        command = [str(args.binary), "server", "--host", "127.0.0.1", "--port", str(port), "--document-root", str(root), "--workers", str(args.workers)]
        if mode == "worker":
            command += ["--worker", str(root / "index.php")]
        environment = dict(os.environ, POX_PHP_RUNTIME=str(args.runtime))
        process = subprocess.Popen(command, cwd=root, env=environment, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.PIPE, text=True, errors="replace")
        log_tail = collections.deque(maxlen=10)
        replacement_count = [0]
        def consume_logs():
            for line in process.stderr:
                log_tail.append(line.rstrip())
                try:
                    entry = json.loads(line)
                    if entry.get("event") == "php_worker_replaced":
                        replacement_count[0] += entry["count"]
                except json.JSONDecodeError:
                    pass
        log_reader = threading.Thread(target=consume_logs)
        log_reader.start()
        samples = collections.deque(maxlen=1440)
        peak_rss = [0]
        completed = [0] * args.clients
        failures = [0] * args.clients
        stop = threading.Event()

        def sample():
            next_report = 0
            while not stop.wait(0.1):
                try:
                    value = resources(process.pid)
                    peak_rss[0] = max(peak_rss[0], value["rss_bytes"])
                    elapsed = time.monotonic() - started
                    if elapsed >= next_report:
                        point = dict(value, elapsed_seconds=round(elapsed, 1), requests=sum(completed), failed_requests=sum(failures),
                                     worker_replacements=replacement_count[0])
                        samples.append(point)
                        if args.duration_seconds:
                            print(json.dumps(dict(event="load_progress", mode=mode, **point)), file=sys.stderr, flush=True)
                        next_report = elapsed + 60
                except (FileNotFoundError, StopIteration, ProcessLookupError):
                    return

        monitor = None
        try:
            deadline = time.monotonic() + 15
            while True:
                if process.poll() is not None:
                    raise RuntimeError("\n".join(log_tail))
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                        break
                except OSError:
                    if time.monotonic() > deadline:
                        raise RuntimeError("server did not listen within 15 seconds")
                    time.sleep(0.02)
            # Warm initialization before taking the memory baseline.
            warm = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
            for _ in range(args.workers * 8):
                warm.request("GET", "/warm")
                response = warm.getresponse()
                response.read()
                if response.status != 200:
                    raise RuntimeError(f"warmup returned {response.status}")
            warm.close()
            initial = resources(process.pid)
            started = time.monotonic()
            monitor = threading.Thread(target=sample)
            monitor.start()

            def client(number):
                connection = http.client.HTTPConnection("127.0.0.1", port, timeout=12)
                latencies = collections.Counter()
                statuses = collections.Counter()
                errors = []
                try:
                    sequences = itertools.count(number, args.clients) if args.duration_seconds else range(number, args.requests, args.clients)
                    for sequence in sequences:
                        elapsed = time.monotonic() - started
                        if args.duration_seconds and elapsed >= args.duration_seconds:
                            break
                        if elapsed >= args.timeout:
                            statuses["deadline"] += 1
                            break
                        uri = f"/request/{number}/{sequence}"
                        payload = (f"{number}:{sequence}:".encode() + b"x" * args.body_bytes)[:args.body_bytes]
                        start = time.monotonic()
                        try:
                            connection.request("POST", uri, payload, {"Cookie": f"client={number}", "Content-Type": "application/octet-stream"})
                            response = connection.getresponse()
                            if args.flush and response.getheader("Transfer-Encoding") != "chunked":
                                raise AssertionError("flushed response was not streamed")
                            data = response.read()
                            statuses[str(response.status)] += 1
                            if response.status != 200:
                                failures[number] += 1
                            if response.status == 200:
                                value = json.loads(data)
                                assert value["uri"] == uri and value["cookie"] == str(number)
                                assert value["hash"] == hashlib.sha256(payload).hexdigest()
                                if mode == "standard":
                                    assert value["counter"] == 1
                                else:
                                    assert value["counter"] > 0
                                    assert args.recycle == 0 or value["counter"] <= args.recycle
                        except Exception as error:
                            statuses["error"] += 1
                            failures[number] += 1
                            if len(errors) < 3:
                                errors.append(f"{type(error).__name__}: {error}")
                            connection.close()
                        latency_ms = (time.monotonic() - start) * 1000
                        # Bounded logarithmic histogram: <=1% bucket width, 1us floor,
                        # one overflow bucket above 1000 seconds. No per-request retention.
                        bucket = math.ceil(math.log(max(0.001, min(latency_ms, 1_000_000))) / math.log(1.01))
                        latencies[bucket] += 1
                        completed[number] += 1
                finally:
                    connection.close()
                return statuses, latencies, errors

            with concurrent.futures.ThreadPoolExecutor(max_workers=args.clients) as pool:
                results = list(pool.map(client, range(args.clients)))
            elapsed = time.monotonic() - started
            stop.set()
            monitor.join()
            time.sleep(0.2)  # Let closed client sockets leave the server resource count.
            final = resources(process.pid)
            process.terminate()
            shutdown_started = time.monotonic()
            exit_code = process.wait(timeout=12)
            shutdown_seconds = time.monotonic() - shutdown_started
            statuses = sum((result[0] for result in results), collections.Counter())
            latencies = sum((result[1] for result in results), collections.Counter())
            request_count = sum(latencies.values())
            def percentile_value(percentile):
                rank = max(1, math.ceil(request_count * percentile / 100))
                total = 0
                for bucket, count in sorted(latencies.items()):
                    total += count
                    if total >= rank:
                        return round(1.01 ** bucket, 3)
                return None
            log_reader.join(timeout=5)
            if log_reader.is_alive():
                raise RuntimeError("server log reader did not terminate")
            report = {
                "mode": mode,
                "requests": request_count,
                "requested_count": None if args.duration_seconds else args.requests,
                "requested_duration_seconds": args.duration_seconds,
                "resource_samples": list(samples),
                "clients": args.clients,
                "workers": args.workers,
                "recycle": args.recycle,
                "body_bytes": args.body_bytes,
                "flush": args.flush,
                "elapsed_seconds": round(elapsed, 3),
                "requests_per_second": round(request_count / elapsed, 1),
                "statuses": dict(statuses),
                "latency_ms": {str(p): percentile_value(p) for p in [50, 95, 99]},
                "latency_histogram_relative_resolution": 0.01,
                "initial": initial,
                "final": final,
                "peak_rss_bytes": max(initial["rss_bytes"], final["rss_bytes"], peak_rss[0]),
                "worker_replacements": replacement_count[0],
                "shutdown_exit": exit_code,
                "shutdown_seconds": round(shutdown_seconds, 3),
                "errors": [error for result in results for error in result[2]][:10],
                "provenance": args.provenance,
                "rss_growth_bytes": final["rss_bytes"] - initial["rss_bytes"],
                "resource_bounds": {
                    "max_rss_growth_bytes": args.max_rss_growth_mib * 1024 * 1024,
                    "max_final_fds": initial["fds"] + args.workers + 2,
                },
            }
            report["passed"] = (
                request_count > 0
                and statuses == {"200": request_count}
                and (elapsed >= args.duration_seconds if args.duration_seconds else request_count == args.requests)
                and not report["errors"]
                and exit_code == 0
                and report["rss_growth_bytes"] <= report["resource_bounds"]["max_rss_growth_bytes"]
                and final["fds"] <= report["resource_bounds"]["max_final_fds"]
            )
            if not report["passed"]:
                report["log_tail"] = list(log_tail)
            return report
        except Exception as error:
            return {
                "mode": mode,
                "passed": False,
                "error": f"{type(error).__name__}: {error}",
                "server_exit": process.poll(),
                "provenance": args.provenance,
                "log_tail": list(log_tail),
            }
        finally:
            stop.set()
            if monitor is not None:
                monitor.join()
            if process.poll() is None:
                process.kill()
                process.wait()
            log_reader.join(timeout=5)
            if not log_reader.is_alive():
                process.stderr.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/pox"))
    parser.add_argument("--runtime", type=Path, default=Path("../pox-runtime/build/libpox_php.so"))
    parser.add_argument("--mode", choices=["both", "standard", "worker"], default="both")
    parser.add_argument("--requests", type=int, default=20000)
    parser.add_argument("--duration-seconds", type=int, default=0,
                        help="Run each mode for this duration instead of a request count; progress goes to stderr")
    parser.add_argument("--clients", type=int, default=16)
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--body-bytes", type=int, default=4096)
    parser.add_argument("--flush", action="store_true", help="Flush every PHP response and require chunked streaming")
    parser.add_argument("--recycle", type=int, default=1000)
    parser.add_argument("--timeout", type=int, default=120)
    parser.add_argument("--max-rss-growth-mib", type=int, default=64)
    args = parser.parse_args()
    if not 1 <= args.clients <= 512 or not 1 <= args.workers <= 128 or (not args.duration_seconds and args.requests < args.clients) or not 1 <= args.body_bytes <= 1048576 or args.recycle < 0 or not 1 <= args.timeout <= 86400 or not 0 <= args.duration_seconds <= 43200 or (args.duration_seconds and args.timeout < args.duration_seconds + 30) or args.max_rss_growth_mib < 0:
        parser.error("invalid workload bounds")
    args.binary = args.binary.resolve(strict=True)
    args.runtime = args.runtime.resolve(strict=True)
    def digest(path):
        with path.open("rb") as file:
            return hashlib.file_digest(file, "sha256").hexdigest()
    args.provenance = {"binary": str(args.binary), "binary_sha256": digest(args.binary), "runtime": str(args.runtime), "runtime_sha256": digest(args.runtime), "platform": platform.platform(), "python": sys.version.split()[0], "cpu_count": os.cpu_count(), "harness_sha256": digest(Path(__file__).resolve()), "started_at_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    passed = True
    for mode in ["standard", "worker"] if args.mode == "both" else [args.mode]:
        report = run(args, mode)
        print(json.dumps(report, sort_keys=True), flush=True)
        passed = passed and report["passed"]
    raise SystemExit(0 if passed else 1)


if __name__ == "__main__":
    main()
