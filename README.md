# Pox

## This is TOTALLY WIP and experimental

Pox (temporary name) is an idea to build a all-in-one single binary PHP distribution, which contains all PHP extensions, a Webserver, Package Manager, Formatter/Linter for best development experience working with PHP.

Ideas:

- Production Ready Webserver
- Package Manager that is equal to Composer
- Formatter/Linter that uses Mago
- A builtin test runner?

## Current Status

### General

- [X] A `pox.toml` file to configure PHP settings like `memory_limit` or other things

### Web Server

- [X] Regular Webserver
- [X] Worker Mode similar to FrankenPHP
- [_] Production Ready

### Package Manager

- [X] Install Packages
- [X] Update Packages
- [X] Remove Packages
- [X] Audit Packages
- [_] Composer Plugins

### Formatter/Linter

- [_] Formatter/Linter that uses Mago

## The CLI

```
pox 0.1.0 - PHP 8.5.1 embedded in Rust

Usage: pox [options] [-f] <file> [--] [args...]
       pox [options] -r <code> [--] [args...]
       pox server [options] [router.php]

Options:
  -d key[=value]  Define INI entry
  -i              PHP information (phpinfo)
  -l              Syntax check only (lint)
  -m              Show compiled in modules
  -r <code>       Run PHP <code> without script tags
  -v              Version information
  -h, --help      Show this help message

Subcommands:
  init            Create a new composer.json in current directory
  install         Install project dependencies from composer.lock
  update          Update dependencies to their latest versions
  add             Add a package to the project
  remove          Remove a package from the project
  run             Run a script defined in composer.json
  server          Start a PHP development server
  pm              Other package manager commands (show, validate, etc.)

Run 'pox --help' for more options.
```

## Configuration (pox.toml)

Pox supports a local `pox.toml` configuration file in your project directory. This file allows you to configure PHP runtime settings and development server options.

### Example Configuration

```toml
# PHP runtime configuration
[php.ini]
memory_limit = "256M"
max_execution_time = "30"
display_errors = "On"
error_reporting = "E_ALL"

# Development server configuration
[server]
host = "0.0.0.0"
port = 8080
document_root = "public"
router = "index.php"
# worker = "worker.php"  # Optional: Enable worker mode
# workers = 4            # Number of worker threads
# watch = ["**/*.php"]   # File patterns to watch for auto-reload
```

### Configuration Options

#### `[php.ini]`

Any PHP INI setting can be specified here as key-value pairs. These settings are applied when PHP is initialized. CLI arguments (`-d`) take precedence over config file settings.

#### `[server]`

| Option | Type | Description |
|--------|------|-------------|
| `host` | string | Address to bind to (default: "127.0.0.1") |
| `port` | number | Port to listen on (default: 8000) |
| `document_root` | string | Document root directory (default: ".") |
| `router` | string | Router script path (optional) |
| `worker` | string | Worker script for long-running mode (optional) |
| `workers` | number | Number of worker threads (default: CPU cores) |
| `watch` | array | Glob patterns for file watching (optional) |

### Configuration Priority

Settings are merged with the following priority (highest to lowest):

1. CLI arguments (e.g., `-d memory_limit=512M`)
2. `pox.toml` configuration file
3. Built-in defaults

### PM Commands

```
❯ pox pm
Package manager commands (show, validate, dump-autoload)

Usage: pox pm <COMMAND>

Commands:
  audit          Check for security vulnerabilities in installed packages
  bump           Bump version constraints in composer.json to locked versions
  exec           Execute a vendored binary/script
  search         Search for packages on Packagist
  show           Show information about packages
  validate       Validate composer.json and composer.lock
  dump-autoload  Regenerate the autoloader
  why            Show why a package is installed
  outdated       Show outdated packages
  clear-cache    Clear the Composer cache
  help           Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
```
