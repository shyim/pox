//! PHP Embed - Rust bindings to embed PHP via the PHP embed SAPI
//!
//! This crate provides a safe Rust interface to execute PHP scripts and code
//! using PHP's embed SAPI (Server API).
//!
//! # Example
//!
//! ```no_run
//! use pox_embed::Php;
//!
//! // Execute PHP code directly
//! let exit_code = Php::execute_code(r#"echo "Hello from PHP!\n";"#, &[] as &[&str]).unwrap();
//!
//! // Execute a PHP script file
//! let exit_code = Php::execute_script("script.php", &["arg1", "arg2"]).unwrap();
//! ```
//!
//! # Building
//!
//! This crate requires PHP to be compiled with the embed SAPI enabled.
//!
//! ## Dynamic Linking (default)
//!
//! By default, this crate links dynamically against libphp.so/libphp.dylib.
//! Set the `PHP_CONFIG` environment variable to point to your php-config if
//! it's not in PATH:
//!
//! ```bash
//! PHP_CONFIG=/opt/php/bin/php-config cargo build
//! ```
//!
//! ## Static Linking
//!
//! For a fully self-contained binary, you can statically link PHP. This requires
//! PHP to be compiled with `--enable-embed=static`:
//!
//! ```bash
//! # Build PHP with static embed SAPI
//! ./configure --enable-embed=static --prefix=/opt/php-static [other options...]
//! make && make install
//!
//! # Build with static linking (auto-detected if libphp.a exists)
//! PHP_CONFIG=/opt/php-static/bin/php-config cargo build --release
//!
//! # Or force static linking via environment variable
//! PHPX_STATIC=1 PHP_CONFIG=/path/to/php-config cargo build --release
//!
//! # Or use the cargo feature
//! PHP_CONFIG=/path/to/php-config cargo build --release --features static
//! ```
//!
//! Static linking will automatically include all PHP dependencies (libxml2, openssl,
//! zlib, etc.) that PHP was compiled with.

use std::ffi::{CStr, CString, NulError};
use std::os::raw::{c_char, c_int, c_void};
use thiserror::Error;

// FFI bindings to our C code - CLI mode
extern "C" {
    fn pox_execute_script(
        script_path: *const c_char,
        argc: c_int,
        argv: *mut *mut c_char,
    ) -> c_int;
    fn pox_execute_code(code: *const c_char, argc: c_int, argv: *mut *mut c_char) -> c_int;
    fn pox_lint_file(script_path: *const c_char, argc: c_int, argv: *mut *mut c_char) -> c_int;
    fn pox_info(flag: c_int, argc: c_int, argv: *mut *mut c_char) -> c_int;
    fn pox_print_modules(argc: c_int, argv: *mut *mut c_char) -> c_int;
    fn pox_set_ini_entries(entries: *const c_char);
    fn pox_get_version() -> *const c_char;
    fn pox_get_version_id() -> c_int;
    fn pox_get_zend_version() -> *const c_char;
    fn pox_get_loaded_extensions(argc: c_int, argv: *mut *mut c_char) -> *mut c_char;
    fn pox_free_string(s: *mut c_char);

    // Build-time platform info
    fn pox_is_debug() -> c_int;
    fn pox_is_zts() -> c_int;
    fn pox_get_icu_version() -> *const c_char;
    fn pox_get_libxml_version() -> *const c_char;
    fn pox_get_openssl_version() -> *const c_char;
    fn pox_get_pcre_version() -> *const c_char;
    fn pox_get_zlib_version() -> *const c_char;
    fn pox_get_curl_version() -> *const c_char;
}

// FFI bindings to our C code - Web mode
extern "C" {
    fn pox_web_init() -> c_int;
    fn pox_web_shutdown();
    fn pox_web_execute(ctx: *mut c_void) -> c_int;
    fn pox_free_response(ctx: *mut c_void);
}

/// Errors that can occur when executing PHP
#[derive(Error, Debug)]
pub enum PhpError {
    #[error("Invalid string argument: {0}")]
    InvalidString(#[from] NulError),

    #[error("PHP initialization failed")]
    InitFailed,

    #[error("PHP execution failed with exit code {0}")]
    ExecutionFailed(i32),
}

/// Result type for PHP operations
pub type Result<T> = std::result::Result<T, PhpError>;

/// PHP version information
#[derive(Debug, Clone)]
pub struct PhpVersion {
    /// Version string (e.g., "8.3.0")
    pub version: &'static str,
    /// Version ID (e.g., 80300 for PHP 8.3.0)
    pub version_id: i32,
    /// Major version number
    pub major: i32,
    /// Minor version number
    pub minor: i32,
    /// Release version number
    pub release: i32,
    /// Zend Engine version
    pub zend_version: &'static str,
}

impl PhpVersion {
    /// Get the PHP version information
    pub fn get() -> Self {
        let version = unsafe {
            let ptr = pox_get_version();
            CStr::from_ptr(ptr).to_str().unwrap_or("unknown")
        };
        let version_id = unsafe { pox_get_version_id() };
        let zend_version = unsafe {
            let ptr = pox_get_zend_version();
            CStr::from_ptr(ptr).to_str().unwrap_or("unknown")
        };

        Self {
            version,
            version_id,
            major: version_id / 10000,
            minor: (version_id / 100) % 100,
            release: version_id % 100,
            zend_version,
        }
    }
}

impl std::fmt::Display for PhpVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.version)
    }
}

/// Helper to build argc/argv for PHP
fn build_argv<A: AsRef<str>>(program: &str, args: &[A]) -> Result<(Vec<CString>, Vec<*mut c_char>)> {
    let mut c_args: Vec<CString> = Vec::with_capacity(args.len() + 1);
    c_args.push(CString::new(program)?);
    for arg in args {
        c_args.push(CString::new(arg.as_ref())?);
    }

    let mut c_argv: Vec<*mut c_char> = c_args.iter().map(|s| s.as_ptr() as *mut c_char).collect();
    c_argv.push(std::ptr::null_mut());

    Ok((c_args, c_argv))
}

/// Main interface for executing PHP code
pub struct Php;

impl Php {
    /// Get PHP version information
    pub fn version() -> PhpVersion {
        PhpVersion::get()
    }

    /// Set INI entries before execution
    ///
    /// Entries should be in the format "key=value\nkey2=value2"
    pub fn set_ini_entries(entries: Option<&str>) -> Result<()> {
        match entries {
            Some(e) => {
                let c_entries = CString::new(e)?;
                unsafe { pox_set_ini_entries(c_entries.as_ptr()) };
            }
            None => {
                unsafe { pox_set_ini_entries(std::ptr::null()) };
            }
        }
        Ok(())
    }

    /// Execute a PHP script file
    ///
    /// # Arguments
    ///
    /// * `script_path` - Path to the PHP script to execute
    /// * `args` - Arguments to pass to the script (available in `$argv`)
    ///
    /// # Returns
    ///
    /// Returns `Ok(exit_code)` on successful execution, or an error if execution failed.
    pub fn execute_script<S, A>(script_path: S, args: &[A]) -> Result<i32>
    where
        S: AsRef<str>,
        A: AsRef<str>,
    {
        let script_path = script_path.as_ref();
        let c_script = CString::new(script_path)?;
        let (_c_args, mut c_argv) = build_argv(script_path, args)?;

        let exit_status = unsafe {
            pox_execute_script(
                c_script.as_ptr(),
                c_argv.len() as c_int - 1,
                c_argv.as_mut_ptr(),
            )
        };

        Ok(exit_status)
    }

    /// Execute PHP code directly
    ///
    /// # Arguments
    ///
    /// * `code` - PHP code to execute (without `<?php` tags)
    /// * `args` - Arguments available in `$argv`
    ///
    /// # Returns
    ///
    /// Returns `Ok(exit_code)` on successful execution, or an error if execution failed.
    pub fn execute_code<S, A>(code: S, args: &[A]) -> Result<i32>
    where
        S: AsRef<str>,
        A: AsRef<str>,
    {
        let c_code = CString::new(code.as_ref())?;
        let (_c_args, mut c_argv) = build_argv("php", args)?;

        let exit_status = unsafe {
            pox_execute_code(c_code.as_ptr(), c_argv.len() as c_int - 1, c_argv.as_mut_ptr())
        };

        Ok(exit_status)
    }

    /// Syntax check (lint) a PHP file
    ///
    /// # Arguments
    ///
    /// * `script_path` - Path to the PHP script to check
    ///
    /// # Returns
    ///
    /// Returns `Ok(0)` if syntax is valid, `Ok(1)` if there are syntax errors.
    pub fn lint<S: AsRef<str>>(script_path: S) -> Result<i32> {
        let script_path = script_path.as_ref();
        let c_script = CString::new(script_path)?;
        let (_c_args, mut c_argv) = build_argv::<&str>(script_path, &[])?;

        let result = unsafe {
            pox_lint_file(
                c_script.as_ptr(),
                c_argv.len() as c_int - 1,
                c_argv.as_mut_ptr(),
            )
        };

        Ok(result)
    }

    /// Print phpinfo() output
    ///
    /// # Arguments
    ///
    /// * `flag` - Optional flag to filter output (None for all info)
    pub fn info(flag: Option<i32>) -> Result<i32> {
        let (_c_args, mut c_argv) = build_argv::<&str>("php", &[])?;

        let result = unsafe {
            pox_info(
                flag.unwrap_or(-1),
                c_argv.len() as c_int - 1,
                c_argv.as_mut_ptr(),
            )
        };

        Ok(result)
    }

    /// Print loaded PHP modules
    pub fn print_modules() -> Result<i32> {
        let (_c_args, mut c_argv) = build_argv::<&str>("php", &[])?;

        let result =
            unsafe { pox_print_modules(c_argv.len() as c_int - 1, c_argv.as_mut_ptr()) };

        Ok(result)
    }

    /// Get list of loaded PHP extensions
    ///
    /// Returns a vector of extension names (e.g., ["Core", "date", "json", ...])
    pub fn get_loaded_extensions() -> Result<Vec<String>> {
        let (_c_args, mut c_argv) = build_argv::<&str>("php", &[])?;

        let ptr = unsafe {
            pox_get_loaded_extensions(c_argv.len() as c_int - 1, c_argv.as_mut_ptr())
        };

        if ptr.is_null() {
            return Err(PhpError::InitFailed);
        }

        let result = unsafe {
            let c_str = CStr::from_ptr(ptr);
            let extensions: Vec<String> = c_str
                .to_string_lossy()
                .lines()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();

            pox_free_string(ptr);
            extensions
        };

        Ok(result)
    }

    /// Check if PHP was built with debug mode
    pub fn is_debug() -> bool {
        unsafe { pox_is_debug() != 0 }
    }

    /// Check if PHP was built with ZTS (thread safety)
    pub fn is_zts() -> bool {
        unsafe { pox_is_zts() != 0 }
    }

    /// Get ICU library version (from intl extension)
    pub fn icu_version() -> Option<&'static str> {
        unsafe {
            let ptr = pox_get_icu_version();
            if ptr.is_null() {
                None
            } else {
                Some(CStr::from_ptr(ptr).to_str().unwrap_or(""))
            }
        }
    }

    /// Get libxml version
    pub fn libxml_version() -> Option<&'static str> {
        unsafe {
            let ptr = pox_get_libxml_version();
            if ptr.is_null() {
                None
            } else {
                Some(CStr::from_ptr(ptr).to_str().unwrap_or(""))
            }
        }
    }

    /// Get OpenSSL version text
    pub fn openssl_version() -> Option<&'static str> {
        unsafe {
            let ptr = pox_get_openssl_version();
            if ptr.is_null() {
                None
            } else {
                Some(CStr::from_ptr(ptr).to_str().unwrap_or(""))
            }
        }
    }

    /// Get PCRE version
    pub fn pcre_version() -> Option<&'static str> {
        unsafe {
            let ptr = pox_get_pcre_version();
            if ptr.is_null() {
                None
            } else {
                Some(CStr::from_ptr(ptr).to_str().unwrap_or(""))
            }
        }
    }

    /// Get zlib version
    pub fn zlib_version() -> Option<&'static str> {
        unsafe {
            let ptr = pox_get_zlib_version();
            if ptr.is_null() {
                None
            } else {
                Some(CStr::from_ptr(ptr).to_str().unwrap_or(""))
            }
        }
    }

    /// Get curl version
    pub fn curl_version() -> Option<&'static str> {
        unsafe {
            let ptr = pox_get_curl_version();
            if ptr.is_null() {
                None
            } else {
                Some(CStr::from_ptr(ptr).to_str().unwrap_or(""))
            }
        }
    }
}

// ============================================================================
// Web Server Support
// ============================================================================

/// Request context for web requests - must match the C struct layout exactly
#[repr(C)]
pub struct PhpRequestContext {
    // Request info
    method: *const c_char,
    uri: *const c_char,
    query_string: *const c_char,
    content_type: *const c_char,
    content_length: usize,
    request_body: *const c_char,
    request_body_len: usize,
    request_body_read: usize,

    // Headers (key: value\n format)
    headers: *const c_char,

    // Document root and script
    document_root: *const c_char,
    script_filename: *const c_char,

    // Server info
    server_name: *const c_char,
    server_port: c_int,
    remote_addr: *const c_char,
    remote_port: c_int,

    // Response output buffer (filled by C code)
    response_body: *mut c_char,
    response_body_len: usize,
    response_body_cap: usize,

    // Response headers (filled by C code)
    response_headers: *mut c_char,
    response_headers_len: usize,
    response_headers_cap: usize,

    // Response status
    response_status: c_int,
}

/// HTTP request to execute
pub struct HttpRequest {
    pub method: String,
    pub uri: String,
    pub query_string: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub document_root: String,
    pub script_filename: String,
    pub server_name: String,
    pub server_port: u16,
    pub remote_addr: String,
    pub remote_port: u16,
}

/// HTTP response from PHP execution
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// PHP web server runtime
pub struct PhpWeb {
    _initialized: bool,
}

impl PhpWeb {
    /// Initialize the PHP web runtime
    pub fn new() -> Result<Self> {
        let result = unsafe { pox_web_init() };
        if result != 0 {
            return Err(PhpError::InitFailed);
        }
        Ok(Self { _initialized: true })
    }

    /// Execute an HTTP request and return the response
    pub fn execute(&self, request: HttpRequest) -> Result<HttpResponse> {
        // Convert strings to CStrings, keeping them alive
        let method = CString::new(request.method)?;
        let uri = CString::new(request.uri)?;
        let query_string = CString::new(request.query_string)?;
        let document_root = CString::new(request.document_root)?;
        let script_filename = CString::new(request.script_filename)?;
        let server_name = CString::new(request.server_name)?;
        let remote_addr = CString::new(request.remote_addr)?;

        // Format headers as "Key: Value\n" string
        let headers_str: String = request
            .headers
            .iter()
            .map(|(k, v)| format!("{}: {}\n", k, v))
            .collect();
        let headers = CString::new(headers_str)?;

        // Get content type from headers
        let content_type = request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let content_type_c = CString::new(content_type)?;

        // Create the request context
        let mut ctx = PhpRequestContext {
            method: method.as_ptr(),
            uri: uri.as_ptr(),
            query_string: query_string.as_ptr(),
            content_type: content_type_c.as_ptr(),
            content_length: request.body.len(),
            request_body: request.body.as_ptr() as *const c_char,
            request_body_len: request.body.len(),
            request_body_read: 0,
            headers: headers.as_ptr(),
            document_root: document_root.as_ptr(),
            script_filename: script_filename.as_ptr(),
            server_name: server_name.as_ptr(),
            server_port: request.server_port as c_int,
            remote_addr: remote_addr.as_ptr(),
            remote_port: request.remote_port as c_int,
            response_body: std::ptr::null_mut(),
            response_body_len: 0,
            response_body_cap: 0,
            response_headers: std::ptr::null_mut(),
            response_headers_len: 0,
            response_headers_cap: 0,
            response_status: 200,
        };

        // Execute the request
        let _exit_code = unsafe { pox_web_execute(&mut ctx as *mut _ as *mut c_void) };

        // Extract response body
        let body = if !ctx.response_body.is_null() && ctx.response_body_len > 0 {
            unsafe {
                std::slice::from_raw_parts(ctx.response_body as *const u8, ctx.response_body_len)
                    .to_vec()
            }
        } else {
            Vec::new()
        };

        // Parse response headers
        let mut response_headers = Vec::new();
        if !ctx.response_headers.is_null() && ctx.response_headers_len > 0 {
            let headers_bytes = unsafe {
                std::slice::from_raw_parts(
                    ctx.response_headers as *const u8,
                    ctx.response_headers_len,
                )
            };
            if let Ok(headers_str) = std::str::from_utf8(headers_bytes) {
                for line in headers_str.lines() {
                    if let Some(colon_pos) = line.find(':') {
                        let key = line[..colon_pos].trim().to_string();
                        let value = line[colon_pos + 1..].trim().to_string();
                        response_headers.push((key, value));
                    }
                }
            }
        }

        // Free C-allocated response buffers
        unsafe { pox_free_response(&mut ctx as *mut _ as *mut c_void) };

        Ok(HttpResponse {
            status: ctx.response_status as u16,
            headers: response_headers,
            body,
        })
    }
}

impl Default for PhpWeb {
    fn default() -> Self {
        Self::new().expect("Failed to initialize PHP web runtime")
    }
}

impl Drop for PhpWeb {
    fn drop(&mut self) {
        unsafe { pox_web_shutdown() };
    }
}

// ============================================================================
// Worker Mode Support
// ============================================================================

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

// FFI bindings for worker mode
extern "C" {
    fn pox_worker_global_init() -> c_int;
    fn pox_worker_run(script_filename: *const c_char, document_root: *const c_char) -> c_int;
    fn pox_worker_set_request(ctx: *mut c_void);
}

/// Holds CStrings that must live as long as the request context
struct RequestStrings {
    method: CString,
    uri: CString,
    query_string: CString,
    document_root: CString,
    script_filename: CString,
    server_name: CString,
    remote_addr: CString,
    headers: CString,
    content_type: CString,
    body: Vec<u8>,
}

// Safety: RequestStrings only contains owned data (CString, Vec<u8>) which are Send+Sync
unsafe impl Send for RequestStrings {}
unsafe impl Sync for RequestStrings {}

// Safety: PhpRequestContext contains raw pointers that point to data owned by RequestStrings.
// The data it points to is kept alive by RequestStrings stored alongside it in WorkerThreadState.
// Access is synchronized through Mutex<Option<(Box<PhpRequestContext>, RequestStrings)>>.
unsafe impl Send for PhpRequestContext {}
unsafe impl Sync for PhpRequestContext {}

/// State shared between Rust and the PHP worker thread
struct WorkerThreadState {
    /// The request context to process (context + strings that must stay alive)
    request: Mutex<Option<(Box<PhpRequestContext>, Box<RequestStrings>)>>,
    /// Condition variable to signal worker that a request is available
    request_available: Condvar,
    /// Condition variable to signal Rust that the response is ready
    response_ready: Condvar,
    /// Condition variable to signal C that Rust has finished reading
    response_consumed: Condvar,
    /// Whether the worker should shut down
    shutdown: AtomicBool,
    /// Whether we're currently processing a request
    processing: AtomicBool,
    /// Whether the response is ready
    has_response: AtomicBool,
    /// Whether Rust has finished reading the response
    response_read: AtomicBool,
}

impl WorkerThreadState {
    fn new() -> Self {
        Self {
            request: Mutex::new(None),
            request_available: Condvar::new(),
            response_ready: Condvar::new(),
            response_consumed: Condvar::new(),
            shutdown: AtomicBool::new(false),
            processing: AtomicBool::new(false),
            has_response: AtomicBool::new(false),
            response_read: AtomicBool::new(false),
        }
    }
}

// Thread-local storage for worker state
thread_local! {
    static WORKER_STATE: std::cell::RefCell<Option<Arc<WorkerThreadState>>> = const { std::cell::RefCell::new(None) };
}

/// Called from C when the worker is waiting for a request
#[no_mangle]
pub extern "C" fn pox_worker_wait_for_request() -> c_int {
    WORKER_STATE.with(|state| {
        let state_ref = state.borrow();
        if let Some(ref worker_state) = *state_ref {
            // Check for shutdown
            if worker_state.shutdown.load(Ordering::SeqCst) {
                return 0;
            }

            // Wait for a request
            let mut request = worker_state.request.lock().unwrap_or_else(|e| e.into_inner());
            while request.is_none() && !worker_state.shutdown.load(Ordering::SeqCst) {
                request = worker_state.request_available.wait(request).unwrap_or_else(|e| e.into_inner());
            }

            if worker_state.shutdown.load(Ordering::SeqCst) {
                return 0;
            }

            // Set the request context in C
            if let Some((ref mut ctx, _)) = *request {
                unsafe {
                    pox_worker_set_request(ctx.as_mut() as *mut PhpRequestContext as *mut c_void);
                }
                worker_state.processing.store(true, Ordering::SeqCst);
                return 1;
            }
        }
        0
    })
}

/// Called from C when the worker has finished processing a request
/// This function blocks until Rust has finished reading the response
#[no_mangle]
pub extern "C" fn pox_worker_request_done() {
    WORKER_STATE.with(|state| {
        let state_ref = state.borrow();
        if let Some(ref worker_state) = *state_ref {
            // Signal that the response is ready
            worker_state.has_response.store(true, Ordering::SeqCst);
            worker_state.processing.store(false, Ordering::SeqCst);
            worker_state.response_ready.notify_all();

            // Wait for Rust to finish reading the response
            let req = worker_state.request.lock().unwrap_or_else(|e| e.into_inner());
            let _guard = worker_state.response_consumed.wait_while(req, |_| {
                !worker_state.response_read.load(Ordering::SeqCst)
                    && !worker_state.shutdown.load(Ordering::SeqCst)
            }).unwrap_or_else(|e| e.into_inner());

            // Reset for next request
            worker_state.response_read.store(false, Ordering::SeqCst);
        }
    });
}

/// A worker thread that runs a long-lived PHP script
struct WorkerThread {
    handle: Option<JoinHandle<()>>,
    state: Arc<WorkerThreadState>,
}

impl WorkerThread {
    fn new(script_filename: String, document_root: String) -> Self {
        let state = Arc::new(WorkerThreadState::new());
        let state_clone = state.clone();

        let handle = thread::spawn(move || {
            // Set up thread-local state
            WORKER_STATE.with(|s| {
                *s.borrow_mut() = Some(state_clone);
            });

            // Run the worker script
            let c_script = CString::new(script_filename).unwrap();
            let c_docroot = CString::new(document_root).unwrap();
            unsafe {
                pox_worker_run(c_script.as_ptr(), c_docroot.as_ptr());
            }
        });

        Self {
            handle: Some(handle),
            state,
        }
    }

    fn is_available(&self) -> bool {
        !self.state.processing.load(Ordering::SeqCst)
            && !self.state.shutdown.load(Ordering::SeqCst)
    }

    fn submit_request(&self, request: HttpRequest) -> Result<HttpResponse> {
        // Convert the request to CStrings that will be stored alongside the context
        let method = CString::new(request.method)?;
        let uri = CString::new(request.uri)?;
        let query_string = CString::new(request.query_string)?;
        let document_root = CString::new(request.document_root)?;
        let script_filename = CString::new(request.script_filename)?;
        let server_name = CString::new(request.server_name)?;
        let remote_addr = CString::new(request.remote_addr)?;

        // Format headers
        let headers_str: String = request
            .headers
            .iter()
            .map(|(k, v)| format!("{}: {}\n", k, v))
            .collect();
        let headers = CString::new(headers_str)?;

        // Get content type
        let content_type_str = request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let content_type = CString::new(content_type_str)?;

        let body = request.body;
        let body_len = body.len();
        let server_port = request.server_port;
        let remote_port = request.remote_port;

        // Store strings that need to live as long as the context
        // We Box it so it has a stable address
        let strings = Box::new(RequestStrings {
            method,
            uri,
            query_string,
            document_root,
            script_filename,
            server_name,
            remote_addr,
            headers,
            content_type,
            body,
        });

        // Create the request context pointing to the boxed strings
        let ctx = Box::new(PhpRequestContext {
            method: strings.method.as_ptr(),
            uri: strings.uri.as_ptr(),
            query_string: strings.query_string.as_ptr(),
            content_type: strings.content_type.as_ptr(),
            content_length: body_len,
            request_body: strings.body.as_ptr() as *const c_char,
            request_body_len: body_len,
            request_body_read: 0,
            headers: strings.headers.as_ptr(),
            document_root: strings.document_root.as_ptr(),
            script_filename: strings.script_filename.as_ptr(),
            server_name: strings.server_name.as_ptr(),
            server_port: server_port as c_int,
            remote_addr: strings.remote_addr.as_ptr(),
            remote_port: remote_port as c_int,
            response_body: std::ptr::null_mut(),
            response_body_len: 0,
            response_body_cap: 0,
            response_headers: std::ptr::null_mut(),
            response_headers_len: 0,
            response_headers_cap: 0,
            response_status: 200,
        });

        // Store the request and strings together, then signal the worker
        {
            let mut req = self.state.request.lock().unwrap_or_else(|e| e.into_inner());
            *req = Some((ctx, strings));
            self.state.has_response.store(false, Ordering::SeqCst);
            self.state.request_available.notify_one();
        }

        // Wait for the response
        {
            let req = self.state.request.lock().unwrap_or_else(|e| e.into_inner());
            let _guard = self.state.response_ready.wait_while(req, |_| {
                !self.state.has_response.load(Ordering::SeqCst)
            }).unwrap_or_else(|e| e.into_inner());
        }

        // Extract response
        let mut req = self.state.request.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((ref ctx, _)) = *req {
            // Extract response body
            let body = if !ctx.response_body.is_null() && ctx.response_body_len > 0 {
                unsafe {
                    std::slice::from_raw_parts(ctx.response_body as *const u8, ctx.response_body_len)
                        .to_vec()
                }
            } else {
                Vec::new()
            };

            // Parse response headers
            let mut response_headers = Vec::new();
            if !ctx.response_headers.is_null() && ctx.response_headers_len > 0 {
                let headers_bytes = unsafe {
                    std::slice::from_raw_parts(
                        ctx.response_headers as *const u8,
                        ctx.response_headers_len,
                    )
                };
                if let Ok(headers_str) = std::str::from_utf8(headers_bytes) {
                    for line in headers_str.lines() {
                        if let Some(colon_pos) = line.find(':') {
                            let key = line[..colon_pos].trim().to_string();
                            let value = line[colon_pos + 1..].trim().to_string();
                            response_headers.push((key, value));
                        }
                    }
                }
            }

            let status = ctx.response_status as u16;

            // Free response buffers
            unsafe { pox_free_response(ctx.as_ref() as *const PhpRequestContext as *mut c_void) };

            // Clear the request
            *req = None;

            // Signal that we're done reading, so C can continue
            self.state.response_read.store(true, Ordering::SeqCst);
            self.state.response_consumed.notify_all();

            Ok(HttpResponse {
                status,
                headers: response_headers,
                body,
            })
        } else {
            // Still signal even on error so C doesn't block forever
            self.state.response_read.store(true, Ordering::SeqCst);
            self.state.response_consumed.notify_all();
            Err(PhpError::ExecutionFailed(1))
        }
    }

    fn shutdown(&self) {
        self.state.shutdown.store(true, Ordering::SeqCst);
        self.state.request_available.notify_all();
        self.state.response_consumed.notify_all();
    }

    fn shutdown_and_join(mut self) {
        self.shutdown();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for WorkerThread {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// PHP Worker pool for handling requests with long-lived PHP processes
pub struct PhpWorker {
    workers: Vec<WorkerThread>,
    next_worker: AtomicUsize,
    script_filename: String,
    document_root: String,
    num_workers: usize,
}

impl PhpWorker {
    /// Create a new worker pool
    ///
    /// # Arguments
    ///
    /// * `script_filename` - Path to the worker PHP script
    /// * `document_root` - Document root directory
    /// * `num_workers` - Number of worker threads to create
    pub fn new(script_filename: &str, document_root: &str, num_workers: usize) -> Result<Self> {
        // Initialize PHP globally BEFORE spawning any worker threads
        // This sets up TSRM, SAPI, and other global state that must only be initialized once
        let result = unsafe { pox_worker_global_init() };
        if result != 0 {
            return Err(PhpError::InitFailed);
        }

        let mut workers = Vec::with_capacity(num_workers);

        for _ in 0..num_workers {
            let worker = WorkerThread::new(
                script_filename.to_string(),
                document_root.to_string(),
            );
            workers.push(worker);
        }

        // Give workers time to start up
        std::thread::sleep(std::time::Duration::from_millis(100));

        Ok(Self {
            workers,
            next_worker: AtomicUsize::new(0),
            script_filename: script_filename.to_string(),
            document_root: document_root.to_string(),
            num_workers,
        })
    }

    /// Restart all workers (used for hot reloading on file changes)
    pub fn restart(&mut self) {
        eprintln!("Restarting {} workers...", self.num_workers);

        // Shutdown existing workers
        for worker in self.workers.drain(..) {
            worker.shutdown_and_join();
        }

        // Create new workers
        for _ in 0..self.num_workers {
            let worker = WorkerThread::new(
                self.script_filename.clone(),
                self.document_root.clone(),
            );
            self.workers.push(worker);
        }

        // Give workers time to start up
        std::thread::sleep(std::time::Duration::from_millis(100));

        eprintln!("Workers restarted.");
    }

    /// Handle an HTTP request using an available worker
    pub fn handle_request(&self, request: HttpRequest) -> Result<HttpResponse> {
        // Simple round-robin selection
        let start = self.next_worker.fetch_add(1, Ordering::SeqCst) % self.workers.len();

        // Try to find an available worker, starting from the round-robin position
        for i in 0..self.workers.len() {
            let idx = (start + i) % self.workers.len();
            if self.workers[idx].is_available() {
                return self.workers[idx].submit_request(request);
            }
        }

        // All workers busy, use the round-robin one anyway (it will block)
        self.workers[start % self.workers.len()].submit_request(request)
    }
}

impl Drop for PhpWorker {
    fn drop(&mut self) {
        // Workers will be shut down when dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        let version = Php::version();
        assert!(version.version_id > 0);
        assert!(version.major >= 8);
    }
}
