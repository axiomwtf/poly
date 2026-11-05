//! Poly core: task graph, caching, and execution
//!
//! This crate provides the core build system infrastructure including:
//! - Task graph construction and execution
//! - Build caching (local and remote)
//! - Dependency tracking
//! - Incremental build support

pub mod cache;
pub mod dependency_compiler;
pub mod dependency_downloader;
pub mod graph;
pub mod source_analyzer;
pub mod task;

pub use cache::Cache;
pub use dependency_compiler::{CompiledDependency, DependencyCompiler};
pub use dependency_downloader::{DependencyDownloader, DownloadedDependency};
pub use graph::BuildGraph;
pub use source_analyzer::DependencyAnalyzer;
pub use task::{Task, TaskType, Language};
