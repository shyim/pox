# phpx-embed

Rust bindings for embedding PHP via the PHP embed SAPI. This crate provides a
safe-ish Rust interface for running PHP code, linting scripts, querying runtime
details, and executing HTTP-style requests through a custom SAPI.

**What it does**

- Execute PHP scripts or `-r` style code from Rust.
- Expose PHP runtime/version metadata (PHP, Zend, ICU, OpenSSL, etc.).
- Provide a web/request API for running a single HTTP request.
- Provide a worker pool for long-lived PHP worker scripts.

**Requirements**

- A PHP build with the embed SAPI enabled (`--enable-embed`).
- A PHP ZTS (thread-safe) build for multi-threaded usage (web/worker mode).
- `php-config` from that same PHP build available via `PHP_CONFIG`.

You can verify the build flags with:

```bash
/path/to/php-config --configure-options
```

Look for `--enable-embed` and `--enable-zts` (or `--enable-maintainer-zts` on
older PHP versions).

**Build and link**

Dynamic linking (default):

```bash
PHP_CONFIG=/opt/php-zts/bin/php-config cargo build
```

Static linking (requires `--enable-embed=static` when building PHP):

```bash
PHP_CONFIG=/opt/php-zts-static/bin/php-config cargo build --features static
```

Or force static linking via environment variable:

```bash
PHPX_STATIC=1 PHP_CONFIG=/opt/php-zts-static/bin/php-config cargo build --release
```

**Usage**

Execute PHP code or a script:

```rust
use phpx_embed::Php;

let exit_code = Php::execute_code(r#"echo "Hello from PHP!\n";"#, &[] as &[&str])?;
let exit_code = Php::execute_script("script.php", &["arg1", "arg2"])?;
```

Run a single HTTP request:

```rust
use phpx_embed::{HttpRequest, PhpWeb};

let web = PhpWeb::new()?;
let response = web.execute(HttpRequest {
    method: "GET".to_string(),
    uri: "/index.php".to_string(),
    query_string: "".to_string(),
    headers: vec![("Host".to_string(), "example.local".to_string())],
    body: Vec::new(),
    document_root: "/var/www".to_string(),
    script_filename: "/var/www/index.php".to_string(),
    server_name: "example.local".to_string(),
    server_port: 80,
    remote_addr: "127.0.0.1".to_string(),
    remote_port: 12345,
})?;
```

**Notes**

- `PHP_CONFIG` must point to the PHP build you want to embed. Mixing headers
  and libs from different PHP builds will fail or crash.
- Web/worker modes assume a ZTS build because they use threads.
