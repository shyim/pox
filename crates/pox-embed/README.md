# pox-embed

`pox-embed` is the safe Rust loader for independently distributed Pox PHP
runtimes. It does not compile against PHP, invoke `php-config`, or expose PHP or
Zend structures.

The loaded runtime library (`libpox_php.so` on Linux or `libpox_php.dylib` on
macOS) exports one versioned function table. This crate validates its ABI,
target, and ZTS metadata, then wraps it in owned Rust APIs for CLI execution,
HTTP requests, and long-running workers.

```rust,no_run
use pox_embed::PhpRuntime;

let php = PhpRuntime::load("/path/to/platform/runtime/library")?;
println!("PHP {}", php.version());
php.execute_code(r#"echo "Hello from PHP\n";"#, &[] as &[&str])?;
# Ok::<(), pox_embed::PhpError>(())
```

All response buffers are copied into Rust-owned values and released through
the allocating runtime. Worker integration uses callback pointers and opaque
userdata passed at startup; there are no globally imported Rust callback
symbols.

Set `POX_PHP_RUNTIME` and enable `runtime-integration` to run the real runtime
suite:

```bash
POX_PHP_RUNTIME=/path/to/platform/runtime/library \
  cargo test -p pox-embed --features runtime-integration -- --test-threads=1
```

The canonical C ABI and runtime implementation live in
[`shyim/pox-runtime`](https://github.com/shyim/pox-runtime).
