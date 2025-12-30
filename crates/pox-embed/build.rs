use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    let php_config = env::var("PHP_CONFIG").unwrap_or_else(|_| "php-config".to_string());

    let php_includes = get_php_config(&php_config, "--includes");
    let php_ldflags = get_php_config(&php_config, "--ldflags");
    let php_libs = get_php_config(&php_config, "--libs");
    let php_prefix = get_php_config(&php_config, "--prefix");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let php_lib_dir = format!("{}/lib", php_prefix);

    // Check if static linking is requested or available
    let static_linking = env::var("POX_STATIC").is_ok()
        || env::var("CARGO_FEATURE_STATIC").is_ok()
        || check_static_lib_exists(&php_lib_dir);

    // Parse include paths
    let include_paths: Vec<&str> = php_includes
        .split_whitespace()
        .filter(|s| s.starts_with("-I"))
        .map(|s| &s[2..])
        .collect();

    // Build our C code
    let mut build = cc::Build::new();
    build.file("src/embed.c");

    for path in &include_paths {
        build.include(path);
    }

    build.flag("-Wall");

    if target_os == "macos" {
        build.flag("-Wno-deprecated-declarations");
    }

    // On Linux, we need _GNU_SOURCE to get memrchr and mempcpy
    // which PHP headers use (GNU extensions in string.h)
    if target_os == "linux" {
        build.define("_GNU_SOURCE", None);
    }

    build.compile("php_embed_c");

    // Link against PHP embed library
    println!("cargo:rustc-link-search=native={}", php_lib_dir);

    // Parse and emit linker flags from php-config --ldflags
    for flag in php_ldflags.split_whitespace() {
        if flag.starts_with("-L") {
            println!("cargo:rustc-link-search=native={}", &flag[2..]);
        }
    }

    if static_linking {
        println!("cargo:warning=Using static linking for PHP");
        // Static linking - link against libphp.a
        // Use static:+whole-archive to ensure the library is fully included
        println!("cargo:rustc-link-lib=static:+whole-archive,-bundle=php");

        // Detect if we're using musl (Alpine/static-php-cli)
        let is_musl = env::var("CARGO_CFG_TARGET_ENV")
            .map(|e| e == "musl")
            .unwrap_or(false);

        // When statically linking, we need all the dependencies
        if target_os == "linux" && !is_musl {
            // Prefer the static bz2 archive to satisfy libphp.a symbols.
            // Note: On musl/Alpine, bz2 is typically already linked via php-config --libs
            // First check the PHP prefix lib dir (for static-php-cli builds)
            emit_linux_bz2_search_paths(&target_arch, &php_lib_dir);
            println!("cargo:rustc-link-lib=static=bz2");
        }
        link_php_dependencies(&php_libs, &target_os, true);

        // Additional libraries needed for static linking
        if target_os == "linux" {
            if is_musl {
                // musl includes most of these in libc, only need math and pthread
                println!("cargo:rustc-link-lib=m");
                println!("cargo:rustc-link-lib=pthread");

                // libgcc for __clear_cache (used by JIT on ARM)
                println!("cargo:rustc-link-lib=gcc");
            } else {
                // glibc needs these separate libraries
                println!("cargo:rustc-link-lib=dl");
                println!("cargo:rustc-link-lib=m");
                println!("cargo:rustc-link-lib=pthread");
                println!("cargo:rustc-link-lib=rt");
                println!("cargo:rustc-link-lib=resolv");
                println!("cargo:rustc-link-lib=crypt");
            }
        }

        if target_os == "macos" {
            // Note: iconv is handled via link-arg in pox's build.rs to ensure
            // we use the system dylib, not the incomplete static libiconv.a from PHP.
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=SystemConfiguration");
            println!("cargo:rustc-link-lib=resolv");
        }
    } else {
        // Dynamic linking
        println!("cargo:rustc-link-lib=php");

        // Note: rpath is set in the pox crate's build.rs, not here
        // because link-arg from library crates doesn't propagate to binaries

        // Link dependencies
        link_php_dependencies(&php_libs, &target_os, false);

        if target_os == "linux" {
            println!("cargo:rustc-link-lib=dl");
            println!("cargo:rustc-link-lib=m");
            println!("cargo:rustc-link-lib=pthread");
            println!("cargo:rustc-link-lib=resolv");
        }

        if target_os == "macos" {
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
        }
    }

    println!("cargo:rerun-if-changed=src/embed.c");
    println!("cargo:rerun-if-changed=src/embed.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PHP_CONFIG");
    println!("cargo:rerun-if-env-changed=POX_STATIC");
}

/// Check if libphp.a exists (static library)
fn check_static_lib_exists(lib_dir: &str) -> bool {
    let static_lib = Path::new(lib_dir).join("libphp.a");
    if static_lib.exists() {
        println!(
            "cargo:warning=Found static PHP library at {}",
            static_lib.display()
        );
        true
    } else {
        false
    }
}

/// Link PHP dependencies from php-config --libs
fn link_php_dependencies(php_libs: &str, target_os: &str, static_linking: bool) {
    // Detect if we're using musl (Alpine/static-php-cli)
    let is_musl = env::var("CARGO_CFG_TARGET_ENV")
        .map(|e| e == "musl")
        .unwrap_or(false);

    for flag in php_libs.split_whitespace() {
        if flag.starts_with("-l") {
            let lib = &flag[2..];
            // Skip some libraries that we handle separately or that cause issues
            if lib == "php" || lib == "c" || (static_linking && lib == "bz2") {
                continue;
            }
            // On musl, some libs don't exist (like rt, crypt, resolv)
            if is_musl && ["rt", "crypt", "resolv", "dl"].contains(&lib) {
                continue;
            }
            // On macOS, skip iconv from php-config - it's a partial libiconv that
            // doesn't have iconv/iconv_open/iconv_close. The system provides these.
            if target_os == "macos" && lib == "iconv" {
                continue;
            }
            // On macOS, replace libstdc++ with libc++ since system libraries
            // (like ICU) are built with libc++ and the two are ABI-incompatible
            let lib = if target_os == "macos" && lib == "stdc++" {
                "c++"
            } else {
                lib
            };
            println!("cargo:rustc-link-lib={}", lib);
            // libwebp depends on libsharpyuv, add it immediately after webp
            if lib == "webp" {
                println!("cargo:rustc-link-lib=sharpyuv");
            }
        } else if flag.starts_with("-L") {
            println!("cargo:rustc-link-search=native={}", &flag[2..]);
        } else if flag.starts_with("-framework") {
            // macOS framework - next arg is the framework name
            continue;
        } else if flag.ends_with(".a") && Path::new(flag).exists() {
            // static-php-cli outputs absolute paths to .a files
            // Extract library name and path
            let path = Path::new(flag);
            if let Some(parent) = path.parent() {
                println!("cargo:rustc-link-search=native={}", parent.display());
            }
            if let Some(filename) = path.file_stem() {
                let libname = filename.to_string_lossy();
                // Remove "lib" prefix if present
                let libname = libname.strip_prefix("lib").unwrap_or(&libname);
                println!("cargo:rustc-link-lib=static={}", libname);
            }
        } else if flag.ends_with(".o") && Path::new(flag).exists() {
            // Object files (like mimalloc.o) - link directly
            println!("cargo:rustc-link-arg={}", flag);
        } else if target_os == "macos" && !flag.starts_with("-") {
            // Could be a framework name following -framework
            // Try to link it as a framework
            if flag != "framework" {
                println!("cargo:rustc-link-lib=framework={}", flag);
            }
        }
    }
}

fn emit_linux_bz2_search_paths(target_arch: &str, php_lib_dir: &str) {
    // First, check the PHP lib directory (for static-php-cli builds like /buildroot/lib)
    let bz2_in_php_lib = Path::new(php_lib_dir).join("libbz2.a");
    if bz2_in_php_lib.exists() {
        println!("cargo:rustc-link-search=native={}", php_lib_dir);
        return;
    }

    let candidates: &[&str] = match target_arch {
        "x86_64" => &["/usr/lib/x86_64-linux-gnu", "/usr/lib64", "/usr/lib"],
        "aarch64" => &["/usr/lib/aarch64-linux-gnu", "/usr/lib64", "/usr/lib"],
        "arm" => &["/usr/lib/arm-linux-gnueabihf", "/usr/lib"],
        _ => &["/usr/lib"],
    };

    let mut found = false;
    for path in candidates {
        if Path::new(path).exists() {
            println!("cargo:rustc-link-search=native={}", path);
            found = true;
        }
    }

    if !found {
        println!(
            "cargo:warning=No known bz2 libdir for arch {}; falling back to /usr/lib",
            target_arch
        );
        println!("cargo:rustc-link-search=native=/usr/lib");
    }
}

fn get_php_config(php_config: &str, arg: &str) -> String {
    let output = Command::new(php_config)
        .arg(arg)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "Failed to run {} {}: {}. Make sure PHP is installed with embed SAPI.",
                php_config, arg, e
            )
        });

    if !output.status.success() {
        panic!(
            "{} {} failed: {}",
            php_config,
            arg,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout)
        .expect("php-config output is not valid UTF-8")
        .trim()
        .to_string()
}
