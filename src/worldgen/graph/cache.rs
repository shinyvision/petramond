use super::{GraphId, NodeId, ScalarGraph};

#[derive(Debug)]
pub(crate) struct GraphEvaluationCache {
    graph_id: GraphId,
    values: Vec<f64>,
    stamps: Vec<u32>,
    generation: u32,
    y_invariant_values: Vec<f64>,
    y_invariant_stamps: Vec<u32>,
    y_invariant_generation: u32,
    y_invariant_scope_active: bool,
}

impl GraphEvaluationCache {
    pub(super) fn new(graph: &ScalarGraph) -> Self {
        Self {
            graph_id: graph.graph_id,
            values: vec![0.0; graph.nodes.len()],
            stamps: vec![0; graph.nodes.len()],
            generation: 0,
            y_invariant_values: vec![0.0; graph.nodes.len()],
            y_invariant_stamps: vec![0; graph.nodes.len()],
            y_invariant_generation: 0,
            y_invariant_scope_active: false,
        }
    }

    pub(super) fn begin_sample(&mut self, graph: &ScalarGraph) {
        self.ensure_graph_capacity(graph);
        advance_generation(&mut self.generation, &mut self.stamps);
    }

    pub(crate) fn begin_y_invariant_column(&mut self, graph: &ScalarGraph) {
        self.ensure_graph_capacity(graph);
        self.y_invariant_scope_active = true;
        advance_generation(
            &mut self.y_invariant_generation,
            &mut self.y_invariant_stamps,
        );
    }

    fn ensure_graph_capacity(&mut self, graph: &ScalarGraph) {
        assert_eq!(
            self.graph_id, graph.graph_id,
            "graph evaluation cache belongs to a different scalar graph"
        );
        if self.values.len() < graph.nodes.len() {
            self.values.resize(graph.nodes.len(), 0.0);
            self.stamps.resize(graph.nodes.len(), 0);
            self.y_invariant_values.resize(graph.nodes.len(), 0.0);
            self.y_invariant_stamps.resize(graph.nodes.len(), 0);
        }
    }

    pub(super) fn get(&self, node: NodeId, y_invariant: bool) -> Option<f64> {
        if self.use_y_invariant_scope(y_invariant) {
            (self.y_invariant_stamps[node.index] == self.y_invariant_generation)
                .then_some(self.y_invariant_values[node.index])
        } else {
            (self.stamps[node.index] == self.generation).then_some(self.values[node.index])
        }
    }

    pub(super) fn store(&mut self, node: NodeId, value: f64, y_invariant: bool) -> f64 {
        if self.use_y_invariant_scope(y_invariant) {
            self.y_invariant_values[node.index] = value;
            self.y_invariant_stamps[node.index] = self.y_invariant_generation;
        } else {
            self.values[node.index] = value;
            self.stamps[node.index] = self.generation;
        }
        value
    }

    fn use_y_invariant_scope(&self, y_invariant: bool) -> bool {
        y_invariant && self.y_invariant_scope_active
    }
}

fn advance_generation(generation: &mut u32, stamps: &mut [u32]) {
    if *generation == u32::MAX {
        stamps.fill(0);
        *generation = 1;
    } else {
        *generation += 1;
    }
}
