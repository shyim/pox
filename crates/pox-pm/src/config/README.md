# Composer Configuration System

This module implements a complete configuration system for Composer, matching the behavior of the PHP implementation.

## Overview

The configuration system supports multiple sources with the following priority order (highest to lowest):

1. **Environment Variables** (`COMPOSER_*`)
2. **Project Configuration** (`composer.json` config section)
3. **Global Configuration** (`~/.composer/config.json` or `COMPOSER_HOME/config.json`)
4. **Built-in Defaults**

## Architecture

### Modules

- **`config.rs`** - Main `Config` struct with all configuration options and merging logic
- **`source.rs`** - Configuration sources, loaders, and environment variable handling
- **`mod.rs`** - Module exports and documentation

### Key Types

#### `Config`

The main configuration structure containing all Composer settings:

```rust
pub struct Config {
    // Directories
    pub vendor_dir: PathBuf,
    pub bin_dir: PathBuf,
    pub cache_dir: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub home_dir: Option<PathBuf>,

    // Behavior
    pub process_timeout: u64,
    pub preferred_install: PreferredInstall,
    pub store_auths: StoreAuths,
    pub optimize_autoloader: bool,
    pub lock: bool,
    pub platform_check: PlatformCheck,

    // Network & Security
    pub secure_http: bool,
    pub github_protocols: Vec<String>,
    pub github_domains: Vec<String>,
    pub cafile: Option<PathBuf>,
    pub capath: Option<PathBuf>,

    // Authentication
    pub http_basic: HashMap<String, HttpBasicAuth>,
    pub github_oauth: HashMap<String, String>,
    pub gitlab_oauth: HashMap<String, String>,
    pub bearer: HashMap<String, String>,

    // Platform overrides
    pub platform: HashMap<String, String>,

    // ... and many more
}
```

#### `ConfigSource`

Tracks where each configuration value comes from:

```rust
pub enum ConfigSource {
    Default,           // Built-in default
    Global,            // ~/.composer/config.json
    Project,           // composer.json
    Environment(String), // COMPOSER_* env var
    Command,           // Set programmatically
    Unknown,
}
```

#### Configuration Enums

```rust
pub enum PreferredInstall {
    Auto,    // Auto-detect based on package
    Source,  // Install from VCS source
    Dist,    // Install from distribution archive
}

pub enum StoreAuths {
    True,    // Always store credentials
    False,   // Never store credentials
    Prompt,  // Ask user each time
}

pub enum DiscardChanges {
    True,    // Discard local changes
    False,   // Keep local changes (fail on conflict)
    Stash,   // Stash local changes
}

pub enum PlatformCheck {
    PhpOnly,  // Only check PHP version
    True,     // Check all platform requirements
    False,    // Skip platform checks
}
```

## Usage

### Basic Usage

```rust
use pox_pm::config::Config;

// Create with defaults
let config = Config::default();

// Create with base directory
let config = Config::with_base_dir("/path/to/project");
```

### Loading Configuration

```rust
use pox_pm::config::Config;
use std::path::Path;

// Build configuration from all sources
let config = Config::build(
    Some(Path::new("/path/to/project")),
    true  // use environment variables
)?;

// Access values
println!("Vendor dir: {:?}", config.get_vendor_dir());
println!("Timeout: {}", config.process_timeout);
println!("Secure HTTP: {}", config.secure_http);

// Check where a value came from
if let Some(source) = config.get_source("vendor-dir") {
    println!("vendor-dir from: {}", source.as_str());
}
```

### Configuration Loader

```rust
use pox_pm::config::ConfigLoader;

let loader = ConfigLoader::new(true);

// Get directories
let home = loader.get_composer_home();
let cache = loader.get_cache_dir();

// Load global config
let global_config = loader.load_global_config()?;

// Load project config
let project_config = loader.load_project_config("/path/to/project")?;

// Get environment variables
if let Some(timeout) = loader.get_env_u64("process-timeout") {
    println!("COMPOSER_PROCESS_TIMEOUT: {}", timeout);
}
```

### Environment Variables

The system supports all standard Composer environment variables:

- `COMPOSER_HOME` - Composer home directory
- `COMPOSER_CACHE_DIR` - Cache directory
- `COMPOSER_VENDOR_DIR` - Vendor directory
- `COMPOSER_BIN_DIR` - Binary directory
- `COMPOSER_PROCESS_TIMEOUT` - Process timeout in seconds
- `COMPOSER_DISCARD_CHANGES` - How to handle uncommitted changes
- `COMPOSER_CACHE_READ_ONLY` - Set cache to read-only mode
- `COMPOSER_HTACCESS_PROTECT` - Enable .htaccess protection
- And many more...

## Configuration Files

### Global Configuration

Location: `~/.composer/config.json` (or `$COMPOSER_HOME/config.json`)

Example:
```json
{
  "config": {
    "process-timeout": 300,
    "preferred-install": "dist",
    "github-oauth": {
      "github.com": "ghp_xxxxxxxxxxxx"
    },
    "platform": {
      "php": "8.2.0"
    }
  }
}
```

### Project Configuration

Location: `composer.json` (in project root)

Example:
```json
{
  "name": "my/project",
  "require": {
    "php": "^8.1"
  },
  "config": {
    "vendor-dir": "lib/vendor",
    "optimize-autoloader": true,
    "sort-packages": true,
    "platform": {
      "php": "8.1.0"
    }
  }
}
```

## Default Values

The configuration system provides sensible defaults matching Composer's behavior:

| Setting | Default | Description |
|---------|---------|-------------|
| `vendor-dir` | `vendor` | Where to install packages |
| `bin-dir` | `{$vendor-dir}/bin` | Where to install binaries |
| `cache-dir` | Platform-specific | Cache directory |
| `process-timeout` | `300` | Timeout in seconds |
| `preferred-install` | `dist` | Installation method |
| `store-auths` | `prompt` | Authentication storage |
| `secure-http` | `true` | Require HTTPS |
| `lock` | `true` | Create composer.lock |
| `platform-check` | `php-only` | Platform requirement checks |
| `cache-ttl` | `15552000` | Cache TTL (6 months) |
| `cache-files-maxsize` | `300MiB` | Max cache file size |
| `github-protocols` | `["https", "ssh", "git"]` | Allowed Git protocols |
| `github-domains` | `["github.com"]` | GitHub domains |
| `gitlab-domains` | `["gitlab.com"]` | GitLab domains |

## Path Resolution

Paths can be:
- **Absolute**: Used as-is
- **Relative**: Resolved relative to the project's base directory
- **With placeholders**: `{$vendor-dir}`, `{$home}`, `{$cache-dir}`, etc.

Example:
```rust
let mut config = Config::with_base_dir("/project");
config.vendor_dir = PathBuf::from("lib/vendor");
config.bin_dir = PathBuf::from("{$vendor-dir}/bin");

// After resolution:
// vendor_dir = /project/lib/vendor
// bin_dir = /project/lib/vendor/bin
```

## Authentication

The configuration system supports multiple authentication methods:

### HTTP Basic Authentication
```rust
config.http_basic.insert(
    "example.com".to_string(),
    HttpBasicAuth {
        username: "user".to_string(),
        password: "pass".to_string(),
    }
);
```

### OAuth Tokens
```rust
config.github_oauth.insert("github.com".to_string(), "ghp_xxx".to_string());
config.gitlab_oauth.insert("gitlab.com".to_string(), "glpat-xxx".to_string());
```

### Bearer Tokens
```rust
config.bearer.insert("api.example.com".to_string(), "token".to_string());
```

## Platform Overrides

Platform requirements can be overridden to simulate different environments:

```rust
config.platform.insert("php".to_string(), "8.2.0".to_string());
config.platform.insert("ext-mbstring".to_string(), "*".to_string());
config.platform.insert("ext-pdo".to_string(), "8.2.0".to_string());
```

## Testing

The configuration system includes comprehensive tests:

```bash
# Run all config tests
cargo test -p pox-pm --lib config

# Run specific test module
cargo test -p pox-pm config::config::tests
cargo test -p pox-pm config::source::tests
```

## Examples

See `examples/config_demo.rs` for a complete demonstration:

```bash
cargo run --example config_demo
```

## Implementation Notes

### Differences from PHP Composer

While this implementation closely follows Composer's behavior, there are some differences:

1. **Type Safety**: Rust's type system provides compile-time guarantees about configuration values
2. **No Runtime Type Coercion**: Values must be the correct type in JSON (no string-to-bool conversion)
3. **Path Handling**: Uses Rust's `PathBuf` for cross-platform path handling
4. **Error Handling**: Uses `Result` types instead of exceptions

### Future Enhancements

Potential improvements:

- [ ] Support for `auth.json` separate authentication file
- [ ] Config validation and warnings for deprecated options
- [ ] Config migration from older Composer versions
- [ ] Better placeholder expansion (recursive references)
- [ ] Support for per-repository configuration
- [ ] Config schema validation

## References

- [Composer Configuration Documentation](https://getcomposer.org/doc/06-config.md)
- [Composer Schema](https://getcomposer.org/doc/04-schema.md#config)
- PHP Implementation: `composer/src/Composer/Config.php`
