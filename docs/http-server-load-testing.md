# HTTP load verification

`scripts/stress-http-server.py` runs real standard and persistent-worker PHP
requests against a temporary loopback server. It needs Linux `/proc` and Python
3.11 or later. Build the native runtime and CLI first; keep those binaries
unchanged while the workload runs:

```sh
mise run runtime:build
mise x rust -- cargo build -p pox-cli --release --locked
python3 scripts/stress-http-server.py --requests 100000 --clients 32 --workers 8 --timeout 180
```

Each request has a unique URI, cookie and body hash, and the harness checks every
response. Standard PHP state must start fresh; worker counters must respect the
recycling budget. There are no request retries. Non-200 responses, mismatched
output, client errors, deadline expiry or unclean shutdown fail the run.

JSON output records latency percentiles, throughput, worker replacements, RSS,
file descriptors and shutdown results, plus binary/runtime SHA-256 digests and
host details. The default resource gate permits at most 64 MiB of RSS growth
from the warmed baseline and bounds final descriptors after clients close.
`--max-rss-growth-mib` makes the memory gate explicit for other workloads.
`--recycle 200` stresses frequent replacement; zero disables automatic recycling.
The workload deadline bounds the run, with up to 12 additional seconds for an
already-running client operation.

This is a correctness/resource workload, not a maximum-throughput benchmark.
Python's client, loopback networking and other machine activity affect rates.
It does not establish behavior for real application dependencies, blocking native
extensions, slow external clients, TLS proxies, other operating systems or
multi-hour operation. Those remain separate deployment validation requirements.

## Local evidence, 2026-09-06

The [release workload report](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-load-linux-2026-09-06.json) includes
exact binary and runtime digests. Both modes used 32 clients, eight PHP threads,
4 KiB request bodies and the default 1000-request recycling budget.

| Mode | Result | Elapsed | RSS growth | Shutdown |
| --- | --- | --- | --- | --- |
| Standard | 66,535 verified 200 responses; driver deadline prevented completing 100,000 | 180.1 s | 17.0 MiB | clean, exit 0 |
| Worker | 100,000 verified 200 responses; 96 replacements; no errors | 37.5 s | 11.0 MiB | clean, exit 0 |

This baseline standard run failed the workload gate despite having no response mismatches
or HTTP failures. Its final descriptor count and RSS growth met the resource
bounds; the profile below identified the cost subsequently addressed by thread reuse. A smaller debug
baseline also exposed 129 HTTP 503 responses in 2000 worker requests during normal
recycling. After bounded queue waiting was implemented, that same workload
completed with 2000 successful responses and eight replacements, without retries.

A [Callgrind sample](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-standard-callgrind-2026-09-06.txt) covering
200 requests plus warmup attributes about 65% of recorded instructions to TSRM
resource allocation and 18% to TSRM cleanup. These are inclusive instruction
counts across the process, not wall-time percentages. Standard requests currently
recreate PHP thread resources on every call. Reusing them for each dispatch
thread, with explicit lifetime cleanup and full PHP request shutdown retained,
was the measured optimization implemented and validated below.


## After reusable standard-thread resources

The [new release report](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-load-linux-reused-threads-2026-09-06.json)
uses the same 100,000-request command, 32 clients, eight PHP threads, 4 KiB bodies
and 180-second deadline. Both modes passed without retries or HTTP/data errors:

| Mode | Verified responses | Elapsed | RSS growth | Replacements | Shutdown |
| --- | --- | --- | --- | --- | --- |
| Standard | 100,000 HTTP 200 | 35.6 s | 2.2 MiB | none | clean, exit 0 |
| Worker | 100,000 HTTP 200 | 31.8 s | 11.1 MiB | 96 | clean, exit 0 |

Standard mode now keeps TSRM resources for each dispatch thread while retaining
full PHP request startup/shutdown. The repeated [Callgrind sample](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-standard-callgrind-reused-threads-2026-09-06.txt)
shows about 3.4% of process instructions in thread resource allocation and 1.0%
in cleanup, down from approximately 65% and 18%. These small profiles include
startup/warmup and describe instruction counts, not wall-time attribution.

This closes the local 100,000-request resource/correctness gate for both modes.
It does not close the longer-duration, deployed-proxy, application-dependency or
cross-platform validation gates described above.


## Flushed-response workload

Add `--flush` to call PHP flush on every response. The harness then requires
HTTP/1.1 chunked framing in addition to validating the complete JSON response,
request identity/body hash, resource bounds, worker recycling and clean shutdown.
This exercises streaming startup/completion races even for small responses.

```bash
python3 scripts/stress-http-server.py --binary target/debug/pox \
  --runtime /absolute/path/to/libpox_php.so --requests 10000 \
  --clients 32 --workers 8 --body-bytes 4096 --flush
```

The current local result is recorded in
[the streaming workload artifact](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-load-linux-streaming-2026-09-06.json).
It is a debug-build correctness/resource workload, not a release performance
comparison or a multi-hour soak.


## Sustained workloads

`--duration-seconds` runs each selected mode for the full interval instead of
stopping at `--requests`. Set `--timeout` at least 30 seconds beyond that duration.
For example, a two-hour flushed workload for one mode is:

```bash
python3 scripts/stress-http-server.py --binary target/release/pox \
  --runtime /absolute/path/to/libpox_php.so --mode standard \
  --duration-seconds 7200 --timeout 7300 --clients 32 --workers 8 \
  --body-bytes 4096 --recycle 1000 --flush \
  > standard-result.jsonl 2> standard-progress.jsonl
```

Repeat with `--mode worker`; `--mode both` runs the full duration sequentially
for each mode. Progress on stderr reports completed requests, failures, RSS,
descriptors and worker replacements approximately once per minute. The final
stdout JSON includes resource history and executable/runtime hashes. Progress
alone is not a pass: completion requires the full duration, only correct 200
responses, resource bounds and successful shutdown. Runs do not retry failed
requests. Fixed-count workloads still fail if their driver deadline expires.

The harness retains a bounded log tail and counts replacements while consuming
server logs continuously; it does not accumulate a multi-hour access-log file.
Latency quantiles now use a bounded logarithmic histogram with at most 1% bucket
width (1-microsecond floor and overflow above 1000 seconds). They are approximate
upper bucket bounds, unlike the older reports' exact stored-sample quantiles.
Resource peaks are sampled every 100 ms, and up to 1440 minute-scale history
points are retained. This keeps harness storage independent of request count.
Concurrent mode runs share host resources and cannot be used as isolated
throughput comparisons.


## Current release and recycling-memory check

The [current release workload](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-current-release-load-linux-2026-09-06.json)
includes body outcome metrics and premature-EOF accounting. Both modes passed
100,000 flushed requests with 32 clients/eight PHP threads and clean shutdown.
The [matching deployment suite](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-current-release-deployment-linux-2026-09-06.json)
passes all 15 checks per mode. These short runs do not replace the ongoing
multi-hour snapshot workloads or establish isolated throughput improvements.

A separate [memory comparison](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-worker-recycle-memory-linux-2026-09-06.json)
used 30,000 worker requests, 16 clients and four workers on the earlier snapshot.
Disabling recycling produced 1.2 MiB final RSS growth; recycling every 10 requests
produced 3000 replacements and 12.6 MiB growth. Both runs passed response/resource
checks and exited zero. This associates higher retained RSS with replacement but
does not establish that the growth is unbounded.

A smaller Massif run used 1000 requests and 100 replacements, with Zend's allocator
disabled for allocation visibility. Its early high live-heap sample was 7.50 MB
(decimal), versus a later peak of 7.57 MB. Heap usage fluctuated with active PHP
threads. Instrumented RSS is not comparable to production RSS. This small profile
does not prove or exclude a long-term leak, so no speculative runtime change was
made. The [compressed raw profile](https://github.com/shyim/pox/blob/2c777c4f0fadb8da15f16498dcc3331945a6a6fb/docs/validation/http-worker-recycle-massif-linux-2026-09-06.out.gz)
and parsed samples are retained for follow-up if the sustained trend warrants it.
