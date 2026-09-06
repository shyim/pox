mod admin;
mod execution;
mod proxy;
mod streaming;
mod transport;

use crate::{config::PoxConfig, server_path};
use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::TryStreamExt;
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Full, StreamBody};
use hyper::{
    body::{Frame, Incoming},
    header, Method, Request, Response, StatusCode, Version,
};
use pox_embed::{HttpRequest, PhpRuntime};
use serde::Deserialize;
use std::{
    convert::Infallible,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::{OwnedSemaphorePermit, Semaphore},
    time::{timeout, Instant},
};
use tokio_util::io::ReaderStream;

type Body = UnsyncBoxBody<Bytes, std::io::Error>;
type HttpResult = Result<Response<Body>, StatusCode>;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_connections: usize,
    pub max_inflight_requests: usize,
    pub max_body_bytes: usize,
    pub max_response_bytes: usize,
    pub max_header_bytes: usize,
    pub max_headers: usize,
    pub worker_max_requests: usize,
    pub header_timeout_ms: u64,
    pub body_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub queue_timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub write_timeout_ms: u64,
    pub shutdown_timeout_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_connections: 1024,
            max_inflight_requests: 32,
            max_body_bytes: 8 * 1024 * 1024,
            max_response_bytes: 32 * 1024 * 1024,
            max_header_bytes: 32 * 1024,
            max_headers: 100,
            worker_max_requests: 1000,
            header_timeout_ms: 10_000,
            body_timeout_ms: 30_000,
            request_timeout_ms: 30_000,
            queue_timeout_ms: 1_000,
            idle_timeout_ms: 15_000,
            write_timeout_ms: 30_000,
            shutdown_timeout_ms: 30_000,
        }
    }
}

impl Limits {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            (1..=65_536).contains(&self.max_connections),
            "max_connections must be between 1 and 65536"
        );
        anyhow::ensure!(
            (1..=65_536).contains(&self.max_inflight_requests),
            "max_inflight_requests must be between 1 and 65536"
        );
        anyhow::ensure!(
            (8192..=1024 * 1024).contains(&self.max_header_bytes),
            "max_header_bytes must be between 8192 and 1048576"
        );
        anyhow::ensure!(
            (1..=1024).contains(&self.max_headers),
            "max_headers must be between 1 and 1024"
        );
        anyhow::ensure!(
            self.max_body_bytes > 0
                && self.max_response_bytes > 0
                && self.max_response_bytes <= u32::MAX as usize,
            "body limits must be positive and max_response_bytes must fit in uint32"
        );
        for value in [
            self.header_timeout_ms,
            self.body_timeout_ms,
            self.request_timeout_ms,
            self.queue_timeout_ms,
            self.idle_timeout_ms,
            self.write_timeout_ms,
            self.shutdown_timeout_ms,
        ] {
            anyhow::ensure!(
                (1..=86_400_000).contains(&value),
                "server timeouts must be between 1 and 86400000 milliseconds"
            );
        }
        Ok(())
    }
}

pub(super) fn millis(value: u64) -> Duration {
    Duration::from_millis(value)
}

struct State {
    metrics: Arc<admin::Metrics>,
    trusted_proxies: Vec<ipnet::IpNet>,
    root: PathBuf,
    router: Option<PathBuf>,
    worker: Option<PathBuf>,
    host: String,
    port: u16,
    limits: Limits,
    admission: Arc<Semaphore>,
    executor: execution::Executor,
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    php: &PhpRuntime,
    host: &str,
    port: u16,
    root: &Path,
    router: Option<&Path>,
    worker: Option<&Path>,
    workers: usize,
    watch: Vec<String>,
    config: Option<&PoxConfig>,
) -> Result<i32> {
    let limits = config
        .map(|config| config.server.limits.clone())
        .unwrap_or_default();
    limits.validate()?;
    let trusted_proxies = config
        .map(|config| config.server.trusted_proxies.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|value| {
            value
                .parse::<ipnet::IpNet>()
                .with_context(|| format!("Invalid trusted proxy CIDR: {value}"))
        })
        .collect::<Result<Vec<_>>>()?;
    anyhow::ensure!(
        trusted_proxies.is_empty() || php.supports_request_scheme(),
        "Trusted proxies require native request scheme support; rebuild pox-runtime"
    );
    anyhow::ensure!(php.supports_response_limits(), "HTTP serving requires a runtime with native response limits (ABI 1.1); rebuild or install an updated pox-runtime");
    anyhow::ensure!(php.supports_response_output(), "HTTP serving requires native response output support; rebuild or install an updated pox-runtime");
    anyhow::ensure!(php.supports_cancellation(), "HTTP serving requires native request cancellation; rebuild or install an updated pox-runtime");
    anyhow::ensure!(php.supports_http_protocol(), "HTTP serving requires native HTTP protocol metadata; rebuild or install an updated pox-runtime");
    let root = root.canonicalize().context("Invalid document root")?;
    anyhow::ensure!(root.is_dir(), "Document root must be a directory");
    let router = router
        .map(Path::canonicalize)
        .transpose()
        .context("Invalid router")?;
    let worker = worker
        .map(Path::canonicalize)
        .transpose()
        .context("Invalid worker script")?;
    for script in router.iter().chain(worker.iter()) {
        anyhow::ensure!(
            script.is_file(),
            "PHP script must be a file: {}",
            script.display()
        );
    }
    anyhow::ensure!(
        router.is_none() || worker.is_none(),
        "Choose either a router or a worker script"
    );
    anyhow::ensure!(
        watch.is_empty() || worker.is_some(),
        "File watching requires worker mode"
    );
    let workers = if workers == 0 {
        std::thread::available_parallelism().map_or(1, |value| value.get())
    } else {
        workers
    };
    anyhow::ensure!(workers <= 1024, "workers must not exceed 1024");
    let ini = crate::build_ini_entries(
        config,
        &[
            "display_errors=0".into(),
            "display_startup_errors=0".into(),
            "log_errors=1".into(),
            "expose_php=0".into(),
        ],
    );
    php.set_ini_entries(ini.as_deref())?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()?;
    let executor = execution::Executor::start(
        php.clone(),
        &root,
        worker.as_deref(),
        workers,
        &watch,
        &limits,
    )?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind((host, port)).await?;
        let admin_listener = match config.and_then(|config| config.server.admin_address) {
            Some(address) => Some(
                tokio::net::TcpListener::bind(address)
                    .await
                    .context("Cannot bind admin listener")?,
            ),
            None => None,
        };
        let bound = listener.local_addr()?;
        println!(
            "PHP {} HTTP server listening on http://{bound}",
            php.version()
        );
        let state = Arc::new(State {
            metrics: Arc::new(admin::Metrics::default()),
            trusted_proxies,
            root,
            router,
            worker,
            host: host.into(),
            port: bound.port(),
            admission: Arc::new(Semaphore::new(limits.max_inflight_requests)),
            limits,
            executor,
        });
        transport::serve(listener, admin_listener, state).await
    })
}

fn full(data: impl Into<Bytes>) -> Body {
    Full::new(data.into())
        .map_err(|never: Infallible| match never {})
        .boxed_unsync()
}

fn error(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CONNECTION, "close")
        .body(full(format!(
            "{}\n",
            status.canonical_reason().unwrap_or("Error")
        )))
        .unwrap()
}

async fn handle(
    request: Request<Incoming>,
    state: Arc<State>,
    remote: SocketAddr,
) -> Response<Body> {
    let started = Instant::now();
    let method = request.method().clone();
    let uri = request.uri().to_string();
    // Hyper follows Transfer-Encoding over Content-Length. Always close after a
    // transfer-coded request so ambiguous framing cannot reuse a connection.
    let close = request.headers().contains_key(header::TRANSFER_ENCODING);
    let result = process(request, &state, remote).await;
    let mut response = result.unwrap_or_else(error);
    if close {
        response.headers_mut().insert(
            header::CONNECTION,
            header::HeaderValue::from_static("close"),
        );
    }
    if method == Method::HEAD {
        *response.body_mut() = full(Bytes::new());
    }
    response.headers_mut().insert(
        "x-content-type-options",
        header::HeaderValue::from_static("nosniff"),
    );
    state.metrics.record(response.status(), started.elapsed());
    // JSON encoding prevents paths from injecting terminal controls or log lines.
    eprintln!(
        "{}",
        serde_json::json!({"event":"http_request", "phase":"response_headers", "method":method.as_str(), "uri":uri, "status":response.status().as_u16(), "remote":remote.to_string(), "duration_ms":started.elapsed().as_millis()})
    );
    response
}

async fn process(mut request: Request<Incoming>, state: &State, remote: SocketAddr) -> HttpResult {
    let admission = Arc::new(
        state
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?,
    );
    validate_request(&request, &state.limits)?;
    let identity = proxy::resolve(
        request.headers(),
        remote,
        &state.trusted_proxies,
        &state.host,
        state.port,
    )?;
    let uri = request.uri().to_string();
    let (path, query_string) =
        server_path::parse_target(&uri).map_err(|code| StatusCode::from_u16(code).unwrap())?;
    let candidate =
        server_path::confined_path(&state.root, &path).map_err(|_| StatusCode::FORBIDDEN)?;
    let method = request.method().clone();
    // Consume and validate the entire bounded body before any PHP side effects.
    let body = timeout(
        millis(state.limits.body_timeout_ms),
        read_body(&mut request, state.limits.max_body_bytes),
    )
    .await
    .map_err(|_| StatusCode::REQUEST_TIMEOUT)??;
    if let Some(response) =
        static_response(&candidate, &state.root, &request, admission.clone()).await?
    {
        return Ok(response);
    }
    if candidate.is_dir() && !path.ends_with('/') && candidate.join("index.php").is_file() {
        let (raw_path, raw_query) = uri
            .split_once('?')
            .map_or((uri.as_str(), None), |(p, q)| (p, Some(q)));
        // A relative redirect cannot turn a //path into an external authority.
        let location = format!(
            "{}/{}",
            raw_path.trim_start_matches('/'),
            raw_query.map_or(String::new(), |q| format!("?{q}"))
        );
        let location = format!("/{}", location);
        return Ok(Response::builder()
            .status(StatusCode::PERMANENT_REDIRECT)
            .header(header::LOCATION, location)
            .body(full(Bytes::new()))
            .unwrap());
    }
    let script = if let Some(worker) = &state.worker {
        worker.clone()
    } else {
        server_path::script(&state.root, &path, state.router.as_deref())
            .map_err(|code| StatusCode::from_u16(code).unwrap())?
    };
    let connection_tokens = request
        .headers()
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| {
            value
                .split(',')
                .map(|part| part.trim().to_ascii_lowercase())
        })
        .collect::<Vec<_>>();
    let mut headers = Vec::new();
    for name in request.headers().keys() {
        if hop_header(name.as_str())
            || name == header::CONTENT_LENGTH
            || connection_tokens.iter().any(|token| token == name.as_str())
        {
            continue;
        }
        // Do not accept transport/identity overrides from an untrusted peer.
        if name == "forwarded" || name.as_str().starts_with("x-forwarded-") {
            continue;
        }
        let separator = if name == header::COOKIE { "; " } else { ", " };
        let values = request
            .headers()
            .get_all(name)
            .iter()
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
            .collect::<Vec<_>>();
        let value = if name == header::HOST {
            identity
                .authority
                .clone()
                .unwrap_or_else(|| values.join(separator))
        } else {
            values.join(separator)
        };
        headers.push((name.as_str().to_owned(), value));
    }
    headers.push(("Content-Length".into(), body.len().to_string()));
    let php_request = HttpRequest {
        output: None,
        cancellation: None,
        secure: identity.secure,
        protocol: if request.version() == Version::HTTP_10 {
            pox_embed::HttpProtocol::Http10
        } else {
            pox_embed::HttpProtocol::Http11
        },
        method: method.to_string(),
        uri,
        query_string,
        headers,
        body,
        document_root: state.root.to_string_lossy().into_owned(),
        script_filename: script.to_string_lossy().into_owned(),
        server_name: identity.host,
        server_port: identity.port,
        remote_addr: identity.remote.ip().to_string(),
        remote_port: identity.remote.port(),
    };
    let response = state
        .executor
        .execute(php_request, admission.clone(), &state.limits)
        .await?;
    Ok(response.map(|body| {
        body.map_frame(move |frame| {
            let _admission = &admission;
            frame
        })
        .boxed_unsync()
    }))
}

fn validate_request(request: &Request<Incoming>, limits: &Limits) -> Result<(), StatusCode> {
    for name in [header::AUTHORIZATION, header::CONTENT_TYPE] {
        if request.headers().get_all(name).iter().count() > 1 {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    let host_count = request.headers().get_all(header::HOST).iter().count();
    if host_count > 1 || (request.version() == Version::HTTP_11 && host_count != 1) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if let Some(host) = request.headers().get(header::HOST) {
        let host = host
            .to_str()
            .ok()
            .and_then(|value| value.parse::<hyper::http::uri::Authority>().ok())
            .ok_or(StatusCode::BAD_REQUEST)?;
        if host.host().is_empty()
            || host.as_str().contains('@')
            || (host.as_str().len() > host.host().len() && host.port_u16().is_none())
        {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    if request.method() == Method::CONNECT || request.method() == Method::TRACE {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    if request.headers().contains_key(header::UPGRADE) {
        return Err(StatusCode::NOT_IMPLEMENTED);
    }
    if request
        .headers()
        .get_all(header::TRANSFER_ENCODING)
        .iter()
        .any(|value| !value.as_bytes().eq_ignore_ascii_case(b"chunked"))
        || request
            .headers()
            .get_all(header::TRANSFER_ENCODING)
            .iter()
            .count()
            > 1
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    if request
        .headers()
        .get(header::EXPECT)
        .is_some_and(|value| !value.as_bytes().eq_ignore_ascii_case(b"100-continue"))
    {
        return Err(StatusCode::EXPECTATION_FAILED);
    }
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|size| size > limits.max_body_bytes as u64)
    {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    Ok(())
}

async fn read_body(request: &mut Request<Incoming>, limit: usize) -> Result<Vec<u8>, StatusCode> {
    let mut body = Vec::new();
    while let Some(frame) = request.body_mut().frame().await {
        let frame = frame.map_err(|_| StatusCode::BAD_REQUEST)?;
        if let Ok(data) = frame.into_data() {
            if data.len() > limit.saturating_sub(body.len()) {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            body.extend_from_slice(&data);
        }
    }
    Ok(body)
}

fn hop_header(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-connection"
            | "transfer-encoding"
            | "te"
            | "trailer"
            | "upgrade"
            | "proxy-authenticate"
            | "proxy-authorization"
    )
}

fn php_response(response: pox_embed::HttpResponse, method: &Method, limit: usize) -> HttpResult {
    if response.body.len() > limit {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let status = StatusCode::from_u16(response.status)
        .ok()
        .filter(|status| !status.is_informational() && status.as_u16() < 600)
        .ok_or(StatusCode::BAD_GATEWAY)?;
    let connection_tokens = response
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| {
            value
                .split(',')
                .map(|token| token.trim().to_ascii_lowercase())
        })
        .collect::<Vec<_>>();
    let bodyless = matches!(
        status,
        StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED | StatusCode::RESET_CONTENT
    );
    // Hyper omits Content-Length on 304; preserve representation length only
    // for ordinary HEAD responses. 204/205/304 never carry payload bytes.
    let metadata_length = method == Method::HEAD && !bodyless;
    let mut declared_length = None;
    let mut result = Response::new(full(Bytes::new()));
    *result.status_mut() = status;
    for (name, value) in response.headers {
        let name =
            header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| StatusCode::BAD_GATEWAY)?;
        let value = header::HeaderValue::from_str(&value).map_err(|_| StatusCode::BAD_GATEWAY)?;
        if name == header::CONTENT_LENGTH {
            if metadata_length
                && !connection_tokens
                    .iter()
                    .any(|token| token == "content-length")
            {
                let text = value.to_str().map_err(|_| StatusCode::BAD_GATEWAY)?;
                if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(StatusCode::BAD_GATEWAY);
                }
                let length = text.parse::<u64>().map_err(|_| StatusCode::BAD_GATEWAY)?;
                if declared_length.replace(length).is_some() {
                    return Err(StatusCode::BAD_GATEWAY);
                }
            }
            continue;
        }
        if hop_header(name.as_str()) || connection_tokens.iter().any(|token| token == name.as_str())
        {
            continue;
        }
        result.headers_mut().append(name, value);
    }
    let length = if status == StatusCode::RESET_CONTENT {
        Some(0)
    } else if metadata_length {
        declared_length
            .or_else(|| (!response.body.is_empty()).then_some(response.body.len() as u64))
    } else if !bodyless {
        Some(response.body.len() as u64)
    } else {
        None
    };
    if let Some(length) = length {
        result.headers_mut().insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from_str(&length.to_string()).unwrap(),
        );
    }
    if !bodyless && method != Method::HEAD {
        *result.body_mut() = full(response.body);
    }
    Ok(result)
}

async fn static_response(
    candidate: &Path,
    root: &Path,
    request: &Request<Incoming>,
    admission: Arc<OwnedSemaphorePermit>,
) -> Result<Option<Response<Body>>, StatusCode> {
    let resolved = match tokio::fs::canonicalize(candidate).await {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    if !resolved.starts_with(root) {
        return Err(StatusCode::FORBIDDEN);
    }
    if server_path::is_php(candidate) || server_path::is_php(&resolved) {
        return Ok(None);
    }
    if resolved
        .strip_prefix(root)
        .unwrap()
        .components()
        .any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| name.starts_with('.') && name != ".well-known")
        })
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let mut file = match tokio::fs::File::open(&resolved).await {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let metadata = file
        .metadata()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !metadata.is_file() {
        return Ok(None);
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        let mut response = error(StatusCode::METHOD_NOT_ALLOWED);
        response
            .headers_mut()
            .insert(header::ALLOW, header::HeaderValue::from_static("GET, HEAD"));
        return Ok(Some(response));
    }
    let length = metadata.len();
    let modified = metadata.modified().ok();
    let etag = modified
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| {
            format!(
                "W/\"{:x}-{:x}-{:x}\"",
                length,
                value.as_secs(),
                value.subsec_nanos()
            )
        });
    let mut builder =
        Response::builder().header(header::CONTENT_TYPE, crate::guess_content_type(candidate));
    if let Some(etag) = &etag {
        builder = builder.header(header::ETAG, etag);
    }
    if let Some(modified) = modified {
        builder = builder.header(header::LAST_MODIFIED, httpdate::fmt_http_date(modified));
    }
    // Our metadata ETag is weak, so If-Match can succeed only for '*'.
    if let Some(value) = request.headers().get(header::IF_MATCH) {
        if value.as_bytes() != b"*" {
            return Err(StatusCode::PRECONDITION_FAILED);
        }
    } else if request
        .headers()
        .get(header::IF_UNMODIFIED_SINCE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| httpdate::parse_http_date(v).ok())
        .zip(modified)
        .is_some_and(|(since, modified)| {
            modified
                .duration_since(since)
                .is_ok_and(|delta| delta >= Duration::from_secs(1))
        })
    {
        return Err(StatusCode::PRECONDITION_FAILED);
    }
    let not_modified = if let Some(value) = request.headers().get(header::IF_NONE_MATCH) {
        value.to_str().is_ok_and(|value| {
            value.split(',').any(|part| {
                part.trim() == "*"
                    || etag.as_ref().is_some_and(|tag| {
                        part.trim().trim_start_matches("W/") == tag.trim_start_matches("W/")
                    })
            })
        })
    } else {
        request
            .headers()
            .get(header::IF_MODIFIED_SINCE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| httpdate::parse_http_date(v).ok())
            .zip(modified)
            .is_some_and(|(since, modified)| {
                modified
                    .duration_since(since)
                    .is_ok_and(|d| d < Duration::from_secs(1))
                    || modified <= since
            })
    };
    if not_modified {
        return Ok(Some(
            builder
                .status(StatusCode::NOT_MODIFIED)
                .body(full(Bytes::new()))
                .unwrap(),
        ));
    }
    builder = builder.header(header::ACCEPT_RANGES, "bytes");
    let mut response_length = length;
    // HEAD ignores Range. Weak ETags cannot satisfy If-Range; dates use exact
    // Last-Modified equality, rather than the <= comparison used for caching.
    let if_range_matches = request.headers().get(header::IF_RANGE).is_none_or(|value| {
        value
            .to_str()
            .ok()
            .and_then(|value| httpdate::parse_http_date(value).ok())
            .zip(modified)
            .is_some_and(|(date, modified)| {
                httpdate::fmt_http_date(date) == httpdate::fmt_http_date(modified)
            })
    });
    if request.method() == Method::GET && if_range_matches {
        let range = request
            .headers()
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| byte_range(value, length));
        match range {
            Some(Ok((start, end))) => {
                response_length = end - start + 1;
                file.seek(std::io::SeekFrom::Start(start))
                    .await
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                builder = builder.status(StatusCode::PARTIAL_CONTENT).header(
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{length}"),
                );
            }
            Some(Err(())) => {
                return Ok(Some(
                    builder
                        .status(StatusCode::RANGE_NOT_SATISFIABLE)
                        .header(header::CONTENT_RANGE, format!("bytes */{length}"))
                        .body(full(Bytes::new()))
                        .unwrap(),
                ))
            }
            None => {}
        }
    }
    builder = builder.header(header::CONTENT_LENGTH, response_length);
    if request.method() == Method::HEAD {
        return Ok(Some(builder.body(full(Bytes::new())).unwrap()));
    }
    let stream =
        ReaderStream::with_capacity(file.take(response_length), 64 * 1024).map_ok(move |data| {
            let _keep_admission = &admission;
            Frame::data(data)
        });
    Ok(Some(
        builder
            .body(StreamBody::new(stream).boxed_unsync())
            .unwrap(),
    ))
}

// Ignore malformed/unsupported (including multipart) ranges; reject a valid
// single range that cannot select any bytes. All arithmetic stays within len.
fn byte_range(value: &str, len: u64) -> Option<Result<(u64, u64), ()>> {
    let value = value.strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    let number = |value: &str| -> Option<u64> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        value.parse().ok()
    };
    if start.is_empty() {
        let suffix = number(end)?;
        return Some(if suffix == 0 || len == 0 {
            Err(())
        } else {
            Ok((len.saturating_sub(suffix), len - 1))
        });
    }
    let start = number(start)?;
    let end = if end.is_empty() {
        len.saturating_sub(1)
    } else {
        number(end)?
    };
    if !value.ends_with('-') && end < start {
        return None;
    }
    Some(if start >= len {
        Err(())
    } else {
        Ok((start, end.min(len - 1)))
    })
}
