//! The shader graph — nodes, connections, and GLSL compilation.

use std::collections::{HashMap, VecDeque};
use crate::types::*;
use crate::nodes;

/// The complete node graph — nodes + connections.
pub struct ShaderGraph {
    nodes: HashMap<NodeId, Node>,
    connections: Vec<Connection>,
    next_node_id: u64,
    next_conn_id: u64,
    /// Helper function definitions collected during GLSL parsing.
    /// Each entry is `(name, full_glsl_source)` so they can be re-emitted.
    pub helper_functions: Vec<(String, String)>,
}

impl Default for ShaderGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl ShaderGraph {
    /// Create a new empty graph with a default Output node.
    pub fn new() -> Self {
        let output_id = NodeId(1);
        let output = Node::new(output_id, NodeKind::Output, [500.0, 250.0]);
        let mut nodes = HashMap::new();
        nodes.insert(output_id, output);
        Self {
            nodes,
            connections: Vec::new(),
            next_node_id: 2,
            next_conn_id: 1,
            helper_functions: Vec::new(),
        }
    }

    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    pub fn nodes_mut(&mut self) -> impl Iterator<Item = &mut Node> {
        self.nodes.values_mut()
    }

    pub fn connections(&self) -> &[Connection] {
        &self.connections
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    pub fn add_node(&mut self, kind: NodeKind, position: [f32; 2]) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;
        self.nodes.insert(id, Node::new(id, kind, position));
        id
    }

    pub fn remove_node(&mut self, id: NodeId) {
        if self.nodes.get(&id).map(|n| n.kind == NodeKind::Output).unwrap_or(false) {
            return;
        }
        self.nodes.remove(&id);
        self.connections.retain(|c| c.from.node != id && c.to.node != id);
    }

    /// Re-insert a previously removed node (used for undo).
    ///
    /// This places the node back into the map without incrementing the ID
    /// counter, preserving the original `NodeId`.
    pub fn restore_node(&mut self, node: Node) {
        self.nodes.insert(node.id, node);
    }

    /// Try to add a connection. Returns None if types are incompatible or
    /// the input port is already connected.
    pub fn add_connection(&mut self, from: PortAddr, to: PortAddr) -> Option<ConnectionId> {
        let from_node = self.nodes.get(&from.node)?;
        let to_node = self.nodes.get(&to.node)?;

        let from_def = from_node.outputs().get(from.port)?;
        let to_def = to_node.inputs().get(to.port)?;

        // Allow connections if types match, or Float can promote to any type,
        // or any type can be implicitly read as Float (e.g., .x component)
        if from_def.data_type != to_def.data_type
            && from_def.data_type != DataType::Float
            && to_def.data_type != DataType::Float
        {
            return None;
        }

        if from.node == to.node {
            return None;
        }

        self.connections.retain(|c| c.to != to);

        let id = ConnectionId(self.next_conn_id);
        self.next_conn_id += 1;
        self.connections.push(Connection { id, from, to });
        Some(id)
    }

    pub fn remove_connection(&mut self, id: ConnectionId) {
        self.connections.retain(|c| c.id != id);
    }

    /// Force-add a connection without validation (used during parsing).
    pub fn force_add_connection(&mut self, from: PortAddr, to: PortAddr) {
        self.connections.retain(|c| c.to != to);
        let id = ConnectionId(self.next_conn_id);
        self.next_conn_id += 1;
        self.connections.push(Connection {
            id,
            from,
            to,
        });
    }

    pub fn connections_mut(&mut self) -> &mut Vec<Connection> {
        &mut self.connections
    }

    /// Find the connection feeding a specific input port.
    fn input_source(&self, addr: PortAddr) -> Option<PortAddr> {
        self.connections.iter().find(|c| c.to == addr).map(|c| c.from)
    }

    // -----------------------------------------------------------------------
    // Auto-layout
    // -----------------------------------------------------------------------

    /// Arrange nodes in left-to-right columns based on their dependency depth.
    ///
    /// Source nodes (no incoming connections) go in column 0; the Output node
    /// is placed in the rightmost column.  Within a column, nodes are spaced
    /// vertically.
    pub fn auto_layout(&mut self) {
        const COL_SPACING: f32 = 260.0;
        const ROW_SPACING: f32 = 100.0;
        const NODE_HEIGHT_ESTIMATE: f32 = 80.0;
        const MARGIN_X: f32 = 60.0;
        const MARGIN_Y: f32 = 60.0;

        let node_ids: Vec<NodeId> = self.nodes.keys().copied().collect();
        if node_ids.is_empty() {
            return;
        }

        // Build adjacency: for each node, which nodes does it feed into?
        // Connection goes from.node → to.node (data flows left to right).
        let mut in_edges: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut out_edges: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for id in &node_ids {
            in_edges.insert(*id, Vec::new());
            out_edges.insert(*id, Vec::new());
        }
        for conn in &self.connections {
            in_edges.entry(conn.to.node).or_default().push(conn.from.node);
            out_edges.entry(conn.from.node).or_default().push(conn.to.node);
        }

        // Compute depth via longest-path from sources (ensures nodes feeding
        // the same target are in different columns).
        let mut depth: HashMap<NodeId, usize> = HashMap::new();
        let mut queue: VecDeque<NodeId> = VecDeque::new();

        // Sources: nodes with no incoming edges
        for &id in &node_ids {
            if in_edges.get(&id).is_none_or(|v| v.is_empty()) {
                depth.insert(id, 0);
                queue.push_back(id);
            }
        }

        // BFS relaxation (longest path in DAG)
        // Guard against cycles: cap iterations to prevent infinite loops.
        let max_iterations = node_ids.len() * node_ids.len();
        let mut iterations = 0;
        while let Some(id) = queue.pop_front() {
            iterations += 1;
            if iterations > max_iterations {
                eprintln!("[kroma-graph] auto_layout: iteration limit reached — graph may contain cycles");
                break;
            }
            let d = depth[&id];
            if let Some(successors) = out_edges.get(&id) {
                for &succ in successors {
                    let current = depth.get(&succ).copied().unwrap_or(0);
                    if d + 1 > current {
                        depth.insert(succ, d + 1);
                        queue.push_back(succ);
                    }
                }
            }
        }

        // Assign depth 0 to any orphan nodes not reached
        for &id in &node_ids {
            depth.entry(id).or_insert(0);
        }

        // Ensure the Output node sits in the rightmost column
        let max_depth = depth.values().copied().max().unwrap_or(0);
        if let Some(output_id) = self.nodes.values()
            .find(|n| n.kind == NodeKind::Output)
            .map(|n| n.id)
        {
            depth.insert(output_id, max_depth);
        }

        // Group nodes by column
        let final_max = depth.values().copied().max().unwrap_or(0);
        let mut columns: Vec<Vec<NodeId>> = vec![Vec::new(); final_max + 1];
        for (&id, &d) in &depth {
            columns[d].push(id);
        }

        // Sort nodes within each column by their first connection order
        // for deterministic and aesthetic results
        for col in &mut columns {
            col.sort_by_key(|id| id.0);
        }

        // Assign positions
        for (col_idx, col_nodes) in columns.iter().enumerate() {
            let x = MARGIN_X + col_idx as f32 * COL_SPACING;
            // Centre the column vertically
            let col_height = col_nodes.len() as f32 * (NODE_HEIGHT_ESTIMATE + ROW_SPACING) - ROW_SPACING;
            let start_y = MARGIN_Y + (600.0 - col_height).max(0.0) / 2.0;
            for (row_idx, &id) in col_nodes.iter().enumerate() {
                let y = start_y + row_idx as f32 * (NODE_HEIGHT_ESTIMATE + ROW_SPACING);
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.position = [x, y];
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // GLSL code generation
    // -----------------------------------------------------------------------

    /// Compile the graph into a standalone Shadertoy-compatible GLSL fragment.
    pub fn compile_glsl(&self) -> Result<String, String> {
        let output_node = self.nodes.values().find(|n| n.kind == NodeKind::Output)
            .ok_or("No Output node in graph")?;

        let mut visited: HashMap<NodeId, String> = HashMap::new();
        let mut code_lines: Vec<String> = Vec::new();
        let mut counter = 0u32;

        let color_input = PortAddr { node: output_node.id, port: 0 };
        let color_expr = if let Some(source) = self.input_source(color_input) {
            let src_expr = self.eval_node(source.node, &mut visited, &mut code_lines, &mut counter)?;
            let src_node = self.nodes.get(&source.node)
                .ok_or_else(|| format!("Missing node {} referenced by connection", source.node))?;
            if src_node.outputs().len() > 1 {
                format!("{}_o{}", src_expr, source.port)
            } else {
                src_expr
            }
        } else {
            output_node.defaults.first()
                .map(|d| d.to_glsl())
                .unwrap_or_else(|| "vec4(0.0, 0.0, 0.0, 1.0)".into())
        };

        let mut glsl = String::new();
        glsl.push_str("// Generated by Kroma Shader Editor\n");
        glsl.push_str("// Shadertoy-compatible fragment shader\n\n");

        glsl.push_str(&nodes::helper_functions(&self.nodes));

        // Emit helper functions stored during GLSL parsing
        for (_name, source) in &self.helper_functions {
            glsl.push_str(source);
            glsl.push_str("\n\n");
        }

        glsl.push('\n');

        glsl.push_str("void mainImage(out vec4 fragColor, in vec2 fragCoord) {\n");
        for line in &code_lines {
            glsl.push_str("    ");
            glsl.push_str(line);
            glsl.push('\n');
        }
        glsl.push_str("    fragColor = ");
        glsl.push_str(&color_expr);
        glsl.push_str(";\n}\n");

        Ok(glsl)
    }

    fn eval_node(
        &self,
        id: NodeId,
        visited: &mut HashMap<NodeId, String>,
        code: &mut Vec<String>,
        counter: &mut u32,
    ) -> Result<String, String> {
        if let Some(expr) = visited.get(&id) {
            // A sentinel of "" means we're currently evaluating this node (cycle).
            if expr.is_empty() {
                return Err(format!("Cycle detected at node {:?}", id));
            }
            return Ok(expr.clone());
        }

        // Mark as "currently evaluating" to detect cycles
        visited.insert(id, String::new());

        let node = self.nodes.get(&id).ok_or("Missing node in graph")?;

        let mut input_exprs: Vec<String> = Vec::new();
        for (i, _port_def) in node.inputs().iter().enumerate() {
            let addr = PortAddr { node: id, port: i };
            let expr = if let Some(source) = self.input_source(addr) {
                let src_expr = self.eval_node(source.node, visited, code, counter)?;
                let src_node = self.nodes.get(&source.node)
                    .ok_or_else(|| format!("Missing node {} referenced by connection", source.node))?;
                if src_node.outputs().len() > 1 {
                    format!("{}_o{}", src_expr, source.port)
                } else {
                    src_expr
                }
            } else {
                node.defaults.get(i)
                    .map(|d| d.to_glsl())
                    .unwrap_or_else(|| "0.0".to_string())
            };
            input_exprs.push(expr);
        }

        let var = format!("n{}", counter);
        *counter += 1;

        let snippet = node.kind.codegen(&input_exprs, &var, &node.defaults, &node.meta);
        if !snippet.is_empty() {
            code.push(snippet);
        }

        visited.insert(id, var.clone());
        Ok(var)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_empty_graph_uses_default_output() {
        let graph = ShaderGraph::new();
        let glsl = graph.compile_glsl().unwrap();
        assert!(glsl.contains("void mainImage("));
        // Default output is black with full alpha
        assert!(glsl.contains("fragColor = vec4(0.0"));
    }

    #[test]
    fn compile_time_to_output() {
        let mut graph = ShaderGraph::new();
        let time_id = graph.add_node(NodeKind::Time, [0.0, 0.0]);
        let combine_id = graph.add_node(NodeKind::Combine4, [200.0, 0.0]);
        // Connect Time→Combine4 port 0
        graph.add_connection(
            PortAddr { node: time_id, port: 0 },
            PortAddr { node: combine_id, port: 0 },
        );
        // The Output node is always NodeId(1) in a new graph
        let output_id = NodeId(1);
        graph.add_connection(
            PortAddr { node: combine_id, port: 0 },
            PortAddr { node: output_id, port: 0 },
        );
        let glsl = graph.compile_glsl().unwrap();
        assert!(glsl.contains("iTime"));
        assert!(glsl.contains("vec4"));
        assert!(glsl.contains("fragColor ="));
    }

    #[test]
    fn compile_add_node_uses_inferred_type() {
        let mut graph = ShaderGraph::new();
        let add_id = graph.add_node(NodeKind::Add, [100.0, 0.0]);
        let output_id = NodeId(1);
        graph.add_connection(
            PortAddr { node: add_id, port: 0 },
            PortAddr { node: output_id, port: 0 },
        );
        let glsl = graph.compile_glsl().unwrap();
        // Add with default Float inputs should declare a float variable
        assert!(glsl.contains("float n"));
    }

    #[test]
    fn cycle_detection_returns_error() {
        let mut graph = ShaderGraph::new();
        let a = graph.add_node(NodeKind::Add, [0.0, 0.0]);
        let b = graph.add_node(NodeKind::Add, [100.0, 0.0]);
        // A→B port 0
        graph.add_connection(
            PortAddr { node: a, port: 0 },
            PortAddr { node: b, port: 0 },
        );
        // B→A port 0 — creates a cycle
        graph.add_connection(
            PortAddr { node: b, port: 0 },
            PortAddr { node: a, port: 0 },
        );
        // Connect B to output to force traversal
        let output_id = NodeId(1);
        graph.add_connection(
            PortAddr { node: b, port: 0 },
            PortAddr { node: output_id, port: 0 },
        );
        let result = graph.compile_glsl();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cycle"));
    }
}
