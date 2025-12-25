mod add;
mod config;
mod pm;
mod init;
mod install;
mod remove;
mod update;

use config::PhpxConfig;

use anyhow::Result;
use clap::{Parser, Subcommand};
use phpx_embed::{HttpRequest, Php, PhpWeb, PhpWorker};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tiny_http::{Header, Response, Server, StatusCode};
use notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebouncedEvent};
use globset::{Glob, GlobSetBuilder};

#[derive(Parser, Debug)]
#[command(name = "phpx")]
#[command(about = "PHP CLI embedded in Rust")]
#[command(disable_version_flag = true)]
#[command(after_help = "See 'php --help' for the original PHP CLI help.")]
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
    #[arg(value_name = "FILE", trailing_var_arg = true, allow_hyphen_values = true)]
    script_and_args: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start a PHP development server
    Server {
        /// Address to listen on
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Port to listen on
        #[arg(short, long, default_value = "8000")]
        port: u16,

        /// Document root directory
        #[arg(short = 't', long, default_value = ".")]
        document_root: PathBuf,

        /// Router script (optional, like php -S router.php)
        #[arg(value_name = "ROUTER")]
        router: Option<PathBuf>,

        /// Worker script for long-running worker mode (like FrankenPHP)
        #[arg(short = 'w', long)]
        worker: Option<PathBuf>,

        /// Number of worker threads (default: number of CPU cores)
        #[arg(long, default_value = "0")]
        workers: usize,

        /// Watch for file changes and restart workers (glob patterns, e.g., "**/*.php")
        #[arg(long, action = clap::ArgAction::Append)]
        watch: Vec<String>,
    },

    /// Create a new composer.json in current directory
    Init(init::InitArgs),

    /// Install project dependencies from composer.lock
    Install(install::InstallArgs),

    /// Update dependencies to their latest versions
    Update(update::UpdateArgs),

    /// Add a package to the project
    Add(add::AddArgs),

    /// Remove a package from the project
    Remove(remove::RemoveArgs),

    /// Package manager commands (show, validate, dump-autoload)
    Pm {
        #[command(subcommand)]
        command: pm::PmCommands,
    },

    /// Run a script defined in composer.json
    Run(pm::RunArgs),
}

fn print_version() {
    let v = Php::version();
    println!("PHP {} (cli) (built: embedded)", v.version);
    println!("Copyright (c) The PHP Group");
    println!("{}", v.zend_version);
}

/// Build INI entries by merging config file and CLI arguments
/// CLI arguments take precedence over config file settings
fn build_ini_entries(config: Option<&PhpxConfig>, defines: &[String]) -> Option<String> {
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

fn run_server(host: &str, port: u16, document_root: &Path, router: Option<&Path>, worker: Option<&Path>, num_workers: usize, watch_patterns: Vec<String>, config: Option<&PhpxConfig>) -> Result<i32> {
    // Apply INI entries from config for server mode
    let ini_entries = build_ini_entries(config, &[]);
    if ini_entries.is_some() {
        Php::set_ini_entries(ini_entries.as_deref())?;
    }

    let addr = format!("{}:{}", host, port);
    let server = Server::http(&addr).map_err(|e| anyhow::anyhow!("Failed to start server: {}", e))?;

    let document_root = document_root
        .canonicalize()
        .unwrap_or_else(|_| document_root.to_path_buf());

    println!(
        "PHP {} Development Server started at http://{}",
        Php::version(),
        addr
    );
    println!("Document root is {}", document_root.display());
    if let Some(router) = router {
        println!("Router script is {}", router.display());
    }
    if let Some(worker_script) = worker {
        let num_workers = if num_workers == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        } else {
            num_workers
        };
        println!("Worker script is {} ({} workers)", worker_script.display(), num_workers);
        if !watch_patterns.is_empty() {
            println!("Watching for file changes: {:?}", watch_patterns);
        }
        return run_worker_server(server, host, port, &document_root, worker_script, num_workers, watch_patterns);
    }
    println!("Press Ctrl-C to quit.");

    // Initialize PHP web runtime
    let php = PhpWeb::new().map_err(|e| anyhow::anyhow!("Failed to initialize PHP: {}", e))?;

    for mut request in server.incoming_requests() {
        let method = request.method().to_string();
        let url = request.url().to_string();
        let (path, query_string) = parse_url(&url);

        // Try to serve static file first
        if let Some((content, content_type)) = get_static_file_content(&document_root, &path) {
            serve_static_file(request, content, &content_type, &method, &url);
            continue;
        }

        // Determine the script to execute
        let script_path = resolve_script_path(&document_root, &path, router);

        // Check if the file exists
        if !script_path.exists() || !script_path.is_file() {
            send_error_response(request, 404, "The requested URL was not found on this server.", &method, &url);
            continue;
        }

        let (headers, body, remote_addr, remote_port) = extract_request_metadata(&mut request);
        let php_request = build_php_request(
            method.clone(),
            url.clone(),
            query_string,
            headers,
            body,
            &document_root,
            &script_path,
            host,
            port,
            remote_addr,
            remote_port,
        );

        let result = php.execute(php_request);
        send_php_response(request, result, &method, &url);
    }

    Ok(0)
}

/// Resolve the PHP script path for a request
fn resolve_script_path(document_root: &Path, url_path: &str, router: Option<&Path>) -> PathBuf {
    if let Some(router) = router {
        return router.to_path_buf();
    }

    let mut file_path = document_root.to_path_buf();
    let url_path = url_path.trim_start_matches('/');
    if url_path.is_empty() {
        file_path.push("index.php");
    } else {
        file_path.push(url_path);
    }

    // If it's a directory, look for index.php
    if file_path.is_dir() {
        file_path.push("index.php");
    }

    // If the file doesn't exist, fall back to index.php (front controller pattern)
    if !file_path.exists() || !file_path.is_file() {
        let index_php = document_root.join("index.php");
        if index_php.exists() && index_php.is_file() {
            return index_php;
        }
    }

    file_path
}

fn run_worker_server(server: Server, host: &str, port: u16, document_root: &Path, worker_script: &Path, num_workers: usize, watch_patterns: Vec<String>) -> Result<i32> {
    let document_root = document_root.to_path_buf();
    let worker_script = worker_script.canonicalize()
        .map_err(|e| anyhow::anyhow!("Worker script not found: {}", e))?;
    let host = host.to_string();

    println!("Press Ctrl-C to quit.");

    // Initialize the worker pool (wrapped in Mutex for restart capability)
    let worker_pool = Arc::new(Mutex::new(PhpWorker::new(
        worker_script.to_string_lossy().as_ref(),
        document_root.to_string_lossy().as_ref(),
        num_workers,
    ).map_err(|e| anyhow::anyhow!("Failed to initialize PHP worker pool: {}", e))?));

    // Set up file watcher if patterns are provided
    let restart_flag = Arc::new(AtomicBool::new(false));
    let _watcher = if !watch_patterns.is_empty() {
        // Build glob set from patterns
        let mut glob_builder = GlobSetBuilder::new();
        for pattern in &watch_patterns {
            match Glob::new(pattern) {
                Ok(glob) => { glob_builder.add(glob); }
                Err(e) => eprintln!("Invalid glob pattern '{}': {}", pattern, e),
            }
        }
        let glob_set = glob_builder.build().map_err(|e| anyhow::anyhow!("Failed to build glob set: {}", e))?;

        // Create debounced watcher
        let restart_flag_clone = restart_flag.clone();
        let (tx, rx) = std::sync::mpsc::channel();

        let mut debouncer = new_debouncer(
            Duration::from_millis(150),
            None,
            move |result: std::result::Result<Vec<DebouncedEvent>, Vec<notify::Error>>| {
                if let Ok(events) = result {
                    let _ = tx.send(events);
                }
            },
        ).map_err(|e| anyhow::anyhow!("Failed to create file watcher: {}", e))?;

        // Watch the document root recursively
        debouncer.watch(&document_root, RecursiveMode::Recursive)
            .map_err(|e| anyhow::anyhow!("Failed to watch directory: {}", e))?;

        // Spawn thread to handle file change events
        let worker_pool_clone = worker_pool.clone();
        let doc_root_clone = document_root.clone();
        std::thread::spawn(move || {
            while let Ok(events) = rx.recv() {
                // Check if any changed file matches our patterns
                let mut should_restart = false;
                for event in events {
                    for path in &event.paths {
                        // Get relative path from document root
                        if let Ok(rel_path) = path.strip_prefix(&doc_root_clone) {
                            let rel_path_str = rel_path.to_string_lossy();
                            if glob_set.is_match(&*rel_path_str) || glob_set.is_match(path) {
                                eprintln!("File changed: {}", path.display());
                                should_restart = true;
                            }
                        } else if glob_set.is_match(path) {
                            eprintln!("File changed: {}", path.display());
                            should_restart = true;
                        }
                    }
                }

                if should_restart {
                    restart_flag_clone.store(true, Ordering::SeqCst);
                    // Restart workers
                    if let Ok(mut pool) = worker_pool_clone.lock() {
                        pool.restart();
                    }
                    restart_flag_clone.store(false, Ordering::SeqCst);
                }
            }
        });

        Some(debouncer)
    } else {
        None
    };

    // Handle incoming requests
    for mut request in server.incoming_requests() {
        let method = request.method().to_string();
        let url = request.url().to_string();
        let (path, query_string) = parse_url(&url);

        // Try to serve static files first
        if let Some((content, content_type)) = get_static_file_content(&document_root, &path) {
            serve_static_file(request, content, &content_type, &method, &url);
            continue;
        }

        // Wait if workers are restarting
        while restart_flag.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }

        let (headers, body, remote_addr, remote_port) = extract_request_metadata(&mut request);
        let php_request = build_php_request(
            method.clone(),
            url.clone(),
            query_string,
            headers,
            body,
            &document_root,
            &worker_script,
            &host,
            port,
            remote_addr,
            remote_port,
        );

        // Execute through worker pool
        let result = {
            let pool = worker_pool.lock().unwrap_or_else(|e| e.into_inner());
            pool.handle_request(php_request)
        };

        send_php_response(request, result, &method, &url);
    }

    Ok(0)
}

fn make_content_type_header(content_type: &str) -> Option<Header> {
    Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).ok()
}

/// Parse URL into path and query string
fn parse_url(url: &str) -> (String, String) {
    if let Some(pos) = url.find('?') {
        (url[..pos].to_string(), url[pos + 1..].to_string())
    } else {
        (url.to_string(), String::new())
    }
}

/// Check if a static file can be served, returns Some(content) if so
fn get_static_file_content(document_root: &Path, path: &str) -> Option<(Vec<u8>, String)> {
    let static_path = document_root.join(path.trim_start_matches('/'));
    if static_path.exists() && static_path.is_file() && !path.ends_with(".php") {
        if let Ok(content) = std::fs::read(&static_path) {
            let content_type = guess_content_type(&static_path);
            return Some((content, content_type));
        }
    }
    None
}

/// Serve a static file response
fn serve_static_file(request: tiny_http::Request, content: Vec<u8>, content_type: &str, method: &str, url: &str) {
    let mut response = Response::from_data(content);
    if let Some(header) = make_content_type_header(content_type) {
        response = response.with_header(header);
    }
    let _ = request.respond(response);
    println!("{} {} - 200", method, url);
}

/// Extract request metadata from tiny_http::Request
fn extract_request_metadata(request: &mut tiny_http::Request) -> (Vec<(String, String)>, Vec<u8>, String, u16) {
    // Read request body
    let mut body = Vec::new();
    if let Err(e) = request.as_reader().read_to_end(&mut body) {
        eprintln!("Failed to read request body: {}", e);
    }

    // Collect headers
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|h| (h.field.to_string(), h.value.to_string()))
        .collect();

    // Get remote address
    let remote_addr = request
        .remote_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let remote_port = request
        .remote_addr()
        .map(|a| a.port())
        .unwrap_or(0);

    (headers, body, remote_addr, remote_port)
}

/// Build an HttpRequest for PHP
fn build_php_request(
    method: String,
    url: String,
    query_string: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    document_root: &Path,
    script_filename: &Path,
    host: &str,
    port: u16,
    remote_addr: String,
    remote_port: u16,
) -> HttpRequest {
    HttpRequest {
        method,
        uri: url,
        query_string,
        headers,
        body,
        document_root: document_root.to_string_lossy().to_string(),
        script_filename: script_filename.to_string_lossy().to_string(),
        server_name: host.to_string(),
        server_port: port,
        remote_addr,
        remote_port,
    }
}

/// Send a PHP response back to the client
fn send_php_response(
    request: tiny_http::Request,
    result: std::result::Result<phpx_embed::HttpResponse, phpx_embed::PhpError>,
    method: &str,
    url: &str,
) {
    match result {
        Ok(response) => {
            let mut http_response = Response::from_data(response.body)
                .with_status_code(StatusCode(response.status));

            for (key, value) in response.headers {
                if let Ok(header) = Header::from_bytes(key.as_bytes(), value.as_bytes()) {
                    http_response.add_header(header);
                }
            }

            let status = response.status;
            let _ = request.respond(http_response);
            println!("{} {} - {}", method, url, status);
        }
        Err(e) => {
            send_error_response(request, 500, &e.to_string(), method, url);
        }
    }
}

/// Send an error response
fn send_error_response(
    request: tiny_http::Request,
    status_code: u16,
    message: &str,
    method: &str,
    url: &str,
) {
    let title = match status_code {
        404 => "404 Not Found",
        500 => "500 Internal Server Error",
        _ => "Error",
    };
    let heading = match status_code {
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let body = format!(
        "<!DOCTYPE html><html><head><title>{}</title></head><body><h1>{}</h1><p>{}</p></body></html>",
        title, heading, message
    );
    let mut response = Response::from_string(body)
        .with_status_code(StatusCode(status_code));
    if let Some(header) = make_content_type_header("text/html") {
        response = response.with_header(header);
    }
    let _ = request.respond(response);
    if message.is_empty() {
        println!("{} {} - {}", method, url, status_code);
    } else {
        println!("{} {} - {} ({})", method, url, status_code, message);
    }
}

fn guess_content_type(path: &Path) -> String {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

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
    let args = Args::parse();

    // Load phpx.toml config if present
    let config = PhpxConfig::load_from_cwd()?;

    // Handle subcommands first
    if let Some(command) = args.command {
        match command {
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
                let effective_host = config.as_ref()
                    .and_then(|c| c.server.host.clone())
                    .unwrap_or(host);
                let effective_port = config.as_ref()
                    .and_then(|c| c.server.port)
                    .unwrap_or(port);
                let effective_doc_root = config.as_ref()
                    .and_then(|c| c.server.document_root.as_ref().map(PathBuf::from))
                    .unwrap_or(document_root);
                let effective_router = router.or_else(|| {
                    config.as_ref()
                        .and_then(|c| c.server.router.as_ref().map(PathBuf::from))
                });
                let effective_worker = worker.or_else(|| {
                    config.as_ref()
                        .and_then(|c| c.server.worker.as_ref().map(PathBuf::from))
                });
                let effective_workers = if workers == 0 {
                    config.as_ref()
                        .and_then(|c| c.server.workers)
                        .unwrap_or(0)
                } else {
                    workers
                };
                let effective_watch = if watch.is_empty() {
                    config.as_ref()
                        .map(|c| c.server.watch.clone())
                        .unwrap_or_default()
                } else {
                    watch
                };

                return run_server(
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
            Commands::Init(init_args) => {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| anyhow::anyhow!("Failed to create async runtime: {}", e))?;
                return rt.block_on(init::execute(init_args));
            }
            Commands::Install(install_args) => {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| anyhow::anyhow!("Failed to create async runtime: {}", e))?;
                return rt.block_on(install::execute(install_args));
            }
            Commands::Update(update_args) => {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| anyhow::anyhow!("Failed to create async runtime: {}", e))?;
                return rt.block_on(update::execute(update_args));
            }
            Commands::Add(add_args) => {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| anyhow::anyhow!("Failed to create async runtime: {}", e))?;
                return rt.block_on(add::execute(add_args));
            }
            Commands::Remove(remove_args) => {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| anyhow::anyhow!("Failed to create async runtime: {}", e))?;
                return rt.block_on(remove::execute(remove_args));
            }
            Commands::Pm { command } => {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| anyhow::anyhow!("Failed to create async runtime: {}", e))?;
                return rt.block_on(pm::execute(command));
            }
            Commands::Run(run_args) => {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| anyhow::anyhow!("Failed to create async runtime: {}", e))?;
                return rt.block_on(pm::run::execute(run_args));
            }
        }
    }

    // Set INI entries from config file and CLI args
    let ini_entries = build_ini_entries(config.as_ref(), &args.define);
    if ini_entries.is_some() {
        Php::set_ini_entries(ini_entries.as_deref())?;
    }

    // Handle -v/--version
    if args.version_flag {
        print_version();
        return Ok(0);
    }

    // Handle -i/--info (phpinfo)
    if args.info {
        return Ok(Php::info(None)?);
    }

    // Handle -m/--modules
    if args.modules {
        return Ok(Php::print_modules()?);
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
            return Ok(Php::lint(path.as_ref())?);
        } else {
            eprintln!("No input file specified for syntax check");
            return Ok(1);
        }
    }

    // Handle -r (run code)
    if let Some(code) = &args.run {
        return Ok(Php::execute_code(code, &script_args)?);
    }

    // Handle script execution
    if let Some(ref s) = script {
        let script_path = s.to_string_lossy();
        return Ok(Php::execute_script(script_path.as_ref(), &script_args)?);
    }

    // No action specified - show usage
    let v = Php::version();
    eprintln!("phpx {} - PHP {} embedded in Rust", env!("CARGO_PKG_VERSION"), v.version);
    eprintln!();
    eprintln!("Usage: phpx [options] [-f] <file> [--] [args...]");
    eprintln!("       phpx [options] -r <code> [--] [args...]");
    eprintln!("       phpx server [options] [router.php]");
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
    eprintln!("  init            Create a new composer.json in current directory");
    eprintln!("  install         Install project dependencies from composer.lock");
    eprintln!("  update          Update dependencies to their latest versions");
    eprintln!("  add             Add a package to the project");
    eprintln!("  remove          Remove a package from the project");
    eprintln!("  run             Run a script defined in composer.json");
    eprintln!("  server          Start a PHP development server");
    eprintln!("  pm              Other package manager commands (show, validate, etc.)");
    eprintln!();
    eprintln!("Run 'phpx --help' for more options.");

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
