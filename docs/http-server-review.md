# HTTP server production-readiness review

Status: server implementation and full source-candidate CI validated; target deployment checks remain open. See the [current completion audit](http-server-completion-audit.md). Historical gate statements below are superseded only by the explicit evidence in that audit.

Reference: FrankenPHP cloned at `/tmp/pox-frankenphp-review`, revision
`2e3342762be51f4f635b5457899c6743ec999ef8` (2026-09-06).
Pox baseline: workspace version 0.0.2. Native runtime is in the sibling
`../pox-runtime` repository; runtime changes require separate build and validation.

## Comparison and required work

| Area | Reference evidence | Pox finding and completion gate |
| --- | --- | --- |
| File routing | `caddy/php-server.go` uses explicit PHP matching, front-controller fallback and directory redirects; `requestoptions.go` splits PHP paths case-insensitively. | Shared decoding/confinement and source-file protection pass in both modes. Directory redirects preserve raw query strings. Static GET/HEAD, weak ETags, dates, preconditions and single byte ranges have real-socket coverage. Existing PHP-file PATH_INFO routing and native SCRIPT_NAME/PHP_SELF/PATH_INFO/PATH_TRANSLATED now have real HTTP coverage, including encoded paths and per-request reset. Deployment requires a document root that untrusted users cannot mutate; canonicalization does not eliminate filesystem TOCTOU races. |
| HTTP transport | FrankenPHP uses Caddy's HTTP server. Hyper's [HTTP/1 builder](https://docs.rs/hyper/1.11.0/hyper/server/conn/http1/struct.Builder.html) provides bounded parser configuration. | Replaced tiny_http with Hyper 1.11.0. Connection, admission, header, body, idle, write, queue, execution and shutdown bounds are configurable. Tests cover conflicting Content-Length, unsupported/malformed transfer coding, Host validation, oversized headers/bodies, slow/incomplete headers/bodies, idle and stalled-writing clients, keepalive and cancellation. Transfer-coded requests always close the connection after response, including Content-Length/Transfer-Encoding ambiguity. Local 100,000-request workloads now pass for both modes with response/resource checks. Thirteen malformed-framing cases also verify that neither the first request nor a pipelined follow-up reaches PHP. Multi-hour and broader platform workloads, plus broader protocol fuzzing, remain. |
| PHP concurrency | `worker.go` dispatches to available threads and rejects excessive queue wait. | HTTP persistent-worker dispatch is now parallel, with atomic worker reservations, bounded admission and queue deadlines. A barrier test proves both workers execute before either finishes; 160 distinct simultaneous library calls retain response ownership. Standard mode also executes on configured parallel PHP threads, with isolated TSRM state, full per-request shutdown and real-socket concurrency/isolation coverage. Sustained resource/performance evidence is tracked below. |
| Native lifecycle | `phpthread.go`, `threadregular.go`, and `threadworker.go` own PHP execution on managed threads. | Web initialization/execution/destruction and worker global initialization/destruction now stay on one dedicated owner thread. TSRM cleanup, standard-mode concurrency, request globals/filter/session/upload isolation, fatal recovery and per-request Zend timers now have local Linux coverage. Shared runtime ownership and metadata caching prevent overlapping initialization; sequential web/worker/CLI transitions are tested. Blocking native calls, initialization failure paths and broader platform/extension combinations remain open. |
| Graceful shutdown | `frankenphp.go` and worker shutdown tests. | SIGINT/SIGTERM stop accepting, drain HTTP connections, then stop PHP and the watcher. A single configured deadline covers the whole drain; expired shutdown terminates the CLI with code 1 without unloading executing PHP. Real subprocess tests verify successful response drain and forced termination of hung PHP in both standard and worker modes. Eight consecutive watcher reloads with both PHP workers active, followed by clean SIGTERM shutdown, now have regression coverage. Shutdown while reload waits for an active callback now has clean-drain and forced-deadline coverage on glibc PHP 8.4/8.5. Longer reload storms and broader native cleanup remain to validate. |
| Worker recovery | `worker.go`, `threadworker.go`, `worker_test.go`, `reload_test.sh`. | Per-worker exclusive reservations allow replacement of a stopped worker while another stays active. Initial startup waits for every worker to enter its request loop. Exits/uncaught exceptions fail the affected request without replay; terminated threads are replaced with 100-ms exponential backoff capped at 5 seconds. Configurable recycling defaults to 1000 completed requests. Socket tests cover invalid/hung startup, repeated exit/fatal recovery, backoff, repaired scripts, recycling and reload with an active callback. A controlled native memory fault is now verified to terminate the process; systemd recovery and interrupted TLS responses pass locally. In-process native crash containment is not provided. Arbitrary hung-extension interruption and broader sustained stress remain open. |
| Request/response semantics | `cgi.go`, `cgi_test.go`, `requestbodytimeout_test.go`, `statusline_test.go`, `finishrequest_realserver_test.go`; HTTP semantics in [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110.html). | Request bodies are fully validated before PHP; decoded Content-Length is regenerated. Duplicate cookies/headers are combined appropriately, response Set-Cookie remains separate, and connection-specific headers are removed. Host/IPv6, forwarded-header suppression, HEAD/keepalive and PHP response framing have socket tests. Script metadata/PATH_INFO are fixed. Protocol/scheme/authentication variables and upload cleanup now have real HTTP coverage. Pipelined HEAD/204/205/304 tests now verify metadata and payload framing. Broader CGI and extension combinations remain. ABI 1.1 now enforces native body/header allocation bounds and reports overflow rather than truncating. The HTTP adapter now streams PHP output after explicit flush or a bounded 64-KiB prefix, using a four-chunk queue. Socket tests cover framing, late errors, backpressure, cancellation and active-body idle handling; broader long-running/platform workloads remain. Request-scoped native cancellation is wired to HTTP timeouts and disconnects, with retry through PHP shutdown callbacks. Real-socket tests cover recovery in both modes and capacity retained by blocking native I/O; arbitrary extension interruption and crash containment remain open. |
| Operational deployment | Caddy supplies TLS and server controls; `metrics.go` instruments execution. | JSON access/error lifecycle logs, secure PHP error-display settings, explicit CLI precedence, and documented limits are implemented. CIDR-gated proxy address/host/port/scheme integration is implemented; untrusted headers cannot set CGI identity. Forwarding headers are stripped after processing. The optional independent admin listener now exposes liveness, capacity readiness and bounded-label Prometheus metrics, with socket coverage for expired/disconnected jobs, failed replacements and drain. The checked-in Caddy deployment passes local certificate-verified TLS, HTTP/2 frontend, forwarding-spoof, process-loss/restart and drain tests in both modes. The systemd service template now passes automatic restart and graceful stop under a local user manager. Public ACME, dedicated-account/boot/multi-host validation and executed CI runtime coverage remain. The scheduled workflow now enables the CLI HTTP feature and Linux load harness, but these changes have not run remotely. |

## Implemented in the first hardening pass

- Decode request paths exactly once, preserve the raw URI/query sent to PHP,
  reject malformed encoding, NUL/control bytes, backslashes, dot segments and
  hidden paths (standard `.well-known` resources remain reachable).
- Reject paths and existing ancestors whose symlinks escape the canonical root;
  apply the same check to directory/front-controller index selection.
- Reject an invalid document root or router at startup.
- Do not send PHP sources with uppercase extensions or static aliases to PHP or
  hidden files. Only select PHP files for direct execution.
- Stream static files from file handles rather than loading each whole file into
  a Rust Vec. This alone does not impose write deadlines or connection limits.
- Keep internal PHP error details in server logs and return a generic HTTP error.
- Protect each worker exchange through response consumption and synchronize
  termination notifications with their condition-variable mutexes.

## Reproducible validation

`mise run test:runtime` runs workspace checks against
`../pox-runtime/build/libpox_php.so` (or the macOS dylib), including the real
socket integration test `crates/pox-cli/tests/http_server.rs` and concurrent
worker regression `crates/pox-embed/tests/runtime_modes.rs`.

`mise run test` covers tests that do not require native PHP.
`mise x rust -- cargo clippy --workspace --all-targets --locked -- -D warnings`
and `git diff --check` cover Rust lint and patch formatting.

Completion requires closing every remaining gate in the table with current
runtime evidence. A passing path test or existing embedding suite does not
establish production readiness.

## Second hardening pass: transport and dispatch

- Network tasks are bounded independently from request admission. PHP runs on
  dedicated threads. Queue/execution timeouts do not release an execution slot
  while PHP still runs; disconnected queued requests never execute.
- Static streaming and buffered PHP responses retain their admission permits
  until their body is consumed or dropped. PHP output is still natively buffered
  before the host sees it; the native allocation gap described here was closed
  in the third pass below.
- Parser bounds, body validation and connection-close-on-transfer-coding protect
  subsequent request framing. The server supports HTTP/1.0 and HTTP/1.1; TLS,
  HTTP/2, HTTP/3 and upgrades are not implemented in this transport.
- HTTP socket tests now include explicit CLI precedence over contradictory
  configuration, two-worker execution barriers, rejected overload, queue timeout,
  disconnect cancellation, static ranges/conditions, IPv6 Host/cookies, idle
  clients, blocked writers, graceful termination and a hung-PHP shutdown deadline.

Current local validation is Linux x86_64 using PHP 8.5.9 and sibling runtime
revision `b3a971492512bfea961c9a7e4ab37885f1ba1060`. It does not establish macOS,
Linux musl, other PHP versions, sustained load, or production-readiness completion.

Second-pass local gates passed: `mise run test:runtime` (60 tests, including 14 HTTP
subprocess/socket tests), Clippy with all targets and runtime-integration enabled
under `-D warnings`, and `git diff --check`. Native output allocation bounds,
standard-mode parallel PHP, worker recovery, CGI/PATH_INFO, trusted proxy support,
operational endpoints, broader platforms and load/CI evidence are still open.

## Third hardening pass: native response bounds and CGI paths

The sibling `pox-runtime` checkout now advertises ABI 1.1 and the native
response-limits feature. The request/response struct sizes remain unchanged:
request reserved slots carry uint32 body/header budgets under an explicit flag;
a response flag reports rejected buffering. CLI and legacy embedding calls can
still load ABI 1.0 runtimes, but the HTTP server refuses runtimes without native
limits instead of silently accepting an ineffective safety setting.

Native growth checks subtraction before addition, caps geometric allocation at
the configured budget, preserves allocation failure, and discards the entire
response on overflow/failure. A standalone C test checks exact bounds, zero,
SIZE_MAX arithmetic and injected allocator failure. Direct Rust ABI tests check
body/header overflow and subsequent-request recovery. Linux HTTP tests generate
512 MiB of output with a 1 KiB native limit in both modes and require less than
16 MiB growth in process peak RSS. These bounds cover native response buffers,
not arbitrary PHP application memory or PHP output-buffer handlers.

CGI metadata now derives script identity from the actual entry point, decodes the
URI path once while preserving literal '+', and separates PATH_INFO and query
strings. The routing layer only splits at existing confined PHP files. Tests cover
front-controller routes, encoded entry points/path info, directories ending in
.php, uppercase extensions, escaping symlinks, and stale PATH_INFO removal.

Latest validation: native runtime rebuild and C smoke/buffer tests; 65 workspace
tests (18 real HTTP tests); all-target runtime-feature Clippy with `-D warnings`;
patch checks in both repositories. Runtime source is the previously recorded
sibling revision plus the current uncommitted changes; ABI 1.1 is not published.
Standard-mode parallel PHP, streaming, recovery/lifecycle, remaining CGI metadata,
proxy support, operational endpoints, platform/load and CI gates remain open.

## Fourth hardening pass: persistent-worker lifecycle

Workers now advertise readiness at the first wait callback. The HTTP owner waits
before binding the listener, fails startup on termination or deadline expiry,
and runs bounded-rate maintenance. Slot mutexes protect request ownership while
allowing stopped slots to be replaced independently; maintenance wakes dispatch
waiters even when it only briefly inspected a slot. Reload still takes the pool's
exclusive lock and drains active callbacks first.

Callback exit/unhandled-exception paths previously could yield an empty 200 or
leave the same worker running after an error. They now terminate that incarnation
and fail the affected request without replay. Native cleanup releases request
buffers, bootstrap response buffers and TSRM resources on termination. A Linux
regression sends 128 MiB of bodies through failing callbacks and bounds additional
RSS to 64 MiB; the prior native request-body leak would retain those bodies.

The new `worker_max_requests` setting defaults to 1000; recycling, replacement
while a peer is active, repeated startup-failure backoff, correction after startup
failure, and active-request reload all have real process tests. These changes
require the current sibling runtime source, not merely a previously built library.

Current local gates: rebuilt native runtime with C smoke/buffer tests, 72 workspace
tests including 25 HTTP tests, runtime-feature all-target Clippy and patch checks
in both repositories. Standard-mode PHP concurrency, streaming/cancellation,
remaining request lifecycle and protocol metadata, trusted proxy support,
operational endpoints, sustained/platform testing and runtime CI remain open.

## Fifth hardening pass: worker request isolation

Comparing the persistent-worker lifecycle with FrankenPHP exposed missing request
boundaries: `$_REQUEST` did not reliably import the current body, filter inputs
and upload globals could remain stale, and sessions needed flushing/resetting
between clients. Each callback now activates and deactivates the SAPI request,
refreshes request globals and filter state, cleans uploaded files, and closes
output before publishing the response. Detached input temporary streams are
released without closing application-owned streams. Cookie bytes are explicitly
owned by the native request because PHP does not free the SAPI cookie pointer.

For built-in PHP sessions, cleanup flushes the session and resets client state
while preserving bootstrap-registered save-handler objects/closures. Full PHP
shutdown still releases those handlers when the worker ends. Builds with session
as a shared extension use module shutdown/startup instead and must register custom
handlers per callback; that fallback is not covered by the local built-in build.

Five new real HTTP tests cover request/filter/cookie isolation, session persistence
and fresh IDs, bootstrap save handlers, upload processing/removal, and retention
of application-owned streams. Local gates pass against the rebuilt sibling runtime:
77 workspace tests including 30 HTTP tests, native C smoke/buffer tests,
runtime-feature all-target Clippy with warnings denied, and both patch checks.
These results apply to Linux and PHP 8.5.9 ZTS with the uncommitted ABI 1.1 runtime.

The overall goal remains open: standard-mode PHP concurrency, native streaming
and cancellation, further extension/lifecycle coverage, remaining CGI protocol
metadata, trusted proxies, operational endpoints, sustained/platform validation
and runtime CI remain production-readiness gates.

## Sixth hardening pass: concurrent standard PHP

Standard requests previously ran on one PHP execution thread even with multiple
configured workers. Both modes now honor `--workers`. Standard mode initializes
PHP on an owner thread and borrows a capability-checked executor into scoped
OS threads, all joined before owner-thread shutdown. The Rust web owner is no
longer `Send`, enforcing PHP initialization/shutdown affinity. Thread creation
failure stops and joins any already-created dispatchers.

The native ZTS runtime advertises `POX_FEATURE_PARALLEL_WEB` without changing the
ABI table. Off-owner web calls allocate independent TSRM globals and release them
after full PHP request shutdown; sequential owner-thread embedding still works.
This deliberately incurs per-request TSRM allocation rather than retaining
unmanaged thread resources. INI parsing also needed repair: `strtok` shared its
cursor across concurrent initialization; it now uses `strtok_r` with a local cursor.
Standard HTTP startup rejects a runtime without the new capability. Existing
non-parallel embed calls remain available on older runtimes.

The saturation test now covers both modes and requires two scripts to reach a
barrier before either can finish, verifying actual overlap and bounded overload.
A repeated parallel test verifies fresh classes/globals, distinct request values,
server INI settings and recovery following fatal exceptions. The loader test
checks rejection of parallel execution for a runtime lacking the capability.
Queue/deadline tests explicitly retain one execution thread where required.

Local validation: rebuilt PHP 8.5.9 ZTS runtime and native smoke/buffer checks,
78 workspace tests (31 HTTP tests), runtime-feature all-target Clippy with warnings
denied, and patch checks in both repositories. All passed on Linux. This closes
the missing standard-mode concurrency implementation and local regression gate;
sustained performance/resource use, other supported platforms and extension
combinations still need evidence. Native streaming/cancellation, remaining CGI
metadata, trusted proxies, operational endpoints and runtime CI remain open.

## Seventh hardening pass: actual HTTP protocol in PHP

PHP previously always received `SERVER_PROTOCOL=HTTP/1.1`. Its internal SAPI
protocol also silently reset to HTTP/1.0 during activation, giving HTTP/1.1 POST
Location responses the wrong implicit redirect status. The host now passes an
explicit HTTP protocol through the existing reserved ABI fields, guarded by
`POX_FEATURE_HTTP_PROTOCOL`. Native activation sets the internal protocol after
PHP's reset, and server-variable registration uses the same request value.
Standard and persistent worker execution both use this path.

Rust requests expose `HttpProtocol`; HTTP/1.0 requests fail explicitly with an
older runtime lacking the capability, while legacy HTTP/1.1 embedding remains
compatible. The HTTP listener requires protocol metadata support at startup.
The native ABI rejects protocol values other than 1000/1001 and defaults to
HTTP/1.1 when the flag is absent. No TLS or proxy identity is inferred from it.

Two HTTP tests alternate protocol versions across requests in both modes, check
HTTP/1.0 without Host, ensure a client header cannot override SERVER_PROTOCOL,
and verify implicit POST Location statuses (302 for 1.0, 303 for 1.1). The loader
regression confirms unsupported-runtime rejection rather than silent mismatch.

Local gates pass: rebuilt PHP 8.5.9 ZTS runtime with native smoke/buffer tests,
80 workspace tests including 33 HTTP tests, runtime-feature all-target Clippy
with warnings denied, and both repository patch checks. The FrankenPHP reference
checkout still resolves to 2e3342762be51f4f635b5457899c6743ec999ef8.
Native streaming/cancellation, trusted proxy/HTTPS metadata, broader CGI and
extension coverage, operational endpoints, sustained/platform validation and
runtime CI remain open; this is not a production-ready completion claim.

## Eighth hardening pass: worker execution timers and PHP error logs

Persistent callbacks previously disabled Zend's execution timer unconditionally.
On builds with `ZEND_MAX_EXECUTION_TIMERS`, the runtime now disables it while
waiting for work and sets a fresh `max_execution_time` budget for each request.
This follows the lifecycle distinction observed in FrankenPHP's wait/callback
path. Zero remains unlimited. Expiry takes PHP's fatal bailout path, producing a
failed request and worker replacement rather than leaving a permanently occupied
slot. No request is replayed. The tested PHP 8.5.9 implementation uses per-thread
wall-clock timers, not CPU-time accounting.

The timeout regression also exposed that both HTTP SAPIs had NULL log callbacks.
PHP failures were therefore missing their cause in default stderr logs. Both now
emit bounded JSON records with severity, escaped message and a truncation flag.
Valid UTF-8 remains intact; arbitrary invalid bytes are escaped. Explicit PHP
error-log destinations still follow PHP configuration. No heap allocation is
needed by this error callback, and records stay below 4096 bytes.

New regressions verify repeated infinite-loop termination, worker replacement,
per-request budget reset across cumulative work longer than the budget, and
bounded JSON logging with quotes, newlines, Unicode, invalid bytes and oversized
messages. Error text remains absent from HTTP responses. A first broader run was
invalidated by rebuilding the shared library during testing (subsequent loads
reported file too short); the final run used the completed, unchanged library.

Final local gates pass: native runtime build and smoke/buffer checks, 82 workspace
tests including 35 real HTTP tests, runtime-feature all-target Clippy with warnings
denied, and patch checks in both repositories. Host-driven cancellation, blocking
native extension behavior and timer behavior on other platform/build combinations
remain unproven. Native streaming, trusted proxy/HTTPS metadata, remaining CGI and
extension coverage, operational endpoints, sustained/platform validation and
runtime CI also remain open production-readiness gates.

## Ninth hardening pass: PHP authentication metadata

FrankenPHP calls PHP's authentication parser when installing a request context.
Pox previously forwarded only HTTP_AUTHORIZATION, leaving standard PHP Basic and
Digest variables unset. HTTP activation now calls `php_handle_auth_data` with a
request-owned Authorization value. PHP supplies PHP_AUTH_USER/PW/DIGEST and the
SAPI registers AUTH_TYPE for parsed credentials. SAPI deactivation already frees
and nulls the parsed fields, so this works across persistent callbacks as well.
The raw header remains available for application handling of Bearer/other schemes.
This parses supplied credentials; application authorization is still required.

The host rejects duplicate Authorization and Content-Type fields with 400 before
PHP dispatch, avoiding ambiguous header combination. SERVER_SOFTWARE registration
also used length 4 for the three-byte string pox, exposing a trailing NUL; it now
uses the correct length.

Two new real HTTP regressions cover both execution modes: credential transitions
across Basic, mixed-case Basic, Digest, invalid Basic, Bearer and missing headers;
absence of stale user/password/digest/type values; exact SERVER_SOFTWARE; and
rejection of duplicate sensitive headers before any application side effect.

Local gates pass against the rebuilt PHP 8.5.9 ZTS runtime: 84 workspace tests
including 37 HTTP tests, native smoke/buffer checks, runtime-feature all-target
Clippy with warnings denied, and both repository patch checks. Host-driven
cancellation, native streaming, trusted proxy/HTTPS metadata, further CGI and
extension coverage, operational endpoints, sustained/platform validation and
runtime CI remain open production-readiness gates.

## Tenth hardening pass: process-wide runtime ownership

The Rust API allowed overlapping web owners, worker pools, CLI calls and INI
mutation against the same native PHP module state. A shared atomic lease now
rejects conflicting operations with RuntimeBusy before native entry. Web/pool
leases last through native shutdown; requests within their owner remain parallel.
Clones and independent loads of the same library share ownership state.

The new real-runtime regression exposed two additional native crashes. A second
load invoked metadata discovery while web mode was active; GDB traced the fault
through pox_get_loaded_extensions -> php_embed_init -> TSRM initialization.
Validated metadata is now cached per loaded library, eliminating that re-entry.
After that fix, sequential web-to-worker startup still crashed; GDB traced it to
stale TSRM/INI state. Both HTTP shutdown paths omitted tsrm_shutdown, unlike PHP's
embed shutdown implementation. They now release TSRM after module/SAPI shutdown,
with the host already having joined execution threads.

The regression attempts conflicting operations from multiple threads and through
an independently loaded handle, confirms the active web owner still executes,
then completes web -> worker -> CLI -> web transitions. It also checks INI
mutation rejection during ownership and success following shutdown.

Local gates pass: rebuilt native runtime with smoke/buffer checks, 85 workspace
tests including 37 HTTP tests and five native runtime-mode tests, runtime-feature
all-target Clippy with warnings denied, and patch checks in both repositories.
Host-driven cancellation, native streaming, trusted proxy/HTTPS metadata,
further lifecycle/CGI/extension coverage, operational endpoints, sustained/platform
validation and runtime CI remain open production-readiness gates.

## Eleventh hardening pass: trusted proxies and HTTPS metadata

The host now accepts an explicit server.trusted_proxies CIDR list, defaulting to
empty. Forwarding metadata affects CGI identity only when the TCP peer is trusted.
X-Forwarded-For is parsed as IP addresses and walked right-to-left through the
trusted suffix, selecting the first untrusted address and reporting its unknown
port as zero. Client-controlled prefixes cannot override that boundary.

Single-valued X-Forwarded-Proto/Host/Port establish the public scheme and authority.
Conflicting ports, duplicate scalar fields, malformed IPs, unsupported schemes and
invalid authorities return 400 before PHP dispatch. Untrusted metadata is ignored.
HTTP_HOST/SERVER_NAME/SERVER_PORT remain consistent, including IPv6 authorities
and non-default forwarded ports. Forwarding headers are stripped after processing;
RFC Forwarded is intentionally ignored. Access logs retain the actual TCP peer.
The deployment contract requires trusted proxies to overwrite scalar metadata
and append the actual source address or replace the list at the public edge.

The unchanged ABI gains POX_FEATURE_REQUEST_SCHEME / POX_HTTP_SECURE. The host
passes validated HTTPS state, and native CGI registration sets REQUEST_SCHEME and
HTTPS while clearing HTTPS on plain requests. Rust rejects secure requests on an
unsupported runtime; proxy-enabled HTTP startup requires the new capability.

Three new HTTP tests cover trusted chains, alternating HTTPS/HTTP in both modes,
malformed trusted metadata without application side effects, and spoofed headers
with empty or nonmatching trust lists. Three unit tests cover the trust boundary,
IPv6/mapped peers and authority/port consistency; the loader checks unsupported
scheme rejection. A process smoke check confirms invalid CIDRs fail before listen.

Local gates pass: rebuilt native runtime with smoke/buffer checks, 91 workspace
tests including 40 HTTP tests, runtime-feature all-target Clippy with warnings
denied, and both repository patch checks. Deployed-proxy/TLS verification remains
open, along with host-driven cancellation, native streaming, remaining lifecycle/
CGI/extension coverage, operational endpoints, sustained/platform validation and
runtime CI. This is not a production-readiness completion claim.

## Twelfth hardening pass: recycling under load and sustained-workload evidence

A new standard-library Python harness verifies every response's unique URI,
cookie and body hash, checks standard/worker state behavior, records latency,
RSS/descriptors/replacements, and requires clean shutdown. It makes no request
retries and reports exact binary/runtime SHA-256 provenance. Resource checks
bound warmed RSS growth and final file descriptors; the workload deadline is
explicit and a partial run fails.

The initial 2000-request debug run exposed 129 worker 503 responses during normal
recycling. Dispatch now waits for planned replacements within the original queue
deadline, checking cancellation before PHP submission. This preserves the total
queue budget instead of restarting it after semaphore admission. Failed callbacks
are not replayed. The recycling regression now requires direct success without
retry loops. A new socket regression blocks replacement bootstrap and verifies
that expired/disconnected queued requests produce no application side effects.
The same 2000-request workload then completed entirely successfully with eight
worker replacements.

The configured release build passed 100,000 worker requests with 32 clients/eight
PHP threads and 96 replacements. The standard run returned 66,535 verified 200
responses before the 180-second driver deadline; it remains a failed workload
gate, although both modes stayed within resource bounds and shut down cleanly.
Full numbers and artifact digests are in the [load report](http-server-load-testing.md).
Callgrind on a smaller passing standard workload identifies per-request TSRM
allocation (~65% of process instructions) and cleanup (~18%) as the dominant
cost. Reusable standard-thread resources are the next measured optimization;
this profile does not establish wall-time percentages or justify weakening
request isolation.

The scheduled released-runtime workflow previously enabled only the embedding
feature, leaving CLI HTTP tests disabled. It now selects pox-cli/runtime-integration
and runs a bounded Linux load workload. Native source CI now runs the standalone
response-buffer allocation test on its Linux/macOS matrix. Workflow YAML and the
native test command pass locally; neither workflow change has run remotely, and
released-runtime compatibility still requires coordinated publication.

Local regression gates pass: 92 workspace tests including 41 HTTP tests,
runtime-feature all-target Clippy with warnings denied, native buffer validation,
Python syntax/YAML parsing and patch checks in both repositories. The standard
sustained-workload gate remains open, together with native streaming/cancellation,
remaining lifecycle/CGI/extension coverage, deployed TLS/proxy verification,
operational endpoints, broader platforms/multi-hour testing and executed runtime CI.


## Thirteenth hardening pass: reusable standard PHP thread resources

The prior Callgrind profile attributed most standard-mode instructions to
allocating and freeing TSRM resources for every request. The native ABI now
advertises POX_FEATURE_WEB_THREADS and consumes two reserved function-pointer
slots for web_thread_enter/leave without changing table size. Enter initializes
resources once for an off-owner dispatch thread; each web call retains full PHP
request startup/shutdown and clears request context afterward. Leave releases
thread resources after the last call. Legacy embedding calls still use the
per-call allocation path when no thread is attached.

The Rust borrowed WebThread guard is non-Send/non-Sync and keeps the web owner
alive. It detaches on its creating thread. Duplicate attachment and owner-thread
attachment are rejected. The HTTP service waits for successful attachment of
every dispatch thread before opening the listener, and joins all threads before
native module shutdown. Startup failure stops already-attached dispatchers.

A new native integration regression performs 160 isolated requests across four
threads and two attach/detach generations, checks duplicate/owner rejection, and
then executes on the original owner. Existing real-HTTP tests continue checking
fresh classes/globals, fatal recovery, cookies, authentication, CGI state and
bounded native responses on the reusable path.

All 93 workspace tests pass, including 41 HTTP tests and six native mode tests;
Clippy with all targets/runtime features and denied warnings, native build/smoke/
buffer checks, and both patch checks pass. The configured LTO release build then
passed 100,000 requests per mode with 32 clients/eight PHP threads: standard mode
35.6 seconds with 2.2 MiB RSS growth; worker mode 31.8 seconds with 11.1 MiB growth
and 96 replacements. Both had only verified HTTP 200 responses, bounded final
descriptors and clean shutdown. The previous standard run stopped at 66,535
responses after 180 seconds. Exact artifacts and comparison are in the
[load report](http-server-load-testing.md).

A repeated passing Callgrind workload reduces TSRM allocation/cleanup from roughly
65%/18% to 3.4%/1.0% of process instructions. The local standard sustained-workload
gate is now closed; these percentages are not wall-time measurements. Native
streaming/cancellation, remaining lifecycle/CGI/extension coverage, operational
endpoints, deployed TLS/proxy verification, multi-hour/cross-platform validation
and executed runtime CI remain open production-readiness gates.


## Fourteenth hardening pass: HEAD and bodyless response semantics

PHP response conversion previously discarded declared Content-Length values and
regenerated them from captured output even for HEAD. Applications that emit only
GET bodies could therefore receive an incorrect zero representation length on
HEAD. A single valid declared HEAD length is now preserved; duplicate/malformed
values return 502. Without a declared length, captured nonempty output supplies
the length, otherwise it is omitted rather than inventing zero.

204 and 304 responses omit body and Content-Length; Hyper deliberately suppresses
304 length metadata. 205 responses explicitly use zero length and no payload.
Final informational statuses and values outside 200–599 are rejected as bad
upstream responses. These paths cannot append PHP output to a subsequent response.

Two real-socket tests run in standard and worker modes, checking HEAD/204/205/304
combinations followed immediately by a static request on the same connection,
HEAD without a declared length, invalid/duplicate length metadata and unsupported
final statuses. They verify the actual wire response rather than only adapter
objects. The initially expected optional 304 length was corrected to match the
transport's permitted omission; the HEAD representation fix remains effective.

Local gates pass: 95 workspace tests including 43 HTTP tests, runtime-feature
all-target Clippy with warnings denied and both repository patch checks. No native
runtime change was needed in this pass. Native streaming/cancellation, remaining
lifecycle/CGI/extension coverage, operational endpoints, deployed TLS/proxy checks,
multi-hour/cross-platform validation and executed runtime CI remain open.


## Fifteenth hardening pass: operational health and metrics

`server.admin_address` enables a separate management listener, disabled by default.
It provides GET/HEAD `/live`, `/ready` and `/metrics`, independent of public
connection/admission limits, with 64 connections, an 8-KiB parser buffer, 32
headers, no keepalive and a two-second whole-connection deadline. The endpoint
has no authentication; bind it to an operator-controlled management interface.

Readiness excludes draining servers, failed PHP initialization and capacity
retained past HTTP request deadlines. Busy workers within their deadline remain
ready. Job-owned tracking survives HTTP timeout and client disconnect until PHP
returns. Native worker readiness uses an atomic incarnation count, sampled by
the owner every 50 ms without taking busy worker locks. Standard dispatch threads
track successful attachment and remove their count on exit. No native ABI or
library rebuild was needed for this pass.

Metrics expose public response counts by status class, cumulative handler time,
readiness, initialized PHP workers, expired jobs, free PHP execution slots and
admitted requests. They have bounded labels and exclude transport/parser errors
and cancelled handlers; durations exclude response transmission. These measure
server capacity rather than application dependencies.

Four new socket tests cover both execution modes where applicable: saturated
public admission, normal busy readiness, request expiry and recovery, disconnected
native work, failed worker bootstrap/recovery, admin connection expiry, HEAD and
method handling, and liveness with readiness disabled throughout graceful drain.
The admin listener closes after draining PHP and public connections.

Local validation passes: 99 workspace tests including 47 real HTTP tests,
all-target/runtime-feature Clippy with warnings denied, and patch checks in both
repositories. Logs are `/tmp/pox-admin-final-tests.log` and
`/tmp/pox-admin-final-clippy.log`. The prior release load measurements predate
this pass; no new release performance claim is made here. Native streaming and
host-driven cancellation, remaining lifecycle/extension coverage, deployed
TLS/proxy verification, multi-hour/cross-platform validation and executed
runtime CI remain open.


## Sixteenth hardening pass: native request cancellation ownership

The FrankenPHP reference (`frankenphp.c`, force-kill registration and invocation)
uses Zend interrupt and timeout flags to trigger a VM-boundary bailout. Pox now
adds a feature-gated, single-use native cancellation handle with reference
ownership and mutex-protected attachment. Unlike a bare saved TSRM pointer, the
handle fences concurrent cancellation against request completion and detaches
before PHP thread resources can be freed or reused. Worker response callbacks
may release host memory before native cleanup; a separate native reference spans
that interval. Three reserved API pointer slots and two reserved request words
carry the feature without changing ABI 1.1 layouts.

`PhpRuntime::cancellation()` exposes this through `RequestCancellation` and an
optional `HttpRequest::cancellation` field. Reused handles and unsupported
runtimes return explicit errors. Cancelled web responses discard their captured
output. Cancelled workers exit and can be replaced without replay. A native
integration regression exercises eight cancellation/recovery cycles per mode,
pre-cancellation, rejected reuse and late cancellation concurrent with a healthy
subsequent request. It requires host interruption before a three-second PHP
fallback timer. Native smoke tests also exercise control ownership outside an
active execution mode.

The HTTP server currently supplies no cancellation handle. Wiring timeouts and
disconnects to this API, preserving admission until native cleanup finishes, and
verifying blocking extension/shutdown behavior are the next required work.
Zend's bailout currently logs the ordinary PHP maximum-execution-time error;
host cancellation is distinguished by the Rust error and response flag. No
signal-based interruption of blocking syscalls is implemented.

Memory checking also found the persistent INI string survived unloading the
native library. It now frees at library unload, preserving configuration across
normal execution-mode transitions. Detailed validation and memory-check limits
are recorded in the native cancellation report under `docs/validation`.

Local validation passes: 100 workspace tests (47 HTTP, seven runtime modes),
runtime-feature/all-target Clippy, native build/smoke/buffer tests and both patch
checks. The focused Memcheck address/allocation run also passes within its stated
limits. See [the cancellation evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-native-cancellation-linux-2026-09-06.md).
The overall production-readiness goal remains incomplete.


## Seventeenth hardening pass: HTTP cancellation and live readiness

Every PHP job now receives a single-use cancellation handle. An HTTP future's
cancellation or deadline requests native interruption; a separate 50-ms network
watchdog retries for retained expired/cancelled jobs. This covers a timeout
bailout entering a stuck PHP shutdown callback and PHP code resetting timeout
flags. Neither HTTP cancellation nor a 504 response releases native execution
or admission permits early. Completed requests disarm the cancellation guard,
and late watchdog requests are fenced by native detachment.

Two new real-socket regressions verify timeout interruption of CPU loops and
stuck PHP shutdown callbacks, subsequent healthy requests and client disconnect
interruption before the five-second request deadline, in both modes. Existing
readiness tests now block inside a PHP socket read: they prove native I/O can
remain active despite cancellation, retains its capacity and recovers after the
peer releases it. Existing bounded process shutdown remains the containment
mechanism for native calls that never return.

The initial test run exposed stale readiness after cancellation killed a worker:
the sampled count briefly reported capacity before a replacement initialized.
Readiness now holds a read-only atomic lifecycle counter independent of the PHP
runtime's lifetime and request locks. It reads initialization directly instead
of polling a cached count. Reload suspends readiness until replacement starts.
The regression passes with this race fixed.

Validation passes: 102 workspace tests, including 49 real HTTP tests, all-target
runtime-feature Clippy with warnings denied, and patch checks in both repositories.
The current debug binary also passes 10,000 requests per mode with 32 clients,
eight PHP threads and 4-KiB bodies: only verified 200 responses, bounded RSS/file
descriptors, eight worker replacements and clean process shutdown. This checks
the per-request cancellation handle lifecycle under load; it is not a release
performance comparison. [Exact load artifact](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-load-linux-cancellation-2026-09-06.json).
Logs: `/tmp/pox-http-cancel-final-tests.log` and
`/tmp/pox-http-cancel-final-clippy.log`.

HTTP cancellation of PHP VM execution and stuck PHP shutdown callbacks is now
implemented and locally verified. Arbitrary blocking extensions, native crash
containment, broader extension/platform lifecycle coverage, streaming/backpressure,
deployed TLS/proxy verification, multi-hour workloads and executed runtime CI
remain open production-readiness gates.


## Eighteenth hardening pass: TLS reverse-proxy deployment

Added `examples/deployment/Caddyfile` and a repeatable loopback-only deployment
harness. The template uses Caddy's X-Forwarded-For/Proto/Host handling and removes
client-supplied X-Forwarded-Port so it cannot override the public authority seen
by Pox. The harness adapts this same template with a temporary TLS certificate,
without altering system trust or disabling certificate verification.

Caddy 2.11.4 was downloaded from its official release; the archive passed the
published SHA512 checksum and GitHub SHA256 digest checks before execution. Both
PHP modes pass verified TLS, forwarding spoof rejection, binary body forwarding,
HTTP/2 frontend to HTTP/1.1 backend, management-listener separation, forced process
loss producing 502, proxy recovery after backend restart and active TLS request
drain on SIGTERM. Exact provenance is in the
[deployment result](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-caddy-linux-2026-09-06.json); the operational
instructions and scope are in the [deployment guide](http-server-deployment.md).

The process-loss test uses SIGKILL while PHP is busy and restarts Pox explicitly
from the harness. It does not prove native-extension crash behavior or automatic
supervisor recovery. An earlier software SIGSEGV attempt did not reliably kill
the process, so it was not accepted as crash evidence. Public ACME issuance and
renewal, HTTP/3, multi-host networking and production supervisor verification
remain open. No Rust/native implementation changed in this pass; the prior 102
workspace tests and Clippy cover the same binaries. The deployment harness and
both repository patch checks pass.


## Nineteenth hardening pass: native streaming and safe status parsing

The FrankenPHP reference's `frankenphp_ub_write` delivers output to its host,
`frankenphp_sapi_flush` sends headers and flushes the host, and output rejection
uses PHP's aborted-connection path. Pox now provides an opt-in, feature-gated
native equivalent through a request-scoped output callback table. Headers are
delivered once, body chunks are at most 16 KiB, flush events are explicit, and
no native response-body buffer is allocated on that path. Existing response
limits still bound total output bytes and serialized header allocations.

Rust `HttpOutput`/`ResponseOutput` keeps callback state alive during execution,
rejects reuse and unsupported runtimes, and reports sink rejection. Synchronous
callbacks allow bounded host backpressure but must arrange to unblock on
cancellation/disconnect/deadline. The final execution Result is authoritative:
a fatal after early output is an error, not a successfully completed body. The
HTTP adapter still supplies no sink; bounded transport queues, framing, abort
propagation, write deadlines and body/admission lifetimes remain to integrate.

Three native regressions run in both modes: a zero-capacity output channel proves
that headers/first output arrive before PHP proceeds beyond flush, all chunks
stay bounded, and the final buffered body is empty; sink rejection and zero-byte
limits return errors and permit subsequent healthy requests; a fatal after an
early flush reports failure despite output already delivered.

The reference comparison also exposed a native bounds bug: Pox reparsed the raw
HTTP status string at a fixed offset of nine bytes. It now uses PHP's parsed
status code, avoiding reads past strings such as `HTTP/`. A real-socket regression
covers two short forms and an ordinary explicit 404 in both execution modes.

Two additional output tests cover rejected headers during an explicit flush and
worker reuse with ignore_user_abort enabled. Worker request activation now resets
PHP's connection status so a prior rejected output does not report the next
client as disconnected. The callback sequence stops immediately after rejected
headers; normal aborted-connection policy applies before PHP's next statement.

Final local validation passes: 108 workspace tests (50 HTTP, 12 runtime modes),
Clippy, native build/smoke/buffer checks and both repository patch checks. Five
output regressions also pass the focused Memcheck address/allocation check with
zero definite leaks, within the explicitly documented limits. See
[the output evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-native-output-linux-2026-09-06.md).
At this pass, HTTP streaming was still incomplete. The twentieth pass below
adds bounded Hyper body integration and framing/cancellation tests.


## Twentieth hardening pass: bounded HTTP streaming

The native output API now feeds Hyper responses. A prefix capped at 64 KiB keeps
small responses transactional; explicit flush or reaching the prefix commits
headers and uses four queued chunks of at most 16 KiB each. PHP callbacks pause
when the queue fills and check cancellation every five milliseconds. This keeps
backpressure outside Tokio's network threads without trapping cancellation inside
a permanently blocked Rust callback. Output limits still bound total PHP bytes.

Streamed HTTP/1.1 uses chunked encoding with application Content-Length removed;
HTTP/1.0 uses connection closure. HEAD and bodyless statuses retain their metadata
paths. Final success waits for native completion and queued output consumption.
Fatal errors, limits, timeouts or disconnects after commitment abort the body
without a successful chunk terminator. Before commitment, invalid metadata returns
502 without unnecessarily killing a worker. An early-header/completion race is
handled explicitly so a fast flush cannot lose its queued body.

Native standard threads now reset exit status before reuse and include shutdown
callback failure in the final streamed result. A fatal before output still uses
PHP's ordinary 500 response. Transport activity covers response-body lifetime and
pending writes, so idle keepalive eviction cannot interrupt an active stream.
Execution and body admission lifetimes remain independently retained.

Six new socket regressions cover early flush, late fatal errors, repeated fast
flush and large-response keepalive pipelines, HTTP/1.0 closure, active-body idle
handling, disconnect/stalled-writer capacity recovery, committed deadline/output
limit failures, and standard shutdown-callback failure. The TLS deployment harness
also verifies early delivery through Caddy before PHP finishes. The load harness
now has a --flush mode that requires chunked responses and validates their data;
the scheduled Linux workflow includes this workload, but has not run remotely.

Validation: the rebuilt PHP 8.5.9 ZTS runtime passes the complete 114-test suite
(including 56 HTTP socket tests), and workspace/all-target Clippy with runtime
integration and warnings denied passes. The [flushed load evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-load-linux-streaming-2026-09-06.json)
records 10,000 successful requests per mode with 32 clients, eight PHP threads,
4-KiB bodies and exit-zero shutdown. Standard RSS growth was 2.2 MiB and worker
RSS growth was 4.5 MiB, with eight worker replacements. This is a short debug-build
regression workload, not release performance or multi-hour stability evidence.
The [Caddy streaming evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-caddy-streaming-linux-2026-09-06.json)
records all nine deployment checks passing in both modes, including early flush
through verified TLS, and exact executable/runtime hashes.

Production readiness remains open for arbitrary blocking native extensions,
native crashes and automatic supervisor recovery, broader lifecycle/platform
coverage, multi-hour workloads on the current release build, public ACME and
multi-host deployment, executed remote CI and coordinated native publication.


## Twenty-first hardening pass: automatic supervisor recovery

Added `examples/deployment/pox.service` with failure restart, a bounded start rate,
SIGTERM/control-group stop, a shutdown timeout longer than the Pox default, a
dedicated service account, no new privileges, restricted umask and journal logs.
The deployment guide describes installation, readiness, crash-loop recovery and
permissions. These install commands were documented, not executed on this host.

The TLS harness now supports `--systemd`. It renders a uniquely named temporary
user unit from the checked-in template, removes only account selection and replaces
paths/arguments for its isolated application. Both standard and persistent-worker
modes pass forced SIGKILL -> in-flight 502 -> automatic systemd restart -> new PID
and PHP/TLS recovery. Exactly one restart is observed. `systemctl stop` drains an
active TLS request and leaves the unit inactive beyond its restart interval. The
harness records the identities/counter/state and cleans up its temporary units.
See [the recorded evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-caddy-systemd-linux-2026-09-06.json).

This closes the local automatic-supervisor recovery check for process loss. It
does not establish native-extension crash behavior, dedicated-account system
service/boot integration, crash-loop rate-limit behavior, public ACME or multi-host
operations. The remaining runtime, platform, sustained-load and remote CI gates
remain open. No Rust/native behavior changed in this pass.


## Twenty-second hardening pass: native memory-fault recovery

Added a test-only PHP extension that performs a volatile write to an anonymous
PROT_NONE page. The build helper uses the matching PHP SDK and records compiler,
PHP version and fixture source/library hashes. The fixture is loaded only by the
isolated deployment harness through its temporary PHP INI configuration; it is
not linked into or installed alongside the application runtime.

The harness's `--fault-extension` mode verifies the function loaded, invokes it
inside PHP and requires actual SIGSEGV process termination. Before commitment,
Caddy returns 502. After a flushed prefix, the TLS client observes an incomplete
body rather than successful termination. Both standard and worker modes recover
through automatic systemd restart, with two successive faults yielding two
replacements and restored PHP readiness/responses. Graceful service stop and all
previous TLS/proxy checks still pass. A separate run exercises the harness-managed
restart path as well. See [systemd evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-native-crash-linux-2026-09-06.json),
[direct evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-native-crash-direct-linux-2026-09-06.json) and
[fixture build provenance](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-native-fault-build-linux-2026-09-06.json).

This establishes Linux/PHP 8.5.9 ZTS process-level recovery from a synchronous
extension memory fault, including committed responses. It does not claim that
native faults can be isolated to one PHP thread, that arbitrary extension calls
can be interrupted, or that other platforms/extension combinations are covered.
System-service boot/account integration, sustained release-build workloads and
executed remote CI/publication remain open. No Rust or runtime C implementation
changed in this pass; the new C file is exclusively a fault-injection fixture.


## Twenty-third hardening pass: sustained release validation started

The load driver now supports a full-duration workload, bounded log consumption,
bounded latency histograms, minute-scale resource history and live progress.
Fixed-count semantics still require the requested count; a deliberate one-second
deadline against ten million requests correctly returns a failed report and exit
status 1. Short duration/count workloads pass in both modes. A 65-second flushed
worker run completed 165,606 verified responses, 828 replacements, 3.4 MiB RSS
growth and exit-zero shutdown, with two resource-history points. See
[driver validation](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-duration-driver-linux-2026-09-06.json).

The current Rust implementation was rebuilt with the configured release profile
(opt-level 3, LTO, one codegen unit, panic abort), completing in 4m14s. A copy of
that executable, the matching native library and the load driver was made read-only
in an isolated temporary directory so future builds cannot overwrite an active
workload. The [start record](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-release-soak-start-linux-2026-09-06.json)
contains exact hashes, commands and live process observations.

Two-hour flushed workloads have started concurrently for standard and worker
modes, each using 32 clients, eight PHP threads, 4-KiB bodies and recycling at
1000 requests. Both drivers and servers were observed live with initial correct
responses. This is a start record, not a completed soak result. The final response,
resource and shutdown reports must be inspected before closing the sustained-run
gate. Concurrent runs share host resources and are not isolated throughput
comparisons. Other platform, application/extension and deployment/CI gates remain.


## Twenty-fourth hardening pass: release deployment and crash-loop limits

The deployment harness now has an optional `--crash-loop` check requiring
`--systemd`. After ordinary shutdown validation it starts Pox, resets the rate
window while the unit is active, and kills the process repeatedly. The current
process predates that window; five replacement starts are permitted, after which
systemd refuses another restart with `Result=start-limit-hit`. The backend has no
PID, Caddy returns 502, and an immediate manual start is refused. The documented
operator reset then restores PHP readiness and TLS responses. These checks use
the service template's actual five-start/60-second policy and two-second delay.

A first harness attempt tried resetting an already-unloaded successful unit;
systemd correctly refused that operation. Resetting while active makes the tested
window explicit and independent of successful-unit garbage collection.

The complete 15-check suite passes in both PHP modes against the configured LTO
release binary, including real extension SIGSEGV before headers and after flush,
automatic process replacement, stream abortion, verified TLS, forwarding identity,
HTTP/2 frontend, management separation and graceful stop. See
[release deployment evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-release-crash-loop-linux-2026-09-06.json).
All temporary service units were removed. No production service was installed,
and no Rust/native implementation changed in this pass.

The two-hour standard/worker workloads remain active in their isolated snapshots;
progress is not a final result. System-service boot/account integration, broader
platform/application/extension coverage and executed remote CI remain open.


## Twenty-fifth hardening pass: PHP-version build correctness

Preparing the PHP 8.4 coverage run exposed a cached-source problem in the runtime
build recipe. With an 8.5.9 download cache present, SPC 2.8.6 accepted
`--with-php=8.4.25` but reused the 8.5.9 archive. Explicitly refreshing `php-src`
retrieved 8.4.25. Its tar.xz SHA-256 matches the
[official PHP release metadata](https://www.php.net/releases/index.php?json&version=8.4):
`dc1ad8b4109898d9db49744450403874858c23efc685b1032a50bd1e83906848`.

The sibling runtime recipe now refreshes PHP source selection and requires
`php-config --version` to match the requested exact version before runtime linking
or packaging. A controlled mismatched-SDK execution returned exit 1 and invoked
neither build nor packaging; shell syntax validation passes. The README now
requires an isolated working directory for each PHP version, since refreshing a
download alone cannot prove an existing extracted source tree was replaced.

A fresh PHP 8.4.25 ZTS SDK build is active in its own directory with the runtime's
full extension/library selection and four build jobs. See the
[build start record](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-php84-build-start-linux-2026-09-06.json).
This is preparation, not PHP 8.4 test evidence: SDK completion, native ABI linking,
HTTP/runtime tests and deployment tests remain to run. The independent two-hour
PHP 8.5 release workloads continue from their immutable snapshots.


## Twenty-sixth hardening pass: body outcome observability

Added `pox_http_response_bodies_total` with three bounded outcomes: complete,
error and dropped. Response-class and handler-duration counters retain their
header-production semantics. Access logs now explicitly carry
`phase: response_headers`. A flushed 200 can subsequently produce a body error;
operators can observe that without treating an already-sent status as replaceable.
Body completion means production/consumption by Hyper, not verified client receipt.
Native process loss cannot emit an outcome and still requires supervisor monitoring.

The existing transport body wrapper records one outcome independently of its
activity lifetime. It recognizes Content-Length exhaustion and empty HEAD/bodyless
responses, because Hyper need not poll another EOF frame in those cases. Pending
streams do not increment a terminal outcome; dropping one counts abandonment.
The metric state is retained independently of the server/executor lifetime.

Two socket tests cover both PHP modes: pending/successful/fatal streams, static
length completion, HEAD and disconnect accounting. The full runtime-enabled suite
passes 116 tests, and workspace/all-target Clippy with warnings denied passes.
See [validation evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-body-metrics-linux-2026-09-06.json).

PHP 8.4 SDK compilation and both two-hour workloads remain active. The workload
snapshots predate this observability addition, so their eventual results validate
the earlier release's HTTP/PHP behavior and do not establish the new counters'
sustained behavior. No running snapshot was overwritten.


## Twenty-seventh hardening pass: premature static EOF accounting

A real-socket truncation regression exposed a false complete outcome in the new
body metrics: a static file could shrink after its Content-Length was sent,
causing early EOF. The wrapper now records incomplete declared lengths as errors,
including EOF discovered at drop, and detects data exceeding the declared length.
Already-empty HEAD/bodyless responses retain their metadata exception. The test
uses a sparse 64-MiB file, truncates it after headers, verifies fewer response
bytes and requires an error outcome with no completion count.

All three body-metric socket tests pass; the full suite passes 117 tests and
workspace/all-target Clippy passes with warnings denied. See
[regression evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-premature-body-linux-2026-09-06.json).
PHP 8.4 dependency compilation and the two-hour release workloads continue;
the latter still exercise snapshots predating the metric changes. No completion
claim is made for either ongoing validation effort.


## Twenty-eighth hardening pass: PHP 8.4 and real Symfony coverage

The isolated PHP 8.4.25 ZTS SDK completed with the configured extension set.
The native ABI library compiled against it and passed the native smoke checks;
all 117 runtime-enabled Rust/socket tests pass on that library. The 15-check TLS,
native-crash, streaming and supervisor suite also passes in both modes. See
[PHP 8.4 runtime evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-php84-runtime-linux-2026-09-06.json) and
[deployment evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-php84-deployment-linux-2026-09-06.json).
The host lacks static libstdc++.a, so this local build uses the documented dynamic
C++ opt-in. It is not a published or release-qualified native artifact.

Added a reproducible HTTP harness for a copied installed Symfony Demo application.
On PHP 8.5.9 ZTS, both modes pass real Twig/Doctrine SQLite blog rendering, populated
RSS, CSRF login forms, fixture-account authentication/session continuity,
anonymous-request isolation, 80 concurrent alternating-locale requests and graceful
shutdown. It retains the Symfony Kernel in worker mode, using Symfony's own
per-main-request reset, with recycling configured at 20 requests. Dependencies
include Symfony 8.1.0, Doctrine ORM 3.6.7 and Twig 3.27.1. See the
[framework guide](http-server-framework-testing.md) and
[application evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-symfony-linux-2026-09-06.json).

The source app lacked generated Runtime/importmap artifacts. Preparation now
regenerates the Runtime loader from its installed template and runs normal Symfony
asset/cache commands in the copy. A transient CDN TLS failure was recovered by
bounded importmap setup retries; request assertions are not retried and certificate
verification remains enabled. The source checkout is not modified.

The two-hour PHP 8.5 release workloads remain active in their earlier snapshots.
This pass closes the local PHP 8.4 ABI/HTTP/deployment checks and adds one real
framework fixture; it does not establish cross-platform, every-extension,
release publication, system-service boot or remote CI coverage.


## Twenty-ninth hardening pass: Symfony on both supported PHP series

The unchanged real-application harness passes all seven checks in standard and
worker modes on PHP 8.4.25 ZTS as well as 8.5.9. This verifies database-backed
rendering, RSS, CSRF generation, fixture login/session continuity, anonymous
isolation and concurrent locale requests against the same Symfony 8.1 dependency
manifests. See [PHP 8.4 application evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-symfony-php84-linux-2026-09-06.json).
No host/server implementation was changed for this result.

A release-profile rebuild with the latest body metrics is active. The existing
two-hour workloads continue on their earlier immutable snapshots and have no
reported failures at the latest observation. Neither build intent nor workload
progress substitutes for completed artifact/shutdown validation.


## Thirtieth hardening pass: current release and memory attribution

The release rebuild with body metrics completed in 4m28s. It passes 100,000 flushed
requests per PHP mode and all 15 TLS/native-crash/supervisor checks per mode. See
[current release load](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-current-release-load-linux-2026-09-06.json)
and [deployment evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-current-release-deployment-linux-2026-09-06.json).
The two-hour workloads remain separate, active runs on their earlier snapshots.

Slowly rising worker RSS prompted a controlled recycling comparison. At 30,000
requests, no recycling grew RSS by 1.2 MiB; 3000 replacements grew it by 12.6 MiB.
Both completed successfully. A small Massif run with 100 replacements and
USE_ZEND_ALLOC=0 showed an early high live-heap sample of 7.50 MB and a late peak
of 7.57 MB, with fluctuating active-thread allocations. This is insufficient to
claim an unbounded leak or stable long-term memory. The profile and limits are
recorded in the [load-testing guide](http-server-load-testing.md); no speculative
memory-management change was made. The final sustained reports remain required.


## Thirty-first hardening pass: musl dynamic loading

Cross-platform preparation found a release-blocking musl issue: the release recipe
forced a static PIE executable, but PHP is loaded as a separate shared library.
The checksum-verified published v0.0.2 musl executable, paired with the matching
checksum-verified PHP 8.5.9-r2 musl runtime, fails with `Dynamic loading not
supported`. This failure occurs before PHP initialization; passing `--help` did
not exercise the required loader. See [reproduction evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-musl-loader-linux-2026-09-06.json).

The musl Dockerfile now disables static CRT linkage and runs all three real
shared-library ABI-loader tests before building the release binary. Those tests
pass inside Alpine with Rust 1.98. The release workflow checks for the musl ELF
interpreter and runs the executable in Alpine with libgcc, since the glibc runner
need not have the musl loader. README requirements now describe dynamic loading.
These source/workflow corrections have not been published or run in remote CI.

The corrected musl release build and a separate current PHP 8.5.9 musl runtime
build are active in local containers. The latter uses isolated copied sources,
cached dependency downloads and four build jobs. Full PHP/HTTP/deployment tests
remain pending; fake-ABI loader success does not close that compatibility gate.
The original glibc sustained workloads continue independently.


## Thirty-second hardening pass: native archive notice payload

The runtime packager searched `.spc/source`, but the current builder stores source
and its collected notices outside the SPC executable directory. As a result, SDK
notices were absent from the archive. Packaging now copies the selected SDK's
`license` directory into `licenses/php`, supports an explicit `PHP_LICENSE_DIR`
and refuses a missing/empty notice directory.

A real local PHP 8.4 runtime archive contains all 24 SDK-collected notice files,
verified byte-for-byte, and the archived library hash matches runtime.json. A
missing-directory test exits 1 without creating an archive. See
[packaging evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-runtime-license-packaging-linux-2026-09-06.json).
This validates preservation of the SDK payload, not an independent legal review.
No archive was published.

Both musl builds and the glibc sustained workloads remain active. The musl build
context predates this packaging correction; after it completes, its runtime must
be repackaged with the corrected script before validating an archive payload.


## Thirty-third hardening pass: real musl runtime and HTTP validation

The corrected x86_64 musl executable and current PHP 8.5.9 ZTS runtime both built
successfully. The native smoke check passes, and all 117 runtime-enabled Rust and
socket tests pass in Alpine. The release executable uses the musl interpreter and
links libgcc_s plus musl libc. A fresh Alpine 3.22 container with only libgcc added
passes `--help` and actual PHP execution through the shared runtime, exercising
the release workflow's updated verification path.

All nine direct Caddy/TLS checks pass in standard and worker modes, including
verified certificates, HTTP/2 frontend, forwarding protection, early flush,
forced-process-loss recovery and graceful drain. These container tests use the
harness restart path, not systemd or the native SIGSEGV extension fixture. See
[musl runtime evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-musl-runtime-linux-2026-09-06.json),
[deployment evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-musl-deployment-linux-2026-09-06.json) and
[native archive evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-musl-native-linux-2026-09-06.json).

The completed runtime was repackaged with the corrected notice script; its archive
contains 24 SDK notices and a matching library digest. This replaces the earlier
build-context packaging output for validation. No source correction or artifact
has been published. The glibc two-hour workloads remain active; aarch64, Darwin,
remote CI and the remaining deployment/sustained checks are still unverified.

## Thirty-fourth hardening pass: musl native faults and flushed load

The native-fault fixture initially failed to load on musl because its parameter
and error helpers imported private PHP symbols. The fixture now uses argument
metadata directly and libc aborts for setup failures. A setup failure therefore
produces SIGABRT, distinct from the protected-page write's expected SIGSEGV.
This changes only the test fixture; it does not establish support for arbitrary
shared PHP extensions against the runtime's private symbol interface.

All 11 direct Caddy/TLS checks pass in both modes on musl and glibc with the
corrected fixture, including crashes before headers and after a committed flush.
See [musl evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-musl-portable-fault-linux-2026-09-06.json) and
[glibc evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-glibc-portable-fault-linux-2026-09-06.json).
These runs use harness-managed restarts, not systemd.

The musl release also passes 100,000 flushed requests per mode with 32 clients,
eight workers and recycling every 1,000 requests. Both runs meet the harness's
RSS/file-descriptor bounds and exit cleanly. See
[load evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-musl-load-linux-2026-09-06.json).
Concurrent glibc soaks share the host, so these timings are not isolated
performance comparisons. The two-hour glibc runs are still pending; the other
unverified platform, deployment and publication gates remain open.

## Thirty-fifth hardening pass: repeated active-worker reloads

A new real-socket regression performs eight watcher-triggered reloads with both
workers held inside active callbacks each time. Each callback must return its
original generation and request identity; a probe then requires the next
generation. After all eight reloads, SIGTERM must exit successfully and record
shutdown_complete. Atomic script replacement avoids testing partially written
application files.

The targeted test passes on Linux x86_64 glibc PHP 8.4.25 and 8.5.9, and Alpine
musl PHP 8.5.9. CLI all-target runtime-feature Clippy and both repository patch
checks pass. See [evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-repeated-reload-linux-2026-09-06.json).
The full suite was not rerun for this test-only addition. Simultaneous shutdown
and reload, arbitrary reload storms and non-Linux execution remain unverified.
The original two-hour glibc workloads are still live with no reported failures.

## Thirty-sixth hardening pass: shutdown during a pending reload

A real-socket regression holds a PHP callback, starts a watcher reload, then sends
SIGTERM while the reload is waiting. It observes the listener close before
releasing PHP. The released case must preserve the original response and exit
cleanly; the blocked case must exit 1 at the configured shutdown deadline with
shutdown_deadline_exceeded, without claiming a completed drain.

Both cases pass on Linux x86_64 glibc PHP 8.4.25 and 8.5.9. The full current PHP
8.5.9 runtime suite passes all 119 tests, including both newly added lifecycle
tests. CLI all-target runtime-feature Clippy and patch checks pass. See
[evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-shutdown-reload-linux-2026-09-06.json). This overlap
test has not run on musl or other platforms. The two-hour workloads remain
active; their final resource bounds and clean shutdown are not yet verified.

## Thirty-seventh hardening pass: malformed framing and pipeline isolation

A deterministic corpus covers conflicting comma-separated lengths, negative and
overflow lengths, whitespace before header colons, duplicate/chained transfer
codings, invalid chunk sizes and delimiters, and malformed trailers. Each case
appends a valid pipelined request. The socket test requires exactly one 400
response, no PHP side effects from either request, and a successful fresh request
after the corpus.

All 13 cases pass in both standard and worker modes on glibc PHP 8.4.25 and 8.5.9.
CLI all-target runtime-feature Clippy and patch checks pass. See
[evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-framing-corpus-linux-2026-09-06.json). This is targeted
coverage, not exhaustive protocol fuzzing or a new full-suite result.

Local platform inspection found no registered aarch64 interpreter or available
qemu-aarch64-static executable; Docker reports amd64 variants only. No Darwin
runner is available here. Those execution gates remain open. Both original
glibc two-hour processes are still live, each beyond 18 million requests with
zero reported failures; their final resource and shutdown results remain pending.

## Thirty-eighth hardening pass: extension module startup failure

A new test-only extension returns FAILURE from MINIT. The extension builder now
accepts an explicit --source while retaining the crash fixture as its default.
With this extension configured, both server modes exit 254 within 40 ms and log
the named module startup failure; repeated loopback probes observe no reachable
HTTP listener. After removing the bad extension setting, fresh processes serve
a valid request and exit cleanly on SIGTERM. See
[evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-startup-failure-linux-2026-09-06.json).

This local glibc PHP 8.5.9 result establishes process-failure behavior and recovery
after configuration correction. PHP terminates during this module failure, so
it does not validate same-process retry or cleanup after a returned failure from
php_module_startup. Source inspection found earlier return paths during stream
wrapper and module registration that need separate fault-injection evidence
before claiming safe partial-initialization recovery. No speculative native
cleanup change was made. Platform and two-hour workload gates remain open.

## Thirty-ninth hardening pass: unprivileged deployment and repeatable startup checks

The musl deployment harness passes all 11 checks per mode as UID/GID 65532, with
all Linux capabilities dropped, no-new-privileges, a read-only root filesystem
and read-only artifact/script mounts. Only /tmp is writable, using a bounded
128-MiB tmpfs for the isolated application, certificates and Caddy state. This
includes native faults, interrupted committed TLS responses, recovery and drain.
See [container evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-unprivileged-container-linux-2026-09-06.json).
It does not establish host system-service account permissions or boot behavior.

The startup-failure probe is now checked in as scripts/test-http-startup-failure.py,
with bounded subprocess cleanup, artifact hashes, both server modes and a fresh
process recovery check. Matching startup-failure fixtures built against PHP
8.4.25 and 8.5.9 both pass this script locally. See
[repeatable evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-startup-repeatable-linux-2026-09-06.json).
This retains the earlier limit: extension MINIT failure terminates PHP, so it
does not prove safe retry following a returned partial-initialization failure.

## Fortieth hardening pass: current musl suite and evidence summary

The complete current musl runtime-enabled suite passes all 120 tests, including
repeated reloads, shutdown/reload overlap and malformed framing with pipelined
follow-up isolation. See [suite evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-musl-current-suite-linux-2026-09-06.json).
The startup-failure harness also passes both modes under the unprivileged,
read-only musl container restrictions, using a matching SDK-built fixture; see
[startup evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-musl-startup-recovery-linux-2026-09-06.json).

The [current readiness summary](http-server-readiness.md) consolidates evidence
by platform and behavior without accumulating historical test totals. It makes
the unpublished native ABI dependency and outstanding gates explicit. Both
original glibc two-hour workloads remain live, each beyond 20 million requests
with no failures reported; final resource bounds and shutdown remain pending.

## Forty-first hardening pass: musl release packaging execution

Release verification had been moved into Alpine, but the later package-release.sh
step independently executes the binary's --help on its host. The current musl
binary reproduces exit 127 there on glibc (required interpreter not found),
preventing archive creation. The workflow now executes musl packaging in Alpine
3.22 with bash and libgcc; other target packaging remains on its native runner.

The corrected path creates a real archive with a matching checksum, exactly the
expected binary/README/LICENSE payload, original binary digest and mode 0755.
The extracted executable starts in a fresh Alpine container with libgcc. The
release workflow passes actionlint. See
[packaging evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-musl-release-packaging-linux-2026-09-06.json).
This is local execution of the packaging path, not a remote CI run or publication.

## Forty-second hardening pass: loader checks across release builders

The glibc Docker builder and macOS release step now run the existing three
shared-runtime loader tests before producing a release binary, matching the
musl builder's guard. These tests load a compiled fixture, exercise the versioned
ABI and response ownership, and reject wrong targets and overlapping versions.
They require no PHP SDK; real PHP execution remains a separate validation gate.

The updated release workflow passes actionlint and patch checks. A local build
of the actual glibc release Dockerfile is running against rust:1.98-bullseye; its
loader result and final artifact are not yet available. This configuration
change does not establish macOS or aarch64 execution. No release was published.

## Forty-third hardening pass: glibc release baseline distinction

The locally built PHP 8.5.9 runtime requires glibc symbols through 2.44. Loading
dependencies in a Bullseye container reports missing glibc versions; local
HTTP successes therefore do not establish release-baseline portability. The
CLI release builder uses Bullseye, while the native runtime release builder uses
Bookworm. These must be validated as separate artifacts and as a pair.

An isolated copy of the current native sources, source-download cache and SPC
executable is now building PHP 8.5.9 in the Bookworm release Dockerfile, with four
jobs. The original local native libraries and running soak snapshots are intact.
The glibc CLI release build also remains active. See
[baseline evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-local-glibc-baseline-linux-2026-09-06.json).
Neither release-container build has completed, so no new portability claim is
made. Earlier local glibc results retain their original host-only scope.

## Forty-fourth hardening pass: real glibc release CLI artifact

The Bullseye CLI release image completed successfully: three loader tests passed
and the configured release build finished in 428.5 seconds. The exported binary
requires glibc symbols through 2.30 and starts in an unmodified minimal Bullseye
container. Its dependencies include liblzma and libgcc plus glibc libraries.

On the development host, the exported binary also executes PHP 8.5.9 using the
existing native runtime. See [CLI evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-glibc-release-cli-linux-2026-09-06.json).
The Bookworm native runtime build remains active; that artifact and the complete
release pair still need validation. No host-runtime compatibility result is
reinterpreted as Bookworm compatibility, and nothing has been published.

## Forty-fifth hardening pass: Bookworm runtime and release pair

The native Bookworm image completed. Its library requires glibc through 2.36 and
links only libm/libc plus the loader; the dynamic C++ escape hatch was not used.
C smoke and response-allocation tests pass. The Bullseye CLI plus this runtime
executes PHP 8.5.9 ZTS in a minimal Bookworm container. The runtime archive has a
matching manifest/library digest and all 24 SDK notices verified byte-for-byte.
See [native artifact evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-glibc-release-native-linux-2026-09-06.json).

All nine direct Caddy/TLS checks pass in both modes using this release pair inside
the Bookworm builder, including forwarding protection, early flush, process loss
and harness restart, and graceful drain. See
[deployment evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-glibc-release-pair-deployment-linux-2026-09-06.json).
This run does not include the native-fault fixture or systemd, and full regression
and load refresh against the new runtime remain pending. Earlier host-built
libraries and the still-running sustained snapshots were not replaced.

## Forty-sixth hardening pass: release-native regression, load and supervision

All 120 current Rust/runtime tests pass against the Bookworm-built native library
on the development host. The release CLI/native pair separately passes 100,000
flushed requests per mode inside Bookworm, with 32 clients and eight workers.
Standard/worker elapsed times were 23.805/24.021 seconds, RSS growth was
2,482,176/3,772,416 bytes, descriptor bounds passed and shutdown exited 0.
The shared host also runs the original soaks; these are not isolated throughput
comparisons. See [suite](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-bookworm-native-suite-linux-2026-09-06.json)
and [load](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-bookworm-release-load-linux-2026-09-06.json) evidence.

A matching SDK-built crash fixture passes all 11 checks per mode in an
unprivileged read-only Bookworm container. The same release artifacts also pass
all 15 checks per mode under the host user systemd manager, including native
crash recovery, committed TLS response interruption, restart limiting and reset.
The startup-failure fixture passes both modes in Bookworm. See
[native faults](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-bookworm-native-fault-linux-2026-09-06.json),
[systemd](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-bookworm-systemd-linux-2026-09-06.json) and
[startup](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-bookworm-startup-linux-2026-09-06.json).

These results close the local release-pair refresh identified in pass 45. The
original two-hour runs still use their recorded older immutable snapshots;
publication, broader platforms and real system-service boot remain unverified.

## Forty-seventh hardening pass: complete workflow lint coverage

Linting all Pox workflows exposed an unregistered custom runner label in the
existing CI and builder workflows. .github/actionlint.yaml now lists the exact
namespace-profile-pox label; no general runner-label checks are suppressed.
All Pox and sibling runtime workflows pass actionlint. Rust formatting, both
repository patch checks, native shell syntax and Python script parsing pass.
This validates configuration syntax, not runner availability or remote execution.
The original sustained runs remain active and their final reports are pending.

## Forty-eighth hardening pass: completed two-hour workloads

Both original glibc release-snapshot runs completed their full 7,200 seconds and
passed. Standard mode served 31,334,866 requests; worker mode served 30,847,271.
All responses were 200, with no recorded contract errors. Both native servers
and harnesses exited 0. Shutdown took 0.412/0.453 seconds. Standard RSS grew
3,371,008 bytes; worker RSS grew 34,271,232 bytes, both within the configured
64-MiB bound. Both ended with 11 descriptors. Worker mode recorded 30,845
replacements. See [final evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-two-hour-soak-linux-2026-09-06.json).

The binary, runtime and harness snapshot hashes were reverified against the start
record and both final reports. This completes the previously pending sustained
run, not the overall readiness audit. The snapshots predate body-outcome metrics
and the Bookworm native release build. Runs shared the host, and worker RSS
increased over the test; this result does not prove indefinite memory stability.
Remote CI/publication, broader platform execution and deployment-specific gates
remain open as recorded in the current readiness summary.

## Forty-ninth hardening pass: candidate native-source CI path

The existing integration workflow only installed released native runtimes, so it
could not validate the unpublished ABI changes. Manual dispatch now optionally
accepts a full native commit SHA, rejects mutable/malformed references, verifies
the checked-out revision, builds the selected native source and exports its
absolute library path to the full HTTP-enabled suite. Scheduled/release-triggered
runs retain published-runtime installation. Linux aarch64 joins the matrix.

Candidate Linux builds reuse the Bookworm Dockerfile and native smoke checks;
macOS builds reuse the native release prerequisites and scripts. Test logs, Linux
buffered/flushed load reports and source/artifact provenance are retained. All
workflows pass actionlint; the embedded Python parses, and full-SHA acceptance/
rejection checks pass. No remote run is claimed, and musl remote execution remains
outside this matrix. The source changes are still local and unpublished.

## Fiftieth hardening pass: review branches and live candidate CI

The prepared patches were committed in isolated checkouts and pushed to
codex/http-server-hardening in both repositories. Native commit is
10c8b5d21b620c0e2e5868da28f526d0e1efd246; Pox commit is
785e7bd3df5fc0a8ad77cc0b5080e011a712ded2. Original worktrees and indexes were
preserved. No tag, release, merge or pull-request message was created.

The actual staged check found end-of-line whitespace in two generated Callgrind
annotation reports that earlier unstaged checks did not inspect. Exact-file Git
attributes preserve those reports verbatim while keeping other whitespace checks.
The complete staged patch then passed.

[Candidate run 34062110978](https://github.com/shyim/pox/actions/runs/34062110978)
is verified in progress at the recorded Pox commit. Its five jobs cover Linux
x86_64 PHP 8.4/8.5, Linux aarch64 PHP 8.5, Darwin x86_64 PHP 8.4 and Darwin
aarch64 PHP 8.5, each building the pinned native source. See
[start record](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-candidate-ci-start-2026-09-06.json). Execution has
started, but no job result or platform success is claimed yet.

## Fifty-first hardening pass: live musl candidate CI

The source-candidate workflow now covers x86_64 and aarch64 musl in separate
Alpine jobs. Native candidates use their musl Dockerfile; Rust tests retain
-crt-static so they can load the shared PHP library. Both jobs run the full
HTTP-enabled suite, ZTS execution, and buffered/flushed load checks and retain
provenance/test artifacts. A musl_only dispatch option avoids repeating the
already-running glibc/Darwin jobs.

The workflow passes actionlint, embedded Python parsing and local Alpine shell
capability checks. Pox review commit 367965a adds this path.
[Run 34062300114](https://github.com/shyim/pox/actions/runs/34062300114) is verified
active at the matching commit and pinned native SHA. The original five-job run
continues independently. See [start record](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-musl-candidate-ci-start-2026-09-06.json).
No completed remote test result is claimed yet.

## Fifty-second hardening pass: candidate dependency download authentication

The first musl x86_64 candidate job failed before PHP compilation when SPC's
GitHub release API downloads returned HTTP 403. Candidate builds omitted the
read-only GITHUB_TOKEN used by the existing native release workflow. Linux
candidates now pass it through the native Dockerfile's BuildKit secret mount;
macOS candidates receive it in the build process environment. No token is placed
in a build argument or artifact.

Pox review commit 49a66ca contains the correction and passes actionlint/staged
checks. [Run 34062409646](https://github.com/shyim/pox/actions/runs/34062409646) is
verified active at that commit with the same native source SHA. The earlier
five-job run remains active without a reported failure. See
[start record](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-musl-candidate-auth-start-2026-09-06.json).
The corrected job results remain pending.

## Fifty-third hardening pass: full authenticated candidate run

The original Linux aarch64 candidate failed before native compilation: several
dependency endpoints returned 502/504, GitHub API requests returned 403, and SPC's
libcares fallback raised a filename-type error. This job predates the token
correction. A full run of the corrected workflow is now dispatched at commit
49a66ca9b34609851826c50e465ea7822a4c7d7c, still using native
10c8b5d21b620c0e2e5868da28f526d0e1efd246.

[Run 34062511910](https://github.com/shyim/pox/actions/runs/34062511910) is verified
in progress with seven jobs. The authenticated musl-only run is also still
building without a reported failure. Earlier in-progress jobs were not restarted
because of observation timeouts. See
[start record](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-full-candidate-ci-start-2026-09-06.json).
Remote platform success remains unproven until the actual test jobs finish.

## Fifty-fourth hardening pass: remote recycling test and optional runtime version

The original Darwin ARM candidate built successfully, then passed 58 HTTP tests
and failed the recycling abandonment test with a 503 where 200 was expected.
Its assertion helper did not identify the request phase. The test immediately
checked recovery after releasing a paused bootstrap while retaining a 100 ms
queue deadline. It now waits for independent admin readiness before that recovery
request; abandonment deadlines and exact PHP side-effect checks remain intact.
This addresses a startup timing race, but the Darwin result still needs rerunning.

A local invocation without POX_PHP_RUNTIME also reproduced a startup panic from
indexing absent PHP configuration in a server-only pox.toml. Version selection now
uses optional lookup, with regression cases for empty, server-only, INI-only and
explicit-version configurations. The full local suite passed 121 tests against
the Bookworm PHP 8.5.9 library. See [validation record](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-ci-recovery-fixes-2026-09-06.json).

Both authenticated musl-only jobs progressed beyond native compilation into the
Rust integration stage, establishing that the earlier dependency-download failure
was cleared in those jobs. Remote test outcomes remain pending.

The initial ARM musl job completed successfully: 120 tests and four 5,000-request
load checks (buffered/flushed, standard/worker), with no errors and clean shutdowns.
The overall run failed on the separate x86_64 download job. See [ARM musl evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-ci-musl-arm-linux-2026-09-06.json).
The latest Pox commit 3cfef6d95bd1af1009879872064e087d765bb73d is dispatched in
[run 34063032366](https://github.com/shyim/pox/actions/runs/34063032366), using the
same native commit, to validate the optional-version fix and readiness-synchronized
recycling test across the full matrix. Its results remain pending.

## Fifty-fifth hardening pass: authenticated musl and Darwin ARM results

Both jobs in authenticated musl run 34062409646 completed successfully. Each ran
120 tests and four 5,000-request load checks (standard/worker, buffered/flushed)
with no errors and clean exits. Darwin ARM in run 34062511910 also completed
successfully with all 117 applicable tests; the earlier recycling failure did
not recur on that unchanged test. Three Linux-specific HTTP tests and the Linux
resource-load harness do not run on Darwin.

These results validate Pox 49a66ca and native 10c8b5d, preceding the optional
version fix and readiness synchronization. The latest run 34063032366 is live
on all seven jobs. See [platform evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-ci-authenticated-platforms-2026-09-06.json).

## Fifty-sixth hardening pass: shutdown coverage and platform timer policy

A second ARM musl job in run 34062511910 passed 61 HTTP tests but failed the
shutdown-cancellation fixture because its cleanup marker was absent. The fixture
previously entered shutdown only after cancelling an infinite main loop; repeated
cancellation can interrupt the shutdown callback before its first statement.
It now uses `exit` to enter an infinite shutdown callback before the deadline.
Separate `/loop` requests retain main-execution cancellation coverage. Both modes
passed ten local repetitions against the Bookworm runtime.

The original Darwin Intel PHP 8.4 job passed its HTTP tests but failed reusable
web-thread isolation: a trivial request returned empty output with a reported
30-second PHP timeout within milliseconds. PHP's fallback `setitimer` is
process-wide; FrankenPHP disables its INI timeouts on builds without
ZEND_MAX_EXECUTION_TIMERS. Pox now follows that startup policy in HTTP modes and
prevents its own runtime INI reapplication from restoring those timer settings.
CLI settings are preserved. This is a likely explanation for the Darwin failure,
not yet a remotely proven resolution. See PHP [bug 79464](https://bugs.php.net/bug.php?id=79464)
and the cloned FrankenPHP `go_get_custom_php_ini` implementation. A regression
checks both HTTP modes' platform-specific configured values.

The rebuilt local native library passed its smoke test and all 122 Rust/runtime
tests. The no-per-thread-timer branch passed strict C syntax validation using a
forced include that undefines the feature macro. This does not substitute for
Darwin execution. See [timer validation](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-platform-timer-fix-2026-09-06.json).

## Fifty-seventh hardening pass: glibc platform evidence

The original x86_64 glibc jobs completed successfully for PHP 8.4.25 and 8.5.9.
The authenticated ARM glibc PHP 8.5.9 job also passed. Each job ran 120 tests
and four 5,000-request load checks with no errors and clean exits. These jobs
use native 10c8b5d and predate the latest fixes; their overall runs contain
other failed jobs. See [glibc evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-ci-glibc-platforms-2026-09-06.json).

The latest pair, Pox 1efbd9946d8523bd314b795da1d56e9887fbc7c9 and native
28da99e2b32667845ea9aafdfa631fa0f65d3829, is running in all seven jobs of
[run 34063460630](https://github.com/shyim/pox/actions/runs/34063460630).
No corrected macOS runtime result is claimed yet.

The second Darwin Intel job (101565426228, run 34062511910) reproduced the
same empty-response failure during reusable-thread execution on native 10c8b5d,
with premature 30-second timeout messages. This strengthens the reproduction
evidence; corrected native behavior is still pending.

## Fifty-eighth hardening pass: corrected Darwin ARM validation

The corrected native 28da99e and Pox 1efbd99 pair passed all 119 applicable
tests on Darwin ARM PHP 8.5.9 in job 101568023580. The platform INI test,
shutdown-cancellation test and reusable-thread isolation test all passed.
Darwin Intel and the remaining matrix jobs are still pending. See
[corrected ARM evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-ci-timer-darwin-arm-2026-09-06.json).

## Fifty-ninth hardening pass: corrected musl validation

Both musl jobs in current run 34063460630 passed 122 tests and four 5,000-request
load checks each, with no errors and clean exits. These validate Pox 1efbd99 and
native 28da99e. See [corrected musl evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-ci-timer-musl-linux-2026-09-06.json).
The readiness summary now separates the current candidate from historical
results instead of retaining obsolete in-progress status paragraphs.

## Sixtieth hardening pass: corrected Darwin Intel and glibc ARM

Darwin Intel PHP 8.4.25 passed all 119 applicable tests on the corrected native
28da99e/Pox 1efbd99 pair, including the previously failing reusable-thread
isolation check, timer-policy regression and shutdown-cancellation test.
Glibc ARM passed 122 tests and four 5,000-request load checks with no errors
and clean exits. See [Intel evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-ci-timer-darwin-intel-2026-09-06.json)
and [glibc ARM evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-ci-timer-glibc-arm-2026-09-06.json).
The two x86_64 glibc jobs remain live; full-matrix validation is still pending.

## Sixty-first hardening pass: full corrected candidate matrix passed

Run 34063460630 completed successfully on all seven jobs at Pox 1efbd99 and
native 28da99e. The final x86_64 glibc PHP 8.4.25 and 8.5.9 jobs each passed
122 tests and four 5,000-request load checks. Across the matrix, five Linux
jobs passed 122 tests each and two Darwin jobs passed 119 applicable tests each:
848 test executions and 100,000 Linux load requests, with no load errors and
clean shutdowns. See [full matrix evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-ci-full-matrix-2026-09-06.json)
and [final glibc evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-ci-timer-glibc-x86-2026-09-06.json).
This is source-candidate validation, not release publication or deployment.

## Sixty-second hardening pass: generated framing validation

`scripts/fuzz-http-framing.py` generates a seeded corpus of invalid numeric
Content-Length values, conflicting duplicate lengths and malformed chunk sizes.
Each malformed request includes a pipelined PHP request; the harness requires
exactly one 400 response, connection termination and no PHP side effect. It
then requires a fresh healthy request and checks its exact side effect.
Both modes passed 1,024 generated cases each against the current local native
28da99e build, with clean shutdowns. This broadens the fixed corpus without
claiming exhaustive fuzzing. See [generated framing evidence](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-generated-framing-linux-2026-09-06.json).

Strict workspace/all-target Clippy with runtime-integration enabled also passed
on the current source. The final completion audit is reconciling the original
comparison table with the measured platform, load and deployment evidence.
