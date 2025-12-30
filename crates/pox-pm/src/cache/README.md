# Composer Cache System

This module provides a filesystem-based caching system for Composer package management, based on the PHP Composer implementation.

## Features

- **Thread-safe operations**: All cache operations are safe for concurrent use
- **Atomic writes**: Uses temporary files and atomic rename to ensure data consistency
- **Key sanitization**: Automatically sanitizes keys to be filesystem-safe
- **TTL-based garbage collection**: Removes old cache entries based on age
- **Multiple cache types**: Supports different cache directories (files, repo, vcs)
- **SHA256 hashing**: Built-in file hashing support
- **Read-only mode**: Support for read-only cache access

## Usage

### Basic Example

```rust
use pox_pm::cache::Cache;
use std::path::PathBuf;

// Create a cache instance
let cache = Cache::new(PathBuf::from("/tmp/composer-cache"));

// Write data
cache.write("repo/packages.json", b"package data")?;

// Read data
if let Some(data) = cache.read("repo/packages.json")? {
    println!("Read {} bytes", data.len());
}

// Check if exists
if cache.has("repo/packages.json") {
    println!("Cache hit!");
}
```

### Cache Directories

The cache supports three main directory types:

1. **files/** - Downloaded package archives (zip, tar.gz)
   ```rust
   cache.write("files/symfony-console-5.4.0.zip", zip_data)?;
   ```

2. **repo/** - Repository metadata (packages.json, provider files)
   ```rust
   cache.write("repo/packagist.org/p2/symfony/console.json", metadata)?;
   ```

3. **vcs/** - VCS clones (git repositories)
   ```rust
   // VCS caches are typically directories
   cache.copy_from("vcs/github.com/symfony/console", &repo_path)?;
   ```

### Copying Files

```rust
// Copy from cache to filesystem
cache.copy_to("files/package.zip", Path::new("/tmp/package.zip"))?;

// Copy from filesystem to cache
cache.copy_from("files/package.zip", Path::new("/tmp/source.zip"))?;
```

### Hashing

```rust
// Get SHA256 hash of cached file
if let Some(hash) = cache.sha256("files/package.zip")? {
    println!("SHA256: {}", hash);
}
```

### Garbage Collection

```rust
use std::time::Duration;

// Remove files older than 30 days
let freed_bytes = cache.gc(Duration::from_secs(30 * 24 * 3600))?;
println!("Freed {} bytes", freed_bytes);

// Special GC for VCS directories (removes entire directories)
let freed_bytes = cache.gc_vcs(Duration::from_secs(90 * 24 * 3600))?;
```

### Cache Statistics

```rust
// Get total cache size
let size = cache.size()?;
println!("Cache size: {} bytes", size);

// Get file age
if let Some(age) = cache.age("repo/packages.json")? {
    println!("File is {} seconds old", age.as_secs());
}
```

### Read-Only Mode

```rust
let mut cache = Cache::new(PathBuf::from("/usr/share/composer-cache"));

// Enable read-only mode
cache.set_read_only(true);

// Reads work
let data = cache.read("repo/packages.json")?;

// Writes are silently ignored (no error)
cache.write("repo/new.json", b"data")?; // No-op
```

### Key Sanitization

Cache keys are automatically sanitized to be filesystem-safe:

```rust
// Keys with special characters are sanitized
cache.write("repo/packagist.org/symfony/console", data)?;

// Stored as: repo-packagist.org-symfony-console
// Characters outside allowlist (a-z0-9._) are replaced with dashes
```

### Custom Allowlist

```rust
// Create cache with custom allowed characters
let cache = Cache::with_allowlist(
    PathBuf::from("/tmp/cache"),
    "a-z0-9._/".to_string() // Allow forward slashes
);
```

## API Reference

### Constructor

- `Cache::new(root: PathBuf) -> Self`
  - Creates a new cache with default allowlist `a-z0-9._`

- `Cache::with_allowlist(root: PathBuf, allowlist: String) -> Self`
  - Creates a cache with custom allowlist for key characters

### Configuration

- `set_read_only(&mut self, read_only: bool)`
  - Enable/disable read-only mode

- `is_read_only(&self) -> bool`
  - Check if cache is read-only

- `set_enabled(&mut self, enabled: bool)`
  - Enable/disable cache

- `is_enabled(&self) -> bool`
  - Check if cache is enabled and usable

### File Operations

- `has(&self, key: &str) -> bool`
  - Check if file exists in cache

- `read(&self, key: &str) -> io::Result<Option<Vec<u8>>>`
  - Read file from cache

- `write(&self, key: &str, data: &[u8]) -> io::Result<()>`
  - Write file to cache (atomic)

- `copy_to(&self, key: &str, dest: &Path) -> io::Result<bool>`
  - Copy from cache to filesystem

- `copy_from(&self, key: &str, source: &Path) -> io::Result<()>`
  - Copy from filesystem to cache

- `remove(&self, key: &str) -> io::Result<()>`
  - Delete file from cache

### Bulk Operations

- `clear(&self) -> io::Result<()>`
  - Remove all cache entries

- `gc(&self, ttl: Duration) -> io::Result<u64>`
  - Garbage collect files older than TTL, returns bytes freed

- `gc_vcs(&self, ttl: Duration) -> io::Result<u64>`
  - Garbage collect VCS directories older than TTL

### Information

- `sha256(&self, key: &str) -> io::Result<Option<String>>`
  - Get SHA256 hash of cached file

- `size(&self) -> io::Result<u64>`
  - Get total cache size in bytes

- `age(&self, key: &str) -> io::Result<Option<Duration>>`
  - Get age of cached file

- `root(&self) -> &Path`
  - Get cache root directory

## Implementation Details

### Atomic Writes

All writes use atomic operations:
1. Write to temporary file (`.tmp` extension)
2. Sync file to disk
3. Atomic rename to final location

This ensures cache consistency even if the process is interrupted.

### Thread Safety

The cache implementation is thread-safe for concurrent reads and writes. The filesystem provides natural locking through atomic operations.

### Error Handling

Cache operations never panic. Failed operations either:
- Return `Err(io::Error)` for operations that must succeed
- Return `Ok(())` silently for operations in read-only mode
- Return `Ok(false)` or `Ok(None)` for operations that may not find data

### Performance Considerations

- **Lazy initialization**: Cache directory is created on first write
- **No in-memory caching**: All operations go through filesystem
- **Efficient GC**: Uses `walkdir` for efficient directory traversal
- **Streaming hash**: SHA256 is computed in 8KB chunks to minimize memory usage

## Differences from PHP Implementation

1. **No IOInterface dependency**: Rust implementation doesn't include logging/IO interface
2. **Simplified error handling**: Uses `io::Result` instead of boolean returns
3. **Duration-based TTL**: Uses `std::time::Duration` instead of seconds
4. **No Filesystem dependency**: Uses standard library filesystem operations
5. **Added features**:
   - `age()` method to get file age
   - `set_enabled()` to dynamically control cache
   - Better error messages

## Testing

Run tests with:

```bash
cargo test -p pox-pm cache::
```

Run the demo example:

```bash
cargo run -p pox-pm --example cache_demo
```

## License

This implementation is based on Composer's Cache class, which is MIT licensed.
