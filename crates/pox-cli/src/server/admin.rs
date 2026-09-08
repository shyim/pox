//! Independent, opt-in operational listener. No PHP execution or public admission.
use super::{full, Body, State};
use hyper::{
    body::Incoming, header, server::conn::http1, service::service_fn, Method, Request, Response,
    StatusCode,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{watch, Semaphore},
    task::JoinSet,
};

#[derive(Default)]
pub(super) struct Metrics {
    responses: [AtomicU64; 6],
    duration_us: AtomicU64,
    bodies: [AtomicU64; 3],
}

impl Metrics {
    pub fn record_body(&self, outcome: usize) {
        self.bodies[outcome].fetch_add(1, Ordering::Relaxed);
    }

    pub fn record(&self, status: StatusCode, elapsed: Duration) {
        self.responses[usize::from(status.as_u16() / 100)].fetch_add(1, Ordering::Relaxed);
        self.duration_us.fetch_add(
            elapsed.as_micros().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }
}

fn response(request: Request<Incoming>, state: &State, draining: bool) -> Response<Body> {
    let (status, content_type, text) = if !matches!(*request.method(), Method::GET | Method::HEAD) {
        (
            StatusCode::METHOD_NOT_ALLOWED,
            "text/plain",
            "Method Not Allowed\n".into(),
        )
    } else {
        let health = state.executor.snapshot();
        let ready = !draining && !health.stopped && health.ready > health.expired;
        match request.uri().path() {
            "/live" => (StatusCode::OK, "text/plain", "live\n".into()),
            "/ready" => {
                if ready {
                    (StatusCode::OK, "text/plain", "ready\n".into())
                } else {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "text/plain",
                        "unavailable\n".into(),
                    )
                }
            }
            "/metrics" => {
                let mut text = String::from("# HELP pox_http_responses_total Public responses produced, excluding transport failures and cancelled handlers.\n# TYPE pox_http_responses_total counter\n");
                for class in 1..=5 {
                    text.push_str(&format!(
                        "pox_http_responses_total{{class=\"{class}xx\"}} {}\n",
                        state.metrics.responses[class].load(Ordering::Relaxed)
                    ));
                }
                text.push_str(&format!("# HELP pox_http_handler_duration_seconds_total Handler time for produced responses, excluding transmission.\n# TYPE pox_http_handler_duration_seconds_total counter\npox_http_handler_duration_seconds_total {:.6}\n", state.metrics.duration_us.load(Ordering::Relaxed) as f64 / 1_000_000.0));
                text.push_str("# HELP pox_http_response_bodies_total Body production outcomes; complete does not confirm delivery to the client.\n# TYPE pox_http_response_bodies_total counter\n");
                for (index, outcome) in ["complete", "error", "dropped"].into_iter().enumerate() {
                    text.push_str(&format!(
                        "pox_http_response_bodies_total{{outcome=\"{outcome}\"}} {}\n",
                        state.metrics.bodies[index].load(Ordering::Relaxed)
                    ));
                }
                for (name, help, value) in [
                    ("pox_ready", "Whether initialized PHP capacity remains within its deadline and the server is not draining.", usize::from(ready)),
                    ("pox_php_initialized_workers", "Initialized PHP workers, including busy workers.", health.ready),
                    ("pox_php_expired_jobs", "PHP jobs retained past their HTTP request deadline.", health.expired),
                    ("pox_php_available_slots", "Unused PHP execution permits.", health.available),
                    ("pox_http_admitted_requests", "Public requests holding admission, including retained native jobs.", state.limits.max_inflight_requests - state.admission.available_permits()),
                ] {
                    text.push_str(&format!("# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}\n"));
                }
                (
                    StatusCode::OK,
                    "text/plain; version=0.0.4; charset=utf-8",
                    text,
                )
            }
            _ => (StatusCode::NOT_FOUND, "text/plain", "Not Found\n".into()),
        }
    };
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, text.len())
        .header(header::CONNECTION, "close")
        .header(header::CACHE_CONTROL, "no-store");
    if status == StatusCode::METHOD_NOT_ALLOWED {
        builder = builder.header(header::ALLOW, "GET, HEAD");
    }
    builder
        .body(full(if request.method() == Method::HEAD {
            String::new()
        } else {
            text
        }))
        .unwrap()
}

pub(super) async fn serve(listener: TcpListener, state: Arc<State>, drain: watch::Receiver<bool>) {
    let capacity = Arc::new(Semaphore::new(64));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(value) => value,
                    Err(error) => {
                        eprintln!("Admin accept failed: {error}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let Ok(permit) = capacity.clone().try_acquire_owned() else { continue };
                let state = state.clone();
                let drain = drain.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    let service = service_fn(move |request| {
                        std::future::ready(Ok::<_, Infallible>(response(request, &state, *drain.borrow())))
                    });
                    let mut builder = http1::Builder::new();
                    builder.timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(2))
                        .max_buf_size(8192).max_headers(32).keep_alive(false);
                    let _ = tokio::time::timeout(Duration::from_secs(2), builder.serve_connection(TokioIo::new(stream), service)).await;
                });
            }
        }
    }
}
