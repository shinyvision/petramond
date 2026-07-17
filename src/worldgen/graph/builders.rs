use super::node::Node;
use super::spline::{CubicSpline, SplineAxis};
#[cfg(test)]
use super::Axis;
use super::{Channel, NodeId, SampledScalarField, ScalarGraph};
use std::sync::Arc;

/// Declares `ScalarGraph` node-builder methods. Every argument tagged
/// `=> "context"` is a `NodeId` input asserted to belong to this graph before
/// the constructed node is pushed; untagged arguments pass through as plain
/// parameters. Only the construction scaffold lives here — evaluation is
/// `Node::sample`.
macro_rules! node_builders {
    ($(
        $(#[$meta:meta])*
        fn $name:ident($($arg:ident: $ty:ty $(=> $context:literal)?),* $(,)?) -> $node:expr;
    )*) => {
        $(
            $(#[$meta])*
            pub(crate) fn $name(&mut self $(, $arg: $ty)*) -> NodeId {
                $($(self.assert_existing_node($arg, $context);)?)*
                self.push($node)
            }
        )*
    };
}

impl ScalarGraph {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    node_builders! {
        fn constant(value: f64) -> Node::Constant(value);
        #[cfg(test)]
        fn axis(axis: Axis) -> Node::Axis(axis);
        fn sampled_field(field: impl SampledScalarField + 'static)
            -> Node::SampledField(Arc::new(field));
        fn add(a: NodeId => "add left input", b: NodeId => "add right input")
            -> Node::Add(a, b);
        fn multiply(a: NodeId => "multiply left input", b: NodeId => "multiply right input")
            -> Node::Multiply(a, b);
        #[cfg(test)]
        fn min(a: NodeId => "min left input", b: NodeId => "min right input")
            -> Node::Min(a, b);
        #[cfg(test)]
        fn max(a: NodeId => "max left input", b: NodeId => "max right input")
            -> Node::Max(a, b);
        fn abs(input: NodeId => "abs input") -> Node::Abs(input);
        fn ridge_fold(input: NodeId => "ridge fold input") -> Node::RidgeFold(input);
        fn terrace(input: NodeId => "terrace input", step: f64) -> Node::Terrace { input, step };
        fn clamp(input: NodeId => "clamp input", min: f64, max: f64)
            -> Node::Clamp { input, min, max };
        fn lerp(
            a: NodeId => "lerp first input",
            b: NodeId => "lerp second input",
            t: NodeId => "lerp selector input",
        ) -> Node::Lerp { a, b, t };
        #[cfg(test)]
        fn vertical_ramp(y_min: f64, y_max: f64) -> Node::VerticalRamp { y_min, y_max };
        fn vertical_bias(base_height: NodeId => "vertical bias base height input")
            -> Node::VerticalBias { base_height };
        fn floor_clamp(
            input: NodeId => "floor clamp input",
            floor_y: f64,
            fade_height: f64,
            solid_density: f64,
        ) -> Node::FloorClamp { input, floor_y, fade_height, solid_density };
        #[cfg(test)]
        fn range_select(
            selector: NodeId => "range selector input",
            min: f64,
            max: f64,
            inside: NodeId => "range inside input",
            outside: NodeId => "range outside input",
        ) -> Node::RangeSelect { selector, min, max, inside, outside };
    }

    pub(crate) fn spline(
        &mut self,
        spline: CubicSpline,
        inputs: impl Into<Vec<(SplineAxis, NodeId)>>,
    ) -> NodeId {
        let inputs = inputs.into();
        for required_axis in spline.required_axes() {
            assert!(
                inputs.iter().any(|(axis, _)| axis == &required_axis),
                "spline graph node must bind required axis '{}'",
                required_axis.as_str()
            );
        }
        for (_, node) in &inputs {
            self.assert_existing_node(*node, "spline input");
        }
        self.push(Node::Spline {
            spline: Box::new(spline),
            inputs,
        })
    }

    pub(crate) fn set_channel(&mut self, channel: Channel, node: NodeId) {
        self.assert_existing_node(node, "channel output");
        self.outputs.insert(channel.0, node);
    }

    fn push(&mut self, node: Node) -> NodeId {
        let id = NodeId::new(self.graph_id, self.nodes.len());
        let depends_on_y = self.node_definition_depends_on_y(&node);
        self.nodes.push(node);
        self.y_dependencies.push(depends_on_y);
        id
    }

    pub(super) fn assert_existing_node(&self, node: NodeId, context: &str) {
        assert_eq!(
            node.graph_id, self.graph_id,
            "{context} belongs to a different scalar graph"
        );
        assert!(
            node.index < self.nodes.len(),
            "{context} must reference an existing scalar graph node"
        );
    }
}
