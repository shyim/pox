use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    let php_config = env::var("PHP_CONFIG").unwrap_or_else(|_| "php-config".to_string());
    let php_prefix = get_php_config(&php_config, "--prefix");
    let php_lib_dir = format!("{}/lib", php_prefix);

    // Check if static linking is being used
    let static_lib = Path::new(&php_lib_dir).join("libphp.a");
    let static_linking = env::var("POX_STATIC").is_ok()
        || env::var("CARGO_FEATURE_STATIC").is_ok()
        || static_lib.exists();

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if static_linking {
        // Ensure dependent shared libs are not dropped when linking static PHP.
        if target_os == "linux" {
            println!("cargo:rustc-link-arg=-Wl,--no-as-needed");
        }
        // On macOS, libgit2-sys and PHP need iconv. The PHP build includes a
        // partial libiconv.a that only has iconv_canonicalize but not the
        // standard iconv/iconv_open/iconv_close functions that libgit2 needs.
        // Force the system iconv dylib via -reexport_library which takes the
        // full path and ignores any .a files in search paths.
        if target_os == "macos" {
            // The system libiconv is in /usr/lib/libiconv.dylib but on modern
            // macOS it's in the dyld cache. We use the TBD stub from the SDK.
            let sdk_path = std::process::Command::new("xcrun")
                .args(["--show-sdk-path"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|_| "/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk".to_string());
            let iconv_tbd = format!("{}/usr/lib/libiconv.tbd", sdk_path);
            println!("cargo:rustc-link-arg=-Wl,-reexport_library,{}", iconv_tbd);
        }
    } else {
        // Only add rpath for dynamic linking
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", php_lib_dir);
    }

    println!("cargo:rerun-if-env-changed=PHP_CONFIG");
    println!("cargo:rerun-if-env-changed=POX_STATIC");
}

fn get_php_config(php_config: &str, arg: &str) -> String {
    let output = Command::new(php_config)
        .arg(arg)
        .output()
        .unwrap_or_else(|e| panic!("Failed to run {}: {}", php_config, e));

    String::from_utf8(output.stdout)
        .expect("php-config output is not valid UTF-8")
        .trim()
        .to_string()
}
