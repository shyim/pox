# pox-semver

Semantic version parsing, comparison, and constraint matching compatible with Composer/semver.

## Usage

Basic checks:

```rust
use pox_semver::Semver;

assert!(Semver::satisfies("1.2.3", "^1.2"));
assert!(!Semver::satisfies("2.0.0", "^1.2"));
```

Filter a list:

```rust
use pox_semver::Semver;

let versions = vec!["1.0", "1.2", "1.9999.9999", "2.0", "2.1"];
let result = Semver::satisfied_by(&versions, "~1.0");
assert_eq!(result, vec!["1.0", "1.2", "1.9999.9999"]);
```

Reusable parsed constraints (avoid repeated parsing when checking many versions):

```rust
use pox_semver::Semver;

let parsed = Semver::parse_constraints("^1.2").unwrap();
assert!(Semver::satisfies_parsed("1.2.3", &parsed));
assert!(!Semver::satisfies_parsed("2.0.0", &parsed));
```

Sorting:

```rust
use pox_semver::Semver;

let versions = vec!["1.0", "0.1", "3.2.1", "2.4.0-alpha", "2.4.0"];
let sorted = Semver::sort(&versions);
assert_eq!(sorted, vec!["0.1", "1.0", "2.4.0-alpha", "2.4.0", "3.2.1"]);
```

## Benchmarks

Run the Criterion suite:

```bash
cargo bench --bench semver
```
