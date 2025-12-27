# Dynamic PHP Version Selection

This document describes the architecture for runtime PHP version selection in phpx.

## Overview

phpx can support multiple PHP versions through a hybrid approach:
1. **Embedded default**: A default PHP version is compiled into the binary
2. **Dynamic loading**: Additional versions can be downloaded and loaded at runtime

## Configuration

Specify the desired PHP version in `phpx.toml`:

```toml
[php]
version = "8.3"       # Use PHP 8.3.x (latest patch)
# version = "8.3.15"  # Use exact version
# version = "^8.2"    # Use any PHP 8.2.x or higher

[php.ini]
memory_limit = "256M"
```

## Architecture

### Component Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         phpx binary                              │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  Built-in PHP (default)                                   │  │
│  │  - Always available                                       │  │
│  │  - No download needed                                     │  │
│  └───────────────────────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  Version Manager                                          │  │
│  │  - Parse phpx.toml version requirement                    │  │
│  │  - Check if version is installed                          │  │
│  │  - Download missing versions                              │  │
│  │  - Load appropriate library                               │  │
│  └───────────────────────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  Dynamic Loader                                           │  │
│  │  - dlopen() version-specific library                      │  │
│  │  - Get function table from library                        │  │
│  │  - Route calls through function pointers                  │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ dlopen()
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│  ~/.phpx/versions/                                               │
│  ├── 8.2.26-linux-gnu-x86_64/                                   │
│  │   └── libphpx.so                                             │
│  ├── 8.3.15-linux-gnu-x86_64/                                   │
│  │   └── libphpx.so                                             │
│  └── 8.4.2-darwin-arm64/                                        │
│      └── libphpx.dylib                                          │
└─────────────────────────────────────────────────────────────────┘
```

### Stable ABI Layer

Each version-specific library (`libphpx.so`) exports a stable ABI defined in `abi.h`:

```c
// Entry point exported from each library
const phpx_function_table* phpx_get_function_table(void);

// Function table contains pointers to all PHP operations
typedef struct {
    phpx_get_version_info_fn get_version_info;
    phpx_execute_script_fn execute_script;
    phpx_execute_code_fn execute_code;
    // ... more functions
} phpx_function_table;
```

This ensures binary compatibility across PHP versions - the calling convention
and data structures remain stable even as PHP internals change.

### Version Resolution

1. Read `phpx.toml` for version requirement
2. Check if requested version matches built-in version
3. If not, check `~/.phpx/versions/` for installed version
4. If not found, download from release server
5. Load the library and use it for execution

## File Locations

| Path | Purpose |
|------|---------|
| `~/.phpx/` | phpx home directory |
| `~/.phpx/versions/` | Downloaded PHP runtime libraries |
| `~/.phpx/cache/` | Download cache |

The `PHPX_HOME` environment variable can override the home directory.

## Version Library Format

Each downloadable version is a platform-specific archive:

```
phpx-8.3.15-linux-gnu-x86_64.tar.gz
├── libphpx.so          # The PHP runtime library
├── php.ini             # Default INI settings (optional)
└── metadata.json       # Version metadata
```

Supported platforms:
- `linux-gnu-x86_64` - Linux with glibc (x86_64)
- `linux-gnu-aarch64` - Linux with glibc (ARM64)
- `linux-musl-x86_64` - Alpine/musl Linux (x86_64)
- `linux-musl-aarch64` - Alpine/musl Linux (ARM64)
- `darwin-x86_64` - macOS Intel
- `darwin-arm64` - macOS Apple Silicon
- `windows-x86_64` - Windows (future)

## Building Version Libraries

To build a version-specific library:

```bash
# Configure PHP with embed SAPI
./configure --enable-embed=shared \
    --prefix=/opt/php-8.3 \
    [other options...]

# Build and install
make -j$(nproc)
make install

# Build the phpx library
cd /path/to/phpx
PHP_CONFIG=/opt/php-8.3/bin/php-config \
    cargo build --release -p phpx-embed \
    --features dynamic-loader

# The library is at target/release/libphpx.so
```

## Implementation Status

### Completed
- [x] ABI header definition (`abi.h`)
- [x] Config schema with version field
- [x] Version manager (parsing, path management)
- [x] Dynamic loader (library loading, function dispatch)
- [x] ABI export wrapper (`embed_abi.c`)

### TODO
- [ ] Integrate version manager into CLI startup
- [ ] Build and host version libraries for common platforms
- [ ] Add `phpx version` command to manage versions
- [ ] Implement download with progress bar
- [ ] Add checksum verification
- [ ] Handle version fallback (e.g., `^8.2` finding `8.2.26`)
- [ ] Test across all platforms

## Security Considerations

1. **Download verification**: Libraries are verified with SHA-256 checksums
2. **Trusted sources**: Only download from official phpx releases
3. **Sandboxing**: Downloaded libraries run with same permissions as phpx
4. **No code execution during download**: Archives are extracted, not executed

## Example Usage

```bash
# Use PHP 8.3 (will download if needed)
$ cat phpx.toml
[php]
version = "8.3"

$ phpx script.php
Downloading PHP 8.3.15 for linux-gnu-x86_64...
Downloaded PHP 8.3.15 successfully
Hello from PHP 8.3.15!

# List installed versions
$ phpx version list
Installed PHP versions:
  8.2.26 (built-in)
  8.3.15

# Use a specific version for one command
$ PHPX_PHP_VERSION=8.2 phpx script.php
Hello from PHP 8.2.26!
```

## Comparison with Other Tools

| Feature | phpx | nvm (Node) | pyenv | phpenv |
|---------|------|------------|-------|--------|
| Per-project version | ✓ (phpx.toml) | ✓ (.nvmrc) | ✓ (.python-version) | ✓ (.php-version) |
| Auto-download | ✓ | ✓ | ✓ | ✗ |
| Single binary | ✓ | ✗ | ✗ | ✗ |
| Embedded runtime | ✓ | ✗ | ✗ | ✗ |
| Version constraint | ✓ (^8.2) | ✗ | ✗ | ✗ |
