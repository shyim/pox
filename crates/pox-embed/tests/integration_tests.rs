//! Integration tests for pox-embed crate
//!
//! Note: These tests must be run sequentially (not in parallel) because
//! PHP can only be initialized once per process. Use:
//! ```
//! cargo test -- --test-threads=1
//! ```

use pox_embed::{Php, PhpVersion};
use std::io::Write;
use tempfile::NamedTempFile;

/// Helper to create a temporary PHP file with given content
fn create_php_file(content: &str) -> NamedTempFile {
    let mut file = NamedTempFile::with_suffix(".php").unwrap();
    file.write_all(content.as_bytes()).unwrap();
    file.flush().unwrap();
    file
}

#[test]
fn test_version_info() {
    let version = Php::version();

    // Version ID should be a positive number
    assert!(version.version_id > 0, "Version ID should be positive");

    // Major version should be at least 8
    assert!(version.major >= 8, "Major version should be at least 8");

    // Version string should not be empty
    assert!(!version.version.is_empty(), "Version string should not be empty");

    // Zend version should not be empty
    assert!(
        !version.zend_version.is_empty(),
        "Zend version should not be empty"
    );

    // Version ID should match major.minor.release calculation
    let expected_id = version.major * 10000 + version.minor * 100 + version.release;
    assert_eq!(
        version.version_id, expected_id,
        "Version ID should match major.minor.release"
    );
}

#[test]
fn test_version_display() {
    let version = Php::version();
    let display = format!("{}", version);

    // Display should equal the version string
    assert_eq!(display, version.version);
}

#[test]
fn test_execute_code_simple() {
    // Simple echo test - exit code should be 0
    let result = Php::execute_code("echo 'test';", &[] as &[&str]);
    assert!(result.is_ok(), "execute_code should succeed");
    assert_eq!(result.unwrap(), 0, "Exit code should be 0");
}

#[test]
fn test_execute_code_with_exit() {
    // Test explicit exit code
    let result = Php::execute_code("exit(42);", &[] as &[&str]);
    assert!(result.is_ok(), "execute_code should succeed");
    assert_eq!(result.unwrap(), 42, "Exit code should be 42");
}

#[test]
fn test_execute_code_with_args() {
    // Test that args are available in $argv
    let result = Php::execute_code(
        r#"
        if ($argv[1] === 'arg1' && $argv[2] === 'arg2') {
            exit(0);
        }
        exit(1);
        "#,
        &["arg1", "arg2"],
    );
    assert!(result.is_ok(), "execute_code should succeed");
    assert_eq!(result.unwrap(), 0, "Args should be passed correctly");
}

#[test]
fn test_execute_script_simple() {
    let file = create_php_file("<?php echo 'hello'; exit(0);");
    let path = file.path().to_str().unwrap();

    let result = Php::execute_script(path, &[] as &[&str]);
    assert!(result.is_ok(), "execute_script should succeed");
    assert_eq!(result.unwrap(), 0, "Exit code should be 0");
}

#[test]
fn test_execute_script_with_exit_code() {
    let file = create_php_file("<?php exit(123);");
    let path = file.path().to_str().unwrap();

    let result = Php::execute_script(path, &[] as &[&str]);
    assert!(result.is_ok(), "execute_script should succeed");
    assert_eq!(result.unwrap(), 123, "Exit code should be 123");
}

#[test]
fn test_execute_script_with_args() {
    let file = create_php_file(
        r#"<?php
        // $argv[0] is the script name
        if ($argc === 3 && $argv[1] === 'foo' && $argv[2] === 'bar') {
            exit(0);
        }
        exit(1);
        "#,
    );
    let path = file.path().to_str().unwrap();

    let result = Php::execute_script(path, &["foo", "bar"]);
    assert!(result.is_ok(), "execute_script should succeed");
    assert_eq!(result.unwrap(), 0, "Script args should be passed correctly");
}

#[test]
fn test_execute_script_with_shebang() {
    let file = create_php_file(
        r#"#!/usr/bin/env php
<?php
echo 'shebang test';
exit(0);
"#,
    );
    let path = file.path().to_str().unwrap();

    let result = Php::execute_script(path, &[] as &[&str]);
    assert!(result.is_ok(), "execute_script should handle shebang");
    assert_eq!(result.unwrap(), 0, "Exit code should be 0");
}

#[test]
fn test_lint_valid_syntax() {
    let file = create_php_file(
        r#"<?php
        function test($a, $b) {
            return $a + $b;
        }
        echo test(1, 2);
        "#,
    );
    let path = file.path().to_str().unwrap();

    let result = Php::lint(path);
    assert!(result.is_ok(), "lint should succeed");
    assert_eq!(result.unwrap(), 0, "Valid syntax should return 0");
}

#[test]
fn test_lint_invalid_syntax() {
    let file = create_php_file(
        r#"<?php
        echo "unclosed string
        "#,
    );
    let path = file.path().to_str().unwrap();

    let result = Php::lint(path);
    assert!(result.is_ok(), "lint should succeed even for invalid syntax");
    assert_ne!(result.unwrap(), 0, "Invalid syntax should return non-zero");
}

#[test]
fn test_lint_missing_semicolon() {
    let file = create_php_file(
        r#"<?php
        $a = 1
        $b = 2;
        "#,
    );
    let path = file.path().to_str().unwrap();

    let result = Php::lint(path);
    assert!(result.is_ok(), "lint should succeed");
    assert_ne!(result.unwrap(), 0, "Missing semicolon should be caught");
}

#[test]
fn test_print_modules() {
    // This just tests that print_modules doesn't crash
    let result = Php::print_modules();
    assert!(result.is_ok(), "print_modules should succeed");
    assert_eq!(result.unwrap(), 0, "Exit code should be 0");
}

#[test]
fn test_info() {
    // This just tests that info doesn't crash
    let result = Php::info(None);
    assert!(result.is_ok(), "info should succeed");
    assert_eq!(result.unwrap(), 0, "Exit code should be 0");
}

#[test]
fn test_set_ini_entries() {
    // Test setting INI entries
    let result = Php::set_ini_entries(Some("memory_limit=256M\n"));
    assert!(result.is_ok(), "set_ini_entries should succeed");

    // Verify the INI was set
    let result = Php::execute_code(
        r#"
        $limit = ini_get('memory_limit');
        exit($limit === '256M' ? 0 : 1);
        "#,
        &[] as &[&str],
    );
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0, "INI entry should be set correctly");

    // Clear INI entries for subsequent tests
    let _ = Php::set_ini_entries(None);
}

#[test]
fn test_set_multiple_ini_entries() {
    let result = Php::set_ini_entries(Some("error_reporting=0\ndisplay_errors=Off\n"));
    assert!(result.is_ok(), "set_ini_entries should succeed");

    let result = Php::execute_code(
        r#"
        $er = ini_get('error_reporting');
        $de = ini_get('display_errors');
        exit(($er === '0' && ($de === 'Off' || $de === '')) ? 0 : 1);
        "#,
        &[] as &[&str],
    );
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0, "Multiple INI entries should be set");

    // Clear INI entries
    let _ = Php::set_ini_entries(None);
}

#[test]
fn test_execute_code_invalid_string() {
    // Test that null bytes in code are handled
    let result = Php::execute_code("echo 'test\0null';", &[] as &[&str]);
    assert!(result.is_err(), "Null byte in code should cause error");
}

#[test]
fn test_execute_script_nonexistent() {
    let result = Php::execute_script("/nonexistent/path/to/script.php", &[] as &[&str]);
    // PHP should return an error for non-existent files
    assert!(result.is_ok(), "execute_script should not panic");
    // The exit code will be non-zero due to the file not existing
}

#[test]
fn test_php_version_struct() {
    let v = PhpVersion::get();

    // Test the get() method directly
    assert!(v.version_id > 0);
    assert!(v.major >= 8);
    assert!(v.minor >= 0 && v.minor < 100);
    assert!(v.release >= 0 && v.release < 100);
}

#[test]
fn test_execute_code_php_error() {
    // Test PHP runtime error handling
    let result = Php::execute_code(
        r#"
        // This will cause an error but not crash
        trigger_error('test error', E_USER_WARNING);
        exit(0);
        "#,
        &[] as &[&str],
    );
    assert!(result.is_ok(), "PHP errors should not crash");
}

#[test]
fn test_server_variables() {
    let file = create_php_file(
        r#"<?php
        // Check that $_SERVER variables are set
        $required = ['PHP_SELF', 'SCRIPT_NAME', 'SCRIPT_FILENAME'];
        foreach ($required as $var) {
            if (!isset($_SERVER[$var])) {
                exit(1);
            }
        }
        exit(0);
        "#,
    );
    let path = file.path().to_str().unwrap();

    let result = Php::execute_script(path, &[] as &[&str]);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0, "Server variables should be set");
}

#[test]
fn test_stdin_stdout_stderr_constants() {
    let result = Php::execute_code(
        r#"
        // Check that STDIN, STDOUT, STDERR constants exist
        if (!defined('STDIN') || !defined('STDOUT') || !defined('STDERR')) {
            exit(1);
        }
        exit(0);
        "#,
        &[] as &[&str],
    );
    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        0,
        "STDIN/STDOUT/STDERR constants should be defined"
    );
}
