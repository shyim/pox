# Current HTTP readiness evidence

Production hardening remains in progress. This summary groups the latest local
evidence; the [review ledger](http-server-review.md) preserves the comparison
with FrankenPHP and the implementation history. Historical pass counts are
snapshots, not cumulative totals or certification of later artifacts.

| Area | Latest evidence | Remaining limit |
| --- | --- | --- |
| Linux glibc PHP 8.5.9 host build | [119-test full suite](validation/http-shutdown-reload-linux-2026-09-06.json), then [targeted framing corpus](validation/http-framing-corpus-linux-2026-09-06.json) | The local native library requires glibc through 2.44; the separately built Bookworm runtime now passes [native and minimal-container checks](validation/http-glibc-release-native-linux-2026-09-06.json). See [baseline evidence](validation/http-local-glibc-baseline-linux-2026-09-06.json). |
| Linux glibc PHP 8.5.9 release pair | [120-test suite against the Bookworm library on the host](validation/http-bookworm-native-suite-linux-2026-09-06.json); [Bookworm release load](validation/http-bookworm-release-load-linux-2026-09-06.json), [native faults](validation/http-bookworm-native-fault-linux-2026-09-06.json), [15 systemd checks per mode](validation/http-bookworm-systemd-linux-2026-09-06.json) and [startup failure](validation/http-bookworm-startup-linux-2026-09-06.json) | Systemd tests use the host user manager; no publication or real host boot validation. |
| Linux glibc PHP 8.4.25 | [117-test full suite](validation/http-php84-runtime-linux-2026-09-06.json), then targeted [reload](validation/http-repeated-reload-linux-2026-09-06.json), [shutdown overlap](validation/http-shutdown-reload-linux-2026-09-06.json) and [framing](validation/http-framing-corpus-linux-2026-09-06.json) tests | Local SDK uses dynamic C++ linkage; this is not a published release validation. |
| Linux musl PHP 8.5.9 | [120-test current full suite](validation/http-musl-current-suite-linux-2026-09-06.json), [minimal Alpine execution](validation/http-musl-runtime-linux-2026-09-06.json) | Local execution only; no remote CI or publication. |
| Native startup failure | [glibc PHP 8.4/8.5](validation/http-startup-repeatable-linux-2026-09-06.json) and [musl PHP 8.5](validation/http-musl-startup-recovery-linux-2026-09-06.json) process failure and fresh-process recovery | MINIT failure exits PHP. Same-process recovery after a returned partial-initialization failure is unverified. |
| TLS, crash recovery and shutdown | [15 checks per mode under a glibc user systemd manager](validation/http-current-release-deployment-linux-2026-09-06.json); [11 musl checks per mode in an unprivileged read-only container](validation/http-unprivileged-container-linux-2026-09-06.json) | Host system-service boot/account setup, public ACME and multi-host deployment remain unverified. |
| Release load | 100,000 flushed requests per mode on [glibc](validation/http-current-release-load-linux-2026-09-06.json) and [musl](validation/http-musl-load-linux-2026-09-06.json), with response/resource bounds and clean shutdown | Runs shared the host; throughput is not an isolated comparison. |
| Sustained operation | [Two-hour glibc workloads passed in both modes](validation/http-two-hour-soak-linux-2026-09-06.json) | 31.33M/30.85M requests, clean exits, bounded RSS/FD growth. Snapshots predate body-outcome metrics and the Bookworm native build; worker RSS grew 32.7 MiB. |
| Framework behavior | Symfony/Twig/Doctrine/session tests on [PHP 8.5](validation/http-symfony-linux-2026-09-06.json) and [PHP 8.4](validation/http-symfony-php84-linux-2026-09-06.json) | Covers the documented demo workload, not every application or extension. |
| glibc CLI release baseline | [Bullseye-built CLI, three loader tests, minimal-container startup](validation/http-glibc-release-cli-linux-2026-09-06.json) | [Bookworm runtime and minimal-container pair](validation/http-glibc-release-native-linux-2026-09-06.json) and [nine TLS checks per mode](validation/http-glibc-release-pair-deployment-linux-2026-09-06.json) pass; [full regression and load refresh passed](validation/http-bookworm-native-suite-linux-2026-09-06.json). |
| Runtime packaging | [SDK notices preserved and archive digest checked](validation/http-runtime-license-packaging-linux-2026-09-06.json); [musl runtime archive](validation/http-musl-native-linux-2026-09-06.json) and [musl CLI archive](validation/http-musl-release-packaging-linux-2026-09-06.json) | Local artifacts only; coordinated ABI publication and remote CI are outstanding. |
| Other platforms | Release configurations include aarch64 and Darwin | Current execution evidence is missing. This Linux host has neither an aarch64 emulator nor a Darwin runner. |

The HTTP server requires the updated sibling native runtime and its ABI
capabilities. The checked-in sources and locally validated artifacts have not
been published. A successful test against a local runtime does not prove that
the default downloaded runtime can run this server.

The process remains the isolation boundary for native crashes and nonreturning
native code. The tested cancellation and deadline behavior does not establish
that arbitrary PHP extensions can be interrupted safely within a thread.

## Testing an unpublished native revision in CI

The runtime-integration workflow accepts an optional runtime_source_ref on manual
dispatch. Supply the full lowercase 40-character pox-runtime commit SHA. It
verifies that checkout, builds PHP 8.4.25/8.5.9 from the native source, then runs
the HTTP-enabled suite. Linux builds use the Bookworm native Dockerfile and also
run buffered/flushed load checks. The matrix includes Linux x86_64/aarch64 and
Darwin x86_64/aarch64; it does not add musl CI execution.

Omitting the input retains released-runtime installation for scheduled and
release-triggered checks. Test logs, load reports and source/library provenance
are uploaded as workflow artifacts. The revised workflow has passed static
validation locally but has not executed remotely; the relevant source commits
and workflow revision must be available on GitHub before it can run.
