<p align="center">
  <img src=".github/logo.png" alt="Pox logo" width="180">
</p>

<h1 align="center">Pox</h1>

<p align="center">
  A fast PHP runtime manager, development server, and Composer-compatible
  package manager in one Rust CLI.
</p>

Pox and PHP are versioned independently. The `pox` executable contains the
server and [Riff](https://github.com/shyim/riff) package manager, while PHP is a
separately installed ZTS runtime loaded through a stable Pox ABI. Updating Pox
does not silently replace PHP, and changing PHP does not require rebuilding
Pox.

## Highlights

- **PHP version management** — Install, pin, inspect, and remove PHP 8.4/8.5
  runtimes per project or globally.
- **Stable runtime boundary** — PHP and Zend internals stay inside a
  self-contained platform library; Rust uses opaque handles and owned values.
- **Verified downloads** — Release metadata is signed with Ed25519 and every
  runtime archive and library is checked with SHA-256.
- **Development server** — Standard request mode, long-running worker mode,
  and file-watching restarts.
- **Riff package manager** — Read and write Composer projects without invoking
  PHP or Composer as subprocesses.

## Getting started

Build Pox, install a runtime, and select it globally:

```bash
git clone https://github.com/shyim/pox
cd pox
mise install
mise run build

./target/debug/pox php install 8.5
./target/debug/pox php use 8.5 --global
./target/debug/pox -v
```

Pin the exact installed patch in a project:

```bash
cd my-project
pox php use 8.5
pox install
pox server --document-root public
```

`pox php use 8.5` resolves the newest installed matching patch and writes the
exact version to `pox.toml`, keeping project selection reproducible. Normal PHP,
server, and package-manager commands never download a missing runtime; their
error tells you which explicit install command to run.

## PHP runtime management

```text
pox php install <version> [--force]  Download a signed exact or series match
pox php use <version> [--global]     Pin an installed runtime
pox php list [--remote]              List installed or available releases
pox php current                      Show version, target, ABI, and library
pox php remove <version> [--force]   Remove an installed runtime
```

Runtime selection precedence is:

1. `POX_PHP_RUNTIME`, an explicit library path for runtime development
2. `POX_PHP_VERSION`
3. The nearest `pox.toml` while walking up from the current directory
4. The global Pox configuration

Pox follows XDG paths. Runtimes live below
`$XDG_DATA_HOME/pox/runtimes`, downloads below `$XDG_CACHE_HOME/pox`, and the
global selection in `$XDG_CONFIG_HOME/pox/config.toml`.

Runtime artifacts are published by
[`shyim/pox-runtime`](https://github.com/shyim/pox-runtime) for macOS and Linux
glibc/musl on x86_64 and aarch64. A Pox binary and runtime must use the same OS
and architecture; Linux builds must also use the same libc.

## PHP CLI

Pox preserves the familiar PHP CLI forms:

```bash
pox script.php arg1 arg2
pox -r 'echo PHP_VERSION;'
pox -l script.php
pox -m
pox -i
pox -d memory_limit=512M script.php
pox -v
```

## Project configuration

Create `pox.toml` in the project root:

```toml
[php]
version = "8.5.9"

[php.ini]
memory_limit = "256M"
display_errors = "On"
error_reporting = "E_ALL"

[server]
host = "0.0.0.0"
port = 8080
document_root = "public"
router = "index.php"
# worker = "worker.php"
# workers = 4
# watch = ["**/*.php"]
```

CLI `-d` values override `pox.toml` INI settings.

## Development server

```bash
pox server
pox server --port 8080 --document-root public
pox server --document-root public public/index.php
pox server --worker worker.php --workers 4 --watch '**/*.php'
```

Worker scripts use the `pox_handle_request()` function supplied by the runtime:

```php
<?php

while (pox_handle_request(function (): void {
    echo 'Hello from a persistent PHP worker';
})) {
}
```

## Package management

Package commands are powered by Riff and consume platform facts reported by
the selected PHP runtime:

```bash
pox init
pox install
pox update
pox add vendor/package
pox remove vendor/package
pox show vendor/package
pox check-platform-reqs --lock
pox audit --locked
```

The compatibility prefix remains available, so `pox show` and `pox pm show`
are equivalent. Composer script references to `@php` and `@composer` route back
through the active Pox executable. Riff cannot execute arbitrary Composer PHP
plugins inside its Rust process; see its
[compatibility guide](https://github.com/shyim/riff/blob/main/docs/compatibility.md).

## Architecture

| Component | Responsibility |
| --- | --- |
| `pox-cli` | Runtime manager, PHP-compatible CLI, server, and Riff routing |
| `pox-embed` | Safe dynamic loader and owned CLI/web/worker Rust APIs |
| `libpox_php.so` / `libpox_php.dylib` | Versioned ABI adapter, PHP/Zend internals, and native libraries |
| `pox-runtime` | PHP 8.4/8.5 builds, signed release index, and target artifacts |

The shared library exports only `pox_php_get_api`. ABI major versions are
breaking; minor versions only append capabilities. PHP request structures,
Zend types, allocators, and SAPI state never cross into Rust.

## Developing Pox and the runtime

Pox itself builds without PHP headers or `php-config`:

```bash
mise install
mise run check
mise run test
```

For real PHP integration tests, keep `pox-runtime` next to this checkout:

```bash
git clone https://github.com/shyim/pox-runtime ../pox-runtime
mise run runtime:build
mise run test:runtime
```

The runtime helper reuses `pox-php-config` when available; otherwise the runtime
repository builds ZTS/embed PHP through static-php-cli. The Riff dependency is
pinned to an exact Git revision in `Cargo.toml` and `Cargo.lock`.

## License

Pox is MIT licensed. Downloadable runtime archives include the applicable PHP
and native dependency license notices.
