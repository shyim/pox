//! Safe loader for independently distributed Pox PHP runtimes.
//!
//! PHP and Zend internals live entirely inside the platform runtime library.
//! This crate only speaks the versioned, Pox-owned C ABI and exposes owned Rust
//! values.

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
use std::time::{Duration, Instant};
use thiserror::Error;

const ABI_MAJOR: u32 = 1;
const ABI_MINOR: u32 = 0;
const STATUS_OK: i32 = 0;
const FEATURE_RESPONSE_LIMITS: u64 = 1;
const FEATURE_PARALLEL_WEB: u64 = 2;
const FEATURE_HTTP_PROTOCOL: u64 = 4;
const FEATURE_REQUEST_SCHEME: u64 = 8;
const FEATURE_WEB_THREADS: u64 = 16;
const FEATURE_CANCELLATION: u64 = 32;
const FEATURE_RESPONSE_OUTPUT: u64 = 64;
const HTTP_RESPONSE_OUTPUT: u32 = 16;
const RESPONSE_OUTPUT_FAILED: u16 = 4;
const HTTP_CANCELLATION: u32 = 8;
const RESPONSE_CANCELLED: u16 = 2;
const HTTP_SECURE: u32 = 4;
const HTTP_PROTOCOL: u32 = 2;
const HTTP_RESPONSE_LIMITS: u32 = 1;
const RESPONSE_BUFFER_FAILED: u16 = 1;

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
    web_thread_enter: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
    web_thread_leave: Option<unsafe extern "C" fn(*mut c_void)>,
    cancellation_create: Option<unsafe extern "C" fn(*mut *mut c_void) -> i32>,
    cancellation_request: Option<unsafe extern "C" fn(*mut c_void)>,
    cancellation_release: Option<unsafe extern "C" fn(*mut c_void)>,
    reserved: [*mut c_void; 11],
}

type GetApiFn = unsafe extern "C" fn(u32, u32) -> *const AbiApi;

#[derive(Debug, Error)]
pub enum PhpError {
    #[error("runtime does not support response output callbacks")]
    ResponseOutputUnsupported,
    #[error("response output handle was already used")]
    ResponseOutputUsed,
    #[error("PHP response output failed during execution or delivery")]
    ResponseOutputFailed,
    #[error("runtime does not support request cancellation")]
    CancellationUnsupported,
    #[error("PHP request cancellation handle was already used")]
    CancellationUsed,
    #[error("PHP request was cancelled by its host")]
    RequestCancelled,
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
    #[error("PHP runtime is already in use by another execution mode or configuration operation")]
    RuntimeBusy,
    #[error("PHP worker stopped before producing a response")]
    WorkerStopped,
    #[error("No PHP worker is currently available")]
    WorkersUnavailable,
    #[error("PHP worker did not become ready before the startup deadline")]
    WorkerStartupTimeout,
    #[error("PHP runtime does not support native response limits (ABI 1.1 required)")]
    ResponseLimitsUnsupported,
    #[error("runtime does not support parallel web execution")]
    ParallelWebUnsupported,
    #[error("runtime does not support reusable web threads")]
    WebThreadsUnsupported,
    #[error("runtime does not support HTTP/1.0 request metadata")]
    HttpProtocolUnsupported,
    #[error("runtime does not support HTTPS request metadata")]
    RequestSchemeUnsupported,
    #[error("PHP response buffering failed or exceeded the configured native limit")]
    ResponseBufferFailed,
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
    active: AtomicBool,
    metadata: OnceLock<Arc<RuntimeMetadata>>,
    _library: Library,
    api: NonNull<AbiApi>,
}

type LoadedRuntime = Option<(PathBuf, Weak<RuntimeInner>)>;

static LOADED_RUNTIME: OnceLock<Mutex<LoadedRuntime>> = OnceLock::new();

// The function table is immutable and remains valid while `_library` is held.
// PHP's own mode-specific safety is enforced by the safe handles below.
unsafe impl Send for RuntimeInner {}
unsafe impl Sync for RuntimeInner {}

// PHP module lifecycle and INI storage are process-wide even in ZTS builds.
// The registry shares this lease across clones and loads of the same library.
struct RuntimeLease {
    inner: Arc<RuntimeInner>,
}

impl RuntimeLease {
    fn acquire(inner: &Arc<RuntimeInner>) -> Result<Self> {
        inner
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| PhpError::RuntimeBusy)?;
        Ok(Self {
            inner: inner.clone(),
        })
    }
}

impl Drop for RuntimeLease {
    fn drop(&mut self) {
        self.inner.active.store(false, Ordering::Release);
    }
}

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
    pub fn supports_response_output(&self) -> bool {
        self.inner.api().feature_flags & FEATURE_RESPONSE_OUTPUT != 0
    }

    pub fn supports_cancellation(&self) -> bool {
        let api = self.inner.api();
        api.feature_flags & FEATURE_CANCELLATION != 0
            && api.cancellation_create.is_some()
            && api.cancellation_request.is_some()
            && api.cancellation_release.is_some()
    }

    /// Allocate a single-use request cancellation handle. Cancellation requests
    /// are thread-safe, but only take effect when PHP reaches a VM boundary.
    pub fn cancellation(&self) -> Result<RequestCancellation> {
        let api = self.inner.api();
        if api.feature_flags & FEATURE_CANCELLATION == 0
            || api.cancellation_request.is_none()
            || api.cancellation_release.is_none()
        {
            return Err(PhpError::CancellationUnsupported);
        }
        let create = api
            .cancellation_create
            .ok_or(PhpError::CancellationUnsupported)?;
        let mut handle = std::ptr::null_mut();
        // SAFETY: output is writable and the optional ABI feature was checked.
        self.inner.check(unsafe { create(&mut handle) })?;
        Ok(RequestCancellation(Arc::new(CancellationInner {
            runtime: self.clone(),
            handle: NonNull::new(handle).ok_or(PhpError::InvalidApi)?,
            claimed: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
        })))
    }

    pub fn supports_request_scheme(&self) -> bool {
        self.inner.api().feature_flags & FEATURE_REQUEST_SCHEME != 0
    }

    pub fn supports_http_protocol(&self) -> bool {
        self.inner.api().feature_flags & FEATURE_HTTP_PROTOCOL != 0
    }

    pub fn supports_response_limits(&self) -> bool {
        self.inner.api().abi_minor >= 1
            && self.inner.api().feature_flags & FEATURE_RESPONSE_LIMITS != 0
    }

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
            active: AtomicBool::new(false),
            metadata: OnceLock::new(),
            _library: library,
            api,
        });

        let runtime = Self::from_inner(path.clone(), inner.clone())?;
        *registry = Some((path, Arc::downgrade(&inner)));
        Ok(runtime)
    }

    fn from_inner(path: PathBuf, inner: Arc<RuntimeInner>) -> Result<Self> {
        if let Some(metadata) = inner.metadata.get() {
            return Ok(Self {
                metadata: metadata.clone(),
                inner,
                path: Arc::new(path),
            });
        }
        // Metadata discovery may initialize embedded PHP. The loaded-library
        // registry serializes first discovery; cache it before publishing handles.
        let _lease = RuntimeLease::acquire(&inner)?;
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
        let metadata = Arc::new(metadata);
        let _ = inner.metadata.set(metadata.clone());
        Ok(Self {
            inner,
            metadata,
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

    /// Configure PHP before starting execution; returns RuntimeBusy while a mode owns PHP.
    pub fn set_ini_entries(&self, entries: Option<&str>) -> Result<()> {
        let _lease = RuntimeLease::acquire(&self.inner)?;
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
        let _lease = RuntimeLease::acquire(&self.inner)?;
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

    /// Acquire exclusive ownership of PHP module lifecycle until this web owner is dropped.
    pub fn web(&self) -> Result<WebRuntime> {
        WebRuntime::new(self.clone())
    }

    /// Acquire PHP module lifecycle ownership for this pool and all of its worker threads.
    pub fn workers(
        &self,
        script_filename: &str,
        document_root: &str,
        count: usize,
    ) -> Result<WorkerPool> {
        WorkerPool::new(self.clone(), script_filename, document_root, count)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HttpProtocol {
    Http10,
    #[default]
    Http11,
}

/// Synchronous PHP-thread output sink. Returning false aborts output. A sink
/// must bound any buffering/backpressure and arrange to unblock on cancellation.
/// The caller must check the final execution Result before treating the stream
/// as complete: headers/chunks may precede a later fatal, limit or sink failure.
pub trait HttpOutput: Send + Sync + 'static {
    fn start(&self, status: u16, headers: Vec<(String, String)>) -> bool;
    fn write(&self, chunk: &[u8]) -> bool;
    fn flush(&self) -> bool;
}

#[repr(C)]
struct AbiOutputCallbacks {
    struct_size: u32,
    reserved0: u32,
    userdata: *mut c_void,
    start: unsafe extern "C" fn(*mut c_void, u16, AbiSlice) -> i32,
    write: unsafe extern "C" fn(*mut c_void, AbiSlice) -> i32,
    flush: unsafe extern "C" fn(*mut c_void) -> i32,
}

struct OutputState {
    sink: Box<dyn HttpOutput>,
    failed: AtomicBool,
    claimed: AtomicBool,
}

struct OutputInner {
    callbacks: AbiOutputCallbacks,
    state: Box<OutputState>,
}

// The userdata pointer targets the stable boxed state. The ABI invokes it only
// during the request, while HttpRequest retains this Arc; sink state is Sync.
unsafe impl Send for OutputInner {}
unsafe impl Sync for OutputInner {}

#[derive(Clone)]
pub struct ResponseOutput(Arc<OutputInner>);

impl fmt::Debug for ResponseOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResponseOutput")
            .finish_non_exhaustive()
    }
}

impl ResponseOutput {
    pub fn new(sink: impl HttpOutput) -> Self {
        let state = Box::new(OutputState {
            sink: Box::new(sink),
            failed: AtomicBool::new(false),
            claimed: AtomicBool::new(false),
        });
        let callbacks = AbiOutputCallbacks {
            struct_size: std::mem::size_of::<AbiOutputCallbacks>() as u32,
            reserved0: 0,
            userdata: (&*state as *const OutputState).cast_mut().cast(),
            start: output_start,
            write: output_write,
            flush: output_flush,
        };
        Self(Arc::new(OutputInner { callbacks, state }))
    }

    fn claim(&self, runtime: &PhpRuntime) -> Result<()> {
        if !runtime.supports_response_output() {
            return Err(PhpError::ResponseOutputUnsupported);
        }
        if self.0.state.claimed.swap(true, Ordering::AcqRel) {
            return Err(PhpError::ResponseOutputUsed);
        }
        Ok(())
    }

    fn apply(&self, request: &mut AbiHttpRequest) {
        let address = &self.0.callbacks as *const AbiOutputCallbacks as usize as u64;
        request.reserved0 |= HTTP_RESPONSE_OUTPUT;
        request.reserved[5] = address as u32;
        request.reserved[6] = (address >> 32) as u32;
    }

    fn failed(&self) -> bool {
        self.0.state.failed.load(Ordering::Acquire)
    }
}

unsafe fn output_event(userdata: *mut c_void, invoke: impl FnOnce(&dyn HttpOutput) -> bool) -> i32 {
    // SAFETY: native code borrows this stable boxed state only during execution.
    let state = unsafe { &*userdata.cast::<OutputState>() };
    let accepted =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| invoke(state.sink.as_ref())))
            .unwrap_or(false);
    if !accepted {
        state.failed.store(true, Ordering::Release);
    }
    i32::from(accepted)
}

unsafe fn output_bytes<'a>(slice: AbiSlice) -> &'a [u8] {
    if slice.len == 0 {
        &[]
    } else {
        // SAFETY: native callback slices are valid for the callback duration.
        unsafe { std::slice::from_raw_parts(slice.data, slice.len) }
    }
}

unsafe extern "C" fn output_start(userdata: *mut c_void, status: u16, headers: AbiSlice) -> i32 {
    unsafe {
        output_event(userdata, |sink| {
            sink.start(status, parse_headers(output_bytes(headers)))
        })
    }
}

unsafe extern "C" fn output_write(userdata: *mut c_void, chunk: AbiSlice) -> i32 {
    unsafe { output_event(userdata, |sink| sink.write(output_bytes(chunk))) }
}

unsafe extern "C" fn output_flush(userdata: *mut c_void) -> i32 {
    unsafe { output_event(userdata, |sink| sink.flush()) }
}

#[derive(Debug)]
struct CancellationInner {
    runtime: PhpRuntime,
    handle: NonNull<c_void>,
    claimed: AtomicBool,
    cancelled: AtomicBool,
}

// The native handle serializes cancellation and detachment; no PHP globals are
// accessed after detachment. The retained runtime keeps its code loaded.
unsafe impl Send for CancellationInner {}
unsafe impl Sync for CancellationInner {}

impl Drop for CancellationInner {
    fn drop(&mut self) {
        // SAFETY: this Arc owns the host's reference to a validated native handle.
        unsafe { (self.runtime.inner.api().cancellation_release.unwrap())(self.handle.as_ptr()) };
    }
}

/// A single-use cancellation handle. Clones refer to the same request; cancel()
/// after completion cannot interrupt a later request on the reused PHP thread.
#[derive(Debug, Clone)]
pub struct RequestCancellation(Arc<CancellationInner>);

impl RequestCancellation {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        // SAFETY: Arc retains the native control while its mutex fences PHP teardown.
        unsafe {
            (self.0.runtime.inner.api().cancellation_request.unwrap())(self.0.handle.as_ptr())
        };
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    fn claim(&self, runtime: &PhpRuntime) -> Result<()> {
        if !std::ptr::eq(self.0.runtime.inner.api(), runtime.inner.api()) {
            return Err(PhpError::InvalidApi);
        }
        if self.0.claimed.swap(true, Ordering::AcqRel) {
            return Err(PhpError::CancellationUsed);
        }
        if self.is_cancelled() {
            return Err(PhpError::RequestCancelled);
        }
        Ok(())
    }

    fn apply(&self, request: &mut AbiHttpRequest) {
        let address = self.0.handle.as_ptr() as usize as u64;
        request.reserved0 |= HTTP_CANCELLATION;
        request.reserved[3] = address as u32;
        request.reserved[4] = (address >> 32) as u32;
    }
}

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub output: Option<ResponseOutput>,
    pub cancellation: Option<RequestCancellation>,
    /// HTTPS established by the host transport or a validated trusted proxy.
    pub secure: bool,
    pub protocol: HttpProtocol,
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

/// Native buffer allocation bounds (ABI 1.1). Zero prohibits output in that buffer.
#[derive(Debug, Clone, Copy)]
pub struct ResponseLimits {
    pub body_bytes: u32,
    pub header_bytes: u32,
}

fn apply_response_limits(request: &mut AbiHttpRequest, limits: Option<ResponseLimits>) {
    if let Some(limits) = limits {
        request.reserved0 |= HTTP_RESPONSE_LIMITS;
        request.reserved[0] = limits.body_bytes;
        request.reserved[1] = limits.header_bytes;
    }
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
    limits: Option<ResponseLimits>,
}

impl OwnedPreparedRequest {
    fn new(request: HttpRequest, limits: Option<ResponseLimits>) -> Self {
        let headers = request
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}\n"))
            .collect();
        Self {
            headers,
            request,
            limits,
        }
    }

    fn abi(&self) -> AbiHttpRequest {
        let mut request = AbiHttpRequest {
            struct_size: std::mem::size_of::<AbiHttpRequest>() as u32,
            reserved0: if self.request.protocol == HttpProtocol::Http10 {
                HTTP_PROTOCOL
            } else {
                0
            } | if self.request.secure { HTTP_SECURE } else { 0 },
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
            reserved: [0, 0, 1000, 0, 0, 0, 0, 0],
        };
        if let Some(output) = &self.request.output {
            output.apply(&mut request);
        }
        if let Some(control) = &self.request.cancellation {
            control.apply(&mut request);
        }
        apply_response_limits(&mut request, self.limits);
        request
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
        let mut request = AbiHttpRequest {
            struct_size: std::mem::size_of::<AbiHttpRequest>() as u32,
            reserved0: if self.request.protocol == HttpProtocol::Http10 {
                HTTP_PROTOCOL
            } else {
                0
            } | if self.request.secure { HTTP_SECURE } else { 0 },
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
            reserved: [0, 0, 1000, 0, 0, 0, 0, 0],
        };
        if let Some(output) = &self.request.output {
            output.apply(&mut request);
        }
        if let Some(control) = &self.request.cancellation {
            control.apply(&mut request);
        }
        request
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

fn copy_response(response: &AbiHttpResponse) -> Result<HttpResponse> {
    if response.reserved0 & RESPONSE_OUTPUT_FAILED != 0 {
        return Err(PhpError::ResponseOutputFailed);
    }
    if response.reserved0 & RESPONSE_CANCELLED != 0 {
        return Err(PhpError::RequestCancelled);
    }
    if response.reserved0 & RESPONSE_BUFFER_FAILED != 0 {
        return Err(PhpError::ResponseBufferFailed);
    }
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
    Ok(HttpResponse {
        status: response.status,
        headers,
        body,
    })
}

pub struct WebRuntime {
    _lease: RuntimeLease,
    runtime: PhpRuntime,
    handle: NonNull<c_void>,
}

// PHP initialization and shutdown must remain on the same thread.
// Only the capability-checked borrowed executor may cross threads.
pub struct ParallelWebExecutor<'a> {
    web: &'a WebRuntime,
}

// SAFETY: constructed only when the runtime advertises isolated ZTS execution;
// the borrow keeps the runtime alive until all scoped execution has completed.
unsafe impl Send for ParallelWebExecutor<'_> {}
unsafe impl Sync for ParallelWebExecutor<'_> {}

impl<'a> ParallelWebExecutor<'a> {
    /// Attach reusable PHP resources to this thread until the returned guard is dropped.
    pub fn attach(&self) -> Result<WebThread<'a>> {
        let api = self.web.runtime.inner.api();
        if api.feature_flags & FEATURE_WEB_THREADS == 0 || api.web_thread_leave.is_none() {
            return Err(PhpError::WebThreadsUnsupported);
        }
        let enter = api
            .web_thread_enter
            .ok_or(PhpError::WebThreadsUnsupported)?;
        // SAFETY: the borrowed owner outlives this thread-affine attachment.
        let status = unsafe { enter(self.web.handle.as_ptr()) };
        self.web.runtime.inner.check(status)?;
        Ok(WebThread { web: self.web })
    }

    pub fn execute_with_limits(
        &self,
        request: HttpRequest,
        limits: ResponseLimits,
    ) -> Result<HttpResponse> {
        self.web.execute_with_limits(request, limits)
    }
}

/// Thread-affine PHP resources; the borrowed WebRuntime makes this guard !Send/!Sync.
pub struct WebThread<'a> {
    web: &'a WebRuntime,
}

impl WebThread<'_> {
    pub fn execute_with_limits(
        &self,
        request: HttpRequest,
        limits: ResponseLimits,
    ) -> Result<HttpResponse> {
        self.web.execute_with_limits(request, limits)
    }
}

impl Drop for WebThread<'_> {
    fn drop(&mut self) {
        // SAFETY: successful attachment checked this function; the guard cannot
        // move between threads and no synchronous execution call remains active.
        unsafe {
            (self.web.runtime.inner.api().web_thread_leave.unwrap())(self.web.handle.as_ptr())
        };
    }
}

impl WebRuntime {
    pub fn parallel_executor(&self) -> Result<ParallelWebExecutor<'_>> {
        if self.runtime.inner.api().feature_flags & FEATURE_PARALLEL_WEB == 0 {
            return Err(PhpError::ParallelWebUnsupported);
        }
        Ok(ParallelWebExecutor { web: self })
    }

    fn new(runtime: PhpRuntime) -> Result<Self> {
        let lease = RuntimeLease::acquire(&runtime.inner)?;
        let mut handle = std::ptr::null_mut();
        // SAFETY: output is valid writable storage.
        let status = unsafe { (runtime.inner.api().web_create)(&mut handle) };
        runtime.inner.check(status)?;
        let handle = NonNull::new(handle).ok_or(PhpError::InvalidApi)?;
        Ok(Self {
            runtime,
            handle,
            _lease: lease,
        })
    }

    pub fn execute(&self, request: HttpRequest) -> Result<HttpResponse> {
        self.execute_inner(request, None)
    }

    pub fn execute_with_limits(
        &self,
        request: HttpRequest,
        limits: ResponseLimits,
    ) -> Result<HttpResponse> {
        if !self.runtime.supports_response_limits() {
            return Err(PhpError::ResponseLimitsUnsupported);
        }
        self.execute_inner(request, Some(limits))
    }

    fn execute_inner(
        &self,
        request: HttpRequest,
        limits: Option<ResponseLimits>,
    ) -> Result<HttpResponse> {
        if request.protocol == HttpProtocol::Http10 && !self.runtime.supports_http_protocol() {
            return Err(PhpError::HttpProtocolUnsupported);
        }
        if request.secure && !self.runtime.supports_request_scheme() {
            return Err(PhpError::RequestSchemeUnsupported);
        }
        if let Some(output) = &request.output {
            output.claim(&self.runtime)?;
        }
        if let Some(control) = &request.cancellation {
            control.claim(&self.runtime)?;
        }
        let prepared = PreparedRequest::new(&request);
        let mut abi_request = prepared.abi();
        apply_response_limits(&mut abi_request, limits);
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
        value
    }
}

impl Drop for WebRuntime {
    fn drop(&mut self) {
        // SAFETY: this handle was returned by web_create and is unique here.
        unsafe { (self.runtime.inner.api().web_destroy)(self.handle.as_ptr()) };
    }
}

struct WorkerState {
    ready_total: Arc<AtomicUsize>,
    ready: AtomicBool,
    completed: AtomicUsize,
    max_requests: AtomicUsize,
    request: Mutex<Option<OwnedPreparedRequest>>,
    request_available: Condvar,
    response: Mutex<Option<Result<HttpResponse>>>,
    response_ready: Condvar,
    shutdown: AtomicBool,
}

impl WorkerState {
    fn new(ready_total: Arc<AtomicUsize>) -> Self {
        Self {
            ready_total,
            ready: AtomicBool::new(false),
            completed: AtomicUsize::new(0),
            max_requests: AtomicUsize::new(0),
            request: Mutex::new(None),
            request_available: Condvar::new(),
            response: Mutex::new(None),
            response_ready: Condvar::new(),
            shutdown: AtomicBool::new(false),
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
        if !state.ready.swap(true, Ordering::SeqCst) {
            state.ready_total.fetch_add(1, Ordering::SeqCst);
            state.request_available.notify_all();
        }
        let max_requests = state.max_requests.load(Ordering::SeqCst);
        if max_requests != 0 && state.completed.load(Ordering::SeqCst) >= max_requests {
            return 0;
        }
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
            .request
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        state.completed.fetch_add(1, Ordering::SeqCst);
        *state
            .response
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(value);
        state.response_ready.notify_all();
    });
}

struct WorkerRuntimeHandle {
    ready_total: Arc<AtomicUsize>,
    _lease: RuntimeLease,
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
    retry_at: Option<Instant>,
    failures: u32,
    state: Arc<WorkerState>,
    handle: Option<JoinHandle<()>>,
}

impl WorkerThread {
    fn spawn(runtime: Arc<WorkerRuntimeHandle>, script: String, root: String) -> Self {
        let state = Arc::new(WorkerState::new(runtime.ready_total.clone()));
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
            if state.ready.swap(false, Ordering::SeqCst) {
                state.ready_total.fetch_sub(1, Ordering::SeqCst);
            }
            let mut request = state
                .request
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            // The native call has returned and released its copy. Release the
            // host request before waking its caller, not after restart backoff.
            *request = None;
            state.request_available.notify_all();
            let _response = state
                .response
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.response_ready.notify_all();
        });
        Self {
            retry_at: None,
            failures: 0,
            state,
            handle: Some(handle),
        }
    }

    fn accepting(&self) -> bool {
        let max = self.state.max_requests.load(Ordering::SeqCst);
        !self.state.shutdown.load(Ordering::SeqCst)
            && (max == 0 || self.state.completed.load(Ordering::SeqCst) < max)
    }

    fn wait_ready(&self, deadline: Instant) -> Result<()> {
        let mut request = self.state.request.lock().unwrap_or_else(|e| e.into_inner());
        while !self.state.ready.load(Ordering::SeqCst) {
            if self.state.shutdown.load(Ordering::SeqCst) {
                return Err(PhpError::WorkerStopped);
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(PhpError::WorkerStartupTimeout)?;
            request = self
                .state
                .request_available
                .wait_timeout(request, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if self.state.shutdown.load(Ordering::SeqCst) {
            return Err(PhpError::WorkerStopped);
        }
        Ok(())
    }

    fn submit(&self, request: HttpRequest, limits: Option<ResponseLimits>) -> Result<HttpResponse> {
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
            debug_assert!(
                slot.is_none(),
                "worker request reservation must be exclusive"
            );
            if self.state.shutdown.load(Ordering::SeqCst) {
                return Err(PhpError::WorkerStopped);
            }
            *slot = Some(OwnedPreparedRequest::new(request, limits));
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
        response.take().unwrap_or(Err(PhpError::WorkerStopped))
    }

    fn shutdown(&self) {
        self.state.shutdown.store(true, Ordering::SeqCst);
        let _request = self
            .state
            .request
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.state.request_available.notify_all();
        let _response = self
            .state
            .response
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.state.response_ready.notify_all();
    }

    fn join(mut self) {
        self.shutdown();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Read-only worker initialization count, independent of PHP request locks and
/// the lifetime of the pool's native runtime. Includes busy initialized workers.
#[derive(Clone)]
pub struct WorkerReadiness(Arc<AtomicUsize>);

impl WorkerReadiness {
    pub fn ready_count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

pub struct WorkerPool {
    runtime: Arc<WorkerRuntimeHandle>,
    workers: Vec<Mutex<WorkerThread>>,
    next_worker: AtomicUsize,
    generation: Mutex<u64>,
    max_requests: AtomicUsize,
    available: Condvar,
    script_filename: String,
    document_root: String,
    count: usize,
}

impl WorkerPool {
    pub fn readiness(&self) -> WorkerReadiness {
        WorkerReadiness(self.runtime.ready_total.clone())
    }

    /// Initialized worker incarnations, including busy workers, without taking request locks.
    pub fn ready_count(&self) -> usize {
        self.runtime.ready_total.load(Ordering::SeqCst)
    }

    fn new(
        runtime: PhpRuntime,
        script_filename: &str,
        document_root: &str,
        count: usize,
    ) -> Result<Self> {
        if count == 0 {
            return Err(PhpError::NoWorkers);
        }
        let lease = RuntimeLease::acquire(&runtime.inner)?;
        let mut handle = std::ptr::null_mut();
        // SAFETY: output points to valid writable storage.
        let status = unsafe { (runtime.inner.api().worker_create)(&mut handle) };
        runtime.inner.check(status)?;
        let handle = NonNull::new(handle).ok_or(PhpError::InvalidApi)?;
        let runtime = Arc::new(WorkerRuntimeHandle {
            ready_total: Arc::new(AtomicUsize::new(0)),
            runtime,
            handle,
            _lease: lease,
        });
        let mut pool = Self {
            runtime,
            workers: Vec::new(),
            next_worker: AtomicUsize::new(0),
            generation: Mutex::new(0),
            max_requests: AtomicUsize::new(0),
            available: Condvar::new(),
            script_filename: script_filename.to_string(),
            document_root: document_root.to_string(),
            count,
        };
        pool.start_workers();
        Ok(pool)
    }

    fn start_workers(&mut self) {
        for _ in 0..self.count {
            let worker = WorkerThread::spawn(
                self.runtime.clone(),
                self.script_filename.clone(),
                self.document_root.clone(),
            );
            worker
                .state
                .max_requests
                .store(self.max_requests.load(Ordering::SeqCst), Ordering::SeqCst);
            self.workers.push(Mutex::new(worker));
        }
    }

    pub fn wait_ready(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        for worker in &self.workers {
            worker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .wait_ready(deadline)?;
        }
        Ok(())
    }

    pub fn set_max_requests(&mut self, maximum: usize) {
        self.max_requests.store(maximum, Ordering::SeqCst);
        for worker in &self.workers {
            worker
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .state
                .max_requests
                .store(maximum, Ordering::SeqCst);
        }
    }

    /// Replace only terminated worker threads. Active workers are never joined
    /// here, and a request whose worker failed is never replayed.
    pub fn repair_stopped(&self) -> usize {
        let mut repaired = 0;
        for slot in &self.workers {
            let Some(mut worker) = try_worker(slot) else {
                continue;
            };
            if !worker.handle.as_ref().is_some_and(JoinHandle::is_finished) {
                continue;
            }
            let max = worker.state.max_requests.load(Ordering::SeqCst);
            let completed = worker.state.completed.load(Ordering::SeqCst);
            if worker.retry_at.is_none() {
                let planned = max != 0 && completed >= max;
                worker.failures = if completed > 0 {
                    1
                } else {
                    worker.failures.saturating_add(1)
                };
                let delay = if planned {
                    0
                } else {
                    (100u64 << worker.failures.saturating_sub(1).min(6)).min(5000)
                };
                worker.retry_at = Some(Instant::now() + Duration::from_millis(delay));
            }
            if worker
                .retry_at
                .is_some_and(|deadline| Instant::now() < deadline)
            {
                continue;
            }
            let mut replacement = WorkerThread::spawn(
                self.runtime.clone(),
                self.script_filename.clone(),
                self.document_root.clone(),
            );
            replacement.failures = worker.failures;
            replacement
                .state
                .max_requests
                .store(self.max_requests.load(Ordering::SeqCst), Ordering::SeqCst);
            let old = std::mem::replace(&mut *worker, replacement);
            old.join();
            repaired += 1;
        }
        // Scanners can briefly observe a slot locked by maintenance even when
        // no replacement is needed. Wake them after releasing those locks too.
        self.changed();
        repaired
    }

    fn changed(&self) {
        let mut generation = self.generation.lock().unwrap_or_else(|e| e.into_inner());
        *generation = generation.wrapping_add(1);
        self.available.notify_all();
    }

    pub fn restart(&mut self) {
        for worker in &mut self.workers {
            worker
                .get_mut()
                .unwrap_or_else(|e| e.into_inner())
                .shutdown();
        }
        for worker in self.workers.drain(..) {
            worker
                .into_inner()
                .unwrap_or_else(|e| e.into_inner())
                .join();
        }
        self.start_workers();
    }

    pub fn handle_request(&self, request: HttpRequest) -> Result<HttpResponse> {
        self.handle_request_inner(request, None, None)
    }

    pub fn handle_request_with_limits(
        &self,
        request: HttpRequest,
        limits: ResponseLimits,
    ) -> Result<HttpResponse> {
        if !self.runtime.runtime.supports_response_limits() {
            return Err(PhpError::ResponseLimitsUnsupported);
        }
        self.handle_request_inner(request, Some(limits), None)
    }

    /// Wait through planned recycling within the caller's original queue deadline.
    /// Maintenance must run independently. Cancellation is checked before dispatch;
    /// an executing request is never replayed or interrupted by this method.
    pub fn handle_request_with_limits_queued(
        &self,
        request: HttpRequest,
        limits: ResponseLimits,
        deadline: Instant,
        cancelled: impl Fn() -> bool,
    ) -> Result<HttpResponse> {
        if !self.runtime.runtime.supports_response_limits() {
            return Err(PhpError::ResponseLimitsUnsupported);
        }
        self.handle_request_inner(request, Some(limits), Some((deadline, &cancelled)))
    }

    fn handle_request_inner(
        &self,
        request: HttpRequest,
        limits: Option<ResponseLimits>,
        queue: Option<(Instant, &dyn Fn() -> bool)>,
    ) -> Result<HttpResponse> {
        if request.protocol == HttpProtocol::Http10
            && !self.runtime.runtime.supports_http_protocol()
        {
            return Err(PhpError::HttpProtocolUnsupported);
        }
        if request.secure && !self.runtime.runtime.supports_request_scheme() {
            return Err(PhpError::RequestSchemeUnsupported);
        }
        if let Some(output) = &request.output {
            output.claim(&self.runtime.runtime)?;
        }
        let output = request.output.clone();
        if let Some(control) = &request.cancellation {
            control.claim(&self.runtime.runtime)?;
        }
        let control = request.cancellation.clone();
        let start = self.next_worker.fetch_add(1, Ordering::SeqCst) % self.workers.len();
        let mut generation = self.generation.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if control
                .as_ref()
                .is_some_and(RequestCancellation::is_cancelled)
            {
                return Err(PhpError::RequestCancelled);
            }
            if queue.is_some_and(|(deadline, cancelled)| Instant::now() >= deadline || cancelled())
            {
                return Err(PhpError::WorkersUnavailable);
            }
            let mut busy = false;
            for offset in 0..self.workers.len() {
                let index = (start + offset) % self.workers.len();
                let Some(worker) = try_worker(&self.workers[index]) else {
                    busy = true;
                    continue;
                };
                if !worker.accepting() {
                    let maximum = worker.state.max_requests.load(Ordering::SeqCst);
                    if queue.is_some()
                        && maximum != 0
                        && worker.state.completed.load(Ordering::SeqCst) >= maximum
                    {
                        busy = true;
                    }
                    continue;
                }
                if queue.is_some() && !worker.state.ready.load(Ordering::SeqCst) {
                    busy = true;
                    continue;
                }
                if queue
                    .is_some_and(|(deadline, cancelled)| Instant::now() >= deadline || cancelled())
                {
                    return Err(PhpError::WorkersUnavailable);
                }
                drop(generation);
                let result = worker.submit(request, limits);
                drop(worker);
                self.changed();
                if control
                    .as_ref()
                    .is_some_and(RequestCancellation::is_cancelled)
                {
                    return Err(PhpError::RequestCancelled);
                }
                if output.as_ref().is_some_and(ResponseOutput::failed) {
                    return Err(PhpError::ResponseOutputFailed);
                }
                return result;
            }
            if !busy {
                return Err(PhpError::WorkersUnavailable);
            }
            generation = if let Some((deadline, _)) = queue {
                self.available
                    .wait_timeout(
                        generation,
                        deadline
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(50)),
                    )
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            } else if control.is_some() {
                self.available
                    .wait_timeout(generation, Duration::from_millis(50))
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            } else {
                self.available
                    .wait(generation)
                    .unwrap_or_else(|e| e.into_inner())
            };
        }
    }
}

fn try_worker(slot: &Mutex<WorkerThread>) -> Option<std::sync::MutexGuard<'_, WorkerThread>> {
    match slot.try_lock() {
        Ok(worker) => Some(worker),
        Err(std::sync::TryLockError::Poisoned(error)) => Some(error.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        for worker in &mut self.workers {
            worker
                .get_mut()
                .unwrap_or_else(|e| e.into_inner())
                .shutdown();
        }
        for worker in self.workers.drain(..) {
            worker
                .into_inner()
                .unwrap_or_else(|e| e.into_inner())
                .join();
        }
    }
}

pub const fn runtime_target() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "musl"
    )) {
        "x86_64-unknown-linux-musl"
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "aarch64",
        target_env = "musl"
    )) {
        "aarch64-unknown-linux-musl"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
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
