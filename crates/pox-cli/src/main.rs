mod config;
mod package_manager;
mod runtime_manager;
mod server;
mod server_path;

use config::PoxConfig;
use runtime_manager::RuntimeManager;

use anyhow::Result;
use clap::{Parser, Subcommand};
use pox_embed::PhpRuntime;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(name = "pox")]
#[command(about = "PHP runtime manager, development server, and package manager")]
#[command(disable_version_flag = true)]
#[command(
    after_help = "PHP runtimes are managed with 'pox php'. Package management is powered by Riff.\nRun 'pox <command> --help' for command-specific help.\n\nCommon package-manager commands:\n  init, create-project, install, update, require, add, remove, run\n  show, search, outdated, audit, validate, status, check-platform-reqs\n\nThe compatibility form 'pox pm <command>' is also supported."
)]
#[command(args_conflicts_with_subcommands = true)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Run PHP code directly (like php -r)
    #[arg(short = 'r', value_name = "CODE", conflicts_with_all = ["script_and_args", "lint", "info", "modules", "version_flag"])]
    run: Option<String>,

    /// Syntax check only (lint)
    #[arg(short = 'l', long = "lint", conflicts_with_all = ["run", "info", "modules", "version_flag"])]
    lint: bool,

    /// PHP information (phpinfo)
    #[arg(short = 'i', long = "info", conflicts_with_all = ["script_and_args", "run", "lint", "modules", "version_flag"])]
    info: bool,

    /// Show compiled in modules
    #[arg(short = 'm', long = "modules", conflicts_with_all = ["script_and_args", "run", "lint", "info", "version_flag"])]
    modules: bool,

    /// Version information
    #[arg(short = 'v', long = "version", conflicts_with_all = ["script_and_args", "run", "lint", "info", "modules"])]
    version_flag: bool,

    /// Define INI entry (can be used multiple times)
    #[arg(short = 'd', value_name = "KEY=VALUE", action = clap::ArgAction::Append)]
    define: Vec<String>,

    /// PHP script to execute and its arguments
    #[arg(
        value_name = "FILE",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    script_and_args: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Install and select independently versioned PHP runtimes
    Php {
        #[command(subcommand)]
        command: PhpCommands,
    },

    /// Start the PHP HTTP server
    Server {
        /// Address to listen on
        #[arg(long)]
        host: Option<String>,

        /// Port to listen on
        #[arg(short, long)]
        port: Option<u16>,

        /// Document root directory
        #[arg(short = 't', long)]
        document_root: Option<PathBuf>,

        /// Router script (optional, like php -S router.php)
        #[arg(value_name = "ROUTER")]
        router: Option<PathBuf>,

        /// Worker script for long-running worker mode (like FrankenPHP)
        #[arg(short = 'w', long)]
        worker: Option<PathBuf>,

        /// PHP execution threads in standard or worker mode (default: number of CPU cores)
        #[arg(long, default_value = "0")]
        workers: usize,

        /// Watch for file changes and restart workers (glob patterns, e.g., "**/*.php")
        #[arg(long, action = clap::ArgAction::Append)]
        watch: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum PhpCommands {
    /// Download and install a signed PHP runtime
    Install {
        /// Exact version or release series, such as 8.5 or 8.5.9
        version: String,
        /// Replace an existing installation
        #[arg(long)]
        force: bool,
    },
    /// Select an installed PHP runtime
    Use {
        /// Exact version or installed release series
        version: String,
        /// Set the fallback outside projects
        #[arg(long)]
        global: bool,
    },
    /// List installed or remotely available runtimes
    List {
        /// Read available versions from the signed release index
        #[arg(long)]
        remote: bool,
    },
    /// Show the selected runtime and ABI details
    Current,
    /// Remove an installed runtime
    Remove {
        /// Exact version or installed release series
        version: String,
        /// Remove even when selected in the current context
        #[arg(long)]
        force: bool,
    },
}

fn print_version(php: &PhpRuntime) {
    let v = php.version();
    println!(
        "PHP {} (cli) (Pox runtime {})",
        v.version,
        php.metadata().runtime_revision
    );
    println!("Copyright (c) The PHP Group");
    println!("{}", v.zend_version);
}

fn run_php_manager(manager: &RuntimeManager, command: PhpCommands) -> Result<i32> {
    match command {
        PhpCommands::Install { version, force } => {
            manager.install(&version, force)?;
        }
        PhpCommands::Use { version, global } => {
            manager.use_version(&version, global)?;
        }
        PhpCommands::List { remote } => manager.list(remote)?,
        PhpCommands::Current => {
            if let Some(path) = std::env::var_os("POX_PHP_RUNTIME") {
                let php = PhpRuntime::load(&path)?;
                println!("PHP {}", php.metadata().php_version);
                println!("Runtime revision: {}", php.metadata().runtime_revision);
                println!("Target: {}", php.metadata().target);
                println!(
                    "ABI: {}.{}",
                    php.metadata().abi_major,
                    php.metadata().abi_minor
                );
                println!("Library: {}", PathBuf::from(path).display());
            } else {
                let installed = manager.current()?;
                println!("PHP {}", installed.manifest.php_version);
                println!("Runtime revision: {}", installed.manifest.runtime_revision);
                println!("Target: {}", installed.manifest.target);
                println!(
                    "ABI: {}.{}",
                    installed.manifest.abi_major, installed.manifest.abi_minor
                );
                println!("Library: {}", installed.library_path().display());
            }
        }
        PhpCommands::Remove { version, force } => manager.remove(&version, force)?,
    }
    Ok(0)
}

/// Build INI entries by merging config file and CLI arguments
/// CLI arguments take precedence over config file settings
fn build_ini_entries(config: Option<&PoxConfig>, defines: &[String]) -> Option<String> {
    use std::collections::HashMap;

    let mut ini_map: HashMap<String, String> = HashMap::new();

    // First, load from config file (lower priority)
    if let Some(cfg) = config {
        for (key, value) in &cfg.php.ini {
            ini_map.insert(key.clone(), value.clone());
        }
    }

    // Then, apply CLI arguments (higher priority, overrides config)
    for d in defines {
        if let Some(pos) = d.find('=') {
            let key = d[..pos].to_string();
            let value = d[pos + 1..].to_string();
            ini_map.insert(key, value);
        } else {
            ini_map.insert(d.clone(), "1".to_string());
        }
    }

    if ini_map.is_empty() {
        return None;
    }

    let entries: Vec<String> = ini_map
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();

    Some(entries.join("\n") + "\n")
}

fn guess_content_type(path: &Path) -> String {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match extension.to_lowercase().as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn run() -> Result<i32> {
    let raw_arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let manager = RuntimeManager::new()?;
    if package_manager::should_delegate(&raw_arguments) {
        let php = manager.load_selected()?;
        return package_manager::execute(raw_arguments, &php);
    }

    let args = Args::parse();

    // Load pox.toml config if present
    let config = PoxConfig::load_from_cwd()?;

    // Handle subcommands first
    if let Some(command) = args.command {
        match command {
            Commands::Php { command } => return run_php_manager(&manager, command),
            Commands::Server {
                host,
                port,
                document_root,
                router,
                worker,
                workers,
                watch,
            } => {
                // Merge CLI args with config file settings (CLI takes precedence)
                let effective_host = host
                    .or_else(|| config.as_ref().and_then(|c| c.server.host.clone()))
                    .unwrap_or_else(|| "127.0.0.1".into());
                let effective_port = port
                    .or_else(|| config.as_ref().and_then(|c| c.server.port))
                    .unwrap_or(8000);
                let effective_doc_root = document_root
                    .or_else(|| {
                        config
                            .as_ref()
                            .and_then(|c| c.server.document_root.as_ref().map(PathBuf::from))
                    })
                    .unwrap_or_else(|| PathBuf::from("."));
                let effective_router = router.or_else(|| {
                    config
                        .as_ref()
                        .and_then(|c| c.server.router.as_ref().map(PathBuf::from))
                });
                let effective_worker = worker.or_else(|| {
                    config
                        .as_ref()
                        .and_then(|c| c.server.worker.as_ref().map(PathBuf::from))
                });
                let effective_workers = if workers == 0 {
                    config.as_ref().and_then(|c| c.server.workers).unwrap_or(0)
                } else {
                    workers
                };
                let effective_watch = if watch.is_empty() {
                    config
                        .as_ref()
                        .map(|c| c.server.watch.clone())
                        .unwrap_or_default()
                } else {
                    watch
                };

                return server::run(
                    &manager.load_selected()?,
                    &effective_host,
                    effective_port,
                    &effective_doc_root,
                    effective_router.as_deref(),
                    effective_worker.as_deref(),
                    effective_workers,
                    effective_watch,
                    config.as_ref(),
                );
            }
        }
    }

    let php = manager.load_selected()?;

    // Set INI entries from config file and CLI args
    let ini_entries = build_ini_entries(config.as_ref(), &args.define);
    if ini_entries.is_some() {
        php.set_ini_entries(ini_entries.as_deref())?;
    }

    // Handle -v/--version
    if args.version_flag {
        print_version(&php);
        return Ok(0);
    }

    // Handle -i/--info (phpinfo)
    if args.info {
        return Ok(php.info(None)?);
    }

    // Handle -m/--modules
    if args.modules {
        return Ok(php.print_modules()?);
    }

    // Parse script and args from combined vector
    let (script, script_args): (Option<PathBuf>, Vec<String>) = if args.script_and_args.is_empty() {
        (None, Vec::new())
    } else {
        let script = PathBuf::from(&args.script_and_args[0]);
        let script_args = args.script_and_args[1..].to_vec();
        (Some(script), script_args)
    };

    // Handle -l/--lint
    if args.lint {
        if let Some(ref s) = script {
            let path = s.to_string_lossy();
            return Ok(php.lint(path.as_ref(), &script_args)?);
        } else {
            eprintln!("No input file specified for syntax check");
            return Ok(1);
        }
    }

    // Handle -r (run code)
    if let Some(code) = &args.run {
        return Ok(php.execute_code(code, &script_args)?);
    }

    // Handle script execution
    if let Some(ref s) = script {
        let script_path = s.to_string_lossy();
        return Ok(php.execute_script(script_path.as_ref(), &script_args)?);
    }

    // No action specified - show usage
    let v = php.version();
    eprintln!(
        "pox {} - PHP {} runtime",
        env!("CARGO_PKG_VERSION"),
        v.version
    );
    eprintln!();
    eprintln!("Usage: pox [options] [-f] <file> [--] [args...]");
    eprintln!("       pox [options] -r <code> [--] [args...]");
    eprintln!("       pox server [options] [router.php]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -d key[=value]  Define INI entry");
    eprintln!("  -i              PHP information (phpinfo)");
    eprintln!("  -l              Syntax check only (lint)");
    eprintln!("  -m              Show compiled in modules");
    eprintln!("  -r <code>       Run PHP <code> without script tags");
    eprintln!("  -v              Version information");
    eprintln!("  -h, --help      Show this help message");
    eprintln!();
    eprintln!("Subcommands:");
    eprintln!("  php             Install and select PHP runtimes");
    eprintln!("  server          Start a PHP development server");
    eprintln!("  init            Create a new composer.json in current directory (Riff)");
    eprintln!("  create-project  Create a project from a package (Riff)");
    eprintln!("  install         Install project dependencies from composer.lock (Riff)");
    eprintln!("  update          Update dependencies to their latest versions (Riff)");
    eprintln!("  require, add    Add a package to the project (Riff)");
    eprintln!("  remove          Remove a package from the project (Riff)");
    eprintln!("  run             Run a script defined in composer.json (Riff)");
    eprintln!("  pm              Riff package-manager compatibility prefix");
    eprintln!("  completion      Generate Riff-powered shell completion scripts");
    eprintln!();
    eprintln!("Run 'pox --help' for more options.");

    Ok(0)
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("Error: {}", e);
            // Print the error chain for debugging
            for cause in e.chain().skip(1) {
                eprintln!("  Caused by: {}", cause);
            }
            ExitCode::FAILURE
        }
    }
}
