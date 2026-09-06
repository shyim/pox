use super::{handle, millis, State};
use anyhow::Result;
use http_body_util::BodyExt;
use hyper::{server::conn::http1, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{
    convert::Infallible,
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{watch, Semaphore},
    task::JoinSet,
    time::{Instant, Sleep},
};

#[derive(Clone, Copy)]
struct Activity {
    busy: usize,
    writing: bool,
    last: Instant,
}

struct ResponseBody {
    body: super::Body,
    activity: watch::Sender<Activity>,
    finished: bool,
    metrics: Arc<super::admin::Metrics>,
    recorded: bool,
    remaining: Option<u64>,
}

impl ResponseBody {
    fn record(&mut self, outcome: usize) {
        if !self.recorded {
            self.recorded = true;
            self.metrics.record_body(outcome);
        }
    }

    fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            self.activity.send_modify(|activity| {
                activity.busy = activity.busy.saturating_sub(1);
                activity.last = Instant::now();
            });
        }
    }
}

impl hyper::body::Body for ResponseBody {
    type Data = bytes::Bytes;
    type Error = io::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<bytes::Bytes>, io::Error>>> {
        let frame = Pin::new(&mut self.body).poll_frame(cx);
        match &frame {
            Poll::Ready(None) => {
                let outcome = if self.remaining.is_some_and(|remaining| remaining != 0) {
                    1
                } else {
                    0
                };
                self.record(outcome);
                self.finish();
            }
            Poll::Ready(Some(Err(_))) => {
                self.record(1);
                self.finish();
            }
            Poll::Ready(Some(Ok(frame))) => {
                if let (Some(remaining), Some(data)) = (self.remaining, frame.data_ref()) {
                    self.remaining = remaining.checked_sub(data.len() as u64);
                    if self.remaining.is_none() {
                        self.record(1);
                    }
                }
            }
            Poll::Pending => {}
        }
        frame
    }
    fn size_hint(&self) -> hyper::body::SizeHint {
        self.body.size_hint()
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
}

impl Drop for ResponseBody {
    fn drop(&mut self) {
        use hyper::body::Body;
        // Hyper need not poll EOF after consuming Content-Length bytes, or an
        // already-empty body (HEAD/204). Count those as produced, not abandoned.
        let outcome = if self.body.is_end_stream() {
            if self.remaining.is_some_and(|remaining| remaining != 0) {
                1
            } else {
                0
            }
        } else if self.remaining == Some(0) {
            0
        } else {
            2
        };
        self.record(outcome);
        self.finish();
    }
}

struct ConnectionIo {
    stream: TcpStream,
    activity: watch::Sender<Activity>,
    write_timeout: std::time::Duration,
    blocked_write: Option<Pin<Box<Sleep>>>,
}

impl ConnectionIo {
    fn progress(&self) {
        self.activity
            .send_modify(|activity| activity.last = Instant::now());
    }
    fn check_write(&mut self, cx: &mut Context<'_>, pending: bool) -> Poll<io::Result<()>> {
        if !pending {
            if self.blocked_write.take().is_some() {
                self.activity
                    .send_modify(|activity| activity.writing = false);
            }
            return Poll::Ready(Ok(()));
        }
        if self.blocked_write.is_none() {
            self.activity
                .send_modify(|activity| activity.writing = true);
        }
        let timer = self
            .blocked_write
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(self.write_timeout)));
        if timer.as_mut().poll(cx).is_ready() {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "HTTP write timeout",
            )))
        } else {
            Poll::Pending
        }
    }
}

impl AsyncRead for ConnectionIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, buffer);
        if buffer.filled().len() > before {
            self.progress();
        }
        result
    }
}

impl AsyncWrite for ConnectionIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.stream).poll_write(cx, buffer);
        if let Poll::Ready(Err(error)) = self.check_write(cx, result.is_pending()) {
            return Poll::Ready(Err(error));
        }
        if matches!(result, Poll::Ready(Ok(size)) if size > 0) {
            self.progress();
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.stream).poll_flush(cx);
        if let Poll::Ready(Err(error)) = self.check_write(cx, result.is_pending()) {
            return Poll::Ready(Err(error));
        }
        result
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

pub(super) async fn serve(
    listener: TcpListener,
    admin_listener: Option<TcpListener>,
    state: Arc<State>,
) -> Result<i32> {
    // Install signal handlers before accepting traffic; idle and active
    // connections share the same cancellation/draining path.
    #[cfg(unix)]
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    let shutdown = async {
        #[cfg(unix)]
        tokio::select! { _ = &mut interrupt => {}, _ = term.recv() => {} }
        #[cfg(not(unix))]
        let _ = interrupt.await;
    };
    tokio::pin!(shutdown);
    let (drain, _) = watch::channel(false);
    let admin = admin_listener.map(|listener| {
        let state = state.clone();
        let drain = drain.subscribe();
        tokio::spawn(super::admin::serve(listener, state, drain))
    });
    let weak_state = Arc::downgrade(&state);
    let cancellation_watchdog = tokio::spawn(async move {
        let mut interval = tokio::time::interval(millis(50));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let Some(state) = weak_state.upgrade() else {
                break;
            };
            state.executor.cancel_expired();
        }
    });
    let connections = Arc::new(Semaphore::new(state.limits.max_connections));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(error) = result { eprintln!("HTTP connection task failed: {error}"); }
            }
            accepted = listener.accept() => {
                let (stream, remote) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => { eprintln!("HTTP accept failed: {error}"); tokio::time::sleep(millis(50)).await; continue; }
                };
                let Ok(permit) = connections.clone().try_acquire_owned() else {
                    // Do not allocate a task or wait to write to an excess client.
                    drop(stream);
                    continue;
                };
                stream.set_nodelay(true)?;
                let state = state.clone();
                let mut drain = drain.subscribe();
                tasks.spawn(async move {
                    let _permit = permit;
                    let (activity, mut observed) = watch::channel(Activity { busy: 0, writing: false, last: Instant::now() });
                    let io = ConnectionIo { stream, activity: activity.clone(), write_timeout: millis(state.limits.write_timeout_ms), blocked_write: None };
                    let service_state = state.clone();
                    let service = service_fn(move |request| {
                        let state = service_state.clone();
                        let activity = activity.clone();
                        async move {
                            activity.send_modify(|value| { value.busy += 1; value.last = Instant::now(); });
                            let metrics = state.metrics.clone();
                            let response = handle(request, state, remote).await;
                            let remaining = if hyper::body::Body::size_hint(response.body()).exact() == Some(0) {
                                // HEAD and bodyless statuses may retain representation metadata.
                                Some(0)
                            } else {
                                response.headers().get(hyper::header::CONTENT_LENGTH)
                                    .and_then(|value| value.to_str().ok()).and_then(|value| value.parse().ok())
                            };
                            Ok::<_, Infallible>(response.map(|body| ResponseBody { body, activity, metrics, remaining, recorded: false, finished: false }.boxed_unsync()))
                        }
                    });
                    let mut builder = http1::Builder::new();
                    builder.timer(TokioTimer::new())
                        .header_read_timeout(millis(state.limits.header_timeout_ms))
                        .max_buf_size(state.limits.max_header_bytes)
                        .max_headers(state.limits.max_headers)
                        .keep_alive(true).half_close(false);
                    let connection = builder.serve_connection(TokioIo::new(io), service);
                    tokio::pin!(connection);
                    let mut draining = false;
                    loop {
                        let activity = *observed.borrow_and_update();
                        tokio::select! {
                            result = &mut connection => {
                                if let Err(error) = result {
                                    eprintln!("{}", serde_json::json!({"event":"http_connection_error", "remote":remote.to_string(), "error":error.to_string()}));
                                }
                                break;
                            }
                            _ = drain.changed(), if !draining => {
                                draining = true;
                                connection.as_mut().graceful_shutdown();
                            }
                            _ = observed.changed() => {},
                            _ = tokio::time::sleep_until(activity.last + millis(state.limits.idle_timeout_ms)), if activity.busy == 0 && !activity.writing => break,
                        }
                    }
                });
            }
        }
    }
    drop(listener);
    drain.send_replace(true);
    let drained = tokio::time::timeout(millis(state.limits.shutdown_timeout_ms), async {
        while tasks.join_next().await.is_some() {}
        state.executor.shutdown().await;
        if let Some(admin) = admin {
            admin.abort();
            let _ = admin.await;
        }
    })
    .await;
    if drained.is_err() {
        // PHP cannot be safely killed inside a Rust thread. The CLI is the
        // isolation boundary: stop the process instead of unloading a live SAPI.
        eprintln!("{{\"event\":\"shutdown_deadline_exceeded\"}}");
        std::process::exit(1);
    }
    cancellation_watchdog.abort();
    let _ = cancellation_watchdog.await;
    eprintln!("{{\"event\":\"shutdown_complete\"}}");
    Ok(0)
}
