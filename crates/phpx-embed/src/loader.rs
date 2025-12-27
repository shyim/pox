//! Dynamic PHP Runtime Loader
//!
//! This module provides functionality to dynamically load PHP runtime libraries
//! at runtime, enabling version switching without recompilation.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use libloading::{Library, Symbol};
use thiserror::Error;

/// Errors that can occur during dynamic loading
#[derive(Error, Debug)]
pub enum LoaderError {
    #[error("Failed to load library: {0}")]
    LoadFailed(#[from] libloading::Error),

    #[error("Symbol not found: {0}")]
    SymbolNotFound(String),

    #[error("ABI version mismatch: expected {expected}, got {actual}")]
    AbiMismatch { expected: i32, actual: i32 },

    #[error("Library not loaded")]
    NotLoaded,

    #[error("Library already loaded")]
    AlreadyLoaded,

    #[error("PHP execution failed: {0}")]
    ExecutionFailed(String),
}

pub type Result<T> = std::result::Result<T, LoaderError>;

/// ABI version - must match PHPX_ABI_VERSION in abi.h
const PHPX_ABI_VERSION: i32 = 1;

/// Version information from the loaded library
#[repr(C)]
pub struct VersionInfo {
    pub abi_version: i32,
    pub php_version: *const c_char,
    pub php_version_id: i32,
    pub zend_version: *const c_char,
    pub is_debug: i32,
    pub is_zts: i32,
    pub icu_version: *const c_char,
    pub libxml_version: *const c_char,
    pub openssl_version: *const c_char,
    pub pcre_version: *const c_char,
    pub zlib_version: *const c_char,
    pub curl_version: *const c_char,
}

/// Request context for web requests - must match C struct
#[repr(C)]
pub struct RequestContext {
    // Request info
    pub method: *const c_char,
    pub uri: *const c_char,
    pub query_string: *const c_char,
    pub content_type: *const c_char,
    pub content_length: usize,
    pub request_body: *const c_char,
    pub request_body_len: usize,
    pub request_body_read: usize,

    // Headers
    pub headers: *const c_char,

    // Document root and script
    pub document_root: *const c_char,
    pub script_filename: *const c_char,

    // Server info
    pub server_name: *const c_char,
    pub server_port: c_int,
    pub remote_addr: *const c_char,
    pub remote_port: c_int,

    // Response output buffer
    pub response_body: *mut c_char,
    pub response_body_len: usize,
    pub response_body_cap: usize,

    // Response headers
    pub response_headers: *mut c_char,
    pub response_headers_len: usize,
    pub response_headers_cap: usize,

    // Response status
    pub response_status: c_int,
}

/// Function table containing all PHP runtime functions
#[repr(C)]
pub struct FunctionTable {
    // Version info
    pub get_version_info: Option<unsafe extern "C" fn() -> *const VersionInfo>,

    // CLI mode
    pub set_ini_entries: Option<unsafe extern "C" fn(*const c_char)>,
    pub execute_script: Option<unsafe extern "C" fn(*const c_char, c_int, *mut *mut c_char) -> c_int>,
    pub execute_code: Option<unsafe extern "C" fn(*const c_char, c_int, *mut *mut c_char) -> c_int>,
    pub lint_file: Option<unsafe extern "C" fn(*const c_char, c_int, *mut *mut c_char) -> c_int>,
    pub info: Option<unsafe extern "C" fn(c_int, c_int, *mut *mut c_char) -> c_int>,
    pub print_modules: Option<unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int>,
    pub get_loaded_extensions: Option<unsafe extern "C" fn(c_int, *mut *mut c_char) -> *mut c_char>,
    pub free_string: Option<unsafe extern "C" fn(*mut c_char)>,

    // Web mode
    pub web_init: Option<unsafe extern "C" fn() -> c_int>,
    pub web_shutdown: Option<unsafe extern "C" fn()>,
    pub web_execute: Option<unsafe extern "C" fn(*mut RequestContext) -> c_int>,
    pub free_response: Option<unsafe extern "C" fn(*mut RequestContext)>,

    // Worker mode
    pub worker_global_init: Option<unsafe extern "C" fn() -> c_int>,
    pub worker_run: Option<unsafe extern "C" fn(*const c_char, *const c_char) -> c_int>,
    pub worker_set_request: Option<unsafe extern "C" fn(*mut RequestContext)>,
}

/// PHP version information (safe Rust wrapper)
#[derive(Debug, Clone)]
pub struct PhpVersionInfo {
    pub php_version: String,
    pub php_version_id: i32,
    pub zend_version: String,
    pub is_debug: bool,
    pub is_zts: bool,
    pub icu_version: Option<String>,
    pub libxml_version: Option<String>,
    pub openssl_version: Option<String>,
    pub pcre_version: Option<String>,
    pub zlib_version: Option<String>,
    pub curl_version: Option<String>,
}

/// Dynamically loaded PHP runtime
pub struct DynamicPhpRuntime {
    library: Library,
    function_table: *const FunctionTable,
    version_info: PhpVersionInfo,
}

// Safety: The library handle and function pointers are only accessed through
// safe wrappers that ensure proper synchronization
unsafe impl Send for DynamicPhpRuntime {}
unsafe impl Sync for DynamicPhpRuntime {}

impl DynamicPhpRuntime {
    /// Load a PHP runtime library from the given path
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        // Load the library
        let library = unsafe { Library::new(path)? };

        // Get the function table entry point
        let get_function_table: Symbol<unsafe extern "C" fn() -> *const FunctionTable> =
            unsafe { library.get(b"phpx_get_function_table\0")? };

        let function_table = unsafe { get_function_table() };
        if function_table.is_null() {
            return Err(LoaderError::SymbolNotFound(
                "phpx_get_function_table returned null".to_string(),
            ));
        }

        // Get version info and validate ABI
        let version_info_ptr = unsafe {
            let ft = &*function_table;
            if let Some(get_version) = ft.get_version_info {
                get_version()
            } else {
                return Err(LoaderError::SymbolNotFound(
                    "get_version_info".to_string(),
                ));
            }
        };

        if version_info_ptr.is_null() {
            return Err(LoaderError::SymbolNotFound(
                "version info is null".to_string(),
            ));
        }

        let version_info = unsafe { &*version_info_ptr };

        // Validate ABI version
        if version_info.abi_version != PHPX_ABI_VERSION {
            return Err(LoaderError::AbiMismatch {
                expected: PHPX_ABI_VERSION,
                actual: version_info.abi_version,
            });
        }

        // Convert version info to safe Rust types
        let php_version_info = PhpVersionInfo {
            php_version: unsafe { cstr_to_string(version_info.php_version) },
            php_version_id: version_info.php_version_id,
            zend_version: unsafe { cstr_to_string(version_info.zend_version) },
            is_debug: version_info.is_debug != 0,
            is_zts: version_info.is_zts != 0,
            icu_version: unsafe { optional_cstr_to_string(version_info.icu_version) },
            libxml_version: unsafe { optional_cstr_to_string(version_info.libxml_version) },
            openssl_version: unsafe { optional_cstr_to_string(version_info.openssl_version) },
            pcre_version: unsafe { optional_cstr_to_string(version_info.pcre_version) },
            zlib_version: unsafe { optional_cstr_to_string(version_info.zlib_version) },
            curl_version: unsafe { optional_cstr_to_string(version_info.curl_version) },
        };

        Ok(Self {
            library,
            function_table,
            version_info: php_version_info,
        })
    }

    /// Get PHP version information
    pub fn version_info(&self) -> &PhpVersionInfo {
        &self.version_info
    }

    /// Set INI entries before execution
    pub fn set_ini_entries(&self, entries: Option<&str>) -> Result<()> {
        let ft = unsafe { &*self.function_table };
        if let Some(set_ini) = ft.set_ini_entries {
            if let Some(e) = entries {
                let c_entries = CString::new(e).unwrap();
                unsafe { set_ini(c_entries.as_ptr()) };
            } else {
                unsafe { set_ini(std::ptr::null()) };
            }
            Ok(())
        } else {
            Err(LoaderError::SymbolNotFound("set_ini_entries".to_string()))
        }
    }

    /// Execute a PHP script
    pub fn execute_script(&self, script_path: &str, args: &[String]) -> Result<i32> {
        let ft = unsafe { &*self.function_table };
        if let Some(execute) = ft.execute_script {
            let c_script = CString::new(script_path).unwrap();
            let (c_args, mut c_argv) = build_argv(script_path, args);

            let exit_code = unsafe {
                execute(
                    c_script.as_ptr(),
                    c_argv.len() as c_int - 1,
                    c_argv.as_mut_ptr(),
                )
            };

            // Keep c_args alive until after the call
            drop(c_args);

            Ok(exit_code)
        } else {
            Err(LoaderError::SymbolNotFound("execute_script".to_string()))
        }
    }

    /// Execute PHP code directly
    pub fn execute_code(&self, code: &str, args: &[String]) -> Result<i32> {
        let ft = unsafe { &*self.function_table };
        if let Some(execute) = ft.execute_code {
            let c_code = CString::new(code).unwrap();
            let (c_args, mut c_argv) = build_argv("php", args);

            let exit_code = unsafe {
                execute(
                    c_code.as_ptr(),
                    c_argv.len() as c_int - 1,
                    c_argv.as_mut_ptr(),
                )
            };

            drop(c_args);

            Ok(exit_code)
        } else {
            Err(LoaderError::SymbolNotFound("execute_code".to_string()))
        }
    }

    /// Syntax check a PHP file
    pub fn lint(&self, script_path: &str) -> Result<i32> {
        let ft = unsafe { &*self.function_table };
        if let Some(lint) = ft.lint_file {
            let c_script = CString::new(script_path).unwrap();
            let (c_args, mut c_argv) = build_argv(script_path, &[]);

            let result = unsafe {
                lint(
                    c_script.as_ptr(),
                    c_argv.len() as c_int - 1,
                    c_argv.as_mut_ptr(),
                )
            };

            drop(c_args);

            Ok(result)
        } else {
            Err(LoaderError::SymbolNotFound("lint_file".to_string()))
        }
    }

    /// Print phpinfo()
    pub fn info(&self, flag: Option<i32>) -> Result<i32> {
        let ft = unsafe { &*self.function_table };
        if let Some(info) = ft.info {
            let (c_args, mut c_argv) = build_argv("php", &[]);

            let result = unsafe {
                info(
                    flag.unwrap_or(-1),
                    c_argv.len() as c_int - 1,
                    c_argv.as_mut_ptr(),
                )
            };

            drop(c_args);

            Ok(result)
        } else {
            Err(LoaderError::SymbolNotFound("info".to_string()))
        }
    }

    /// Print loaded modules
    pub fn print_modules(&self) -> Result<i32> {
        let ft = unsafe { &*self.function_table };
        if let Some(print_modules) = ft.print_modules {
            let (c_args, mut c_argv) = build_argv("php", &[]);

            let result = unsafe {
                print_modules(c_argv.len() as c_int - 1, c_argv.as_mut_ptr())
            };

            drop(c_args);

            Ok(result)
        } else {
            Err(LoaderError::SymbolNotFound("print_modules".to_string()))
        }
    }

    /// Get loaded extensions
    pub fn get_loaded_extensions(&self) -> Result<Vec<String>> {
        let ft = unsafe { &*self.function_table };
        let get_ext = ft.get_loaded_extensions.ok_or_else(|| {
            LoaderError::SymbolNotFound("get_loaded_extensions".to_string())
        })?;
        let free_str = ft.free_string.ok_or_else(|| {
            LoaderError::SymbolNotFound("free_string".to_string())
        })?;

        let (c_args, mut c_argv) = build_argv("php", &[]);

        let ptr = unsafe { get_ext(c_argv.len() as c_int - 1, c_argv.as_mut_ptr()) };
        drop(c_args);

        if ptr.is_null() {
            return Err(LoaderError::ExecutionFailed(
                "Failed to get extensions".to_string(),
            ));
        }

        let extensions = unsafe {
            let c_str = CStr::from_ptr(ptr);
            let result: Vec<String> = c_str
                .to_string_lossy()
                .lines()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();

            free_str(ptr);
            result
        };

        Ok(extensions)
    }

    /// Initialize web SAPI
    pub fn web_init(&self) -> Result<()> {
        let ft = unsafe { &*self.function_table };
        if let Some(init) = ft.web_init {
            let result = unsafe { init() };
            if result != 0 {
                return Err(LoaderError::ExecutionFailed(
                    "Web initialization failed".to_string(),
                ));
            }
            Ok(())
        } else {
            Err(LoaderError::SymbolNotFound("web_init".to_string()))
        }
    }

    /// Shutdown web SAPI
    pub fn web_shutdown(&self) {
        let ft = unsafe { &*self.function_table };
        if let Some(shutdown) = ft.web_shutdown {
            unsafe { shutdown() };
        }
    }
}

/// Helper to convert C string to Rust String
unsafe fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

/// Helper to convert optional C string
unsafe fn optional_cstr_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
    }
}

/// Build argc/argv for PHP
fn build_argv(program: &str, args: &[String]) -> (Vec<CString>, Vec<*mut c_char>) {
    let mut c_args: Vec<CString> = Vec::with_capacity(args.len() + 1);
    c_args.push(CString::new(program).unwrap());
    for arg in args {
        c_args.push(CString::new(arg.as_str()).unwrap());
    }

    let mut c_argv: Vec<*mut c_char> = c_args
        .iter()
        .map(|s| s.as_ptr() as *mut c_char)
        .collect();
    c_argv.push(std::ptr::null_mut());

    (c_args, c_argv)
}

/// Global state for managing the loaded runtime
static RUNTIME_LOADED: AtomicBool = AtomicBool::new(false);

/// Global PHP runtime manager
pub struct PhpRuntimeManager {
    runtime: Option<DynamicPhpRuntime>,
}

impl PhpRuntimeManager {
    /// Create a new runtime manager
    pub const fn new() -> Self {
        Self { runtime: None }
    }

    /// Load a runtime from the given path
    pub fn load<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        if RUNTIME_LOADED.load(Ordering::SeqCst) {
            return Err(LoaderError::AlreadyLoaded);
        }

        let runtime = DynamicPhpRuntime::load(path)?;
        self.runtime = Some(runtime);
        RUNTIME_LOADED.store(true, Ordering::SeqCst);

        Ok(())
    }

    /// Get the loaded runtime
    pub fn runtime(&self) -> Result<&DynamicPhpRuntime> {
        self.runtime.as_ref().ok_or(LoaderError::NotLoaded)
    }

    /// Check if a runtime is loaded
    pub fn is_loaded(&self) -> bool {
        self.runtime.is_some()
    }
}

impl Default for PhpRuntimeManager {
    fn default() -> Self {
        Self::new()
    }
}
