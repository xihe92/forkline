//! Named thread pools for blocking or CPU-heavy module work.
//!
//! A pool is a fixed number of threads with a bounded queue. Servers create
//! them at startup from `[pools.<name>]` in `server.toml`. A module looks its
//! pool up while it is built ([`BuildContext::pool`](crate::BuildContext::pool)),
//! so a misspelled name rejects the plan at load time, and runs work on it
//! with [`Pool::run`], which waits for the result without holding a Tokio
//! worker thread.
//!
//! ```ignore
//! // while the module is built
//! let scoring = cx.pool("scoring")?;
//! // in `call`
//! let scores = self.scoring.run(cx, move || rank(&candidates)).await?;
//! ```

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;

use crate::ctx::Ctx;
use crate::error::{ErrorKind, NodeError};

/// Queue-wait histogram buckets, in seconds: 0.1 ms to 10 s.
const WAIT_BUCKETS: [f64; 15] =
    [0.0001, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];

struct Job {
    queued_at: std::time::Instant,
    work: Box<dyn FnOnce() + Send>,
}

/// A named pool of threads with a bounded queue. Cloning shares the pool.
#[derive(Clone)]
pub struct Pool {
    inner: Arc<Inner>,
}

struct Inner {
    name: Arc<str>,
    threads: usize,
    queue: usize,
    /// `None` once shut down; dropping the sender lets the workers finish the
    /// queue and exit.
    sender: RwLock<Option<SyncSender<Job>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    metrics: Handles,
}

impl Pool {
    /// Start `threads` threads named `pool-<name>-<n>`, with room for `queue`
    /// jobs waiting. Both must be at least 1.
    pub fn new(name: &str, threads: usize, queue: usize) -> Result<Self, String> {
        if threads == 0 || queue == 0 {
            return Err(format!("pool `{name}`: threads and queue must be at least 1"));
        }
        let (sender, receiver) = sync_channel::<Job>(queue);
        let receiver = Arc::new(Mutex::new(receiver));
        let metrics = Handles::new(name);
        metrics.threads.set(i64::try_from(threads).unwrap_or(i64::MAX));
        let mut workers = Vec::with_capacity(threads);
        for i in 0..threads {
            let (receiver, metrics) = (receiver.clone(), metrics.clone());
            let handle = std::thread::Builder::new()
                .name(format!("pool-{name}-{i}"))
                .spawn(move || work_loop(&receiver, &metrics))
                .map_err(|e| format!("pool `{name}`: cannot start a thread: {e}"))?;
            workers.push(handle);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                name: Arc::from(name),
                threads,
                queue,
                sender: RwLock::new(Some(sender)),
                workers: Mutex::new(workers),
                metrics,
            }),
        })
    }

    /// The pool's name.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Number of threads.
    pub fn threads(&self) -> usize {
        self.inner.threads
    }

    /// How many jobs may wait for a thread.
    pub fn queue_capacity(&self) -> usize {
        self.inner.queue
    }

    /// Run `work` on the pool and wait for its result without holding the
    /// calling Tokio worker thread.
    ///
    /// * A full queue fails at once with [`ErrorKind::Overloaded`].
    /// * The wait ends at the request's deadline ([`ErrorKind::Deadline`]) or
    ///   on cancellation ([`ErrorKind::Cancelled`]). A job that has started
    ///   cannot be stopped and runs to the end; one still queued when its
    ///   request gives up is skipped.
    /// * A panic in `work` fails with [`ErrorKind::Panic`]; the thread survives.
    pub async fn run<F, R>(&self, cx: &Ctx, work: F) -> Result<R, NodeError>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let name = &self.inner.name;
        if cx.is_cancelled() {
            return Err(NodeError::new(
                ErrorKind::Cancelled,
                format!("request cancelled before pool `{name}` ran the job"),
            ));
        }
        let deadline = cx.deadline();
        if tokio::time::Instant::now() >= deadline {
            return Err(NodeError::new(
                ErrorKind::Deadline,
                format!("deadline passed before pool `{name}` ran the job"),
            ));
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (token, metrics) = (cx.cancellation().clone(), self.inner.metrics.clone());
        let work = Box::new(move || {
            // The request gave up while the job waited: don't spend a thread on it.
            if token.is_cancelled() || tokio::time::Instant::now() >= deadline {
                metrics.skipped.inc();
                return;
            }
            let result = catch_unwind(AssertUnwindSafe(work));
            if result.is_ok() {
                metrics.done.inc()
            } else {
                metrics.panicked.inc()
            };
            let _ = tx.send(result);
        });
        self.submit(Job { queued_at: std::time::Instant::now(), work })?;
        tokio::select! {
            biased;
            () = cx.cancellation().cancelled() => Err(NodeError::new(
                ErrorKind::Cancelled,
                format!("request cancelled while waiting for pool `{name}`"),
            )),
            () = tokio::time::sleep_until(deadline) => Err(NodeError::new(
                ErrorKind::Deadline,
                format!("deadline passed while waiting for pool `{name}`"),
            )),
            result = rx => match result {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(panic)) => Err(NodeError::new(
                    ErrorKind::Panic,
                    format!("a job on pool `{name}` panicked: {}", panic_message(panic.as_ref())),
                )),
                Err(_) => Err(NodeError::new(ErrorKind::Internal, format!("pool `{name}` dropped the job"))),
            },
        }
    }

    fn submit(&self, job: Job) -> Result<(), NodeError> {
        let name = &self.inner.name;
        let sender = self.inner.sender.read().unwrap_or_else(PoisonError::into_inner);
        let Some(sender) = sender.as_ref() else {
            return Err(NodeError::new(ErrorKind::Internal, format!("pool `{name}` is shut down")));
        };
        // Counted before the send, so a worker can never take it below zero.
        self.inner.metrics.queued.inc();
        match sender.try_send(job) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.inner.metrics.queued.dec();
                self.inner.metrics.refused.inc();
                Err(NodeError::new(
                    ErrorKind::Overloaded,
                    format!("pool `{name}` is full ({} waiting)", self.inner.queue),
                ))
            }
            Err(TrySendError::Disconnected(_)) => {
                self.inner.metrics.queued.dec();
                Err(NodeError::new(ErrorKind::Internal, format!("pool `{name}` is shut down")))
            }
        }
    }

    /// Stop taking jobs, let the threads finish the queue, and wait for them
    /// up to `timeout`. Returns true if every thread finished. Blocks the
    /// calling thread, so call it outside the async runtime or through
    /// `spawn_blocking`.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        drop(self.inner.sender.write().unwrap_or_else(PoisonError::into_inner).take());
        let deadline = std::time::Instant::now() + timeout;
        let mut workers = self.inner.workers.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if workers.iter().all(JoinHandle::is_finished) {
                for handle in workers.drain(..) {
                    let _ = handle.join();
                }
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl fmt::Debug for Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool")
            .field("name", &self.inner.name)
            .field("threads", &self.inner.threads)
            .field("queue", &self.inner.queue)
            .finish()
    }
}

fn work_loop(receiver: &Mutex<Receiver<Job>>, metrics: &Handles) {
    loop {
        // One thread waits on the channel at a time; the others wait for the lock.
        let job = match receiver.lock().unwrap_or_else(PoisonError::into_inner).recv() {
            Ok(job) => job,
            Err(_) => return, // shut down and the queue is empty
        };
        metrics.queued.dec();
        metrics.wait.observe(job.queued_at.elapsed().as_secs_f64());
        metrics.busy.inc();
        (job.work)();
        metrics.busy.dec();
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    if let Some(s) = panic.downcast_ref::<&str>() {
        s
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s
    } else {
        "(no message)"
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct PoolLabels {
    pool: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct JobLabels {
    pool: String,
    result: &'static str,
}

struct PoolMetrics {
    threads: Family<PoolLabels, Gauge>,
    queued: Family<PoolLabels, Gauge>,
    busy: Family<PoolLabels, Gauge>,
    jobs: Family<JobLabels, Counter>,
    wait: Family<PoolLabels, Histogram, fn() -> Histogram>,
}

fn wait_histogram() -> Histogram {
    Histogram::new(WAIT_BUCKETS)
}

static METRICS: LazyLock<PoolMetrics> = LazyLock::new(|| PoolMetrics {
    threads: Family::default(),
    queued: Family::default(),
    busy: Family::default(),
    jobs: Family::default(),
    wait: Family::new_with_constructor(wait_histogram as fn() -> Histogram),
});

/// One pool's metric handles, bound to its label once.
#[derive(Clone)]
struct Handles {
    threads: Gauge,
    queued: Gauge,
    busy: Gauge,
    done: Counter,
    refused: Counter,
    panicked: Counter,
    skipped: Counter,
    wait: Histogram,
}

impl Handles {
    fn new(name: &str) -> Self {
        let m = &*METRICS;
        let label = PoolLabels { pool: name.to_owned() };
        let job = |result| m.jobs.get_or_create(&JobLabels { pool: name.to_owned(), result }).clone();
        Self {
            threads: m.threads.get_or_create(&label).clone(),
            queued: m.queued.get_or_create(&label).clone(),
            busy: m.busy.get_or_create(&label).clone(),
            done: job("done"),
            refused: job("refused"),
            panicked: job("panicked"),
            skipped: job("skipped"),
            wait: m.wait.get_or_create(&label).clone(),
        }
    }
}

/// Add the pool metrics to `registry`.
pub fn register_metrics(registry: &mut Registry) {
    let m = &*METRICS;
    registry.register("forkline_pool_threads", "Threads in each pool", m.threads.clone());
    registry.register("forkline_pool_queued", "Jobs waiting for a thread, by pool", m.queued.clone());
    registry.register("forkline_pool_busy", "Threads running a job, by pool", m.busy.clone());
    registry.register(
        "forkline_pool_jobs",
        "Jobs by pool and result: done, refused (queue full), panicked, skipped (request gave up while queued)",
        m.jobs.clone(),
    );
    registry.register(
        "forkline_pool_wait_seconds",
        "Time jobs waited in the queue, by pool, in seconds",
        m.wait.clone(),
    );
}
