//! Pure scalar graph primitives for staged worldgen fields.

mod builders;
mod cache;
mod node;
pub(crate) mod spline;
#[cfg(test)]
mod tests;

pub(crate) use self::cache::GraphEvaluationCache;

use self::node::Node;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_GRAPH_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct SamplePoint {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl SamplePoint {
    pub(crate) const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct Channel(String);

impl Channel {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        assert!(
            !name.is_empty(),
            "worldgen graph channel names cannot be empty"
        );
        Self(name)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Channel {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for Channel {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
struct GraphId(u64);

impl GraphId {
    fn next() -> Self {
        Self(NEXT_GRAPH_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct NodeId {
    graph_id: GraphId,
    index: usize,
}

impl NodeId {
    fn new(graph_id: GraphId, index: usize) -> Self {
        Self { graph_id, index }
    }

    fn with_graph(self, graph_id: GraphId) -> Self {
        Self {
            graph_id,
            index: self.index,
        }
    }
}

#[cfg(test)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum Axis {
    X,
    Y,
    Z,
}

pub(crate) trait SampledScalarField: Debug + Send + Sync {
    fn sample(&self, point: SamplePoint) -> f64;

    fn depends_on_y(&self) -> bool {
        true
    }
}

#[derive(Debug)]
pub(crate) struct ScalarGraph {
    graph_id: GraphId,
    nodes: Vec<Node>,
    y_dependencies: Vec<bool>,
    outputs: BTreeMap<String, NodeId>,
}

impl Default for ScalarGraph {
    fn default() -> Self {
        Self {
            graph_id: GraphId::next(),
            nodes: Vec::new(),
            y_dependencies: Vec::new(),
            outputs: BTreeMap::new(),
        }
    }
}

impl Clone for ScalarGraph {
    fn clone(&self) -> Self {
        let graph_id = GraphId::next();
        Self {
            graph_id,
            nodes: self
                .nodes
                .iter()
                .map(|node| node.clone().with_graph(graph_id))
                .collect(),
            y_dependencies: self.y_dependencies.clone(),
            outputs: self
                .outputs
                .iter()
                .map(|(channel, node)| (channel.clone(), node.with_graph(graph_id)))
                .collect(),
        }
    }
}
