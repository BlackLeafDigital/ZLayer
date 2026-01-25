//! Dependency orchestration for service startup ordering
//!
//! This module provides:
//! - `DependencyGraph`: Builds a DAG from service dependencies and computes startup order
//! - `DependencyConditionChecker`: Checks if dependency conditions (started, healthy, ready) are met

use crate::error::{AgentError, Result};
use crate::health::HealthState;
use crate::runtime::{ContainerId, ContainerState, Runtime};
use proxy::ServiceRegistry;
use spec::{DependencyCondition, DependsSpec, ServiceSpec, TimeoutAction};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Error types specific to dependency operations
#[derive(Debug, Clone)]
pub enum DependencyError {
    /// Circular dependency detected
    CyclicDependency { cycle: Vec<String> },
    /// Service references a non-existent dependency
    MissingService { service: String, missing: String },
    /// Self-dependency detected
    SelfDependency { service: String },
}

impl std::fmt::Display for DependencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DependencyError::CyclicDependency { cycle } => {
                write!(f, "Cyclic dependency detected: {}", cycle.join(" -> "))
            }
            DependencyError::MissingService { service, missing } => {
                write!(
                    f,
                    "Service '{}' depends on non-existent service '{}'",
                    service, missing
                )
            }
            DependencyError::SelfDependency { service } => {
                write!(f, "Service '{}' has a self-dependency", service)
            }
        }
    }
}

impl std::error::Error for DependencyError {}

impl From<DependencyError> for AgentError {
    fn from(err: DependencyError) -> Self {
        AgentError::InvalidSpec(err.to_string())
    }
}

/// A node in the dependency graph
#[derive(Debug, Clone)]
pub struct DependencyNode {
    /// Service name
    pub service_name: String,
    /// Dependencies for this service
    pub depends_on: Vec<DependsSpec>,
}

/// Dependency graph for computing startup order
///
/// Builds a directed acyclic graph (DAG) from service dependencies
/// and provides topological sorting for startup ordering.
#[derive(Debug)]
pub struct DependencyGraph {
    /// Map of service name to its dependency node
    nodes: HashMap<String, DependencyNode>,
    /// Computed startup order (topologically sorted)
    startup_order: Vec<String>,
    /// Adjacency list for graph traversal (service -> services it depends on)
    adjacency: HashMap<String, Vec<String>>,
    /// Reverse adjacency (service -> services that depend on it)
    reverse_adjacency: HashMap<String, Vec<String>>,
}

impl DependencyGraph {
    /// Build a dependency graph from a set of services
    ///
    /// # Arguments
    /// * `services` - Map of service name to service specification
    ///
    /// # Returns
    /// A validated dependency graph with computed startup order
    ///
    /// # Errors
    /// - `DependencyError::CyclicDependency` if a cycle is detected
    /// - `DependencyError::MissingService` if a dependency references a non-existent service
    /// - `DependencyError::SelfDependency` if a service depends on itself
    pub fn build(services: &HashMap<String, ServiceSpec>) -> Result<Self> {
        let mut nodes = HashMap::new();
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        let mut reverse_adjacency: HashMap<String, Vec<String>> = HashMap::new();

        // Initialize nodes and adjacency lists
        for (name, spec) in services {
            nodes.insert(
                name.clone(),
                DependencyNode {
                    service_name: name.clone(),
                    depends_on: spec.depends.clone(),
                },
            );
            adjacency.insert(name.clone(), Vec::new());
            reverse_adjacency.insert(name.clone(), Vec::new());
        }

        // Build adjacency lists and validate dependencies
        for (name, spec) in services {
            for dep in &spec.depends {
                // Check for self-dependency
                if dep.service == *name {
                    return Err(DependencyError::SelfDependency {
                        service: name.clone(),
                    }
                    .into());
                }

                // Check that dependency exists
                if !services.contains_key(&dep.service) {
                    return Err(DependencyError::MissingService {
                        service: name.clone(),
                        missing: dep.service.clone(),
                    }
                    .into());
                }

                // Add edge: name depends on dep.service
                adjacency.get_mut(name).unwrap().push(dep.service.clone());
                reverse_adjacency
                    .get_mut(&dep.service)
                    .unwrap()
                    .push(name.clone());
            }
        }

        let mut graph = Self {
            nodes,
            startup_order: Vec::new(),
            adjacency,
            reverse_adjacency,
        };

        // Detect cycles
        if let Some(cycle) = graph.detect_cycle() {
            return Err(DependencyError::CyclicDependency { cycle }.into());
        }

        // Compute topological order
        graph.startup_order = graph.topological_sort()?;

        Ok(graph)
    }

    /// Detect cycles using DFS with three-color marking
    ///
    /// Colors:
    /// - White (0): Not visited
    /// - Gray (1): Currently being visited (in recursion stack)
    /// - Black (2): Completely visited
    ///
    /// Returns the cycle path if found, None otherwise
    pub fn detect_cycle(&self) -> Option<Vec<String>> {
        let mut color: HashMap<&String, u8> = HashMap::new();
        let mut parent: HashMap<&String, Option<&String>> = HashMap::new();

        // Initialize all nodes as white
        for name in self.nodes.keys() {
            color.insert(name, 0);
            parent.insert(name, None);
        }

        // DFS from each unvisited node
        for start in self.nodes.keys() {
            if color[start] == 0 {
                if let Some(cycle) = self.dfs_cycle_detect(start, &mut color, &mut parent) {
                    return Some(cycle);
                }
            }
        }

        None
    }

    /// DFS helper for cycle detection
    fn dfs_cycle_detect<'a>(
        &'a self,
        node: &'a String,
        color: &mut HashMap<&'a String, u8>,
        parent: &mut HashMap<&'a String, Option<&'a String>>,
    ) -> Option<Vec<String>> {
        // Mark as gray (in progress)
        color.insert(node, 1);

        // Visit all dependencies
        if let Some(deps) = self.adjacency.get(node) {
            for dep in deps {
                match color.get(dep) {
                    Some(0) => {
                        // White: not visited, recurse
                        parent.insert(dep, Some(node));
                        if let Some(cycle) = self.dfs_cycle_detect(dep, color, parent) {
                            return Some(cycle);
                        }
                    }
                    Some(1) => {
                        // Gray: found a cycle
                        let mut cycle = vec![dep.clone()];
                        let mut current = node;
                        while current != dep {
                            cycle.push(current.clone());
                            if let Some(Some(p)) = parent.get(current) {
                                current = p;
                            } else {
                                break;
                            }
                        }
                        cycle.push(dep.clone());
                        cycle.reverse();
                        return Some(cycle);
                    }
                    _ => {
                        // Black: already completely visited, skip
                    }
                }
            }
        }

        // Mark as black (completely visited)
        color.insert(node, 2);
        None
    }

    /// Compute topological order using Kahn's algorithm
    ///
    /// Services with no dependencies come first (they can start immediately).
    /// Returns services in order they should be started.
    pub fn topological_order(&self) -> Vec<String> {
        self.startup_order.clone()
    }

    /// Internal topological sort implementation using Kahn's algorithm
    fn topological_sort(&self) -> Result<Vec<String>> {
        let mut in_degree: HashMap<&String, usize> = HashMap::new();
        let mut queue: VecDeque<&String> = VecDeque::new();
        let mut result = Vec::new();

        // Calculate in-degrees (number of dependencies)
        for name in self.nodes.keys() {
            let degree = self.adjacency.get(name).map(|deps| deps.len()).unwrap_or(0);
            in_degree.insert(name, degree);
            if degree == 0 {
                queue.push_back(name);
            }
        }

        // Process nodes with zero in-degree
        while let Some(node) = queue.pop_front() {
            result.push(node.clone());

            // For each service that depends on this node
            if let Some(dependents) = self.reverse_adjacency.get(node) {
                for dependent in dependents {
                    if let Some(degree) = in_degree.get_mut(dependent) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(dependent);
                        }
                    }
                }
            }
        }

        // If not all nodes are in result, there's a cycle (shouldn't happen if detect_cycle passed)
        if result.len() != self.nodes.len() {
            return Err(AgentError::InvalidSpec(
                "Dependency graph has unresolved cycles".to_string(),
            ));
        }

        Ok(result)
    }

    /// Get the startup order (services with no deps first)
    pub fn startup_order(&self) -> &[String] {
        &self.startup_order
    }

    /// Get dependencies for a specific service
    pub fn dependencies(&self, service: &str) -> Option<&[DependsSpec]> {
        self.nodes.get(service).map(|n| n.depends_on.as_slice())
    }

    /// Get the number of services in the graph
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Check if the graph is empty
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Check if service A depends on service B (directly or transitively)
    pub fn depends_on(&self, a: &str, b: &str) -> bool {
        if a == b {
            return false;
        }

        let mut visited = HashSet::new();
        let mut stack = vec![a];

        while let Some(current) = stack.pop() {
            if visited.contains(current) {
                continue;
            }
            visited.insert(current);

            if let Some(deps) = self.adjacency.get(current) {
                for dep in deps {
                    if dep == b {
                        return true;
                    }
                    if !visited.contains(dep.as_str()) {
                        stack.push(dep);
                    }
                }
            }
        }

        false
    }

    /// Get services that directly depend on the given service
    pub fn dependents(&self, service: &str) -> Vec<String> {
        self.reverse_adjacency
            .get(service)
            .cloned()
            .unwrap_or_default()
    }
}

/// Checks if dependency conditions are satisfied
///
/// Provides methods to check each condition type:
/// - `started`: Container exists and is running
/// - `healthy`: Health check passes
/// - `ready`: Service is registered with proxy and has healthy backends
pub struct DependencyConditionChecker {
    /// Runtime for checking container states
    runtime: Arc<dyn Runtime + Send + Sync>,
    /// Health states for all services
    health_states: Arc<RwLock<HashMap<String, HealthState>>>,
    /// Service registry for checking proxy readiness
    service_registry: Option<Arc<ServiceRegistry>>,
}

impl DependencyConditionChecker {
    /// Create a new condition checker
    ///
    /// # Arguments
    /// * `runtime` - Container runtime for checking container states
    /// * `health_states` - Shared map of service health states
    /// * `service_registry` - Optional service registry for checking proxy readiness
    pub fn new(
        runtime: Arc<dyn Runtime + Send + Sync>,
        health_states: Arc<RwLock<HashMap<String, HealthState>>>,
        service_registry: Option<Arc<ServiceRegistry>>,
    ) -> Self {
        Self {
            runtime,
            health_states,
            service_registry,
        }
    }

    /// Check if a dependency condition is met
    ///
    /// # Arguments
    /// * `dep` - The dependency specification to check
    ///
    /// # Returns
    /// `true` if the condition is satisfied, `false` otherwise
    pub async fn check(&self, dep: &DependsSpec) -> Result<bool> {
        match dep.condition {
            DependencyCondition::Started => self.check_started(&dep.service).await,
            DependencyCondition::Healthy => self.check_healthy(&dep.service).await,
            DependencyCondition::Ready => self.check_ready(&dep.service).await,
        }
    }

    /// Check "started" condition - container exists and is running
    ///
    /// Returns true if at least one replica of the service is in the Running state.
    pub async fn check_started(&self, service: &str) -> Result<bool> {
        // Try to get state for replica 1 (primary replica)
        // In practice, we might need to check all replicas
        let id = ContainerId {
            service: service.to_string(),
            replica: 1,
        };

        match self.runtime.container_state(&id).await {
            Ok(ContainerState::Running) => Ok(true),
            Ok(_) => Ok(false), // Container exists but not running
            Err(AgentError::NotFound { .. }) => Ok(false), // Container doesn't exist yet
            Err(e) => Err(e),   // Propagate other errors
        }
    }

    /// Check "healthy" condition - health check passes
    ///
    /// Returns true only if the service health state is `HealthState::Healthy`.
    /// Returns false for `Unknown`, `Checking`, or `Unhealthy`.
    pub async fn check_healthy(&self, service: &str) -> Result<bool> {
        let health_states = self.health_states.read().await;

        match health_states.get(service) {
            Some(HealthState::Healthy) => Ok(true),
            Some(_) => Ok(false), // Unknown, Checking, or Unhealthy
            None => Ok(false),    // No health state recorded yet
        }
    }

    /// Check "ready" condition - service is available for routing
    ///
    /// Returns true if:
    /// 1. Service is registered with the proxy
    /// 2. Has at least one healthy backend
    ///
    /// Falls back to healthy check if no service registry is configured.
    pub async fn check_ready(&self, service: &str) -> Result<bool> {
        match &self.service_registry {
            Some(registry) => {
                // Check if service has registered routes with healthy backends
                // We need to check if the service is registered and has backends
                let services = registry.list_services();
                if !services.contains(&service.to_string()) {
                    return Ok(false);
                }

                // Try to resolve a route for this service
                // Use a generic host pattern that should match
                let host = format!("{}.default", service);
                match registry.resolve(&host, "/") {
                    Ok(resolved) => {
                        // Service is registered, check if it has backends
                        Ok(!resolved.backends.is_empty())
                    }
                    Err(_) => {
                        // Service not found in routes, not ready
                        Ok(false)
                    }
                }
            }
            None => {
                // No proxy configured, fall back to healthy check with warning
                tracing::warn!(
                    service = %service,
                    "No proxy configured for 'ready' condition check, falling back to 'healthy'"
                );
                self.check_healthy(service).await
            }
        }
    }
}

// ==================== Phase 3: Wait Logic with Timeout ====================

/// Result of waiting for a dependency
#[derive(Debug, Clone)]
pub enum WaitResult {
    /// Condition was satisfied
    Satisfied,
    /// Timed out, but action allows continuing
    TimedOutContinue,
    /// Timed out with warning, continuing
    TimedOutWarn {
        service: String,
        condition: DependencyCondition,
    },
    /// Timed out and should fail (caller should handle this as an error)
    TimedOutFail {
        service: String,
        condition: DependencyCondition,
        timeout: Duration,
    },
}

impl WaitResult {
    /// Returns true if the wait was successful (condition satisfied)
    pub fn is_satisfied(&self) -> bool {
        matches!(self, WaitResult::Satisfied)
    }

    /// Returns true if the wait timed out but should continue
    pub fn should_continue(&self) -> bool {
        matches!(
            self,
            WaitResult::Satisfied | WaitResult::TimedOutContinue | WaitResult::TimedOutWarn { .. }
        )
    }

    /// Returns true if the wait failed and should not continue
    pub fn is_failure(&self) -> bool {
        matches!(self, WaitResult::TimedOutFail { .. })
    }
}

/// Orchestrates waiting for dependencies with configurable timeout and actions
///
/// Polls the condition checker at regular intervals until the condition is
/// satisfied or the timeout is reached.
pub struct DependencyWaiter {
    /// Condition checker for evaluating dependency conditions
    condition_checker: DependencyConditionChecker,
    /// Polling interval for condition checks (default: 1 second)
    poll_interval: Duration,
}

impl DependencyWaiter {
    /// Create a new dependency waiter with default poll interval (1 second)
    pub fn new(condition_checker: DependencyConditionChecker) -> Self {
        Self {
            condition_checker,
            poll_interval: Duration::from_secs(1),
        }
    }

    /// Set the polling interval
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Get the polling interval
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    /// Wait for a single dependency to be satisfied
    ///
    /// # Arguments
    /// * `dep` - The dependency specification to wait for
    ///
    /// # Returns
    /// * `WaitResult::Satisfied` if the condition was met
    /// * `WaitResult::TimedOutContinue` if timed out with on_timeout: continue
    /// * `WaitResult::TimedOutWarn` if timed out with on_timeout: warn
    /// * `WaitResult::TimedOutFail` if timed out with on_timeout: fail
    pub async fn wait_for_dependency(&self, dep: &DependsSpec) -> Result<WaitResult> {
        let timeout = dep.timeout.unwrap_or(Duration::from_secs(300)); // Default 5 minutes
        let start = std::time::Instant::now();

        tracing::info!(
            service = %dep.service,
            condition = ?dep.condition,
            timeout = ?timeout,
            "Waiting for dependency"
        );

        loop {
            // Check the condition
            match self.condition_checker.check(dep).await {
                Ok(true) => {
                    tracing::info!(
                        service = %dep.service,
                        condition = ?dep.condition,
                        elapsed = ?start.elapsed(),
                        "Dependency condition satisfied"
                    );
                    return Ok(WaitResult::Satisfied);
                }
                Ok(false) => {
                    tracing::debug!(
                        service = %dep.service,
                        condition = ?dep.condition,
                        elapsed = ?start.elapsed(),
                        "Dependency condition not yet satisfied"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        service = %dep.service,
                        condition = ?dep.condition,
                        error = %e,
                        "Error checking dependency condition"
                    );
                    // Continue polling despite errors
                }
            }

            // Check if timeout exceeded
            if start.elapsed() >= timeout {
                return Ok(self.handle_timeout(dep, timeout));
            }

            // Sleep before next check
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// Handle timeout based on the on_timeout action
    fn handle_timeout(&self, dep: &DependsSpec, timeout: Duration) -> WaitResult {
        match dep.on_timeout {
            TimeoutAction::Fail => {
                tracing::error!(
                    service = %dep.service,
                    condition = ?dep.condition,
                    timeout = ?timeout,
                    "Dependency timeout - failing startup"
                );
                WaitResult::TimedOutFail {
                    service: dep.service.clone(),
                    condition: dep.condition,
                    timeout,
                }
            }
            TimeoutAction::Warn => {
                tracing::warn!(
                    service = %dep.service,
                    condition = ?dep.condition,
                    timeout = ?timeout,
                    "Dependency timeout - continuing with warning"
                );
                WaitResult::TimedOutWarn {
                    service: dep.service.clone(),
                    condition: dep.condition,
                }
            }
            TimeoutAction::Continue => {
                tracing::info!(
                    service = %dep.service,
                    condition = ?dep.condition,
                    timeout = ?timeout,
                    "Dependency timeout - continuing anyway"
                );
                WaitResult::TimedOutContinue
            }
        }
    }

    /// Wait for all dependencies to be satisfied
    ///
    /// Waits for each dependency in order. Returns early on first failure
    /// (if on_timeout: fail).
    ///
    /// # Arguments
    /// * `deps` - Slice of dependency specifications to wait for
    ///
    /// # Returns
    /// Vector of wait results for each dependency
    pub async fn wait_for_all(&self, deps: &[DependsSpec]) -> Result<Vec<WaitResult>> {
        let mut results = Vec::with_capacity(deps.len());

        for dep in deps {
            let result = self.wait_for_dependency(dep).await?;

            // Check if we should fail immediately
            if result.is_failure() {
                results.push(result);
                // Return early on failure - don't wait for remaining deps
                return Ok(results);
            }

            results.push(result);
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use spec::{DependencyCondition, DependsSpec, TimeoutAction};

    /// Helper to create a minimal ServiceSpec for testing
    fn minimal_spec(depends: Vec<DependsSpec>) -> ServiceSpec {
        use spec::*;
        let yaml = r#"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
"#;
        let mut spec = serde_yaml::from_str::<DeploymentSpec>(yaml)
            .unwrap()
            .services
            .remove("test")
            .unwrap();
        spec.depends = depends;
        spec
    }

    /// Helper to create a DependsSpec
    fn dep(service: &str, condition: DependencyCondition) -> DependsSpec {
        DependsSpec {
            service: service.to_string(),
            condition,
            timeout: Some(std::time::Duration::from_secs(60)),
            on_timeout: TimeoutAction::Fail,
        }
    }

    // ==================== DependencyGraph Tests ====================

    #[test]
    fn test_build_empty_graph() {
        let services: HashMap<String, ServiceSpec> = HashMap::new();
        let graph = DependencyGraph::build(&services).unwrap();
        assert!(graph.is_empty());
        assert!(graph.startup_order().is_empty());
    }

    #[test]
    fn test_build_no_dependencies() {
        let mut services = HashMap::new();
        services.insert("a".to_string(), minimal_spec(vec![]));
        services.insert("b".to_string(), minimal_spec(vec![]));
        services.insert("c".to_string(), minimal_spec(vec![]));

        let graph = DependencyGraph::build(&services).unwrap();
        assert_eq!(graph.len(), 3);
        // All services have no deps, so order doesn't matter but all should be present
        let order = graph.startup_order();
        assert_eq!(order.len(), 3);
        assert!(order.contains(&"a".to_string()));
        assert!(order.contains(&"b".to_string()));
        assert!(order.contains(&"c".to_string()));
    }

    #[test]
    fn test_build_linear_dependencies() {
        // A -> B -> C (A depends on B, B depends on C)
        let mut services = HashMap::new();
        services.insert("c".to_string(), minimal_spec(vec![]));
        services.insert(
            "b".to_string(),
            minimal_spec(vec![dep("c", DependencyCondition::Started)]),
        );
        services.insert(
            "a".to_string(),
            minimal_spec(vec![dep("b", DependencyCondition::Started)]),
        );

        let graph = DependencyGraph::build(&services).unwrap();
        let order = graph.startup_order();

        // C must come before B, B must come before A
        let pos_a = order.iter().position(|x| x == "a").unwrap();
        let pos_b = order.iter().position(|x| x == "b").unwrap();
        let pos_c = order.iter().position(|x| x == "c").unwrap();

        assert!(pos_c < pos_b);
        assert!(pos_b < pos_a);
    }

    #[test]
    fn test_build_diamond_dependencies() {
        //     A
        //    / \
        //   B   C
        //    \ /
        //     D
        // A depends on B and C, B and C both depend on D
        let mut services = HashMap::new();
        services.insert("d".to_string(), minimal_spec(vec![]));
        services.insert(
            "b".to_string(),
            minimal_spec(vec![dep("d", DependencyCondition::Started)]),
        );
        services.insert(
            "c".to_string(),
            minimal_spec(vec![dep("d", DependencyCondition::Started)]),
        );
        services.insert(
            "a".to_string(),
            minimal_spec(vec![
                dep("b", DependencyCondition::Started),
                dep("c", DependencyCondition::Started),
            ]),
        );

        let graph = DependencyGraph::build(&services).unwrap();
        let order = graph.startup_order();

        let pos_a = order.iter().position(|x| x == "a").unwrap();
        let pos_b = order.iter().position(|x| x == "b").unwrap();
        let pos_c = order.iter().position(|x| x == "c").unwrap();
        let pos_d = order.iter().position(|x| x == "d").unwrap();

        // D must come before B and C
        assert!(pos_d < pos_b);
        assert!(pos_d < pos_c);
        // B and C must come before A
        assert!(pos_b < pos_a);
        assert!(pos_c < pos_a);
    }

    #[test]
    fn test_detect_self_dependency() {
        let mut services = HashMap::new();
        services.insert(
            "a".to_string(),
            minimal_spec(vec![dep("a", DependencyCondition::Started)]),
        );

        let result = DependencyGraph::build(&services);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("self-dependency"));
    }

    #[test]
    fn test_detect_simple_cycle() {
        // A -> B -> A
        let mut services = HashMap::new();
        services.insert(
            "a".to_string(),
            minimal_spec(vec![dep("b", DependencyCondition::Started)]),
        );
        services.insert(
            "b".to_string(),
            minimal_spec(vec![dep("a", DependencyCondition::Started)]),
        );

        let result = DependencyGraph::build(&services);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cyclic dependency"));
    }

    #[test]
    fn test_detect_complex_cycle() {
        // A -> B -> C -> D -> B (cycle in B -> C -> D -> B)
        let mut services = HashMap::new();
        services.insert(
            "a".to_string(),
            minimal_spec(vec![dep("b", DependencyCondition::Started)]),
        );
        services.insert(
            "b".to_string(),
            minimal_spec(vec![dep("c", DependencyCondition::Started)]),
        );
        services.insert(
            "c".to_string(),
            minimal_spec(vec![dep("d", DependencyCondition::Started)]),
        );
        services.insert(
            "d".to_string(),
            minimal_spec(vec![dep("b", DependencyCondition::Started)]),
        );

        let result = DependencyGraph::build(&services);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cyclic dependency"));
    }

    #[test]
    fn test_detect_missing_dependency() {
        let mut services = HashMap::new();
        services.insert(
            "a".to_string(),
            minimal_spec(vec![dep("nonexistent", DependencyCondition::Started)]),
        );

        let result = DependencyGraph::build(&services);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("non-existent"));
        assert!(err.contains("nonexistent"));
    }

    #[test]
    fn test_depends_on_transitive() {
        // A -> B -> C
        let mut services = HashMap::new();
        services.insert("c".to_string(), minimal_spec(vec![]));
        services.insert(
            "b".to_string(),
            minimal_spec(vec![dep("c", DependencyCondition::Started)]),
        );
        services.insert(
            "a".to_string(),
            minimal_spec(vec![dep("b", DependencyCondition::Started)]),
        );

        let graph = DependencyGraph::build(&services).unwrap();

        // Direct dependency
        assert!(graph.depends_on("a", "b"));
        assert!(graph.depends_on("b", "c"));

        // Transitive dependency
        assert!(graph.depends_on("a", "c"));

        // No dependency
        assert!(!graph.depends_on("c", "a"));
        assert!(!graph.depends_on("b", "a"));
        assert!(!graph.depends_on("c", "b"));

        // Self
        assert!(!graph.depends_on("a", "a"));
    }

    #[test]
    fn test_get_dependencies() {
        let mut services = HashMap::new();
        services.insert("c".to_string(), minimal_spec(vec![]));
        services.insert(
            "b".to_string(),
            minimal_spec(vec![dep("c", DependencyCondition::Healthy)]),
        );
        services.insert(
            "a".to_string(),
            minimal_spec(vec![
                dep("b", DependencyCondition::Started),
                dep("c", DependencyCondition::Ready),
            ]),
        );

        let graph = DependencyGraph::build(&services).unwrap();

        let a_deps = graph.dependencies("a").unwrap();
        assert_eq!(a_deps.len(), 2);

        let b_deps = graph.dependencies("b").unwrap();
        assert_eq!(b_deps.len(), 1);
        assert_eq!(b_deps[0].service, "c");
        assert_eq!(b_deps[0].condition, DependencyCondition::Healthy);

        let c_deps = graph.dependencies("c").unwrap();
        assert!(c_deps.is_empty());

        assert!(graph.dependencies("nonexistent").is_none());
    }

    #[test]
    fn test_dependents() {
        let mut services = HashMap::new();
        services.insert("c".to_string(), minimal_spec(vec![]));
        services.insert(
            "b".to_string(),
            minimal_spec(vec![dep("c", DependencyCondition::Started)]),
        );
        services.insert(
            "a".to_string(),
            minimal_spec(vec![dep("c", DependencyCondition::Started)]),
        );

        let graph = DependencyGraph::build(&services).unwrap();

        // C is depended on by A and B
        let c_dependents = graph.dependents("c");
        assert_eq!(c_dependents.len(), 2);
        assert!(c_dependents.contains(&"a".to_string()));
        assert!(c_dependents.contains(&"b".to_string()));

        // A and B have no dependents
        assert!(graph.dependents("a").is_empty());
        assert!(graph.dependents("b").is_empty());
    }

    // ==================== DependencyConditionChecker Tests ====================

    #[tokio::test]
    async fn test_check_started_running() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));
        let checker = DependencyConditionChecker::new(runtime.clone(), health_states, None);

        // Create and start a container
        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };
        let spec = minimal_spec(vec![]);
        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // Check started condition
        assert!(checker.check_started("test").await.unwrap());
    }

    #[tokio::test]
    async fn test_check_started_not_running() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));
        let checker = DependencyConditionChecker::new(runtime.clone(), health_states, None);

        // Create but don't start container
        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };
        let spec = minimal_spec(vec![]);
        runtime.create_container(&id, &spec).await.unwrap();

        // Check started condition - should be false (Pending, not Running)
        assert!(!checker.check_started("test").await.unwrap());
    }

    #[tokio::test]
    async fn test_check_started_no_container() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));
        let checker = DependencyConditionChecker::new(runtime, health_states, None);

        // Check started for non-existent service
        assert!(!checker.check_started("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn test_check_healthy() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        // Set health state to Healthy
        {
            let mut states = health_states.write().await;
            states.insert("test".to_string(), HealthState::Healthy);
        }

        let checker = DependencyConditionChecker::new(runtime, Arc::clone(&health_states), None);

        assert!(checker.check_healthy("test").await.unwrap());
    }

    #[tokio::test]
    async fn test_check_healthy_unhealthy() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        // Set health state to Unhealthy
        {
            let mut states = health_states.write().await;
            states.insert(
                "test".to_string(),
                HealthState::Unhealthy {
                    failures: 3,
                    reason: "connection refused".to_string(),
                },
            );
        }

        let checker = DependencyConditionChecker::new(runtime, Arc::clone(&health_states), None);

        assert!(!checker.check_healthy("test").await.unwrap());
    }

    #[tokio::test]
    async fn test_check_healthy_unknown() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        // Set health state to Unknown
        {
            let mut states = health_states.write().await;
            states.insert("test".to_string(), HealthState::Unknown);
        }

        let checker = DependencyConditionChecker::new(runtime, Arc::clone(&health_states), None);

        assert!(!checker.check_healthy("test").await.unwrap());
    }

    #[tokio::test]
    async fn test_check_healthy_no_state() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));
        let checker = DependencyConditionChecker::new(runtime, health_states, None);

        // No health state recorded
        assert!(!checker.check_healthy("test").await.unwrap());
    }

    #[tokio::test]
    async fn test_check_ready_no_registry() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        // Set health state to Healthy for fallback
        {
            let mut states = health_states.write().await;
            states.insert("test".to_string(), HealthState::Healthy);
        }

        let checker = DependencyConditionChecker::new(runtime, Arc::clone(&health_states), None);

        // Should fall back to healthy check
        assert!(checker.check_ready("test").await.unwrap());
    }

    #[tokio::test]
    async fn test_check_ready_with_registry() {
        use std::net::SocketAddr;

        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));
        let registry = Arc::new(ServiceRegistry::new());

        // Register service with backends
        let resolved = proxy::ResolvedService {
            name: "test".to_string(),
            backends: vec!["127.0.0.1:8080".parse::<SocketAddr>().unwrap()],
            use_tls: false,
            sni_hostname: "test.local".to_string(),
        };
        registry.register("test.default", None, resolved);

        let checker = DependencyConditionChecker::new(runtime, health_states, Some(registry));

        assert!(checker.check_ready("test").await.unwrap());
    }

    #[tokio::test]
    async fn test_check_ready_no_backends() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));
        let registry = Arc::new(ServiceRegistry::new());

        // Register service without backends
        let resolved = proxy::ResolvedService {
            name: "test".to_string(),
            backends: vec![], // No backends
            use_tls: false,
            sni_hostname: "test.local".to_string(),
        };
        registry.register("test.default", None, resolved);

        let checker = DependencyConditionChecker::new(runtime, health_states, Some(registry));

        // Should be false because no backends
        assert!(!checker.check_ready("test").await.unwrap());
    }

    #[tokio::test]
    async fn test_check_condition_dispatches_correctly() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        // Set up healthy state
        {
            let mut states = health_states.write().await;
            states.insert("test".to_string(), HealthState::Healthy);
        }

        // Start a container
        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };
        let spec = minimal_spec(vec![]);
        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        let checker = DependencyConditionChecker::new(runtime, Arc::clone(&health_states), None);

        // Test Started condition
        let dep_started = dep("test", DependencyCondition::Started);
        assert!(checker.check(&dep_started).await.unwrap());

        // Test Healthy condition
        let dep_healthy = dep("test", DependencyCondition::Healthy);
        assert!(checker.check(&dep_healthy).await.unwrap());

        // Test Ready condition (falls back to healthy since no registry)
        let dep_ready = dep("test", DependencyCondition::Ready);
        assert!(checker.check(&dep_ready).await.unwrap());
    }

    // ==================== DependencyWaiter Tests ====================

    /// Helper to create a DependsSpec with custom timeout action
    fn dep_with_timeout(
        service: &str,
        condition: DependencyCondition,
        timeout: Duration,
        on_timeout: TimeoutAction,
    ) -> DependsSpec {
        DependsSpec {
            service: service.to_string(),
            condition,
            timeout: Some(timeout),
            on_timeout,
        }
    }

    #[tokio::test]
    async fn test_wait_satisfied_immediately() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        // Pre-set health state to Healthy
        {
            let mut states = health_states.write().await;
            states.insert("db".to_string(), HealthState::Healthy);
        }

        let checker = DependencyConditionChecker::new(runtime, health_states, None);
        let waiter = DependencyWaiter::new(checker).with_poll_interval(Duration::from_millis(50));

        let dep = dep_with_timeout(
            "db",
            DependencyCondition::Healthy,
            Duration::from_secs(5),
            TimeoutAction::Fail,
        );

        let result = waiter.wait_for_dependency(&dep).await.unwrap();
        assert!(result.is_satisfied());
    }

    #[tokio::test]
    async fn test_wait_satisfied_after_delay() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        // Initially unhealthy
        {
            let mut states = health_states.write().await;
            states.insert("db".to_string(), HealthState::Unknown);
        }

        // Clone for the spawned task
        let health_states_clone = Arc::clone(&health_states);

        // Spawn task to make it healthy after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let mut states = health_states_clone.write().await;
            states.insert("db".to_string(), HealthState::Healthy);
        });

        let checker = DependencyConditionChecker::new(runtime, health_states, None);
        let waiter = DependencyWaiter::new(checker).with_poll_interval(Duration::from_millis(50));

        let dep = dep_with_timeout(
            "db",
            DependencyCondition::Healthy,
            Duration::from_secs(5),
            TimeoutAction::Fail,
        );

        let result = waiter.wait_for_dependency(&dep).await.unwrap();
        assert!(result.is_satisfied());
    }

    #[tokio::test]
    async fn test_wait_timeout_fail() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        // Never becomes healthy
        {
            let mut states = health_states.write().await;
            states.insert("db".to_string(), HealthState::Unknown);
        }

        let checker = DependencyConditionChecker::new(runtime, health_states, None);
        let waiter = DependencyWaiter::new(checker).with_poll_interval(Duration::from_millis(50));

        let dep = dep_with_timeout(
            "db",
            DependencyCondition::Healthy,
            Duration::from_millis(200), // Short timeout for test
            TimeoutAction::Fail,
        );

        let result = waiter.wait_for_dependency(&dep).await.unwrap();
        assert!(result.is_failure());

        match result {
            WaitResult::TimedOutFail {
                service,
                condition,
                timeout,
            } => {
                assert_eq!(service, "db");
                assert_eq!(condition, DependencyCondition::Healthy);
                assert_eq!(timeout, Duration::from_millis(200));
            }
            _ => panic!("Expected TimedOutFail"),
        }
    }

    #[tokio::test]
    async fn test_wait_timeout_warn() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        let checker = DependencyConditionChecker::new(runtime, health_states, None);
        let waiter = DependencyWaiter::new(checker).with_poll_interval(Duration::from_millis(50));

        let dep = dep_with_timeout(
            "db",
            DependencyCondition::Healthy,
            Duration::from_millis(100),
            TimeoutAction::Warn,
        );

        let result = waiter.wait_for_dependency(&dep).await.unwrap();
        assert!(result.should_continue());
        assert!(!result.is_satisfied());

        match result {
            WaitResult::TimedOutWarn { service, condition } => {
                assert_eq!(service, "db");
                assert_eq!(condition, DependencyCondition::Healthy);
            }
            _ => panic!("Expected TimedOutWarn"),
        }
    }

    #[tokio::test]
    async fn test_wait_timeout_continue() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        let checker = DependencyConditionChecker::new(runtime, health_states, None);
        let waiter = DependencyWaiter::new(checker).with_poll_interval(Duration::from_millis(50));

        let dep = dep_with_timeout(
            "db",
            DependencyCondition::Healthy,
            Duration::from_millis(100),
            TimeoutAction::Continue,
        );

        let result = waiter.wait_for_dependency(&dep).await.unwrap();
        assert!(result.should_continue());
        assert!(!result.is_satisfied());
        assert!(matches!(result, WaitResult::TimedOutContinue));
    }

    #[tokio::test]
    async fn test_wait_for_all_success() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        // Both services healthy
        {
            let mut states = health_states.write().await;
            states.insert("db".to_string(), HealthState::Healthy);
            states.insert("cache".to_string(), HealthState::Healthy);
        }

        let checker = DependencyConditionChecker::new(runtime, health_states, None);
        let waiter = DependencyWaiter::new(checker).with_poll_interval(Duration::from_millis(50));

        let deps = vec![
            dep_with_timeout(
                "db",
                DependencyCondition::Healthy,
                Duration::from_secs(5),
                TimeoutAction::Fail,
            ),
            dep_with_timeout(
                "cache",
                DependencyCondition::Healthy,
                Duration::from_secs(5),
                TimeoutAction::Fail,
            ),
        ];

        let results = waiter.wait_for_all(&deps).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.is_satisfied()));
    }

    #[tokio::test]
    async fn test_wait_for_all_early_failure() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        // Only cache is healthy, db never becomes healthy
        {
            let mut states = health_states.write().await;
            states.insert("cache".to_string(), HealthState::Healthy);
        }

        let checker = DependencyConditionChecker::new(runtime, health_states, None);
        let waiter = DependencyWaiter::new(checker).with_poll_interval(Duration::from_millis(50));

        let deps = vec![
            dep_with_timeout(
                "db",
                DependencyCondition::Healthy,
                Duration::from_millis(100), // Short timeout
                TimeoutAction::Fail,
            ),
            dep_with_timeout(
                "cache",
                DependencyCondition::Healthy,
                Duration::from_secs(5),
                TimeoutAction::Fail,
            ),
        ];

        let results = waiter.wait_for_all(&deps).await.unwrap();
        // Should return early after first failure
        assert_eq!(results.len(), 1);
        assert!(results[0].is_failure());
    }

    #[tokio::test]
    async fn test_wait_for_all_mixed_results() {
        let runtime = Arc::new(MockRuntime::new());
        let health_states = Arc::new(RwLock::new(HashMap::new()));

        // Only some services healthy
        {
            let mut states = health_states.write().await;
            states.insert("db".to_string(), HealthState::Healthy);
            // cache is missing (not healthy)
        }

        let checker = DependencyConditionChecker::new(runtime, health_states, None);
        let waiter = DependencyWaiter::new(checker).with_poll_interval(Duration::from_millis(50));

        let deps = vec![
            dep_with_timeout(
                "db",
                DependencyCondition::Healthy,
                Duration::from_secs(5),
                TimeoutAction::Fail,
            ),
            dep_with_timeout(
                "cache",
                DependencyCondition::Healthy,
                Duration::from_millis(100),
                TimeoutAction::Warn, // Warn instead of fail
            ),
        ];

        let results = waiter.wait_for_all(&deps).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].is_satisfied()); // db was healthy
        assert!(matches!(results[1], WaitResult::TimedOutWarn { .. })); // cache timed out with warn
    }

    #[test]
    fn test_wait_result_helpers() {
        let satisfied = WaitResult::Satisfied;
        assert!(satisfied.is_satisfied());
        assert!(satisfied.should_continue());
        assert!(!satisfied.is_failure());

        let continue_result = WaitResult::TimedOutContinue;
        assert!(!continue_result.is_satisfied());
        assert!(continue_result.should_continue());
        assert!(!continue_result.is_failure());

        let warn = WaitResult::TimedOutWarn {
            service: "db".to_string(),
            condition: DependencyCondition::Healthy,
        };
        assert!(!warn.is_satisfied());
        assert!(warn.should_continue());
        assert!(!warn.is_failure());

        let fail = WaitResult::TimedOutFail {
            service: "db".to_string(),
            condition: DependencyCondition::Healthy,
            timeout: Duration::from_secs(60),
        };
        assert!(!fail.is_satisfied());
        assert!(!fail.should_continue());
        assert!(fail.is_failure());
    }
}
