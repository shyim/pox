# Native output callback validation, Linux, 2026-09-06

Local PHP 8.5.9 ZTS runtime SHA256:
`b78311e530747b55f703ba76f4b314c7e847396401649e9328a9aa9735a9d031`.
FrankenPHP reference: `2e3342762be51f4f635b5457899c6743ec999ef8`, especially
`frankenphp_ub_write`, `frankenphp_send_headers` and `frankenphp_sapi_flush`.

| Check | Result |
| --- | --- |
| Native build, ABI smoke, response-buffer tests | Pass |
| Workspace/runtime-feature tests | 108 pass: 19 CLI, 50 HTTP, 2 embed, 22 PHP integration, 3 loader, 12 runtime modes |
| All-target/runtime-feature Clippy with denied warnings | Pass |
| Patch checks in both repositories | Pass |
| Focused Memcheck address/allocation check | Five output tests pass, zero invalid-address errors and zero definite/indirect leaks |

The native output tests exercise synchronous zero-capacity backpressure, headers
and body delivery before PHP proceeds beyond flush, bounded 16-KiB chunks, an
empty final buffered body, sink rejection, zero-byte output limits, subsequent
healthy execution, fatal errors after early output, header rejection during
flush, and worker connection-status reset when ignore_user_abort keeps a rejected
request's incarnation alive. Both modes are covered where applicable. A separate
HTTP test verifies short raw status lines and an ordinary explicit 404.

The CLI currently passes no output sink. These results prove the embedding
mechanism and buffered HTTP compatibility, not streaming Hyper body framing,
network backpressure, or stream cancellation. Those integrations remain required.

Focused memory command:

```bash
POX_PHP_RUNTIME=/home/shyim/pox-runtime/build/libpox_php.so \
valgrind --fair-sched=yes --undef-value-errors=no --error-exitcode=99 \
  --leak-check=full --show-leak-kinds=definite --errors-for-leak-kinds=definite \
  --log-file=/tmp/pox-stream-memcheck.log \
  target/debug/deps/runtime_modes-3213131c545dd940 output --test-threads=1
```

Five tests pass in 17.71 seconds. Memcheck reports zero definite/indirect leaks,
48 bytes possibly lost and 83,634 bytes reachable. Zend's normal allocator is
enabled and uninitialized-value reporting is disabled because of the assembly
string-comparison diagnostics described in the cancellation report. This is an
address/allocation check, not proof of race freedom or no uninitialized reads.

Logs: `/tmp/pox-stream-native-final-build.log`, `/tmp/pox-stream-final-tests.log`,
`/tmp/pox-stream-final-clippy.log`, `/tmp/pox-stream-memcheck.log`, and
`/tmp/pox-stream-memcheck-test.log`. No release/performance or cross-platform claim
is made from these runs.
