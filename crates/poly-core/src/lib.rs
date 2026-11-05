//! Poly core: task graph, caching, and execution
//!
//! This crate provides the core build system infrastructure including:
//! - Task graph construction and execution
//! - Build caching (local and remote)
//! - Dependency tracking
//! - Incremental build support

pub mod cache;
pub mod graph;
pub mod task;

pub use cache::Cache;
pub use graph::BuildGraph;
pub use task::{Task, TaskType, Language};
