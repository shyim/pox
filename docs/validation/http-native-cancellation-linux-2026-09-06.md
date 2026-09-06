# Native cancellation validation, Linux, 2026-09-06

Runtime: local PHP 8.5.9 ZTS with per-thread Zend timers, sibling pox-runtime
worktree. Library SHA256:
`7abf1952c82323c84b12e735d9a2ca7ca09148de157b77f62e85fab4b106f611`.
FrankenPHP comparison revision: `2e3342762be51f4f635b5457899c6743ec999ef8`.

| Check | Result |
| --- | --- |
| `mise run runtime:build` | Native build, ABI smoke and buffer tests pass |
| `mise run test:runtime` | 100 tests pass: 19 CLI, 47 HTTP, 2 embed, 22 PHP integration, 3 loader, 7 runtime modes |
| Runtime-feature/all-target Clippy, warnings denied | Pass |
| Both repository patch checks | Pass |
| Cancellation regression | Eight busy-loop cancellations and subsequent healthy requests per mode; pre-cancellation and handle-reuse rejection pass |
| Memcheck with normal Zend allocator | Regression passes; zero invalid-address errors or definite/indirect leaks, with uninitialized-value reporting disabled |

The cancellation regression keeps a three-second native PHP timer as a fallback
and requires the host-triggered call to finish within two seconds. The complete
seven-test runtime-mode suite finishes in 1.69 seconds normally. Late cancellation
runs concurrently with another request after standard request cleanup or worker
replacement. Existing 47 HTTP tests still use the non-cancellable path: HTTP
integration is not proved by these results and remains required.

Focused memory command (from the Pox checkout):

```bash
POX_PHP_RUNTIME=/home/shyim/pox-runtime/build/libpox_php.so \
valgrind --fair-sched=yes --undef-value-errors=no --error-exitcode=99 \
  --leak-check=full --show-leak-kinds=definite --errors-for-leak-kinds=definite \
  --log-file=/tmp/pox-cancel-valgrind-address.log \
  target/debug/deps/runtime_modes-3213131c545dd940 \
  --exact native_cancellation_is_request_scoped_in_both_modes --test-threads=1
```

This passes in 6.49 seconds. Memcheck reports zero definite/indirect leaks,
48 bytes possibly lost and 83,634 bytes reachable. It is an address/allocation
check, not proof of no uninitialized reads or race conditions. Zend's allocator
also limits visibility into allocations within its arenas.

The first unrestricted Memcheck attempt failed the timing assertion: PHP's
fallback timer fired before the cancellation was observed. Fair scheduling
allowed the same regression to pass. The original run reported condition checks
on uninitialized padding in `zend_string_equal_val`; the checked PHP source at
`Zend/zend_string.c:442` uses word-sized assembly reads before masking the final
partial word. The address-focused run disables uninitialized-value reporting
explicitly instead of claiming those diagnostics were resolved.

An experimental `USE_ZEND_ALLOC=0` run passed the regression under fair scheduling
but reported 8,422 directly lost bytes and 28,608 indirectly lost bytes before
the configuration cleanup fix. Its leak stacks include PHP/extension shutdown
allocations and the 51-byte persistent INI string per library load. Those broader
system-allocator shutdown findings have not been resolved or revalidated; no
clean system-allocator claim is made. The INI string now frees at library unload,
and the normal-allocator run above has no definite leaks.

Local logs:

- `/tmp/pox-cancel-native-final-build.log`
- `/tmp/pox-cancel-final-tests.log`
- `/tmp/pox-cancel-final-clippy.log`
- `/tmp/pox-cancel-valgrind-address.log`
- `/tmp/pox-cancel-valgrind-address-test.log`
- `/tmp/pox-cancel-valgrind-system-alloc.log`

Remaining cancellation gates include HTTP timeout/disconnect wiring, blocking
native extension calls, shutdown callback behavior, cross-platform verification
and coordinated native runtime publication. Streaming and the other deployment/
validation gates in the review remain open.
