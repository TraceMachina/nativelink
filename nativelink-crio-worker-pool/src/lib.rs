//! CRI-O based warm worker pool manager for NativeLink.
//!
//! This crate exposes the building blocks needed to provision and manage pools
//! of pre-warmed workers backed by CRI-O containers.  It is intended to be used
//! by a standalone pool manager service as well as future scheduler integrations.
//!
//! ## Platform Support
//!
//! **Unix/Linux only** - This crate requires Unix domain sockets for CRI-O communication
//! and is not available on Windows.
//!
//! ## CRI Client Implementations
//!
//! Two CRI client implementations are available:
//!
//! - **`CriClientGrpc`** (recommended): Native gRPC client for direct CRI-O communication.
//!   Provides superior performance and reliability.
//!
//! - **`CriClient`** (legacy): CLI-based client using `crictl` binary.
//!   Simpler but slower, kept for backwards compatibility.

// CRI-O requires Unix domain sockets - not available on Windows
#![cfg(unix)]

mod cache;
mod config;
mod cri_client; // Legacy crictl-based client
mod cri_client_grpc; // New gRPC-based client
mod isolation; // COW isolation for job security
mod lifecycle;
mod pool_manager;
mod warmup;
mod worker;

pub use cache::CachePrimingAgent;
pub use config::{
    CachePrimingConfig, Language, LifecycleConfig, WarmWorkerPoolsConfig, WarmupCommand,
    WarmupConfig, WorkerPoolConfig,
};
// Export legacy crictl client
pub use cri_client::{
    ContainerConfig, CriClient, ExecResult, PodSandboxConfig, PodSandboxMetadata,
};
// Export new gRPC client (Phase 2 implementation)
pub use cri_client_grpc::{CriClientGrpc, ExecResult as ExecResultGrpc};
pub use isolation::{OverlayFsMount, snapshot_container_filesystem};
pub use lifecycle::LifecyclePolicy;
pub use pool_manager::{
    PoolCreateOptions, WarmWorkerLease, WarmWorkerPoolManager, WarmWorkerPoolMetrics,
};
pub use warmup::WarmupController;
pub use worker::{WorkerOutcome, WorkerState};
