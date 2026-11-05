//! Build graph construction and execution

use crate::task::Task;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use tracing::{debug, info};

/// Build graph of tasks with dependencies
pub struct BuildGraph {
    tasks: HashMap<String, Task>,
}

impl BuildGraph {
    /// Create a new empty build graph
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    /// Add a task to the graph
    pub fn add_task(&mut self, task: Task) -> Result<()> {
        if self.tasks.contains_key(&task.id) {
            anyhow::bail!("Task {} already exists in graph", task.id);
        }
        self.tasks.insert(task.id.clone(), task);
        Ok(())
    }

    /// Get a task by ID
    pub fn get_task(&self, id: &str) -> Option<&Task> {
        self.tasks.get(id)
    }

    /// Validate the graph (check for cycles, missing dependencies)
    pub fn validate(&self) -> Result<()> {
        debug!("Validating build graph with {} tasks", self.tasks.len());

        // Check for missing dependencies
        for task in self.tasks.values() {
            for dep in &task.dependencies {
                if !self.tasks.contains_key(dep) {
                    anyhow::bail!(
                        "Task {} depends on {} which does not exist",
                        task.id,
                        dep
                    );
                }
            }
        }

        // Check for cycles using DFS
        let mut visited = HashSet::new();
        let mut recursion_stack = HashSet::new();

        for task_id in self.tasks.keys() {
            if !visited.contains(task_id) {
                if self.has_cycle(task_id, &mut visited, &mut recursion_stack)? {
                    anyhow::bail!("Cycle detected in build graph");
                }
            }
        }

        debug!("Build graph validation passed");
        Ok(())
    }

    fn has_cycle(
        &self,
        task_id: &str,
        visited: &mut HashSet<String>,
        recursion_stack: &mut HashSet<String>,
    ) -> Result<bool> {
        visited.insert(task_id.to_string());
        recursion_stack.insert(task_id.to_string());

        if let Some(task) = self.tasks.get(task_id) {
            for dep in &task.dependencies {
                if !visited.contains(dep) {
                    if self.has_cycle(dep, visited, recursion_stack)? {
                        return Ok(true);
                    }
                } else if recursion_stack.contains(dep) {
                    return Ok(true);
                }
            }
        }

        recursion_stack.remove(task_id);
        Ok(false)
    }

    /// Get tasks in topological order
    pub fn topological_order(&self) -> Result<Vec<String>> {
        let mut order = Vec::new();
        let mut visited = HashSet::new();

        for task_id in self.tasks.keys() {
            if !visited.contains(task_id) {
                self.visit(task_id, &mut visited, &mut order)?;
            }
        }

        order.reverse();
        Ok(order)
    }

    fn visit(
        &self,
        task_id: &str,
        visited: &mut HashSet<String>,
        order: &mut Vec<String>,
    ) -> Result<()> {
        visited.insert(task_id.to_string());

        if let Some(task) = self.tasks.get(task_id) {
            for dep in &task.dependencies {
                if !visited.contains(dep) {
                    self.visit(dep, visited, order)?;
                }
            }
        }

        order.push(task_id.to_string());
        Ok(())
    }

    /// Execute the build graph
    pub async fn execute(&self) -> Result<()> {
        self.validate()?;
        let order = self.topological_order()?;

        info!("Executing {} tasks in topological order", order.len());

        for task_id in order {
            let task = self.tasks.get(&task_id)
                .context(format!("Task {} not found", task_id))?;
            info!("Executing task: {}", task_id);

            if !task.command.is_empty() {
                self.execute_task(task).await?;
            } else {
                debug!("Task {} has no command, skipping", task_id);
            }
        }

        Ok(())
    }

    async fn execute_task(&self, task: &Task) -> Result<()> {
        use tokio::process::Command as TokioCommand;

        debug!("Executing command: {:?}", task.command);
        debug!("Working directory: {:?}", task.work_dir);
        debug!("Inputs: {:?}", task.inputs);
        debug!("Outputs: {:?}", task.outputs);

        // For MVP, we'll just run the command directly
        // Full implementation would handle caching, parallelism, etc.

        let mut cmd = TokioCommand::new(&task.command[0]);

        // Add remaining command arguments
        for arg in &task.command[1..] {
            cmd.arg(arg);
        }

        // Set working directory
        cmd.current_dir(&task.work_dir);

        // Set environment variables
        for (key, value) in &task.env {
            cmd.env(key, value);
        }

        // Execute
        let output = cmd.output().await
            .context(format!("Failed to execute task {}", task.id))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "Task {} failed with exit code {:?}: {}",
                task.id,
                output.status.code(),
                stderr
            );
        }

        debug!("Task {} completed successfully", task.id);
        Ok(())
    }
}

impl Default for BuildGraph {
    fn default() -> Self {
        Self::new()
    }
}
