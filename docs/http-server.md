# HTTP server configuration

Production hardening is in progress. See the [current readiness evidence](http-server-readiness.md)
for verified behavior and remaining gates, and the [review](http-server-review.md)
for the FrankenPHP comparison and implementation history.

Start the HTTP listener with a public document root:

```bash
pox server --host 127.0.0.1 --port 8000 --document-root public
pox server --document-root public --worker worker.php --workers 4
```

Explicit CLI options take precedence over `pox.toml`. A router and a worker
script are mutually exclusive. Watch patterns require worker mode; invalid glob
patterns fail startup. Both standard requests and persistent worker callbacks
execute on up to `--workers` PHP threads in parallel. Standard requests perform
full PHP request startup/shutdown; workers retain application state.
The default worker count is the available CPU parallelism.

The following `[server.limits]` values are the defaults. Unknown keys and invalid
limits fail startup. All timeouts are milliseconds; they must be positive and no
longer than one day.

```toml
[server.limits]
max_connections = 1024
max_inflight_requests = 32
max_header_bytes = 32768
max_headers = 100
worker_max_requests = 1000
max_body_bytes = 8388608
max_response_bytes = 33554432
header_timeout_ms = 10000
body_timeout_ms = 30000
queue_timeout_ms = 1000
request_timeout_ms = 30000
idle_timeout_ms = 15000
write_timeout_ms = 30000
shutdown_timeout_ms = 30000
```

Connection overflow closes the newly accepted socket without allocating a
connection task. Request admission overflow and queue timeout return 503. The
request limit covers body reads, queued/executing PHP, and response bodies; static
streams hold their permits until the stream ends or the client disconnects.

Header bounds are enforced by the HTTP parser. Body overflow returns 413;
malformed bodies return 400 and a body read deadline returns 408. Errors close the
connection. Transfer-coded requests always close the connection after their
response; ambiguous Content-Length/Transfer-Encoding framing cannot carry a
second request on that connection. Only chunked transfer coding is supported.

`request_timeout_ms` starts after a PHP execution slot is obtained. Expiry returns
504 and requests native cancellation. Disconnecting a client also cancels its
queued or executing PHP request. Cancellation takes effect at a Zend opcode
boundary; it does not forcibly kill an OS thread. A 50-ms watchdog repeats the
request while execution remains retained, including PHP shutdown callbacks.
Admission and execution permits remain held until native cleanup finishes.
Cancelled workers are replaced without replay; standard threads perform normal
request teardown before reuse.

Arbitrary blocking native extension calls can remain active until the syscall
returns. Such jobs retain their capacity and make `/ready` unavailable when all
initialized workers are occupied past their deadline. The configured shutdown
deadline still terminates the process if PHP cannot drain. Current tests cover
busy loops, stuck PHP shutdown callbacks, disconnects, and a blocking socket
read; this is not a guarantee that every extension can be interrupted.

On runtimes built with `ZEND_MAX_EXECUTION_TIMERS` (including the tested Linux
runtime), each persistent callback now receives a fresh PHP `max_execution_time`
budget; the timer is disabled while waiting for the next request. Configure the
budget in `[php.ini]`, for example `max_execution_time = "20"`. Zero disables this
PHP limit. PHP timer expiry terminates that worker incarnation, returns 502 for
the affected request, and triggers replacement without replay. This is separate
from the host's millisecond deadline and is not host-driven cancellation. Native
extension blocking and runtimes without per-thread Zend timers remain open gates.

HTTP serving requires runtime ABI 1.1 with native response-limit and HTTP-protocol
metadata, request-cancellation and response-output capabilities.
Standard mode additionally requires `POX_FEATURE_PARALLEL_WEB` and
`POX_FEATURE_WEB_THREADS` from the current ZTS runtime. Each dispatch thread
attaches PHP resources once, retains full request startup/shutdown for every
script, and releases thread resources before owner shutdown. All dispatch threads
must attach successfully before the listener opens.
Build the sibling runtime with `mise run runtime:build` while these changes are
unreleased; older runtimes remain usable for CLI execution but are rejected for
HTTP serving.

`max_response_bytes` limits total PHP response bytes; `max_header_bytes` bounds
serialized native response headers. Small responses retain a prefix of at most
64 KiB, preserving ordinary Content-Length and pre-commit error responses. PHP
`flush()` or reaching that prefix limit commits headers and streams through four
queued chunks of at most 16 KiB each. HEAD retains its bounded metadata path;
204/205/304 payloads are suppressed and finish with their existing framing.

Streamed HTTP/1.1 responses use chunked encoding; application Content-Length is
removed because the final size is not yet known. HTTP/1.0 uses connection closure.
An output limit, fatal, timeout or disconnect after headers commit aborts the
body; it cannot replace the status already sent. A failed HTTP/1.1 stream never
gets a successful final zero chunk. EOF-delimited HTTP/1.0 cannot distinguish an
aborted stream from ordinary completion without an independently known length.
Before commitment, validation/limit errors can still return 502; ordinary PHP
fatal errors before output retain PHP's 500 response.

Backpressure pauses the PHP output callback with a bounded queue. It checks
cancellation while waiting, and stalled socket writes use `write_timeout_ms`.
Active response bodies and pending writes are excluded from the idle keepalive
timeout. Admission remains held while a body is retained, and execution permits
remain held through PHP completion. Stream termination waits for successful PHP
cleanup, including standard-mode shutdown callbacks.

These limits do not bound PHP application allocations or PHP output-buffer
handlers; configure `memory_limit` separately. Static files stream in bounded
chunks and are not subject to the PHP whole-response limit. Header preparation
and access-log handler duration end when the HTTP response is produced; for a
stream this precedes body completion. Later stream failures appear in the PHP
stream/HTTP connection error logs.

SIGINT/SIGTERM close the listener and drain active responses, then stop PHP and
its watcher. `shutdown_timeout_ms` bounds the entire drain. A successful shutdown
exits 0. An expired deadline exits 1, terminating the process rather than unloading
PHP while its threads are executing. Run the CLI under a process supervisor and
allow its configured shutdown interval before sending a forced termination.

The listener serves plain HTTP/1.0 and HTTP/1.1. TRACE, CONNECT and protocol
upgrades are rejected. By default, PHP sees the actual TCP peer and HTTP scheme.
Forwarding headers are removed before PHP sees them; configured proxies can set
validated CGI identity as described below.

The document root must contain only publicly exposable assets and intended PHP
entry points, and must not be writable by untrusted users during requests. Pox
rejects traversal, hidden paths, escaping symlinks and static PHP-source aliases,
but canonical path checks do not eliminate races with concurrent filesystem
mutation. Standard `.well-known` resources remain accessible.

Static files support GET/HEAD, conditional caching, single byte ranges and
canonical PHP directory redirects. Unsupported multipart ranges are ignored and
return the complete representation. Metadata-based ETags are weak; they do not
satisfy strong If-Match or If-Range validators. Existing PHP entry points accept PATH_INFO. SCRIPT_NAME identifies the entry
point, PHP_SELF includes decoded PATH_INFO, and REQUEST_URI preserves the original
path/query. Missing PHP paths retain the front-controller fallback.
`SERVER_PROTOCOL` and PHP redirect semantics follow the incoming HTTP/1.0 or
HTTP/1.1 version.
PHP HEAD responses preserve a single valid declared Content-Length. If none is
provided, Pox uses captured output length when available and otherwise omits it.
204 and 304 responses omit payload and Content-Length; 205 responses have no
payload and declare length zero. Malformed/duplicate HEAD length metadata and
unsupported final statuses (informational or outside 200–599) return 502.
`REQUEST_SCHEME` reflects validated HTTP/HTTPS metadata and
`HTTPS` is `on` for HTTPS requests, absent for HTTP.

PHP's native authentication parser populates `PHP_AUTH_USER`, `PHP_AUTH_PW`,
`PHP_AUTH_DIGEST` and `AUTH_TYPE` for supported Authorization schemes. The raw
header remains available as `HTTP_AUTHORIZATION`, including Bearer tokens.
These variables describe supplied credentials; the application validates them.
Credentials are cleared between worker requests. Duplicate Authorization or
Content-Type fields return 400 before PHP executes.


Server mode disables `display_errors`, `display_startup_errors`, and `expose_php`,
and enables `log_errors`. These server settings override corresponding INI values
from the configuration. Request access logs are JSON lines on stderr with method,
URI, peer, status, `phase: response_headers`, and time until response headers are prepared. This duration is
not total response transmission time. Connection failures and lifecycle events
are logged separately. PHP SAPI errors default to stderr as `php_error` JSON
records with severity, message and a truncation flag, bounded below 4 KiB per
record. Quotes/control bytes are escaped, valid UTF-8 is preserved, and invalid
bytes are escaped individually. An explicit PHP `error_log` destination still
follows PHP's own configuration. Logs contain request URIs, so apply normal access-log
retention and redaction policies for application query parameters.

## Worker readiness, recovery and recycling

Before opening the HTTP listener, every persistent worker must reach its first
`pox_handle_request()` call within `request_timeout_ms`. A syntax error, an early
script exit or a stalled bootstrap fails startup. This deadline also covers the
runtime owner's initialization handshake.

Callback `exit()` and uncaught exceptions end that worker incarnation. The affected
request gets 502 and is never replayed. The owner replaces only terminated threads;
a healthy worker serving another request does not block replacement. Consecutive
failures retry after 100 ms, doubling up to a 5-second delay, plus the owner's
50-ms polling interval. A corrected script is retried without restarting the HTTP
process. No available worker produces 503; static assets remain serviceable.

`worker_max_requests` controls recycling after completed callbacks (default 1000;
0 disables recycling). Recycling resets the worker's PHP globals and state. During
planned recycling, dispatch waits within the original queue deadline for a ready
replacement. Expired or disconnected queued requests never execute. Available
capacity can decrease temporarily. File-watch reload
waits for active callbacks to finish before replacing scripts. It does not promise
zero downtime if the new script fails to boot.

The native runtime frees request/response buffers even on exit/fatal paths and
releases thread-local PHP resources when an incarnation terminates. Recovery does
not kill a hung native thread or protect against a native process crash; execution
and shutdown deadline behavior above still applies. Streaming and broader lifecycle verification remain in the readiness review.

Each worker callback refreshes request globals and filter inputs, then flushes
sessions and removes uploads before returning its response. Application globals
and application-owned streams persist across callbacks. Built-in session support
preserves custom save handlers registered in bootstrap while clearing each
client's session ID and data; clients resume sessions through their cookies.
If PHP loads session as a shared extension instead, register custom handlers in
each callback because the fallback restarts that module per request. The current
local integration coverage uses built-in sessions on PHP 8.5.9 ZTS.

The embedding API permits one PHP execution owner per loaded library. While a
web runtime or worker pool owns PHP, another owner, CLI execution, or a host INI
configuration change returns `RuntimeBusy`. Concurrent requests through that
owner remain supported. Configure host INI settings before starting the server.
Dropping the owner completes native shutdown before releasing ownership.


## Trusted proxies

Configure only the CIDRs of proxies that connect directly to Pox or appear in the
trusted suffix of a forwarding chain. The default empty list trusts nobody:

```toml
[server]
trusted_proxies = ["127.0.0.1/32", "::1/128"]
```

Proxy configuration requires the current native `POX_FEATURE_REQUEST_SCHEME`
capability. Invalid CIDRs fail startup. IPv4-mapped peers use IPv4 CIDRs.

For a trusted TCP peer, Pox accepts `X-Forwarded-For` as a list of bare IPv4/IPv6
addresses and walks right-to-left through trusted proxies, selecting the first
untrusted address. A supplied address has `REMOTE_PORT=0` because its TCP port is
unknown. With no address list, the actual peer and port remain in effect.

The trusted proxy must overwrite `X-Forwarded-Proto`, `X-Forwarded-Host` and
`X-Forwarded-Port` with authoritative values, and append the actual connecting
address to `X-Forwarded-For` (or replace the list at the public edge). Proto accepts
one `http` or `https` value. Host accepts one valid authority; it updates HTTP_HOST
and SERVER_NAME. An explicit port must be 1–65535 and agree with any port in Host.
Without Forwarded-Host, the original Host supplies the public authority when
proto/port metadata is present. Public ports default to 80/443; non-default
Forwarded-Port values are included in HTTP_HOST. Invalid or ambiguous trusted
metadata returns 400 before PHP executes.

Headers from an untrusted peer never change CGI identity, even when proxy CIDRs
are configured. RFC `Forwarded` is currently ignored. All forwarding headers are
stripped after processing, so applications should consume the resulting CGI
variables. Access logs retain the actual TCP peer for provenance. The checked-in
[Caddy deployment](http-server-deployment.md) has local verified-TLS and HTTP/2
frontend coverage in both modes. Direct Pox TLS, public ACME and broader deployed
network/supervisor verification remain separate gates.

The repeatable [HTTP load harness](http-server-load-testing.md) verifies response
isolation, recycling, resource bounds and clean shutdown under concurrent load.

## Health and metrics listener

Enable a separate listener explicitly in `pox.toml`:

```toml
[server]
admin_address = "127.0.0.1:9180"
```

It is disabled by default. Binding fails startup if the configured address is
unavailable. Keep this unauthenticated listener on a trusted management network;
it is independent of the public listener and does not execute PHP. GET and HEAD
are supported; other methods return 405 and unknown paths return 404.

- `/live` returns 200 while the admin service runs, including during shutdown.
- `/ready` returns 200 when at least one initialized PHP worker remains within
  its request deadline and shutdown has not started; otherwise it returns 503.
  Normal full utilization stays ready. Jobs retained after timeout or client
  disconnect count as expired once their original deadline passes. A failed
  worker bootstrap is not ready; worker counts come directly from the native
  worker lifecycle. This
  checks server capacity, not application dependencies such as databases.
- `/metrics` exports Prometheus text counters for produced public responses by
  status class and cumulative handler duration, plus gauges for readiness,
  initialized PHP workers, expired jobs, free execution slots and admitted
  requests. Counters exclude parser/transport failures and cancelled handlers;
  duration excludes response transmission. `pox_http_response_bodies_total` separately
  reports `complete`, `error`, and `dropped` body-production outcomes. A flushed 200
  can later increment `error`; a body dropped before completion increments `dropped`.
  Completion means Hyper consumed the body or its declared length, not that the
  client received it. EOF before the advertised Content-Length counts as an error
  (for example, a static file truncated during transmission). Empty HEAD/bodyless
  responses count as complete even when they retain representation-length metadata. A native
  process crash cannot emit a final body outcome; monitor supervisor failures too.
  No paths or client labels are used.

Admin requests have a two-second whole-connection deadline, an 8-KiB header
buffer, at most 32 headers and 64 concurrent connections. Connections close after
one response. Public admission limits do not consume these admin slots. During
graceful shutdown the admin listener remains available with readiness 503 until
public requests and PHP have drained, then closes with the process.

See [Symfony application validation](http-server-framework-testing.md) for the
real-framework HTTP fixture and its recorded coverage.
