use core::time::Duration;
use std::time::Instant;

use crate::config::LifecycleConfig;

/// Determines when workers should be recycled.
#[derive(Debug, Clone, Copy)]
pub struct LifecyclePolicy {
    config: LifecycleConfig,
}

impl LifecyclePolicy {
    #[must_use]
    pub const fn new(config: LifecycleConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub const fn ttl(&self) -> Duration {
        Duration::from_secs(self.config.worker_ttl_seconds)
    }

    #[must_use]
    pub const fn max_jobs(&self) -> usize {
        self.config.max_jobs_per_worker
    }

    #[must_use]
    pub fn gc_job_frequency(&self) -> usize {
        self.config.gc_job_frequency.max(1)
    }

    #[must_use]
    pub fn should_recycle(&self, created_at: Instant, jobs_executed: usize) -> bool {
        jobs_executed >= self.config.max_jobs_per_worker || created_at.elapsed() >= self.ttl()
    }

    #[must_use]
    pub fn should_force_gc(&self, jobs_executed: usize) -> bool {
        jobs_executed > 0 && jobs_executed % self.gc_job_frequency() == 0
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::*;

    #[test]
    fn recycle_when_ttl_exceeded() {
        let config = LifecycleConfig {
            worker_ttl_seconds: 60,
            max_jobs_per_worker: 100,
            gc_job_frequency: 10,
        };
        let policy = LifecyclePolicy::new(config);
        let old = Instant::now() - Duration::from_secs(120);
        assert!(policy.should_recycle(old, 0));
    }

    #[test]
    fn recycle_when_job_cap_hit() {
        let config = LifecycleConfig {
            worker_ttl_seconds: 3600,
            max_jobs_per_worker: 2,
            gc_job_frequency: 10,
        };
        let policy = LifecyclePolicy::new(config);
        assert!(policy.should_recycle(Instant::now(), 2));
        assert!(!policy.should_recycle(Instant::now(), 1));
    }

    #[test]
    fn gc_frequency_applies_every_n_jobs() {
        let config = LifecycleConfig {
            worker_ttl_seconds: 3600,
            max_jobs_per_worker: 100,
            gc_job_frequency: 3,
        };
        let policy = LifecyclePolicy::new(config);

        assert!(!policy.should_force_gc(1));
        assert!(!policy.should_force_gc(2));
        assert!(policy.should_force_gc(3));
        assert!(!policy.should_force_gc(4));
        assert!(policy.should_force_gc(6));
    }
}
