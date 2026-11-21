//! Admission control with red/green hysteresis, and the per-listener gate
//! every adapter passes requests through.
//!
//! When in-flight requests reach `red`, new requests are refused at once
//! (HTTP 503, gRPC RESOURCE_EXHAUSTED, TCP status 4) instead of queueing.
//! Admission resumes once the count is back down to `green`, so the server
//! does not flap at the threshold.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use tokio_util::sync::CancellationToken;

use crate::config::AdmissionConfig;

/// Shared in-flight accounting.
#[derive(Debug)]
pub struct Admission {
    red: usize,
    green: usize,
    in_flight: AtomicUsize,
    accepting: AtomicBool,
}

impl Admission {
    /// From config. Validation guarantees `0 < green < red`.
    pub fn new(config: &AdmissionConfig) -> Self {
        Self { red: config.red, green: config.green, in_flight: AtomicUsize::new(0), accepting: AtomicBool::new(true) }
    }

    /// Requests currently admitted.
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    /// True while new requests are admitted.
    pub fn accepting(&self) -> bool {
        self.accepting.load(Ordering::Acquire)
    }

    /// Admit one request, or refuse it. Never admits more than `red`.
    pub fn try_admit(self: &Arc<Self>) -> Option<Admitted> {
        if !self.accepting.load(Ordering::Acquire) {
            return None;
        }
        let n = self.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        if n >= self.red {
            self.accepting.store(false, Ordering::Release);
        }
        if n > self.red {
            self.release();
            return None;
        }
        Some(Admitted { admission: self.clone() })
    }

    fn release(&self) {
        let n = self.in_flight.fetch_sub(1, Ordering::AcqRel).saturating_sub(1);
        if n <= self.green && !self.accepting.load(Ordering::Acquire) {
            self.accepting.store(true, Ordering::Release);
        }
    }
}

/// An admitted request; dropping it frees the slot.
#[derive(Debug)]
pub struct Admitted {
    admission: Arc<Admission>,
}

impl Drop for Admitted {
    fn drop(&mut self) {
        self.admission.release();
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ListenerLabels {
    listener: String,
}

static IN_FLIGHT: std::sync::LazyLock<Family<ListenerLabels, Gauge>> = std::sync::LazyLock::new(Family::default);
static REFUSED: std::sync::LazyLock<Family<ListenerLabels, Counter>> = std::sync::LazyLock::new(Family::default);

/// Add the listeners' metrics to `registry`.
pub fn register(registry: &mut Registry) {
    registry.register("forkline_inflight_requests", "Requests in flight per listener", IN_FLIGHT.clone());
    registry.register("forkline_admission_refused", "Requests refused by admission control", REFUSED.clone());
}

/// What every adapter checks before calling the engine.
#[derive(Clone, Debug)]
pub(crate) struct Gate {
    admission: Option<Arc<Admission>>,
    in_flight: Gauge,
    refused: Counter,
    /// Cancelled when the drain timeout passes: requests still running are dropped.
    hard_stop: CancellationToken,
}

/// Held for the duration of one admitted request.
#[derive(Debug)]
pub(crate) struct Pass {
    _admitted: Option<Admitted>,
    in_flight: Gauge,
}

impl Drop for Pass {
    fn drop(&mut self) {
        self.in_flight.dec();
    }
}

impl Gate {
    pub(crate) fn new(admission: Option<Arc<Admission>>, listener: String, hard_stop: CancellationToken) -> Self {
        Self {
            admission,
            in_flight: IN_FLIGHT.get_or_create(&ListenerLabels { listener: listener.clone() }).clone(),
            refused: REFUSED.get_or_create(&ListenerLabels { listener }).clone(),
            hard_stop,
        }
    }

    /// Run a request future unless the server is hard-stopped; `None` if it was cut off.
    pub(crate) async fn guard<T>(&self, fut: impl std::future::Future<Output = T>) -> Option<T> {
        tokio::select! {
            biased;
            () = self.hard_stop.cancelled() => None,
            out = fut => Some(out),
        }
    }

    /// Admit a request, or `None` if it must be refused.
    pub(crate) fn enter(&self) -> Option<Pass> {
        let admitted = match &self.admission {
            Some(a) => match a.try_admit() {
                Some(admitted) => Some(admitted),
                None => {
                    self.refused.inc();
                    return None;
                }
            },
            None => None,
        };
        self.in_flight.inc();
        Some(Pass { _admitted: admitted, in_flight: self.in_flight.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hysteresis() {
        let a = Arc::new(Admission::new(&AdmissionConfig { red: 4, green: 2, max_queue_ms: None }));
        let mut held: Vec<Admitted> = (0..4).map(|_| a.try_admit().unwrap()).collect();
        assert!(a.try_admit().is_none(), "red reached");
        assert!(!a.accepting());
        held.pop();
        assert!(a.try_admit().is_none(), "3 in flight is above green: still refusing");
        held.pop();
        assert!(a.accepting(), "back to green");
        let extra = a.try_admit();
        assert!(extra.is_some());
        assert_eq!(a.in_flight(), 3);
    }

    #[test]
    fn never_exceeds_red_under_contention() {
        let a = Arc::new(Admission::new(&AdmissionConfig { red: 8, green: 4, max_queue_ms: None }));
        let admitted = AtomicUsize::new(0);
        let max = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..10_000 {
                        if let Some(p) = a.try_admit() {
                            let now = admitted.fetch_add(1, Ordering::SeqCst) + 1;
                            max.fetch_max(now, Ordering::Relaxed);
                            admitted.fetch_sub(1, Ordering::SeqCst);
                            drop(p);
                        }
                    }
                });
            }
        });
        assert!(max.load(Ordering::Relaxed) <= 8);
        assert_eq!(a.in_flight(), 0);
        assert!(a.accepting());
    }
}
