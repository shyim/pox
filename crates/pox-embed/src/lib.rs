//! Safe loader for independently distributed Pox PHP runtimes.
//!
//! PHP and Zend internals live entirely inside `libpox_php.so`. This crate only
//! speaks the versioned, Pox-owned C ABI and exposes owned Rust values.

use libloading::{Library, Symbol};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::{c_void, OsStr};
use std::fmt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::thread::{self, JoinHandle};
use thiserror::Error;

const ABI_MAJOR: u32 = 1;
const ABI_MINOR: u32 = 0;
const STATUS_OK: i32 = 0;

const CLI_EXECUTE_SCRIPT: u32 = 1;
const CLI_EXECUTE_CODE: u32 = 2;
const CLI_LINT: u32 = 3;
const CLI_INFO: u32 = 4;
const CLI_MODULES: u32 = 5;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AbiSlice {
    data: *const u8,
    len: usize,
}

impl AbiSlice {
    fn new(value: &[u8]) -> Self {
        Self {
            data: value.as_ptr(),
            len: value.len(),
        }
    }
}

#[repr(C)]
#[derive(Default)]
struct AbiBuffer {
    data: *mut u8,
    len: usize,
}

#[repr(C)]
struct AbiCliRequest {
    struct_size: u32,
    operation: u32,
    source: AbiSlice,
    arguments: *const AbiSlice,
    argument_count: usize,
    info_flags: i32,
    reserved: [u32; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AbiHttpRequest {
    struct_size: u32,
    reserved0: u32,
    method: AbiSlice,
    uri: AbiSlice,
    query_string: AbiSlice,
    headers: AbiSlice,
    body: AbiSlice,
    document_root: AbiSlice,
    script_filename: AbiSlice,
    server_name: AbiSlice,
    remote_addr: AbiSlice,
    server_port: u16,
    remote_port: u16,
    reserved: [u32; 8],
}

impl Default for AbiHttpRequest {
    fn default() -> Self {
        Self {
            struct_size: std::mem::size_of::<Self>() as u32,
            reserved0: 0,
            method: AbiSlice::default(),
            uri: AbiSlice::default(),
            query_string: AbiSlice::default(),
            headers: AbiSlice::default(),
            body: AbiSlice::default(),
            document_root: AbiSlice::default(),
            script_filename: AbiSlice::default(),
            server_name: AbiSlice::default(),
            remote_addr: AbiSlice::default(),
            server_port: 0,
            remote_port: 0,
            reserved: [0; 8],
        }
    }
}

#[repr(C)]
struct AbiHttpResponse {
    struct_size: u32,
    status: u16,
    reserved0: u16,
    headers: AbiBuffer,
    body: AbiBuffer,
    reserved: [u32; 8],
}

impl Default for AbiHttpResponse {
    fn default() -> Self {
        Self {
            struct_size: std::mem::size_of::<Self>() as u32,
            status: 200,
            reserved0: 0,
            headers: AbiBuffer::default(),
            body: AbiBuffer::default(),
            reserved: [0; 8],
        }
    }
}

type WaitRequestFn = unsafe extern "C" fn(*mut c_void, *mut AbiHttpRequest) -> i32;
type CompleteResponseFn = unsafe extern "C" fn(*mut c_void, *const AbiHttpResponse);

#[repr(C)]
struct AbiWorkerCallbacks {
    struct_size: u32,
    reserved0: u32,
    userdata: *mut c_void,
    wait_request: Option<WaitRequestFn>,
    complete_response: Option<CompleteResponseFn>,
    reserved: [u32; 8],
}

#[repr(C)]
struct AbiApi {
    struct_size: u32,
    abi_major: u16,
    abi_minor: u16,
    feature_flags: u64,
    metadata_json: unsafe extern "C" fn(*mut AbiBuffer) -> i32,
    last_error: unsafe extern "C" fn(*mut AbiBuffer) -> i32,
    free_buffer: unsafe extern "C" fn(*mut AbiBuffer),
    set_ini_entries: unsafe extern "C" fn(AbiSlice) -> i32,
    execute_cli: unsafe extern "C" fn(*const AbiCliRequest, *mut i32) -> i32,
    web_create: unsafe extern "C" fn(*mut *mut c_void) -> i32,
    web_execute: unsafe extern "C" fn(
        *mut c_void,
        *const AbiHttpRequest,
        *mut AbiHttpResponse,
        *mut i32,
    ) -> i32,
    web_destroy: unsafe extern "C" fn(*mut c_void),
    worker_create: unsafe extern "C" fn(*mut *mut c_void) -> i32,
    worker_run: unsafe extern "C" fn(
        *mut c_void,
        AbiSlice,
        AbiSlice,
        *const AbiWorkerCallbacks,
        *mut i32,
    ) -> i32,
    worker_destroy: unsafe extern "C" fn(*mut c_void),
    reserved: [*mut c_void; 16],
}

type GetApiFn = unsafe extern "C" fn(u32, u32) -> *const AbiApi;

#[derive(Debug, Error)]
pub enum PhpError {
    #[error("failed to load PHP runtime {path}: {source}")]
    Load {
        path: PathBuf,
        #[source]
        source: libloading::Error,
    },
    #[error("PHP runtime does not export pox_php_get_api: {0}")]
    MissingEntrypoint(libloading::Error),
    #[error("PHP runtime does not support Pox ABI {major}.{minor}")]
    IncompatibleAbi { major: u32, minor: u32 },
    #[error("PHP runtime returned an invalid ABI table")]
    InvalidApi,
    #[error("PHP runtime metadata is invalid: {0}")]
    InvalidMetadata(#[from] serde_json::Error),
    #[error("PHP runtime target is {actual}, expected {expected}")]
    WrongTarget { expected: String, actual: String },
    #[error("PHP runtime was built without ZTS")]
    ZtsRequired,
    #[error("PHP runtime {loaded} is already active; cannot also load {requested}")]
    DifferentRuntimeLoaded { loaded: PathBuf, requested: PathBuf },
    #[error("PHP runtime operation failed ({status}): {message}")]
    Runtime { status: i32, message: String },
    #[error("PHP worker pool requires at least one worker")]
    NoWorkers,
    #[error("PHP worker stopped before producing a response")]
    WorkerStopped,
}

pub type Result<T> = std::result::Result<T, PhpError>;

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeMetadata {
    pub php_version: String,
    pub php_version_id: i32,
    pub zend_version: String,
    pub zts: bool,
    pub debug: bool,
    pub runtime_revision: String,
    pub target: String,
    pub abi_major: u16,
    pub abi_minor: u16,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub libraries: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct PhpVersion {
    pub version: String,
    pub version_id: i32,
    pub major: i32,
    pub minor: i32,
    pub release: i32,
    pub zend_version: String,
}

impl fmt::Display for PhpVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.version)
    }
}

struct RuntimeInner {
    _library: Library,
    api: NonNull<AbiApi>,
}

type LoadedRuntime = Option<(PathBuf, Weak<RuntimeInner>)>;

static LOADED_RUNTIME: OnceLock<Mutex<LoadedRuntime>> = OnceLock::new();

// The function table is immutable and remains valid while `_library` is held.
// PHP's own mode-specific safety is enforced by the safe handles below.
unsafe impl Send for RuntimeInner {}
unsafe impl Sync for RuntimeInner {}

impl RuntimeInner {
    fn api(&self) -> &AbiApi {
        // SAFETY: `api` was checked for null and the library outlives the table.
        unsafe { self.api.as_ref() }
    }

    fn take_buffer(&self, mut buffer: AbiBuffer) -> Vec<u8> {
        let value = if buffer.data.is_null() || buffer.len == 0 {
            Vec::new()
        } else {
            // SAFETY: the ABI promises a valid buffer until free_buffer.
            unsafe { std::slice::from_raw_parts(buffer.data, buffer.len).to_vec() }
        };
        // SAFETY: the buffer was allocated by this runtime.
        unsafe { (self.api().free_buffer)(&mut buffer) };
        value
    }

    fn error(&self, status: i32) -> PhpError {
        let mut buffer = AbiBuffer::default();
        // SAFETY: output is a valid ABI buffer.
        let error_status = unsafe { (self.api().last_error)(&mut buffer) };
        let message = if error_status == STATUS_OK {
            String::from_utf8_lossy(&self.take_buffer(buffer)).into_owned()
        } else {
            String::new()
        };
        PhpError::Runtime {
            status,
            message: if message.is_empty() {
                "no additional information".to_string()
            } else {
                message
            },
        }
    }

    fn check(&self, status: i32) -> Result<()> {
        if status == STATUS_OK {
            Ok(())
        } else {
            Err(self.error(status))
        }
    }
}

#[derive(Clone)]
pub struct PhpRuntime {
    inner: Arc<RuntimeInner>,
    metadata: Arc<RuntimeMetadata>,
    path: Arc<PathBuf>,
}

impl fmt::Debug for PhpRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhpRuntime")
            .field("path", &self.path)
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl PhpRuntime {
    /// Load and validate an independently installed PHP runtime.
    pub fn load(path: impl AsRef<OsStr>) -> Result<Self> {
        let requested_path = PathBuf::from(path.as_ref());
        let path = requested_path
            .canonicalize()
            .unwrap_or_else(|_| requested_path.clone());
        let registry = LOADED_RUNTIME.get_or_init(|| Mutex::new(None));
        let mut registry = registry.lock().unwrap_or_else(|error| error.into_inner());
        if let Some((loaded_path, weak)) = registry.as_ref() {
            if let Some(inner) = weak.upgrade() {
                if loaded_path != &path {
                    return Err(PhpError::DifferentRuntimeLoaded {
                        loaded: loaded_path.clone(),
                        requested: path,
                    });
                }
                return Self::from_inner(path, inner);
            }
        }
        // SAFETY: library lifetime is retained by RuntimeInner.
        let library = unsafe { Library::new(&path) }.map_err(|source| PhpError::Load {
            path: path.clone(),
            source,
        })?;
        // SAFETY: the symbol type is the stable ABI entrypoint contract.
        let get_api: Symbol<GetApiFn> =
            unsafe { library.get(b"pox_php_get_api\0") }.map_err(PhpError::MissingEntrypoint)?;
        // SAFETY: requesting the supported ABI has no side effects.
        let api_ptr = unsafe { get_api(ABI_MAJOR, ABI_MINOR) };
        let api = NonNull::new(api_ptr.cast_mut()).ok_or(PhpError::IncompatibleAbi {
            major: ABI_MAJOR,
            minor: ABI_MINOR,
        })?;
        // SAFETY: non-null pointer is owned by the loaded library.
        let api_ref = unsafe { api.as_ref() };
        if api_ref.struct_size < std::mem::size_of::<AbiApi>() as u32
            || api_ref.abi_major != ABI_MAJOR as u16
            || api_ref.abi_minor < ABI_MINOR as u16
        {
            return Err(PhpError::InvalidApi);
        }
        let inner = Arc::new(RuntimeInner {
            _library: library,
            api,
        });

        let runtime = Self::from_inner(path.clone(), inner.clone())?;
        *registry = Some((path, Arc::downgrade(&inner)));
        Ok(runtime)
    }

    fn from_inner(path: PathBuf, inner: Arc<RuntimeInner>) -> Result<Self> {
        let mut metadata_buffer = AbiBuffer::default();
        // SAFETY: output points to initialized writable storage.
        let status = unsafe { (inner.api().metadata_json)(&mut metadata_buffer) };
        inner.check(status)?;
        let metadata: RuntimeMetadata =
            serde_json::from_slice(&inner.take_buffer(metadata_buffer))?;
        let expected = runtime_target().to_string();
        if metadata.target != expected {
            return Err(PhpError::WrongTarget {
                expected,
                actual: metadata.target,
            });
        }
        if !metadata.zts {
            return Err(PhpError::ZtsRequired);
        }
        Ok(Self {
            inner,
            metadata: Arc::new(metadata),
            path: Arc::new(path),
        })
    }

    pub fn path(&self) -> &Path {
        self.path.as_ref()
    }

    pub fn metadata(&self) -> &RuntimeMetadata {
        &self.metadata
    }

    pub fn version(&self) -> PhpVersion {
        let id = self.metadata.php_version_id;
        PhpVersion {
            version: self.metadata.php_version.clone(),
            version_id: id,
            major: id / 10_000,
            minor: (id / 100) % 100,
            release: id % 100,
            zend_version: self.metadata.zend_version.clone(),
        }
    }

    pub fn set_ini_entries(&self, entries: Option<&str>) -> Result<()> {
        let slice = AbiSlice::new(entries.unwrap_or_default().as_bytes());
        // SAFETY: input remains valid for the duration of the call.
        let status = unsafe { (self.inner.api().set_ini_entries)(slice) };
        self.inner.check(status)
    }

    fn execute_cli<A: AsRef<str>>(
        &self,
        operation: u32,
        source: &str,
        args: &[A],
        info_flags: i32,
    ) -> Result<i32> {
        let argument_bytes = args
            .iter()
            .map(|argument| argument.as_ref().as_bytes())
            .collect::<Vec<_>>();
        let arguments = argument_bytes
            .iter()
            .map(|argument| AbiSlice::new(argument))
            .collect::<Vec<_>>();
        let request = AbiCliRequest {
            struct_size: std::mem::size_of::<AbiCliRequest>() as u32,
            operation,
            source: AbiSlice::new(source.as_bytes()),
            arguments: arguments.as_ptr(),
            argument_count: arguments.len(),
            info_flags,
            reserved: [0; 8],
        };
        let mut exit_code = 1;
        // SAFETY: all request slices remain valid for the call.
        let status = unsafe { (self.inner.api().execute_cli)(&request, &mut exit_code) };
        self.inner.check(status)?;
        Ok(exit_code)
    }

    pub fn execute_script<A: AsRef<str>>(&self, path: &str, args: &[A]) -> Result<i32> {
        self.execute_cli(CLI_EXECUTE_SCRIPT, path, args, 0)
    }

    pub fn execute_code<A: AsRef<str>>(&self, code: &str, args: &[A]) -> Result<i32> {
        self.execute_cli(CLI_EXECUTE_CODE, code, args, 0)
    }

    pub fn lint<A: AsRef<str>>(&self, path: &str, args: &[A]) -> Result<i32> {
        self.execute_cli(CLI_LINT, path, args, 0)
    }

    pub fn info(&self, flags: Option<i32>) -> Result<i32> {
        self.execute_cli::<&str>(CLI_INFO, "phpinfo", &[], flags.unwrap_or(-1))
    }

    pub fn print_modules(&self) -> Result<i32> {
        self.execute_cli::<&str>(CLI_MODULES, "modules", &[], 0)
    }

    pub fn web(&self) -> Result<WebRuntime> {
        WebRuntime::new(self.clone())
    }

    pub fn workers(
        &self,
        script_filename: &str,
        document_root: &str,
        count: usize,
    ) -> Result<WorkerPool> {
        WorkerPool::new(self.clone(), script_filename, document_root, count)
    }
}

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

struct PreparedRequest<'a> {
    headers: String,
    request: &'a HttpRequest,
}

struct OwnedPreparedRequest {
    headers: String,
    request: HttpRequest,
}

impl OwnedPreparedRequest {
    fn new(request: HttpRequest) -> Self {
        let headers = request
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}\n"))
            .collect();
        Self { headers, request }
    }

    fn abi(&self) -> AbiHttpRequest {
        AbiHttpRequest {
            struct_size: std::mem::size_of::<AbiHttpRequest>() as u32,
            reserved0: 0,
            method: AbiSlice::new(self.request.method.as_bytes()),
            uri: AbiSlice::new(self.request.uri.as_bytes()),
            query_string: AbiSlice::new(self.request.query_string.as_bytes()),
            headers: AbiSlice::new(self.headers.as_bytes()),
            body: AbiSlice::new(&self.request.body),
            document_root: AbiSlice::new(self.request.document_root.as_bytes()),
            script_filename: AbiSlice::new(self.request.script_filename.as_bytes()),
            server_name: AbiSlice::new(self.request.server_name.as_bytes()),
            remote_addr: AbiSlice::new(self.request.remote_addr.as_bytes()),
            server_port: self.request.server_port,
            remote_port: self.request.remote_port,
            reserved: [0; 8],
        }
    }
}

impl<'a> PreparedRequest<'a> {
    fn new(request: &'a HttpRequest) -> Self {
        let headers = request
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}\n"))
            .collect();
        Self { headers, request }
    }

    fn abi(&self) -> AbiHttpRequest {
        AbiHttpRequest {
            struct_size: std::mem::size_of::<AbiHttpRequest>() as u32,
            reserved0: 0,
            method: AbiSlice::new(self.request.method.as_bytes()),
            uri: AbiSlice::new(self.request.uri.as_bytes()),
            query_string: AbiSlice::new(self.request.query_string.as_bytes()),
            headers: AbiSlice::new(self.headers.as_bytes()),
            body: AbiSlice::new(&self.request.body),
            document_root: AbiSlice::new(self.request.document_root.as_bytes()),
            script_filename: AbiSlice::new(self.request.script_filename.as_bytes()),
            server_name: AbiSlice::new(self.request.server_name.as_bytes()),
            remote_addr: AbiSlice::new(self.request.remote_addr.as_bytes()),
            server_port: self.request.server_port,
            remote_port: self.request.remote_port,
            reserved: [0; 8],
        }
    }
}

fn parse_headers(bytes: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            line.split_once(':')
                .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

fn copy_response(response: &AbiHttpResponse) -> HttpResponse {
    let headers = if response.headers.data.is_null() || response.headers.len == 0 {
        Vec::new()
    } else {
        // SAFETY: runtime owns this buffer for the current call/callback.
        parse_headers(unsafe {
            std::slice::from_raw_parts(response.headers.data, response.headers.len)
        })
    };
    let body = if response.body.data.is_null() || response.body.len == 0 {
        Vec::new()
    } else {
        // SAFETY: runtime owns this buffer for the current call/callback.
        unsafe { std::slice::from_raw_parts(response.body.data, response.body.len).to_vec() }
    };
    HttpResponse {
        status: response.status,
        headers,
        body,
    }
}

pub struct WebRuntime {
    runtime: PhpRuntime,
    handle: NonNull<c_void>,
}

unsafe impl Send for WebRuntime {}

impl WebRuntime {
    fn new(runtime: PhpRuntime) -> Result<Self> {
        let mut handle = std::ptr::null_mut();
        // SAFETY: output is valid writable storage.
        let status = unsafe { (runtime.inner.api().web_create)(&mut handle) };
        runtime.inner.check(status)?;
        let handle = NonNull::new(handle).ok_or(PhpError::InvalidApi)?;
        Ok(Self { runtime, handle })
    }

    pub fn execute(&self, request: HttpRequest) -> Result<HttpResponse> {
        let prepared = PreparedRequest::new(&request);
        let abi_request = prepared.abi();
        let mut response = AbiHttpResponse::default();
        let mut exit_code = 1;
        // SAFETY: request inputs and output remain valid during the call.
        let status = unsafe {
            (self.runtime.inner.api().web_execute)(
                self.handle.as_ptr(),
                &abi_request,
                &mut response,
                &mut exit_code,
            )
        };
        self.runtime.inner.check(status)?;
        let value = copy_response(&response);
        // Buffers are transferred to the host for web calls.
        unsafe {
            (self.runtime.inner.api().free_buffer)(&mut response.headers);
            (self.runtime.inner.api().free_buffer)(&mut response.body);
        }
        Ok(value)
    }
}

impl Drop for WebRuntime {
    fn drop(&mut self) {
        // SAFETY: this handle was returned by web_create and is unique here.
        unsafe { (self.runtime.inner.api().web_destroy)(self.handle.as_ptr()) };
    }
}

struct WorkerState {
    request: Mutex<Option<OwnedPreparedRequest>>,
    request_available: Condvar,
    response: Mutex<Option<HttpResponse>>,
    response_ready: Condvar,
    shutdown: AtomicBool,
    processing: AtomicBool,
}

impl WorkerState {
    fn new() -> Self {
        Self {
            request: Mutex::new(None),
            request_available: Condvar::new(),
            response: Mutex::new(None),
            response_ready: Condvar::new(),
            shutdown: AtomicBool::new(false),
            processing: AtomicBool::new(false),
        }
    }
}

unsafe extern "C" fn worker_wait_request(
    userdata: *mut c_void,
    output: *mut AbiHttpRequest,
) -> i32 {
    std::panic::catch_unwind(|| {
        // SAFETY: userdata is an Arc<WorkerState> retained for worker_run.
        let state = unsafe { &*(userdata.cast::<WorkerState>()) };
        let mut request = state
            .request
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while request.is_none() && !state.shutdown.load(Ordering::SeqCst) {
            request = state
                .request_available
                .wait(request)
                .unwrap_or_else(|error| error.into_inner());
        }
        if state.shutdown.load(Ordering::SeqCst) {
            return 0;
        }
        let prepared = request.as_ref().expect("request checked above");
        // The request and serialized headers remain in WorkerState until the
        // matching response callback completes.
        unsafe { *output = prepared.abi() };
        state.processing.store(true, Ordering::SeqCst);
        1
    })
    .unwrap_or(0)
}

unsafe extern "C" fn worker_complete_response(
    userdata: *mut c_void,
    response: *const AbiHttpResponse,
) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: callback arguments are valid for the callback duration.
        let state = unsafe { &*(userdata.cast::<WorkerState>()) };
        let value = copy_response(unsafe { &*response });
        *state
            .response
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(value);
        *state
            .request
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        state.processing.store(false, Ordering::SeqCst);
        state.response_ready.notify_all();
    });
}

struct WorkerRuntimeHandle {
    runtime: PhpRuntime,
    handle: NonNull<c_void>,
}

unsafe impl Send for WorkerRuntimeHandle {}
unsafe impl Sync for WorkerRuntimeHandle {}

impl Drop for WorkerRuntimeHandle {
    fn drop(&mut self) {
        // Worker threads are joined before the last handle is dropped.
        unsafe { (self.runtime.inner.api().worker_destroy)(self.handle.as_ptr()) };
    }
}

struct WorkerThread {
    state: Arc<WorkerState>,
    handle: Option<JoinHandle<()>>,
}

impl WorkerThread {
    fn spawn(runtime: Arc<WorkerRuntimeHandle>, script: String, root: String) -> Self {
        let state = Arc::new(WorkerState::new());
        let thread_state = state.clone();
        let handle = thread::spawn(move || {
            let userdata = Arc::into_raw(thread_state).cast_mut().cast::<c_void>();
            let callbacks = AbiWorkerCallbacks {
                struct_size: std::mem::size_of::<AbiWorkerCallbacks>() as u32,
                reserved0: 0,
                userdata,
                wait_request: Some(worker_wait_request),
                complete_response: Some(worker_complete_response),
                reserved: [0; 8],
            };
            let mut exit_code = 1;
            // SAFETY: callbacks and strings live until worker_run returns.
            let _status = unsafe {
                (runtime.runtime.inner.api().worker_run)(
                    runtime.handle.as_ptr(),
                    AbiSlice::new(script.as_bytes()),
                    AbiSlice::new(root.as_bytes()),
                    &callbacks,
                    &mut exit_code,
                )
            };
            // SAFETY: balances Arc::into_raw above.
            let state = unsafe { Arc::from_raw(userdata.cast::<WorkerState>()) };
            state.shutdown.store(true, Ordering::SeqCst);
            state.request_available.notify_all();
            state.response_ready.notify_all();
        });
        Self {
            state,
            handle: Some(handle),
        }
    }

    fn is_available(&self) -> bool {
        !self.state.processing.load(Ordering::SeqCst)
            && self
                .state
                .request
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_none()
            && !self.state.shutdown.load(Ordering::SeqCst)
    }

    fn submit(&self, request: HttpRequest) -> Result<HttpResponse> {
        *self
            .state
            .response
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        {
            let mut slot = self
                .state
                .request
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            while slot.is_some() && !self.state.shutdown.load(Ordering::SeqCst) {
                drop(slot);
                thread::yield_now();
                slot = self
                    .state
                    .request
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
            }
            if self.state.shutdown.load(Ordering::SeqCst) {
                return Err(PhpError::WorkerStopped);
            }
            *slot = Some(OwnedPreparedRequest::new(request));
            self.state.request_available.notify_one();
        }

        let mut response = self
            .state
            .response
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while response.is_none() && !self.state.shutdown.load(Ordering::SeqCst) {
            response = self
                .state
                .response_ready
                .wait(response)
                .unwrap_or_else(|error| error.into_inner());
        }
        response.take().ok_or(PhpError::WorkerStopped)
    }

    fn shutdown(&self) {
        self.state.shutdown.store(true, Ordering::SeqCst);
        self.state.request_available.notify_all();
        self.state.response_ready.notify_all();
    }

    fn join(mut self) {
        self.shutdown();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub struct WorkerPool {
    runtime: Arc<WorkerRuntimeHandle>,
    workers: Vec<WorkerThread>,
    next_worker: AtomicUsize,
    script_filename: String,
    document_root: String,
    count: usize,
}

impl WorkerPool {
    fn new(
        runtime: PhpRuntime,
        script_filename: &str,
        document_root: &str,
        count: usize,
    ) -> Result<Self> {
        if count == 0 {
            return Err(PhpError::NoWorkers);
        }
        let mut handle = std::ptr::null_mut();
        // SAFETY: output points to valid writable storage.
        let status = unsafe { (runtime.inner.api().worker_create)(&mut handle) };
        runtime.inner.check(status)?;
        let handle = NonNull::new(handle).ok_or(PhpError::InvalidApi)?;
        let runtime = Arc::new(WorkerRuntimeHandle { runtime, handle });
        let mut pool = Self {
            runtime,
            workers: Vec::new(),
            next_worker: AtomicUsize::new(0),
            script_filename: script_filename.to_string(),
            document_root: document_root.to_string(),
            count,
        };
        pool.start_workers();
        Ok(pool)
    }

    fn start_workers(&mut self) {
        for _ in 0..self.count {
            self.workers.push(WorkerThread::spawn(
                self.runtime.clone(),
                self.script_filename.clone(),
                self.document_root.clone(),
            ));
        }
    }

    pub fn restart(&mut self) {
        for worker in self.workers.drain(..) {
            worker.join();
        }
        self.start_workers();
    }

    pub fn handle_request(&self, request: HttpRequest) -> Result<HttpResponse> {
        let start = self.next_worker.fetch_add(1, Ordering::SeqCst) % self.workers.len();
        for offset in 0..self.workers.len() {
            let index = (start + offset) % self.workers.len();
            if self.workers[index].is_available() {
                return self.workers[index].submit(request);
            }
        }
        self.workers[start].submit(request)
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        for worker in self.workers.drain(..) {
            worker.join();
        }
    }
}

pub const fn runtime_target() -> &'static str {
    if cfg!(all(target_arch = "x86_64", target_env = "musl")) {
        "x86_64-unknown-linux-musl"
    } else if cfg!(all(target_arch = "aarch64", target_env = "musl")) {
        "aarch64-unknown-linux-musl"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64-unknown-linux-gnu"
    } else {
        "unsupported"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_php_version_components() {
        let id = 80509;
        assert_eq!((id / 10_000, (id / 100) % 100, id % 100), (8, 5, 9));
    }

    #[test]
    fn target_is_supported_in_ci() {
        assert_ne!(runtime_target(), "unsupported");
    }
}
