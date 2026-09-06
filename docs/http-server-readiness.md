# Current HTTP readiness evidence

Production hardening remains in progress. This summary groups local and remote
evidence; the [review ledger](http-server-review.md) preserves the comparison
with FrankenPHP and the implementation history. Historical pass counts are
snapshots, not cumulative totals or certification of later artifacts.

| Area | Latest evidence | Remaining limit |
| --- | --- | --- |
| Linux glibc PHP 8.5.9 host build | [119-test full suite](validation/http-shutdown-reload-linux-2026-09-06.json), then [targeted framing corpus](validation/http-framing-corpus-linux-2026-09-06.json) | The local native library requires glibc through 2.44; the separately built Bookworm runtime now passes [native and minimal-container checks](validation/http-glibc-release-native-linux-2026-09-06.json). See [baseline evidence](validation/http-local-glibc-baseline-linux-2026-09-06.json). |
| Linux glibc PHP 8.5.9 release pair | [120-test suite against the Bookworm library on the host](validation/http-bookworm-native-suite-linux-2026-09-06.json); [Bookworm release load](validation/http-bookworm-release-load-linux-2026-09-06.json), [native faults](validation/http-bookworm-native-fault-linux-2026-09-06.json), [15 systemd checks per mode](validation/http-bookworm-systemd-linux-2026-09-06.json) and [startup failure](validation/http-bookworm-startup-linux-2026-09-06.json) | Systemd tests use the host user manager; no publication or real host boot validation. |
| Linux glibc PHP 8.4.25 | [117-test full suite](validation/http-php84-runtime-linux-2026-09-06.json), then targeted [reload](validation/http-repeated-reload-linux-2026-09-06.json), [shutdown overlap](validation/http-shutdown-reload-linux-2026-09-06.json) and [framing](validation/http-framing-corpus-linux-2026-09-06.json) tests | Local SDK uses dynamic C++ linkage; this is not a published release validation. |
| Linux musl PHP 8.5.9 | [120-test current full suite](validation/http-musl-current-suite-linux-2026-09-06.json), [minimal Alpine execution](validation/http-musl-runtime-linux-2026-09-06.json) | Both architectures also passed [preceding-revision CI](validation/http-ci-authenticated-platforms-2026-09-06.json); the [corrected native matrix passed](validation/http-ci-full-matrix-2026-09-06.json); no publication claim. |
| Native startup failure | [glibc PHP 8.4/8.5](validation/http-startup-repeatable-linux-2026-09-06.json) and [musl PHP 8.5](validation/http-musl-startup-recovery-linux-2026-09-06.json) process failure and fresh-process recovery | MINIT failure exits PHP. Same-process recovery after a returned partial-initialization failure is unverified. |
| TLS, crash recovery and shutdown | [15 checks per mode under a glibc user systemd manager](validation/http-current-release-deployment-linux-2026-09-06.json); [11 musl checks per mode in an unprivileged read-only container](validation/http-unprivileged-container-linux-2026-09-06.json) | Host system-service boot/account setup, public ACME and multi-host deployment remain unverified. |
| Release load | 100,000 flushed requests per mode on [glibc](validation/http-current-release-load-linux-2026-09-06.json) and [musl](validation/http-musl-load-linux-2026-09-06.json), with response/resource bounds and clean shutdown | Runs shared the host; throughput is not an isolated comparison. |
| Sustained operation | [Two-hour glibc workloads passed in both modes](validation/http-two-hour-soak-linux-2026-09-06.json) | 31.33M/30.85M requests, clean exits, bounded RSS/FD growth. Snapshots predate body-outcome metrics and the Bookworm native build; worker RSS grew 32.7 MiB. |
| Framework behavior | Symfony/Twig/Doctrine/session tests on [PHP 8.5](validation/http-symfony-linux-2026-09-06.json) and [PHP 8.4](validation/http-symfony-php84-linux-2026-09-06.json) | Covers the documented demo workload, not every application or extension. |
| glibc CLI release baseline | [Bullseye-built CLI, three loader tests, minimal-container startup](validation/http-glibc-release-cli-linux-2026-09-06.json) | [Bookworm runtime and minimal-container pair](validation/http-glibc-release-native-linux-2026-09-06.json) and [nine TLS checks per mode](validation/http-glibc-release-pair-deployment-linux-2026-09-06.json) pass; [full regression and load refresh passed](validation/http-bookworm-native-suite-linux-2026-09-06.json). |
| Runtime packaging | [SDK notices preserved and archive digest checked](validation/http-runtime-license-packaging-linux-2026-09-06.json); [musl runtime archive](validation/http-musl-native-linux-2026-09-06.json) and [musl CLI archive](validation/http-musl-release-packaging-linux-2026-09-06.json) | Local artifacts only; coordinated ABI publication and remote CI are outstanding. |
| Other platforms | Release configurations include aarch64 and Darwin | Current execution evidence is missing. This Linux host has neither an aarch64 emulator nor a Darwin runner. |

The HTTP server requires the updated sibling native runtime and its ABI
capabilities. The source changes are on codex/http-server-hardening review branches in both
repositories; the native ABI changes and validated artifacts have not been released. A successful test against a local runtime does not prove that
the default downloaded runtime can run this server.

The process remains the isolation boundary for native crashes and nonreturning
native code. The tested cancellation and deadline behavior does not establish
that arbitrary PHP extensions can be interrupted safely within a thread.

## Current candidate validation

The current pair is Pox `1efbd9946d8523bd314b795da1d56e9887fbc7c9` and native
`28da99e2b32667845ea9aafdfa631fa0f65d3829`, validated by
[run 34063460630](https://github.com/shyim/pox/actions/runs/34063460630).
The rebuilt native library passed [122 local tests](validation/http-platform-timer-fix-2026-09-06.json).
Darwin ARM PHP 8.5.9 passed [119 applicable tests](validation/http-ci-timer-darwin-arm-2026-09-06.json),
including timer-policy, shutdown-cancellation and reusable-thread isolation.
Both musl architectures passed [122 tests and 20,000 load requests each](validation/http-ci-timer-musl-linux-2026-09-06.json),
with no errors and clean shutdowns. [Glibc ARM also passed 122 tests and 20,000 requests](validation/http-ci-timer-glibc-arm-2026-09-06.json).
[Darwin Intel passed all 119 applicable tests](validation/http-ci-timer-darwin-intel-2026-09-06.json),
including the previously failing reusable-thread check. Both x86_64 glibc jobs also passed [122 tests and 20,000 requests each](validation/http-ci-timer-glibc-x86-2026-09-06.json).
The [full seven-job matrix passed](validation/http-ci-full-matrix-2026-09-06.json):
848 test executions and 100,000 Linux load requests. This validates source-built
candidates; it does not publish or deploy them.

Earlier native `10c8b5d` candidates passed 120 tests and 20,000 load requests per
job on [glibc x86_64 PHP 8.4/8.5 and ARM PHP 8.5](validation/http-ci-glibc-platforms-2026-09-06.json)
and [both musl architectures](validation/http-ci-authenticated-platforms-2026-09-06.json).
These results predate the latest changes. Earlier runs also exposed dependency
download failures, two test synchronization problems, and an intermittent
Darwin Intel PHP timer failure. The [review ledger](http-server-review.md)
records their individual outcomes and fixes.

HTTP startup now disables PHP's process-wide timers on ZTS builds without Zend
per-thread timers, following FrankenPHP's policy. Pox request deadlines remain
active. Both corrected Darwin jobs passed the platform policy and reusable-thread
checks. These single-suite results do not establish absence of all intermittent
failures. The [deployment guide](http-server-deployment.md)
explains the application constraints.

## Testing an unpublished native revision in CI

The runtime-integration workflow accepts an optional `runtime_source_ref` on
manual dispatch. Supply the full lowercase 40-character pox-runtime commit SHA.
It verifies that checkout and builds PHP 8.4.25/8.5.9 from native source. The
matrix covers glibc, musl and Darwin on x86_64/aarch64, with PHP versions as
listed in the workflow. Linux jobs run buffered and flushed load checks in
addition to the full runtime suite. Set `musl_only` to run just the musl jobs.

Omitting the input retains released-runtime installation for scheduled and
release-triggered checks. Native dependency downloads use the workflow's
read-only GitHub token through BuildKit secrets or the macOS process environment.
Test logs, load reports and source/library provenance are uploaded as workflow
artifacts. Dispatching this workflow does not publish a release.

## Generated framing checks

[The seeded framing harness](../scripts/fuzz-http-framing.py) passed
[1,024 cases per mode](validation/http-generated-framing-linux-2026-09-06.json)
against the current local native build. Every malformed framing request was
rejected without executing its pipelined PHP request, followed by a successful
healthy request. This is targeted generated testing, not exhaustive protocol fuzzing.
