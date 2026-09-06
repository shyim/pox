use super::{full, php_response, Body};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::{body::Frame, header, Method, Response, StatusCode};
use pox_embed::{HttpOutput, HttpResponse, PhpError, RequestCancellation};
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit};

// Small responses retain transactional status/error behavior. Larger responses
// or explicit flushes commit headers and use at most four queued native chunks.
const PREFIX_BYTES: usize = 64 * 1024;
const QUEUED_CHUNKS: usize = 4;

struct Pending {
    headers: Option<HttpResponse>,
    prefix: Vec<Bytes>,
    length: usize,
    committed: bool,
    suppress: bool,
}

pub(super) struct Output {
    pending: Mutex<Pending>,
    started: Mutex<Option<oneshot::Sender<HttpResponse>>>,
    chunks: mpsc::Sender<Bytes>,
    cancellation: RequestCancellation,
    limit: usize,
}

pub(super) struct Receivers {
    pub started: oneshot::Receiver<HttpResponse>,
    pub chunks: mpsc::Receiver<Bytes>,
}

impl Output {
    pub fn new(cancellation: RequestCancellation, limit: usize) -> (Arc<Self>, Receivers) {
        let (started, headers) = oneshot::channel();
        let (chunks, body) = mpsc::channel(QUEUED_CHUNKS);
        (
            Arc::new(Self {
                pending: Mutex::new(Pending {
                    headers: None,
                    prefix: Vec::new(),
                    length: 0,
                    committed: false,
                    suppress: false,
                }),
                started: Mutex::new(Some(started)),
                chunks,
                cancellation,
                limit,
            }),
            Receivers {
                started: headers,
                chunks: body,
            },
        )
    }

    fn send(&self, mut bytes: Bytes) -> bool {
        loop {
            if self.cancellation.is_cancelled() {
                return false;
            }
            match self.chunks.try_send(bytes) {
                Ok(()) => return true,
                Err(mpsc::error::TrySendError::Closed(_)) => return false,
                Err(mpsc::error::TrySendError::Full(value)) => {
                    bytes = value;
                    // Never block indefinitely inside a Rust callback where a
                    // Zend interrupt cannot unwind us. The host watchdog sets
                    // cancellation independently of this PHP dispatch thread.
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    fn commit(&self, pending: &mut Pending) -> bool {
        if pending.committed {
            return true;
        }
        let Some(headers) = pending.headers.clone() else {
            return false;
        };
        let sender = self
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if !sender.is_some_and(|sender| sender.send(headers).is_ok()) {
            return false;
        }
        pending.committed = true;
        for chunk in pending.prefix.drain(..) {
            if !self.send(chunk) {
                return false;
            }
        }
        true
    }

    pub fn buffered(&self, mut response: HttpResponse) -> HttpResponse {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        for chunk in pending.prefix.drain(..) {
            response.body.extend_from_slice(&chunk);
        }
        response
    }
}

pub(super) struct Sink(pub Arc<Output>);

impl std::ops::Deref for Sink {
    type Target = Output;
    fn deref(&self) -> &Output {
        &self.0
    }
}

impl HttpOutput for Sink {
    fn start(&self, status: u16, headers: Vec<(String, String)>) -> bool {
        let response = HttpResponse {
            status,
            headers,
            body: Vec::new(),
        };
        let invalid = php_response(response.clone(), &Method::GET, self.limit).is_err();
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.suppress = invalid || matches!(status, 204 | 205 | 304);
        pending.headers = Some(response);
        true
    }

    fn write(&self, chunk: &[u8]) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.suppress {
            return true;
        }
        if pending.committed {
            return self.send(Bytes::copy_from_slice(chunk));
        }
        let prefix = (PREFIX_BYTES - pending.length).min(chunk.len());
        pending.length += prefix;
        pending
            .prefix
            .push(Bytes::copy_from_slice(&chunk[..prefix]));
        if pending.length < PREFIX_BYTES {
            return true;
        }
        self.commit(&mut pending)
            && (prefix == chunk.len() || self.send(Bytes::copy_from_slice(&chunk[prefix..])))
    }

    fn flush(&self) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.suppress || self.commit(&mut pending)
    }
}

type Completion = oneshot::Receiver<Result<HttpResponse, PhpError>>;

struct StreamingBody {
    chunks: mpsc::Receiver<Bytes>,
    completion: Completion,
    native_done: bool,
    finished: bool,
    deadline: Pin<Box<tokio::time::Sleep>>,
    cancellation: RequestCancellation,
    _admission: Arc<OwnedSemaphorePermit>,
}

impl hyper::body::Body for StreamingBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        use std::future::Future;
        if self.finished {
            return Poll::Ready(None);
        }
        if !self.native_done {
            match Pin::new(&mut self.completion).poll(cx) {
                Poll::Ready(Ok(Ok(_))) => self.native_done = true,
                Poll::Ready(result) => {
                    eprintln!("PHP response stream failed: {result:?}");
                    self.finished = true;
                    return Poll::Ready(Some(Err(io::Error::other(
                        "PHP response did not complete",
                    ))));
                }
                Poll::Pending => {}
            }
            if !self.native_done && self.deadline.as_mut().poll(cx).is_ready() {
                self.cancellation.cancel();
                self.finished = true;
                return Poll::Ready(Some(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "PHP response deadline",
                ))));
            }
        }
        match self.chunks.poll_recv(cx) {
            Poll::Ready(Some(bytes)) => Poll::Ready(Some(Ok(Frame::data(bytes)))),
            Poll::Ready(None) if self.native_done => {
                self.finished = true;
                Poll::Ready(None)
            }
            _ => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.finished
    }
}

impl Drop for StreamingBody {
    fn drop(&mut self) {
        if !self.native_done {
            self.cancellation.cancel();
        }
    }
}

pub(super) fn response(
    metadata: HttpResponse,
    chunks: mpsc::Receiver<Bytes>,
    completion: Completion,
    cancellation: RequestCancellation,
    deadline: std::time::Instant,
    admission: Arc<OwnedSemaphorePermit>,
    limit: usize,
) -> Result<Response<Body>, StatusCode> {
    let mut response = php_response(metadata, &Method::GET, limit)?;
    response.headers_mut().remove(header::CONTENT_LENGTH);
    *response.body_mut() = full(Bytes::new());
    Ok(response.map(|_| {
        StreamingBody {
            chunks,
            completion,
            native_done: false,
            finished: false,
            deadline: Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(
                deadline,
            ))),
            cancellation,
            _admission: admission,
        }
        .boxed_unsync()
    }))
}
