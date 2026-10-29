//! Dependency graph construction and cycle detection (docs/guide/resources.md
//! §3.5). Cycles must be rejected at configuration load time, never during
//! recovery.

use crate::ids::ResourceId;
use std::collections::{BTreeMap, VecDeque};

/// Directed dependency graph: an edge `A -> B` means "A depends on B".
/// When recovering A, B is evaluated first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyGraph {
    edges: BTreeMap<ResourceId, Vec<ResourceId>>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GraphError {
    /// A dependency references a resource that does not exist.
    #[error("dependency reference to unknown resource `{0}`")]
    UnknownResource(ResourceId),
    /// A hard dependency cycle was found; `path` starts and ends with the
    /// same resource, e.g. `a -> b -> c -> a`.
    #[error("dependency cycle detected: {}", Self::format_path(.0))]
    Cycle(Vec<ResourceId>),
}

impl GraphError {
    fn format_path(path: &[ResourceId]) -> String {
        path.iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>()
            .join(" -> ")
    }
}

impl DependencyGraph {
    /// Builds a graph from `(dependent, dependencies)` pairs and rejects
    /// unknown references and cycles immediately.
    pub fn build(edges: Vec<(ResourceId, Vec<ResourceId>)>) -> Result<Self, GraphError> {
        let mut graph = Self::default();
        for (from, targets) in edges {
            graph.edges.entry(from).or_default().extend(targets);
        }
        graph.validate()?;
        Ok(graph)
    }

    /// All direct dependencies of `id`, empty if it has none.
    pub fn dependencies_of(&self, id: &ResourceId) -> &[ResourceId] {
        self.edges.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn nodes(&self) -> impl Iterator<Item = &ResourceId> {
        self.edges.keys()
    }

    /// Validates reference integrity and detects cycles with an iterative
    /// colored DFS (white/gray/black). The returned error path starts and
    /// ends at the resource where the cycle closes.
    pub fn validate(&self) -> Result<(), GraphError> {
        for (from, targets) in &self.edges {
            for target in targets {
                if !self.edges.contains_key(target) {
                    return Err(GraphError::UnknownResource(target.clone()));
                }
            }
            let _ = from;
        }
        self.detect_cycles()
    }

    fn detect_cycles(&self) -> Result<(), GraphError> {
        // 0 = unvisited (white), 1 = on stack (gray), 2 = done (black).
        let mut color: BTreeMap<&ResourceId, u8> = BTreeMap::new();
        for node in self.edges.keys() {
            color.insert(node, 0);
        }

        for root in self.edges.keys() {
            if color[root] != 0 {
                continue;
            }
            // Stack holds (node, position in its dependency list).
            let mut stack: Vec<(&ResourceId, usize)> = vec![(root, 0)];
            let mut path: Vec<ResourceId> = vec![root.clone()];
            color.insert(root, 1);

            while let Some((node, idx)) = stack.pop() {
                let deps = self.dependencies_of(node);
                if let Some(next) = deps.get(idx) {
                    stack.push((node, idx + 1));
                    match color[next.as_resource()] {
                        0 => {
                            color.insert(next.as_resource(), 1);
                            path.push(next.clone());
                            stack.push((next.as_resource(), 0));
                        }
                        1 => {
                            // Found a gray node: rotate `path` so it starts at
                            // the cycle entry, then close the loop.
                            let pos = path
                                .iter()
                                .position(|p| p == next.as_resource())
                                .unwrap_or(0);
                            let mut cycle: Vec<ResourceId> = path[pos..].to_vec();
                            cycle.push(next.clone());
                            return Err(GraphError::Cycle(cycle));
                        }
                        _ => {}
                    }
                } else {
                    color.insert(node, 2);
                    path.pop();
                }
            }
        }
        Ok(())
    }

    /// Topological order (dependencies first) via Kahn's algorithm. Only
    /// meaningful after `validate` succeeds; disconnected nodes are appended.
    pub fn topological_order(&self) -> Vec<ResourceId> {
        // Edge A -> B means "A depends on B", so B must be emitted before A.
        // `indegree` counts each node's unemitted dependencies; `dependents`
        // is the reverse adjacency used to decrement them.
        let mut indegree: BTreeMap<&ResourceId, usize> = BTreeMap::new();
        let mut dependents: BTreeMap<&ResourceId, Vec<&ResourceId>> = BTreeMap::new();
        for node in self.edges.keys() {
            indegree.insert(node, self.dependencies_of(node).len());
        }
        for (from, targets) in &self.edges {
            for target in targets {
                dependents.entry(target).or_default().push(from);
            }
        }

        // BTreeMap keys are sorted, so the queue is deterministic.
        let mut queue: VecDeque<&ResourceId> = indegree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(node, _)| *node)
            .collect();
        let mut order = Vec::with_capacity(indegree.len());

        while let Some(node) = queue.pop_front() {
            order.push(node.clone());
            for dependent in dependents.get(node).into_iter().flatten() {
                let entry = indegree.get_mut(dependent).unwrap();
                *entry -= 1;
                if *entry == 0 {
                    queue.push_back(dependent);
                }
            }
        }
        order
    }
}

// Internal helper so `&ResourceId` from a `&Vec<ResourceId>` can index the
// color/degree maps without cloning.
trait AsResourceKey {
    fn as_resource(&self) -> &ResourceId;
}

impl AsResourceKey for ResourceId {
    fn as_resource(&self) -> &ResourceId {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid(s: &str) -> ResourceId {
        ResourceId::new(s)
    }

    #[test]
    fn accepts_acyclic_graph() {
        let graph = DependencyGraph::build(vec![
            (rid("tunnel"), vec![rid("proxy")]),
            (rid("proxy"), vec![rid("network")]),
            (rid("network"), vec![]),
        ])
        .unwrap();
        assert_eq!(graph.dependencies_of(&rid("proxy")), &[rid("network")]);
    }

    #[test]
    fn rejects_unknown_dependency_reference() {
        let err = DependencyGraph::build(vec![(rid("tunnel"), vec![rid("ghost")])]).unwrap_err();
        assert_eq!(
            err,
            GraphError::UnknownResource(rid("ghost")),
            "dangling references must fail at load time"
        );
    }

    #[test]
    fn rejects_self_cycle() {
        let err = DependencyGraph::build(vec![(rid("a"), vec![rid("a")])]).unwrap_err();
        match err {
            GraphError::Cycle(path) => assert_eq!(path, vec![rid("a"), rid("a")]),
            other => panic!("expected cycle, got {other:?}"),
        }
    }

    #[test]
    fn rejects_three_node_cycle_with_named_path() {
        // tunnel -> proxy -> network -> tunnel
        let err = DependencyGraph::build(vec![
            (rid("tunnel"), vec![rid("proxy")]),
            (rid("proxy"), vec![rid("network")]),
            (rid("network"), vec![rid("tunnel")]),
        ])
        .unwrap_err();
        match &err {
            GraphError::Cycle(path) => {
                assert_eq!(path.len(), 4, "path must close the loop");
                assert_eq!(path.first(), path.last());
            }
            other => panic!("expected cycle, got {other:?}"),
        }
        assert!(
            err.to_string().contains("->"),
            "error message must show the cycle path, got: {err}"
        );
    }

    #[test]
    fn diamond_without_cycle_is_accepted() {
        // Both tunnel and sunlogin depend on proxy and network, but there is
        // no cycle (docs/guide/resources.md §3.7).
        DependencyGraph::build(vec![
            (rid("tunnel"), vec![rid("proxy")]),
            (rid("sunlogin"), vec![rid("proxy"), rid("network")]),
            (rid("proxy"), vec![rid("network")]),
            (rid("network"), vec![]),
        ])
        .unwrap();
    }

    #[test]
    fn topological_order_puts_dependencies_first() {
        let graph = DependencyGraph::build(vec![
            (rid("tunnel"), vec![rid("proxy")]),
            (rid("proxy"), vec![rid("network")]),
            (rid("network"), vec![]),
        ])
        .unwrap();
        let order = graph.topological_order();
        let pos = |s: &str| order.iter().position(|id| id.as_str() == s).unwrap();
        assert!(pos("network") < pos("proxy"));
        assert!(pos("proxy") < pos("tunnel"));
    }
}
