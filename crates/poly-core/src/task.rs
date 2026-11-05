//! Build task representation

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A build task in the dependency graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique task identifier
    pub id: String,
    /// Task type (compile, link, etc.)
    pub task_type: TaskType,
    /// Language this task is for
    pub language: Language,
    /// Input files
    pub inputs: Vec<PathBuf>,
    /// Output files
    pub outputs: Vec<PathBuf>,
    /// Dependencies (other task IDs)
    pub dependencies: Vec<String>,
    /// Command to execute
    pub command: Vec<String>,
    /// Working directory
    pub work_dir: PathBuf,
    /// Environment variables
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskType {
    Compile,
    Link,
    Test,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Language {
    Rust,
    C,
    Cpp,
    Go,
}

impl Task {
    /// Create a new task
    pub fn new(id: String, task_type: TaskType, language: Language) -> Self {
        Self {
            id,
            task_type,
            language,
            inputs: Vec::new(),
            outputs: Vec::new(),
            dependencies: Vec::new(),
            command: Vec::new(),
            work_dir: PathBuf::from("."),
            env: Vec::new(),
        }
    }

    /// Add an input file
    pub fn with_input(mut self, input: PathBuf) -> Self {
        self.inputs.push(input);
        self
    }

    /// Add an output file
    pub fn with_output(mut self, output: PathBuf) -> Self {
        self.outputs.push(output);
        self
    }

    /// Add a dependency
    pub fn with_dependency(mut self, dep: String) -> Self {
        self.dependencies.push(dep);
        self
    }

    /// Set the command
    pub fn with_command(mut self, command: Vec<String>) -> Self {
        self.command = command;
        self
    }

    /// Set working directory
    pub fn with_work_dir(mut self, work_dir: PathBuf) -> Self {
        self.work_dir = work_dir;
        self
    }

    /// Add an environment variable
    pub fn with_env(mut self, key: String, value: String) -> Self {
        self.env.push((key, value));
        self
    }

    /// Add multiple environment variables
    pub fn with_env_vars(mut self, envs: Vec<(String, String)>) -> Self {
        self.env.extend(envs);
        self
    }
}
