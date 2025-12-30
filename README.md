<p align="center">
  <img src=".github/logo.png" alt="PHPox Logo" width="200">
</p>

<h1 align="center">PHPox</h1>

<p align="center">
  <strong>An all-in-one PHP distribution for modern development</strong>
</p>

<p align="center">
  A single binary that includes PHP runtime, web server, package manager, and all common extensions.<br>
  No more juggling multiple tools or fighting with PHP installations.
</p>

---

## Features

- **Single Binary** — One download, everything included. No dependencies, no setup.
- **Integrated Web Server** — Development server with worker mode (like FrankenPHP) and file watching.
- **Composer-Compatible Package Manager** — Fast, native package management that works with existing `composer.json` files.
- **All Extensions Included** — Common extensions pre-compiled and ready to use.
- **Project Configuration** — Simple `pox.toml` file for PHP settings and server config.

## Quick Start

```bash
# Run a PHP script
pox script.php

# Start development server
pox server

# Install dependencies (reads composer.json)
pox install

# Add a package
pox add vendor/package
```

## Installation

### Download Binary

[Each pipeline run includes as artifact a prebuilt binary for your platform.]()

### Build from Source

```bash
git clone https://github.com/shyim/pox
cd pox
cargo build --release
```

Requires a PHP installation compiled with embed SAPI (`--enable-embed`). Set `PHP_CONFIG` environment variable to point to your PHP installation.

## CLI Reference

```
pox 0.1.0 - PHP embedded in Rust

Usage: pox [options] <file> [args...]
       pox [options] -r <code> [args...]
       pox <command> [options]

Options:
  -d key[=value]  Define INI entry
  -i              Show PHP info
  -l              Syntax check (lint)
  -m              Show compiled modules
  -r <code>       Run PHP code
  -v              Version info
  -h, --help      Show help

Commands:
  server          Start development server
  init            Create new composer.json
  install, i      Install dependencies
  update          Update dependencies
  add, require    Add a package
  remove, rm      Remove a package
  run             Run composer script
  pm              Package manager commands
```

### Package Manager Commands

```bash
pox pm show              # Show package info
pox pm search <query>    # Search Packagist
pox pm outdated          # List outdated packages
pox pm audit             # Security vulnerability check
pox pm why <package>     # Show why package is installed
pox pm dump-autoload     # Regenerate autoloader
pox pm exec <binary>     # Run vendored binary
pox pm clear-cache       # Clear package cache
```

## Configuration

Create a `pox.toml` in your project root:

```toml
# PHP runtime settings
[php.ini]
memory_limit = "256M"
display_errors = "On"
error_reporting = "E_ALL"

# Development server
[server]
host = "0.0.0.0"
port = 8080
document_root = "public"
router = "index.php"

# Worker mode (optional)
# worker = "worker.php"
# workers = 4
# watch = ["**/*.php"]
```

### Configuration Priority

1. CLI arguments (`-d memory_limit=512M`)
2. `pox.toml` settings
3. Built-in defaults

## Web Server

### Standard Mode

```bash
pox server
pox server --port 8080 --document-root public
pox server public/index.php  # With router script
```

### Worker Mode

Long-running PHP processes for better performance (similar to FrankenPHP):

```bash
pox server --worker worker.php --workers 4
```

### File Watching

Auto-restart workers when files change:

```bash
pox server --worker worker.php --watch "**/*.php"
```

## Package Manager

PHPox includes a Composer-compatible package manager written in Rust. It reads and writes standard `composer.json` and `composer.lock` files.

```bash
# Initialize new project
pox init

# Install from lock file
pox install

# Update dependencies
pox update

# Add packages
pox add laravel/framework
pox add --dev phpunit/phpunit

# Remove packages
pox remove vendor/package
```

### Supported Features

- Full dependency resolution (SAT solver)
- PSR-0, PSR-4, classmap, and files autoloading
- Private repositories and authentication
- Platform requirements checking
- Lock file compatibility with Composer

## Architecture

PHPox is built as a Rust workspace with these crates:

| Crate | Description |
|-------|-------------|
| `pox-cli` | Main CLI binary, web server, command handling |
| `pox-embed` | FFI bindings to PHP's embed SAPI |
| `pox-pm` | Package manager (solver, repositories, autoload) |
| `pox-semver` | Semantic versioning for Composer constraints |
| `pox-spdx` | SPDX license identifier validation |

## Contributing

Contributions welcome! See the [Feature Roadmap](crates/pox-pm/FEATURE_GAPS_ISSUES.md) for planned features.

```bash
# Build
cargo build

# Run tests
cargo test

# Run specific crate tests
cargo test -p pox-pm
```

## License

MIT
