use std::time::Instant;

use nativelink_util::action_messages::WorkerId;

/// Lifecycle phase of a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    Warming,
    Ready,
    Active,
    Cooling,
    Recycling,
    Failed,
}

/// Outcome when releasing a worker back to the pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerOutcome {
    Completed,
    Failed,
    Recycle,
}

/// Tracks state for a single worker container.
#[derive(Debug, Clone)]
pub(crate) struct WorkerRecord {
    pub id: WorkerId,
    pub sandbox_id: String,
    pub container_id: String,
    pub created_at: Instant,
    pub last_transition: Instant,
    pub jobs_executed: usize,
    pub state: WorkerState,
}

impl WorkerRecord {
    pub(crate) fn new(
        id: WorkerId,
        sandbox_id: String,
        container_id: String,
        state: WorkerState,
    ) -> Self {
        let now = Instant::now();
        Self {
            id,
            sandbox_id,
            container_id,
            created_at: now,
            last_transition: now,
            jobs_executed: 0,
            state,
        }
    }

    pub(crate) fn transition(&mut self, state: WorkerState) {
        self.state = state;
        self.last_transition = Instant::now();
    }
}
