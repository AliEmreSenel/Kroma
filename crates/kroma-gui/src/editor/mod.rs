//! Node-based shader editor — GUI integration.
//!
//! The pure graph types, parser, and codegen live in the `kroma_graph` crate.
//! This module re-exports them and adds GUI-specific rendering via [`canvas`].

pub mod canvas;

// Re-export everything from kroma-graph so the rest of kroma-gui can use it.
pub use kroma_graph::{
    graph::ShaderGraph,
    lowering::parse_glsl_to_graph,
    types::*,
};

// Re-export sub-modules

// ---------------------------------------------------------------------------
// GUI-specific extensions on Node
// ---------------------------------------------------------------------------

/// Extension trait adding visual layout methods to [`Node`].
pub trait NodeLayout {
    fn width(&self) -> f32;
    fn height(&self) -> f32;
    fn port_position(&self, direction: PortDirection, index: usize) -> [f32; 2];
}

impl NodeLayout for Node {
    /// The visual width of this node in pixels.
    fn width(&self) -> f32 {
        160.0
    }

    /// Visual height of the node.
    fn height(&self) -> f32 {
        let port_count = self.inputs().len().max(self.outputs().len());
        let header = 28.0;
        let port_rows = port_count as f32 * 24.0;
        (header + port_rows + 8.0).max(60.0)
    }

    /// Centre position of a specific port (for drawing wires).
    fn port_position(&self, direction: PortDirection, index: usize) -> [f32; 2] {
        let x = match direction {
            PortDirection::Input => self.position[0],
            PortDirection::Output => self.position[0] + self.width(),
        };
        let y = self.position[1] + 28.0 + index as f32 * 24.0 + 12.0;
        [x, y]
    }
}
