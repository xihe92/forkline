//! jemalloc's own memory statistics, in builds with the `jemalloc` feature.
//!
//! The process's resident memory (RSS) can't tell memory the program still
//! uses from memory the allocator keeps after a free to reuse later. jemalloc
//! knows both; on each scrape of `/metrics` these gauges report:
//!
//! - `forkline_allocator_allocated_bytes`: bytes in live allocations, what the
//!   program actually uses. A leak makes this grow.
//! - `forkline_allocator_active_bytes`: bytes in the pages those allocations
//!   sit in (allocated plus fragmentation inside the pages).
//! - `forkline_allocator_resident_bytes`: bytes of physical memory jemalloc
//!   holds, including freed pages it hasn't returned to the OS yet.
//!
//! jemalloc returns unused pages to the OS after they sit unused for a while
//! (10 s by default); tune it with `_RJEM_MALLOC_CONF`, for example
//! `_RJEM_MALLOC_CONF=background_thread:true,dirty_decay_ms:10000`.

use prometheus_client::collector::Collector;
use prometheus_client::encoding::{DescriptorEncoder, EncodeMetric};
use prometheus_client::metrics::MetricType;
use prometheus_client::metrics::gauge::ConstGauge;
use prometheus_client::registry::{Registry, Unit};

/// Add the allocator gauges to `registry`.
pub(crate) fn register(registry: &mut Registry) {
    registry.register_collector(Box::new(Stats));
}

#[derive(Debug)]
struct Stats;

impl Collector for Stats {
    fn encode(&self, mut encoder: DescriptorEncoder) -> Result<(), std::fmt::Error> {
        // jemalloc caches its statistics; advancing the epoch refreshes them.
        if tikv_jemalloc_ctl::epoch::advance().is_err() {
            return Ok(());
        }
        let stats = [
            (
                "forkline_allocator_allocated",
                "Bytes in live allocations (jemalloc)",
                tikv_jemalloc_ctl::stats::allocated::read(),
            ),
            (
                "forkline_allocator_active",
                "Bytes in pages holding live allocations (jemalloc)",
                tikv_jemalloc_ctl::stats::active::read(),
            ),
            (
                "forkline_allocator_resident",
                "Bytes of physical memory jemalloc holds, including freed pages not yet returned to the OS",
                tikv_jemalloc_ctl::stats::resident::read(),
            ),
        ];
        for (name, help, value) in stats {
            let Ok(value) = value else { continue };
            let metric = encoder.encode_descriptor(name, help, Some(&Unit::Bytes), MetricType::Gauge)?;
            ConstGauge::new(i64::try_from(value).unwrap_or(i64::MAX)).encode(metric)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_gauges_show_live_and_resident_bytes() {
        let mut registry = Registry::default();
        super::register(&mut registry);
        let held = vec![1u8; 4 << 20]; // 4 MiB live while encoding
        let mut text = String::new();
        prometheus_client::encoding::text::encode(&mut text, &registry).unwrap();
        drop(held);
        let value = |name: &str| -> i64 {
            let line = text.lines().find(|l| l.starts_with(name)).unwrap_or_else(|| panic!("no {name} in:\n{text}"));
            line.rsplit(' ').next().unwrap().parse().unwrap()
        };
        let allocated = value("forkline_allocator_allocated_bytes ");
        let resident = value("forkline_allocator_resident_bytes ");
        assert!(allocated >= 4 << 20, "allocated {allocated}");
        assert!(resident >= allocated, "resident {resident} < allocated {allocated}");
    }

    use prometheus_client::registry::Registry;
}
