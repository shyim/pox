# Deploying behind Caddy

Pox's listener implements HTTP/1.0 and HTTP/1.1. Terminate public TLS at a reverse
proxy, and keep the Pox listener on loopback or a private network. The checked-in
[Caddyfile](../examples/deployment/Caddyfile) was exercised with Caddy 2.11.4,
real certificate verification, and both standard and persistent-worker modes.
Overall production hardening remains in progress; see the
[completion review](http-server-review.md).

For a single-host deployment, configure the application:

```toml
[server]
host = "127.0.0.1"
port = 8000
document_root = "public"
trusted_proxies = ["127.0.0.1/32"]
admin_address = "127.0.0.1:9180"

[server.limits]
shutdown_timeout_ms = 30000
```

Start Pox from the application directory with the installed native runtime:

```bash
POX_PHP_RUNTIME=/opt/pox/runtime/libpox_php.so pox server
```

Use the deployment Caddyfile with your public domain:

```bash
POX_SITE=app.example.com POX_UPSTREAM=127.0.0.1:8000 \
  caddy run --config /path/to/pox/examples/deployment/Caddyfile
```

Caddy's [automatic HTTPS](https://caddyserver.com/docs/automatic-https) can obtain
and renew a certificate for an eligible public domain. Domain routing, ACME
issuance and renewal depend on the deployment and were not exercised by the local
test. That test uses a temporary certificate trusted only by its test clients;
it neither disables certificate verification nor modifies system trust.

The template relies on Caddy's documented handling of
[X-Forwarded-For, Proto and Host](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy#headers).
It additionally removes `X-Forwarded-Port`, so a client cannot supply a port
override to Pox. Pox derives the public port from the forwarded authority and
scheme. The loopback CIDR trusts local connections; on separate hosts, use the
proxy's actual source CIDR and restrict backend access accordingly. This example
assumes Caddy is the public edge, with no additional proxy/CDN ahead of it.

Only port 8000 is a proxy upstream. The independent management listener on 9180
is for local probes: `/live`, `/ready`, and `/metrics`. It has no authentication
and should remain accessible only to the management network. Do not route the
management port through the public Caddy site.

Use the [systemd service](../examples/deployment/pox.service) for Linux process
supervision. It runs as the dedicated `pox` account, starts in `/srv/pox/app`,
loads `/opt/pox/runtime/libpox_php.so`, and executes `/usr/local/bin/pox server`.
Create that account and install the application, binary and matching runtime
before installing the unit. Keep application code and the document root owned by
the deployment account; grant `pox` write access only to application data/cache
paths. Configure persistent-worker mode in the application's `pox.toml` if needed.
Adjust the paths and account in the unit for your installation.

```bash
sudo install -m 0644 examples/deployment/pox.service /etc/systemd/system/pox.service
sudo systemctl daemon-reload
sudo systemctl enable --now pox.service
curl --fail http://127.0.0.1:9180/ready
journalctl -u pox.service
```

The service uses `Restart=on-failure`, a two-second restart delay and a maximum
of five starts within 60 seconds to bound crash loops. Monitor failed units and
readiness: `Type=exec` confirms execution, not PHP readiness. After correcting a
persistent startup failure, use `systemctl reset-failed pox.service` followed by
`systemctl start pox.service`. See systemd's
[service documentation](https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html)
for restart behavior. `TimeoutStopSec=35s` allows the configured 30-second Pox drain
to finish; increase both settings together for longer application shutdowns.
`KillMode=control-group` includes child processes in service shutdown.

The deployment harness can use a temporary systemd user unit rendered from this
same template. It removes `User`/`Group` to run as the current account and substitutes
only application/binary/runtime paths and the test's server arguments. It verifies
that forced process loss produces an in-flight 502, systemd automatically replaces
the PID exactly once, PHP readiness and TLS requests recover, and a service stop
drains an active request without restarting. It neither installs nor enables a
persistent service. This verifies the restart/stop policy under a user manager;
system-service account provisioning and boot startup remain separate deployment
checks. The native-fault variant below also exercises a real extension memory fault.

SIGTERM stops public admission and drains active requests within Pox's shutdown
deadline. The test verifies that an active HTTPS request completes before Pox
exits successfully. Long-running native calls may instead require the configured
process shutdown deadline; a 504 response alone never frees active PHP capacity.

## Reproduce the integration test

On Linux or macOS, provide Python 3.11+, OpenSSL, curl with HTTP/2 support, an
installed Caddy, the built Pox binary and its matching native runtime:

```bash
python3 scripts/test-http-deployment.py \
  --binary target/debug/pox \
  --runtime /absolute/path/to/libpox_php.so \
  --caddy /absolute/path/to/caddy
```

The harness binds only loopback addresses, creates isolated application and TLS
files, disables Caddy's management API, and cleans up its child processes. It
checks verified TLS, spoofed forwarding headers, a binary POST body, HTTP/2 at
Caddy with HTTP/1.1 upstream, management-listener separation, forced process loss,
restart recovery, graceful drain and early PHP flush delivery in both PHP modes. HTTP/3, public ACME,
and multi-host networking are outside this test. Without `--systemd`, the harness
restarts the backend itself.

The [recorded result](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-caddy-streaming-linux-2026-09-06.json) includes exact
binary, runtime and Caddy hashes. The Caddy 2.11.4 Linux amd64 archive was checked
against its published SHA512 checksum and GitHub release SHA256 digest
`527fbf917c39189a1e3b31d34fa955601680b2d5c8055d2a87b8b9588dec7bb9`
before execution.

On Linux with an available systemd user manager and `XDG_RUNTIME_DIR`, append
`--systemd` to the command above to exercise automatic restart and service stop.
The harness creates uniquely named temporary units and removes them on exit.
The [systemd result](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-caddy-systemd-linux-2026-09-06.json) records
11 passing checks per PHP mode, original/replacement PIDs, automatic restart
counts, stopped service state and service-template hash. This local run used
systemd 261 and the same debug binary/native runtime as the streaming validation.
The production service's dedicated-account and boot behavior was not exercised.


## Native crash recovery fixture

The Linux-only [test extension](../scripts/fixtures/php-native-fault.c) deliberately
writes to a protected memory page. Unlike sending a software SIGSEGV, this produces
a synchronous memory fault inside a PHP extension. Build it against the exact ZTS
PHP SDK used by the native runtime, and load it only in the isolated test harness:

```bash
python3 scripts/build-test-php-extension.py \
  --php-config /absolute/path/to/matching/php-config \
  --output /tmp/pox-test-native-fault.so
python3 scripts/test-http-deployment.py \
  --binary target/debug/pox \
  --runtime /absolute/path/to/libpox_php.so \
  --caddy /absolute/path/to/caddy \
  --systemd --fault-extension /tmp/pox-test-native-fault.so
```

The harness verifies the extension loaded before triggering the fault. It checks
SIGSEGV termination, a 502 before response commitment, and an incomplete TLS body
when the fault follows an explicit flush. Systemd automatically replaces the
process after each fault, with a new PID, restored readiness and a working PHP
request. The final service stop still drains successfully. Core dumps are disabled
in the test service. The extension is never installed into the runtime or a
persistent application configuration.

The [native-crash evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-native-crash-linux-2026-09-06.json)
records all 13 checks passing in both modes with PHP 8.5.9 ZTS, including two
automatic restarts per mode. [Build provenance](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-native-fault-build-linux-2026-09-06.json)
records the fixture source/library hashes, compiler and matching PHP SDK version.
A native memory fault terminates the whole Pox process, including its other PHP
threads; worker replacement inside the process cannot contain it. This controlled
fault validates process-level recovery, not the correctness or interruption behavior
of arbitrary third-party extensions. Other platforms and system-service boot
integration remain unverified.


## Crash-loop protection

On Linux, append `--crash-loop` to a `--systemd` deployment test to verify the
service's configured restart-rate limit and the documented operator recovery.
The harness resets the start-rate window while Pox is active, then repeatedly
kills the process. Five new starts are allowed; the next restart is refused with
`Result=start-limit-hit`. The existing process at the reset precedes those five
starts. The proxy returns 502 while the backend is stopped, and an immediate
manual start also fails. Resetting the failed state then starting the service
restores PHP readiness and TLS responses.

This exercises the same two-second restart delay and five-start/60-second policy
as the checked-in service, without shortening its settings for the test. The
operator reset changes only the uniquely named temporary test unit. It does not
reset other services or install a persistent service.

The [release-profile result](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-release-crash-loop-linux-2026-09-06.json)
records 15 checks passing in each PHP mode, combining native memory faults,
streamed-response failure, TLS/proxy semantics, graceful stop, restart limiting
and operator recovery. It uses the same release binary/runtime snapshots as the
ongoing sustained workloads. This validates a local user-manager deployment;
system-service boot/account integration remains outside this test.

To check failure during extension startup, build the separate MINIT-failure
fixture against the same PHP SDK as the runtime, then run the isolated probe:

```sh
python3 scripts/build-test-php-extension.py \
  --php-config /path/to/php-sdk/bin/php-config \
  --source scripts/fixtures/php-startup-failure.c \
  --output /tmp/pox-test-startup-failure.so
python3 scripts/test-http-startup-failure.py \
  --binary target/debug/pox \
  --runtime /path/to/libpox_php.so \
  --extension /tmp/pox-test-startup-failure.so
```

The probe requires failure before an HTTP listener is observed, then removes the
bad configuration and verifies a fresh process can serve and shut down cleanly.
It does not attempt to retry PHP initialization within the failed process.

### PHP timers on macOS

On ZTS runtimes built without Zend per-thread execution timers (including the
macOS candidates), HTTP startup forces `max_execution_time=0` and
`max_input_time=-1`, following FrankenPHP's policy. PHP's fallback timer is
process-wide and cannot isolate concurrent requests. Pox's `request_timeout_ms`
and request-scoped cancellation continue to apply. CLI execution keeps its
configured PHP timer behavior. Applications must not re-enable PHP's process-wide
timer with `set_time_limit()` or `ini_set('max_execution_time', ...)` in these HTTP
runtimes; use the Pox request deadline. Native extension calls can still require
external process termination when they do not return to the PHP VM.
