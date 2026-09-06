# HTTP server completion audit

The server implementation and source-candidate CI are validated. Overall completion
is not claimed while the deployment checks in the original review remain open.

The application/native sources in the working directories match tested Pox
`1efbd9946d8523bd314b795da1d56e9887fbc7c9` and native
`28da99e2b32667845ea9aafdfa631fa0f65d3829`. Subsequent changes add documentation
and the generated framing harness. FrankenPHP's reference checkout still points
to `2e3342762be51f4f635b5457899c6743ec999ef8`.

| Original review area | Inspected evidence | Result and limits |
| --- | --- | --- |
| Reference comparison | Cloned revision; comparison table in [review](http-server-review.md) | Completed comparison across all eight areas. |
| File routing | `http_server.rs` confinement, alias/source protection, PATH_INFO and static-semantics tests; [matrix](validation/http-ci-full-matrix-2026-09-06.json) | Passed in both modes across supported candidates. The document root must not be writable by untrusted users; filesystem TOCTOU is not eliminated. |
| HTTP transport | Real-socket limits/framing tests; [generated framing](validation/http-generated-framing-linux-2026-09-06.json); [matrix loads](validation/http-ci-full-matrix-2026-09-06.json) | Passed bounded parser/body/admission/write/deadline behavior and generated malformed framing. HTTP/1.0 and HTTP/1.1 backend; TLS and HTTP/2 belong to the reverse proxy. Fuzzing is targeted, not exhaustive. |
| PHP concurrency | Barrier and ownership tests, reused-thread state isolation; [matrix](validation/http-ci-full-matrix-2026-09-06.json) | Passed standard/worker parallel execution and state isolation, including corrected Darwin timer policy. |
| Native lifecycle | Runtime-mode tests, native allocation tests, [startup failure](validation/http-bookworm-startup-linux-2026-09-06.json), [native faults](validation/http-bookworm-native-fault-linux-2026-09-06.json) | ABI bounds and ordinary lifecycle/recovery pass. MINIT failure is validated as process failure with fresh-process recovery, not safe retry after partial initialization in the same process. Arbitrary native faults require process supervision. |
| Graceful shutdown | Active drain, forced deadline, repeated reload and reload/shutdown overlap tests; [matrix](validation/http-ci-full-matrix-2026-09-06.json) | Passed, including callbacks and blocked native I/O. Deadline expiry terminates the process rather than unloading live PHP. |
| Worker recovery | Exit/fatal/backoff/recycle tests; [systemd fault/restart checks](validation/http-bookworm-systemd-linux-2026-09-06.json) | Passed no-replay failure handling, replacement, crash-loop limiting and operator reset. Native crashes are process-wide. |
| Request/response semantics | Protocol/auth/proxy/session/upload/bodyless/streaming/late-error tests; [matrix](validation/http-ci-full-matrix-2026-09-06.json); [Symfony](validation/http-symfony-linux-2026-09-06.json) | Passed measured combinations. Application and extension compatibility still needs workload-specific validation. |
| Operational deployment | [TLS/container faults](validation/http-bookworm-native-fault-linux-2026-09-06.json), [user-systemd supervision](validation/http-bookworm-systemd-linux-2026-09-06.json) | Local verified TLS, HTTP/2 frontend, private admin, restart and drain passed. Actual deployment account, host boot, public-domain ACME and multi-host network configuration have not been tested. |
| Sustained operation | [Two-hour workloads](validation/http-two-hour-soak-linux-2026-09-06.json) | Both modes passed over 30 million requests with bounded RSS/FD growth and clean exits. These snapshots predate later changes; worker RSS increased 32.7 MiB. This is not proof of indefinitely flat memory. |
| Build and delivery | Seven-job CI; [packaging notices](validation/http-runtime-license-packaging-linux-2026-09-06.json); strict current Clippy and patch checks | Current source candidates pass 848 test executions and 100,000 Linux load requests. Packaging evidence refers to recorded earlier artifacts. No release, tag, merge or deployment has been performed. |

## Remaining target-dependent work

The original operational checklist includes actual host service startup and public
TLS validation. Closing those checks requires a deployment host, domain, application
and access details. Multi-host validation additionally requires the intended proxy
and backend topology. None can be inferred from the local source checkout.

The checked-in [deployment templates](http-server-deployment.md) and harnesses are
ready for that validation. The user has been asked to identify a target or clarify
whether the intended deliverable ends at server code and deployment examples.
Until that scope is resolved, the goal remains active; successful CI alone is not
being used to silently close the deployment gates.
