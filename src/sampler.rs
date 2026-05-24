//! Per-scenario resource usage sampling
//!
//! Polls a child process PID for RSS (resident set size) and CPU% during
//! scenario execution. Provides peak, average, and sample-time-series data.
//!
//! Uses `sysinfo` (already a dependency) for cross-platform process stats.

use log::debug;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A single resource sample point
#[derive(Debug, Clone, Copy)]
pub struct ResourceSample {
    /// Seconds since sampler start
    pub timestamp_secs: f64,
    /// Resident set size in bytes
    pub rss_bytes: u64,
    /// CPU utilization percentage (0.0-100.0)
    pub cpu_percent: f64,
}

/// Samples resource usage of a process during benchmark execution.
///
/// # Example
///
/// ```ignore
/// let mut sampler = ResourceSampler::new(child_process_id);
/// // Take a background sample loop (runs on interval)
/// let handle = sampler.start_loop(Duration::from_secs(30), 1.0);
/// // ... run scenario ...
/// handle.await;
/// let peak_rss = sampler.peak_rss_bytes();
/// ```
pub struct ResourceSampler {
    pid: u32,
    samples: VecDeque<ResourceSample>,
    start: Instant,
}

impl ResourceSampler {
    /// Create a new sampler for the given process PID
    pub fn new(pid: u32) -> Self {
        Self {
            pid,
            samples: VecDeque::new(),
            start: Instant::now(),
        }
    }

    /// Take one sample immediately and return it
    pub fn sample_now(&mut self) -> ResourceSample {
        let (rss, cpu) = Self::read_proc_stats(self.pid);
        let sample = ResourceSample {
            timestamp_secs: self.start.elapsed().as_secs_f64(),
            rss_bytes: rss,
            cpu_percent: cpu,
        };
        self.samples.push_back(sample);
        sample
    }

    /// Run the sampling loop for a given duration at a given frequency (Hz).
    /// Polls every `1/interval_hz` seconds until `duration` has elapsed.
    pub async fn sample_for(&mut self, duration: Duration, interval_hz: f64) {
        let sleep_duration = Duration::from_secs_f64(1.0 / interval_hz);
        let end = Instant::now() + duration;

        while Instant::now() < end {
            self.sample_now();
            tokio::time::sleep(sleep_duration).await;
        }
    }

    /// Get all collected samples (cloned)
    pub fn samples(&self) -> Vec<ResourceSample> {
        self.samples.iter().copied().collect()
    }

    /// Number of samples collected
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// Peak RSS across all samples (maximum resident set size)
    pub fn peak_rss_bytes(&self) -> u64 {
        self.samples.iter().map(|s| s.rss_bytes).max().unwrap_or(0)
    }

    /// Average CPU% across all samples
    pub fn avg_cpu_percent(&self) -> f64 {
        let count = self.samples.len();
        if count == 0 {
            return 0.0;
        }
        self.samples.iter().map(|s| s.cpu_percent).sum::<f64>() / count as f64
    }

    /// Peak CPU% across all samples
    pub fn peak_cpu_percent(&self) -> f64 {
        self.samples
            .iter()
            .map(|s| s.cpu_percent)
            .fold(0.0_f64, |a, b| a.max(b))
    }

    /// Estimated energy impact: RSS * CPU (arbitrary units for comparison)
    pub fn energy_score(&self) -> f64 {
        self.peak_rss_bytes() as f64 * self.avg_cpu_percent() / 100.0
    }

    /// Reset the sampler, clearing all samples and resetting the clock
    pub fn reset(&mut self, new_pid: Option<u32>) {
        if let Some(pid) = new_pid {
            self.pid = pid;
        }
        self.samples.clear();
        self.start = Instant::now();
    }

    /// Read RSS and CPU% statistics for a given PID.
    ///
    /// On Linux: reads `/proc/<pid>/stat`.
    /// On macOS: uses `libc::proc_taskinfo` via the sysinfo crate's internal data.
    /// Falls back to sysinfo::System for cross-platform support.
    fn read_proc_stats(pid: u32) -> (u64, f64) {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
            false,
            ProcessRefreshKind::everything(),
        );

        if let Some(process) = system.process(sysinfo::Pid::from_u32(pid)) {
            let rss = process.memory(); // RSS in bytes
            let cpu = process.cpu_usage() as f64; // CPU% as f64 (0.0-100.0)
            (rss, cpu)
        } else {
            debug!("ResourceSampler: process {} not found, returning 0", pid);
            (0, 0.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_sampler() {
        let sampler = ResourceSampler::new(0);
        assert_eq!(sampler.sample_count(), 0);
        assert_eq!(sampler.peak_rss_bytes(), 0);
        assert_eq!(sampler.avg_cpu_percent(), 0.0);
        assert_eq!(sampler.peak_cpu_percent(), 0.0);
    }

    #[test]
    fn test_sample_now() {
        let mut sampler = ResourceSampler::new(std::process::id());
        let sample = sampler.sample_now();
        assert_eq!(sampler.sample_count(), 1);
        assert!(sample.timestamp_secs >= 0.0);
    }

    #[test]
    fn test_peak_rss() {
        let mut sampler = ResourceSampler::new(0);
        // Manually inject samples
        sampler.samples.push_back(ResourceSample {
            timestamp_secs: 0.0,
            rss_bytes: 100,
            cpu_percent: 10.0,
        });
        sampler.samples.push_back(ResourceSample {
            timestamp_secs: 1.0,
            rss_bytes: 200,
            cpu_percent: 20.0,
        });
        sampler.samples.push_back(ResourceSample {
            timestamp_secs: 2.0,
            rss_bytes: 150,
            cpu_percent: 15.0,
        });
        assert_eq!(sampler.peak_rss_bytes(), 200);
        assert!((sampler.avg_cpu_percent() - 15.0).abs() < 0.001);
        assert!((sampler.peak_cpu_percent() - 20.0).abs() < 0.001);
    }

    #[test]
    fn test_reset() {
        let mut sampler = ResourceSampler::new(42);
        sampler.sample_now();
        assert_eq!(sampler.sample_count(), 1);
        sampler.reset(None);
        assert_eq!(sampler.sample_count(), 0);
    }

    #[test]
    fn test_energy_score() {
        let mut sampler = ResourceSampler::new(0);
        sampler.samples.push_back(ResourceSample {
            timestamp_secs: 0.0,
            rss_bytes: 1_000_000,
            cpu_percent: 50.0,
        });
        // energy = 1_000_000 * 50 / 100 = 500_000
        assert!((sampler.energy_score() - 500_000.0).abs() < 0.001);
    }
}
