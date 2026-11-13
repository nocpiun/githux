//! Aggregate stage costs only: never stores a key, URL, query or credential.
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;
#[derive(Clone, Copy)]
pub enum ImageStage {
    Queue,
    Cache,
    Metadata,
    Fetch,
    Decode,
    FirstReady,
    Animation,
}
#[derive(Default)]
struct Stage {
    count: AtomicU64,
    micros: AtomicU64,
}
#[derive(Clone, Default)]
pub struct ImageMetrics(Arc<[Stage; 7]>);
#[derive(Debug, Clone, Copy)]
pub struct StageMeasurement {
    pub count: u64,
    pub micros: u64,
}
impl ImageMetrics {
    pub fn begin(&self, stage: ImageStage) -> ImageTimer {
        ImageTimer {
            metrics: self.clone(),
            stage,
            started: Instant::now(),
        }
    }
    pub fn record(&self, stage: ImageStage, started: Instant) {
        let measurement = &self.0[stage as usize];
        measurement.count.fetch_add(1, Ordering::Relaxed);
        measurement.micros.fetch_add(
            started.elapsed().as_micros().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }
    pub fn stage(&self, stage: ImageStage) -> StageMeasurement {
        let stage = &self.0[stage as usize];
        StageMeasurement {
            count: stage.count.load(Ordering::Relaxed),
            micros: stage.micros.load(Ordering::Relaxed),
        }
    }
}
pub struct ImageTimer {
    metrics: ImageMetrics,
    stage: ImageStage,
    started: Instant,
}
impl Drop for ImageTimer {
    fn drop(&mut self) {
        self.metrics.record(self.stage, self.started);
    }
}
