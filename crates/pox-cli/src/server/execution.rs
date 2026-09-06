//! PHP owns dedicated OS threads; network cancellation never frees executing PHP.
use super::{millis, streaming, Limits};
use anyhow::Result;
use globset::{Glob, GlobSetBuilder};
use hyper::StatusCode;
use notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebouncedEvent};
use pox_embed::{
    HttpRequest, HttpResponse, PhpError, PhpRuntime, RequestCancellation, ResponseLimits,
    ResponseOutput, WorkerReadiness,
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex, RwLock,
    },
    time::Duration,
};
use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};

#[derive(Default)]
struct Health {
    worker_readiness: RwLock<Option<WorkerReadiness>>,
    reloading: AtomicBool,
    ready: AtomicUsize,
    next: AtomicUsize,
    jobs: Mutex<HashMap<usize, PendingJob>>,
}

struct PendingJob {
    deadline: std::time::Instant,
    cancellation: RequestCancellation,
}

struct CancelOnDrop(Option<RequestCancellation>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(control) = &self.0 {
            control.cancel();
        }
    }
}

struct TrackedJob {
    health: Arc<Health>,
    id: usize,
}

impl Drop for TrackedJob {
    fn drop(&mut self) {
        self.health
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

struct AttachedThread<'a>(&'a Health);

impl Drop for AttachedThread<'_> {
    fn drop(&mut self) {
        self.0.ready.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) struct Snapshot {
    pub ready: usize,
    pub expired: usize,
    pub available: usize,
    pub stopped: bool,
}

struct Job {
    _tracked: TrackedJob,
    queue_deadline: std::time::Instant,
    limits: ResponseLimits,
    request: HttpRequest,
    response: oneshot::Sender<Result<HttpResponse, PhpError>>,
    _admission: Arc<OwnedSemaphorePermit>,
    _execution: OwnedSemaphorePermit,
}

enum Control {
    Reload,
    Shutdown,
}

pub(super) struct Executor {
    cancellation_runtime: PhpRuntime,
    health: Arc<Health>,
    jobs: mpsc::SyncSender<Job>,
    control: mpsc::SyncSender<Control>,
    stopped: Arc<AtomicBool>,
    capacity: Arc<Semaphore>,
    done: Mutex<Option<oneshot::Receiver<()>>>,
}

impl Executor {
    pub fn start(
        php: PhpRuntime,
        root: &Path,
        worker: Option<&Path>,
        workers: usize,
        patterns: &[String],
        limits: &Limits,
    ) -> Result<Self> {
        let cancellation_runtime = php.clone();
        let (jobs, receiver) = mpsc::sync_channel::<Job>(limits.max_inflight_requests);
        let (control, commands) = mpsc::sync_channel::<Control>(1);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (done_tx, done) = oneshot::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let health = Arc::new(Health::default());
        let owner_health = health.clone();
        let root = root.to_owned();
        let worker = worker.map(Path::to_owned);
        let mut globs = GlobSetBuilder::new();
        for pattern in patterns {
            globs.add(Glob::new(pattern)?);
        }
        let globs = globs.build()?;
        let watch = !patterns.is_empty();
        let reload = control.clone();
        let startup_timeout = millis(limits.request_timeout_ms);
        let max_requests = limits.worker_max_requests;
        let concurrency = workers;
        std::thread::Builder::new()
            .name("pox-php-owner".into())
            .spawn(move || {
                let result: Result<()> = (|| {
                    if let Some(worker) = worker {
                        let pool = Arc::new(RwLock::new(
                            php.workers(
                                worker
                                    .to_str()
                                    .ok_or_else(|| anyhow::anyhow!("Worker path is not UTF-8"))?,
                                root.to_str()
                                    .ok_or_else(|| anyhow::anyhow!("Document root is not UTF-8"))?,
                                workers,
                            )?,
                        ));
                        {
                            let mut pool = pool.write().unwrap_or_else(|e| e.into_inner());
                            pool.set_max_requests(max_requests);
                            pool.wait_ready(startup_timeout)?;
                            *owner_health.worker_readiness.write().unwrap_or_else(|e| e.into_inner()) = Some(pool.readiness());
                        }
                        let _watcher = if watch {
                            let watched_root = root.clone();
                            let mut watcher = new_debouncer(
                                Duration::from_millis(150),
                                None,
                                move |events: Result<Vec<DebouncedEvent>, Vec<notify::Error>>| {
                                    if let Ok(events) = events {
                                        if events.iter().flat_map(|event| &event.paths).any(
                                            |path| {
                                                globs.is_match(
                                                    path.strip_prefix(&watched_root)
                                                        .unwrap_or(path),
                                                )
                                            },
                                        ) {
                                            let _ = reload.try_send(Control::Reload);
                                        }
                                    }
                                },
                            )?;
                            watcher.watch(&root, RecursiveMode::Recursive)?;
                            Some(watcher)
                        } else {
                            None
                        };
                        let receiver = Arc::new(Mutex::new(receiver));
                        let mut threads = Vec::new();
                        for number in 0..workers {
                            let receiver = receiver.clone();
                            let pool = pool.clone();
                            let stop = stop.clone();
                            threads.push(
                                std::thread::Builder::new()
                                    .name(format!("pox-dispatch-{number}"))
                                    .spawn(move || {
                                        while !stop.load(Ordering::Acquire) {
                                            let job = receiver
                                                .lock()
                                                .unwrap_or_else(|e| e.into_inner())
                                                .recv_timeout(Duration::from_millis(50));
                                            match job {
                                                Ok(job) if !job.response.is_closed() => {
                                                    let result = pool
                                                        .read()
                                                        .unwrap_or_else(|e| e.into_inner())
                                                        .handle_request_with_limits_queued(
                                                            job.request,
                                                            job.limits,
                                                            job.queue_deadline,
                                                            || job.response.is_closed(),
                                                        );
                                                    let _ = job.response.send(result);
                                                }
                                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                                                _ => {}
                                            }
                                        }
                                    })?,
                            );
                        }
                        owner_health.ready.store(workers, Ordering::Release);
                        let _ = ready_tx.send(Ok(()));
                        while !stop.load(Ordering::Acquire) {
                            let repaired = pool.read().unwrap_or_else(|e| e.into_inner()).repair_stopped();
                            if repaired > 0 { eprintln!("{}", serde_json::json!({"event":"php_worker_replaced", "count":repaired})); }
                            match commands.recv_timeout(Duration::from_millis(50)) {
                                Ok(Control::Reload) => {
                                    eprintln!("{{\"event\":\"php_worker_reload\"}}");
                                    owner_health.reloading.store(true, Ordering::Release);
                                    pool.write().unwrap_or_else(|e| e.into_inner()).restart();
                                    owner_health.reloading.store(false, Ordering::Release);
                                }
                                Ok(Control::Shutdown)
                                | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                                _ => {}
                            }
                        }
                        stop.store(true, Ordering::Release);
                        for thread in threads {
                            let _ = thread.join();
                        }
                        // Last pool reference is dropped here, on its initialization thread.
                    } else {
                        let web = php.web()?;
                        let executor = web.parallel_executor()?;
                        let receiver = Mutex::new(receiver);
                        std::thread::scope(|scope| -> Result<()> {
                            let (attached_tx, attached_rx) = mpsc::channel();
                            for number in 0..workers {
                                let attached_tx = attached_tx.clone();
                                let executor = &executor;
                                let receiver = &receiver;
                                let stop = &stop;
                                let health = &owner_health;
                                std::thread::Builder::new()
                                    .name(format!("pox-web-{number}"))
                                    .spawn_scoped(scope, move || {
                                        let executor = match executor.attach() {
                                            Ok(executor) => executor,
                                            Err(error) => {
                                                stop.store(true, Ordering::Release);
                                                let _ = attached_tx.send(Err(error.to_string()));
                                                return;
                                            }
                                        };
                                        health.ready.fetch_add(1, Ordering::AcqRel);
                                        let _attached = AttachedThread(health);
                                        let _ = attached_tx.send(Ok(()));
                                        while !stop.load(Ordering::Acquire) {
                                            let job = receiver.lock().unwrap_or_else(|e| e.into_inner())
                                                .recv_timeout(Duration::from_millis(50));
                                            match job {
                                                Ok(job) if !job.response.is_closed() => {
                                                    let result = executor.execute_with_limits(job.request, job.limits);
                                                    let _ = job.response.send(result);
                                                }
                                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                                                _ => {}
                                            }
                                        }
                                    }).inspect_err(|_| stop.store(true, Ordering::Release))?;
                            }
                            drop(attached_tx);
                            for _ in 0..workers {
                                match attached_rx.recv_timeout(startup_timeout) {
                                    Ok(Ok(())) => {}
                                    other => {
                                        stop.store(true, Ordering::Release);
                                        anyhow::bail!("PHP thread initialization failed: {other:?}");
                                    }
                                }
                            }
                            let _ = ready_tx.send(Ok(()));
                            while !stop.load(Ordering::Acquire) {
                                match commands.recv_timeout(Duration::from_millis(50)) {
                                    Ok(Control::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                                    _ => {}
                                }
                            }
                            stop.store(true, Ordering::Release);
                            Ok(())
                        })?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    stop.store(true, Ordering::Release);
                    let _ = ready_tx.send(Err(error.to_string()));
                }
                stop.store(true, Ordering::Release);
                owner_health.ready.store(0, Ordering::Release);
                let _ = done_tx.send(());
            })?;
        match ready_rx.recv_timeout(millis(limits.request_timeout_ms)) {
            Ok(Ok(())) => {}
            result => {
                stopped.store(true, Ordering::Release);
                anyhow::bail!("PHP initialization failed: {result:?}");
            }
        }
        Ok(Self {
            cancellation_runtime,
            health,
            jobs,
            control,
            stopped,
            capacity: Arc::new(Semaphore::new(concurrency)),
            done: Mutex::new(Some(done)),
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        let now = std::time::Instant::now();
        Snapshot {
            ready: if self.health.reloading.load(Ordering::Acquire) {
                0
            } else {
                self.health
                    .worker_readiness
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .map_or_else(
                        || self.health.ready.load(Ordering::Acquire),
                        WorkerReadiness::ready_count,
                    )
            },
            expired: self
                .health
                .jobs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .values()
                .filter(|job| job.deadline <= now)
                .count(),
            available: self.capacity.available_permits(),
            stopped: self.stopped.load(Ordering::Acquire),
        }
    }

    // Repeat while native execution is retained: a timeout bailout may enter
    // application shutdown callbacks or PHP may reset its timeout flags.
    pub fn cancel_expired(&self) {
        let now = std::time::Instant::now();
        let jobs = self.health.jobs.lock().unwrap_or_else(|e| e.into_inner());
        for job in jobs.values() {
            if job.deadline <= now || job.cancellation.is_cancelled() {
                job.cancellation.cancel();
            }
        }
    }

    pub async fn execute(
        &self,
        mut request: HttpRequest,
        admission: Arc<OwnedSemaphorePermit>,
        limits: &Limits,
    ) -> super::HttpResult {
        let method = hyper::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        if self.stopped.load(Ordering::Acquire) {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        let queue_deadline = std::time::Instant::now() + millis(limits.queue_timeout_ms);
        let execution = tokio::time::timeout(
            millis(limits.queue_timeout_ms),
            self.capacity.clone().acquire_owned(),
        )
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let cancellation = self.cancellation_runtime.cancellation().map_err(|error| {
            eprintln!("Cannot allocate PHP cancellation handle: {error}");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
        request.cancellation = Some(cancellation.clone());
        let mut cancel_on_drop = CancelOnDrop(Some(cancellation.clone()));
        let (output, mut stream) = if method == hyper::Method::HEAD {
            (None, None)
        } else {
            let (output, receivers) =
                streaming::Output::new(cancellation.clone(), limits.max_response_bytes);
            request.output = Some(ResponseOutput::new(streaming::Sink(output.clone())));
            (Some(output), Some(receivers))
        };
        let deadline = std::time::Instant::now() + millis(limits.request_timeout_ms);
        let (response, mut receiver) = oneshot::channel();
        let id = self.health.next.fetch_add(1, Ordering::Relaxed);
        self.health
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                id,
                PendingJob {
                    deadline,
                    cancellation: cancellation.clone(),
                },
            );
        let job = Job {
            _tracked: TrackedJob {
                health: self.health.clone(),
                id,
            },
            queue_deadline,
            limits: ResponseLimits {
                body_bytes: limits.max_response_bytes as u32,
                header_bytes: limits.max_header_bytes as u32,
            },
            request,
            response,
            _admission: admission.clone(),
            _execution: execution,
        };
        self.jobs
            .try_send(job)
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let result = tokio::select! {
            biased;
            metadata = async { (&mut stream.as_mut().unwrap().started).await }, if stream.is_some() => {
                let metadata = metadata.map_err(|_| StatusCode::BAD_GATEWAY)?;
                let response = streaming::response(metadata, stream.take().unwrap().chunks, receiver,
                    cancellation, deadline, admission, limits.max_response_bytes)?;
                cancel_on_drop.0 = None; // StreamingBody owns disconnect/deadline cancellation now.
                return Ok(response);
            }
            result = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), &mut receiver) => result,
        };
        // A completed native exchange has already detached its cancellation
        // target. Disarm ordinary responses; cancellation of this future or its
        // deadline leaves the guard armed and the job retains all permits.
        if result.is_ok() {
            cancel_on_drop.0 = None;
        }
        match result {
            Ok(Ok(Ok(response))) => {
                // A header can arrive after select polled that branch but just
                // before completion became ready. Do not lose its queued body.
                if let Some(mut stream) = stream {
                    if let Ok(metadata) = stream.started.try_recv() {
                        let (finished, completion) = oneshot::channel();
                        let _ = finished.send(Ok(response));
                        return streaming::response(
                            metadata,
                            stream.chunks,
                            completion,
                            cancellation,
                            deadline,
                            admission,
                            limits.max_response_bytes,
                        );
                    }
                }
                let response = if let Some(output) = output {
                    output.buffered(response)
                } else {
                    response
                };
                super::php_response(response, &method, limits.max_response_bytes)
            }
            Ok(Ok(Err(PhpError::RequestCancelled))) => Err(StatusCode::GATEWAY_TIMEOUT),
            Ok(Ok(Err(PhpError::WorkersUnavailable))) => Err(StatusCode::SERVICE_UNAVAILABLE),
            Ok(Ok(Err(error))) => {
                eprintln!("PHP execution failed: {error}");
                Err(StatusCode::BAD_GATEWAY)
            }
            Ok(Err(_)) => Err(StatusCode::SERVICE_UNAVAILABLE),
            Err(_) => Err(StatusCode::GATEWAY_TIMEOUT),
        }
    }

    pub async fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        self.capacity.close();
        let _ = self.control.try_send(Control::Shutdown);
        let done = self.done.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(done) = done {
            let _ = done.await;
        }
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.control.try_send(Control::Shutdown);
    }
}
