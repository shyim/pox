//! Riff integration for Composer-compatible package management.

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use pox_embed::PhpRuntime;
use riff::CommandContext;
use riff_core::{
    Output, OutputEvent, OutputOptions, OutputSink, OutputStream, Platform, PlatformSnapshot,
    RuntimeContext,
};

/// Decide whether an invocation belongs to Riff instead of the embedded PHP CLI.
pub fn should_delegate(arguments: &[OsString]) -> bool {
    let Some(first) = arguments.first() else {
        return false;
    };
    let first = first.to_string_lossy();

    if first == "pm" || first == "completion" || first.starts_with("__complete") {
        return true;
    }
    if first == "server" || first == "php" || matches!(first.as_ref(), "-h" | "--help") {
        return false;
    }
    if is_php_option(&first) {
        return false;
    }
    if is_riff_global_option(&first) {
        return true;
    }

    !looks_like_php_script(OsStr::new(first.as_ref()))
}

/// Execute raw Riff arguments using platform facts from the embedded PHP runtime.
pub fn execute(arguments: Vec<OsString>, php: &PhpRuntime) -> Result<i32> {
    let arguments = normalize_arguments(arguments);
    let completion = is_completion_invocation(&arguments);
    let capture = completion.then(|| Arc::new(EventCollector::default()));
    let output = capture.as_ref().map_or_else(
        || Output::process(OutputOptions::default()),
        |capture| Output::from_sink(capture.clone()),
    );
    let context =
        CommandContext::new(runtime_context()?, embedded_platform(php)).with_output(output);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("Failed to create package-manager runtime")?;
    let result = runtime.block_on(riff::run_with_args(arguments.clone(), context));

    if let Some(capture) = capture {
        replay_completion(&capture, should_add_server_completion(&arguments))?;
    }

    result
}

fn normalize_arguments(mut arguments: Vec<OsString>) -> Vec<OsString> {
    if arguments.first().is_some_and(|argument| argument == "pm") {
        arguments.remove(0);
        if arguments.is_empty() {
            arguments.push(OsString::from("--help"));
        }
    }
    arguments
}

fn runtime_context() -> Result<RuntimeContext> {
    let executable = std::env::current_exe().context("Failed to locate the pox executable")?;
    Ok(RuntimeContext::new(executable.clone(), executable))
}

fn embedded_platform(php: &PhpRuntime) -> Platform {
    let version = php.version();
    let metadata = php.metadata();
    let extensions = metadata
        .extensions
        .iter()
        .map(|extension| (extension.to_ascii_lowercase(), version.version.to_string()))
        .collect();
    let mut libraries = metadata.libraries.clone();
    if let Some(openssl) = libraries.get_mut("openssl") {
        if let Some(version) = openssl_version(openssl) {
            *openssl = version.to_string();
        }
    }

    Platform::from_snapshot(PlatformSnapshot {
        php_version: version.version.to_string(),
        php_version_id: version.version_id as u64,
        int_size: std::mem::size_of::<isize>() as u64,
        zts: metadata.zts,
        debug: metadata.debug,
        ipv6: true,
        extensions,
        libraries,
    })
}

fn openssl_version(version: &str) -> Option<&str> {
    version
        .split_whitespace()
        .find(|part| part.starts_with(|character: char| character.is_ascii_digit()))
}

fn is_php_option(argument: &str) -> bool {
    matches!(
        argument,
        "-" | "-r"
            | "-l"
            | "--lint"
            | "-i"
            | "--info"
            | "-m"
            | "--modules"
            | "-v"
            | "--version"
            | "-d"
    ) || argument.starts_with("-r")
        || argument.starts_with("-d")
}

fn is_riff_global_option(argument: &str) -> bool {
    matches!(
        argument,
        "-q" | "--quiet" | "--no-progress" | "--ansi" | "--no-ansi" | "--php" | "--output"
    ) || argument.starts_with("--php=")
        || argument.starts_with("--output=")
}

fn looks_like_php_script(argument: &OsStr) -> bool {
    let path = Path::new(argument);
    let value = argument.to_string_lossy();
    path.exists()
        || value.ends_with(".php")
        || value.contains(std::path::MAIN_SEPARATOR)
        || value.contains('/')
        || value.contains('\\')
}

fn is_completion_invocation(arguments: &[OsString]) -> bool {
    arguments.first().is_some_and(|argument| {
        argument == "completion" || argument.to_string_lossy().starts_with("__complete")
    })
}

fn should_add_server_completion(arguments: &[OsString]) -> bool {
    if !arguments
        .first()
        .is_some_and(|argument| argument.to_string_lossy().starts_with("__complete"))
    {
        return false;
    }

    let Some(line) = arguments
        .windows(2)
        .find(|pair| pair[0] == "--line")
        .and_then(|pair| pair[1].to_str())
    else {
        return false;
    };

    let words = line.split_whitespace().collect::<Vec<_>>();
    match words.as_slice() {
        [] | [_] => true,
        [_, prefix] if !line.ends_with(char::is_whitespace) => "server".starts_with(prefix),
        _ => false,
    }
}

#[derive(Default)]
struct EventCollector(Mutex<Vec<OutputEvent>>);

impl OutputSink for EventCollector {
    fn emit(&self, event: OutputEvent) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

fn replay_completion(capture: &EventCollector, add_server: bool) -> Result<()> {
    let events = capture
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for event in events.iter() {
        let message = event.message.replace("Riff", "Pox").replace("riff", "pox");
        let writer: Box<dyn Write> = match event.stream {
            OutputStream::Stdout => Box::new(io::stdout().lock()),
            OutputStream::Stderr => Box::new(io::stderr().lock()),
        };
        write_message(writer, &message, event.newline)?;
    }
    if add_server {
        writeln!(io::stdout().lock(), "server")?;
    }
    Ok(())
}

fn write_message(mut writer: Box<dyn Write>, message: &str, newline: bool) -> io::Result<()> {
    writer.write_all(message.as_bytes())?;
    if newline {
        writer.write_all(b"\n")?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn routes_package_manager_commands_and_aliases() {
        for values in [
            &["install"][..],
            &["create-project", "vendor/project"],
            &["validate"],
            &["pm", "show"],
            &["completion", "bash"],
            &["--quiet", "status"],
        ] {
            assert!(should_delegate(&args(values)), "{values:?}");
        }
    }

    #[test]
    fn keeps_php_and_server_invocations_native() {
        for values in [
            &["server"][..],
            &["php", "list"],
            &["-r", "echo 1;"],
            &["-d", "memory_limit=1G", "script.php"],
            &["script.php"],
            &["./script"],
            &["--help"],
        ] {
            assert!(!should_delegate(&args(values)), "{values:?}");
        }
    }

    #[test]
    fn strips_pm_compatibility_prefix() {
        assert_eq!(
            normalize_arguments(args(&["pm", "install", "--dry-run"])),
            args(&["install", "--dry-run"])
        );
        assert_eq!(normalize_arguments(args(&["pm"])), args(&["--help"]));
    }

    #[test]
    fn extracts_openssl_versions() {
        assert_eq!(openssl_version("OpenSSL 3.0.2 15 Mar 2022"), Some("3.0.2"));
        assert_eq!(openssl_version("LibreSSL 3.3.6"), Some("3.3.6"));
        assert_eq!(openssl_version("unknown"), None);
    }

    #[test]
    fn adds_server_to_top_level_completion() {
        for values in [
            &["__complete_word__", "--line", "pox "][..],
            &["__complete_word__", "--line", "pox ser"],
        ] {
            assert!(should_add_server_completion(&args(values)), "{values:?}");
        }

        for values in [
            &["completion", "bash"][..],
            &["__complete_word__", "--line", "pox install "],
            &["__complete_word__", "--line", "pox unrelated"],
        ] {
            assert!(!should_add_server_completion(&args(values)), "{values:?}");
        }
    }
}
