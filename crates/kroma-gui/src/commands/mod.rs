//! Command pattern for undo/redo — all graph mutations go through commands.
//!
//! Each command implements [`UndoableCommand`] with `execute()` and `undo()`.
//! [`CommandHistory`] manages the undo/redo stacks.

use kroma_graph::graph::ShaderGraph;
use kroma_graph::types::*;
use std::collections::VecDeque;

// ---------------------------------------------------------------------------
// Graph target
// ---------------------------------------------------------------------------

/// Which graph a command operates on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphTarget {
    /// The main editor graph.
    Editor,
    /// The shade package graph.
    Shade,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// A command that can be executed and undone on a shader graph.
pub trait UndoableCommand: std::fmt::Debug + Send {
    /// Execute the command, mutating the graph.
    fn execute(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()>;

    /// Undo the command, restoring the previous state.
    fn undo(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()>;

    /// A human-readable description of this command (for UI display).
    fn description(&self) -> &str;

    /// Which graph this command targets.
    fn target(&self) -> GraphTarget;
}

// ---------------------------------------------------------------------------
// Command history
// ---------------------------------------------------------------------------

/// Manages undo/redo stacks of commands.
pub struct CommandHistory {
    undo_stack: VecDeque<Box<dyn UndoableCommand>>,
    redo_stack: VecDeque<Box<dyn UndoableCommand>>,
    max_history: usize,
}

impl CommandHistory {
    /// Create a new command history with the given maximum stack size.
    pub fn new(max_history: usize) -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            max_history,
        }
    }

    /// Execute a command on the appropriate graph.
    /// Clears the redo stack (you can't redo after a new action).
    pub fn execute(
        &mut self,
        mut cmd: Box<dyn UndoableCommand>,
        editor_graph: &mut ShaderGraph,
        shade_graph: &mut ShaderGraph,
    ) -> anyhow::Result<()> {
        let graph = match cmd.target() {
            GraphTarget::Editor => editor_graph,
            GraphTarget::Shade => shade_graph,
        };
        cmd.execute(graph)?;
        self.undo_stack.push_back(cmd);
        self.redo_stack.clear();

        if self.undo_stack.len() > self.max_history {
            self.undo_stack.pop_front();
        }
        Ok(())
    }

    /// Undo the most recent command.
    pub fn undo(
        &mut self,
        editor_graph: &mut ShaderGraph,
        shade_graph: &mut ShaderGraph,
    ) -> anyhow::Result<bool> {
        if let Some(mut cmd) = self.undo_stack.pop_back() {
            let graph = match cmd.target() {
                GraphTarget::Editor => editor_graph,
                GraphTarget::Shade => shade_graph,
            };
            cmd.undo(graph)?;
            self.redo_stack.push_back(cmd);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Redo the most recently undone command.
    pub fn redo(
        &mut self,
        editor_graph: &mut ShaderGraph,
        shade_graph: &mut ShaderGraph,
    ) -> anyhow::Result<bool> {
        if let Some(mut cmd) = self.redo_stack.pop_back() {
            let graph = match cmd.target() {
                GraphTarget::Editor => editor_graph,
                GraphTarget::Shade => shade_graph,
            };
            cmd.execute(graph)?;
            self.undo_stack.push_back(cmd);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Description of the next undo action, if any.
    pub fn undo_description(&self) -> Option<&str> {
        self.undo_stack.back().map(|cmd| cmd.description())
    }

    /// Description of the next redo action, if any.
    pub fn redo_description(&self) -> Option<&str> {
        self.redo_stack.back().map(|cmd| cmd.description())
    }
}

impl Default for CommandHistory {
    fn default() -> Self {
        Self::new(500)
    }
}

// ===========================================================================
// Concrete graph commands
// ===========================================================================

// ---------------------------------------------------------------------------
// AddNode
// ---------------------------------------------------------------------------

/// Add a node to the graph.
#[derive(Debug)]
pub struct AddNodeCmd {
    target: GraphTarget,
    kind: NodeKind,
    position: [f32; 2],
    /// Filled after execute — the ID of the created node.
    created_id: Option<NodeId>,
}

impl AddNodeCmd {
    pub fn new(target: GraphTarget, kind: NodeKind, position: [f32; 2]) -> Self {
        Self {
            target,
            kind,
            position,
            created_id: None,
        }
    }
}

impl UndoableCommand for AddNodeCmd {
    fn execute(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()> {
        let id = graph.add_node(self.kind.clone(), self.position);
        self.created_id = Some(id);
        Ok(())
    }

    fn undo(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()> {
        if let Some(id) = self.created_id {
            graph.remove_node(id);
        }
        Ok(())
    }

    fn description(&self) -> &str {
        "Add Node"
    }

    fn target(&self) -> GraphTarget {
        self.target
    }
}

// ---------------------------------------------------------------------------
// RemoveNodes — removes one or more selected nodes + their connections
// ---------------------------------------------------------------------------

/// Snapshot of a removed node and its connections (for undo).
#[derive(Debug, Clone)]
struct RemovedNodeSnapshot {
    node: Node,
    connections: Vec<Connection>,
}

/// Remove selected nodes from the graph.
#[derive(Debug)]
pub struct RemoveNodesCmd {
    target: GraphTarget,
    node_ids: Vec<NodeId>,
    /// Snapshots saved during execute for undo restoration.
    snapshots: Vec<RemovedNodeSnapshot>,
}

impl RemoveNodesCmd {
    pub fn new(target: GraphTarget, node_ids: Vec<NodeId>) -> Self {
        Self {
            target,
            node_ids,
            snapshots: Vec::new(),
        }
    }
}

impl UndoableCommand for RemoveNodesCmd {
    fn execute(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()> {
        self.snapshots.clear();
        // Snapshot ALL nodes and their connections BEFORE removing any,
        // so shared inter-node connections are captured.
        for &id in &self.node_ids {
            if let Some(node) = graph.node(id) {
                let node_snapshot = node.clone();
                let conns: Vec<Connection> = graph
                    .connections()
                    .iter()
                    .filter(|c| c.from.node == id || c.to.node == id)
                    .cloned()
                    .collect();
                self.snapshots.push(RemovedNodeSnapshot {
                    node: node_snapshot,
                    connections: conns,
                });
            }
        }
        // Now remove all nodes
        for &id in &self.node_ids {
            graph.remove_node(id);
        }
        Ok(())
    }

    fn undo(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()> {
        // Restore nodes and connections in reverse order
        for snap in self.snapshots.iter().rev() {
            graph.restore_node(snap.node.clone());
            for conn in &snap.connections {
                graph.force_add_connection(conn.from, conn.to);
            }
        }
        Ok(())
    }

    fn description(&self) -> &str {
        "Delete Nodes"
    }

    fn target(&self) -> GraphTarget {
        self.target
    }
}

// ---------------------------------------------------------------------------
// MoveNode
// ---------------------------------------------------------------------------

/// Move a node (and optionally other selected nodes) to a new position.
#[derive(Debug)]
pub struct MoveNodeCmd {
    target: GraphTarget,
    node_id: NodeId,
    old_position: [f32; 2],
    new_position: [f32; 2],
    /// Additional nodes moved as part of a multi-select drag:
    /// (node_id, old_position, new_position).
    others: Vec<(NodeId, [f32; 2], [f32; 2])>,
}

impl MoveNodeCmd {
    /// Create a move command that also tracks companion nodes from a
    /// multi-select drag, so undo restores all of them.
    pub fn new_multi(
        target: GraphTarget,
        node_id: NodeId,
        old_position: [f32; 2],
        new_position: [f32; 2],
        others: Vec<(NodeId, [f32; 2], [f32; 2])>,
    ) -> Self {
        Self {
            target,
            node_id,
            old_position,
            new_position,
            others,
        }
    }
}

impl UndoableCommand for MoveNodeCmd {
    fn execute(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()> {
        if let Some(node) = graph.node_mut(self.node_id) {
            node.position = self.new_position;
        }
        for &(id, _, new_pos) in &self.others {
            if let Some(node) = graph.node_mut(id) {
                node.position = new_pos;
            }
        }
        Ok(())
    }

    fn undo(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()> {
        if let Some(node) = graph.node_mut(self.node_id) {
            node.position = self.old_position;
        }
        for &(id, old_pos, _) in &self.others {
            if let Some(node) = graph.node_mut(id) {
                node.position = old_pos;
            }
        }
        Ok(())
    }

    fn description(&self) -> &str {
        "Move Node"
    }

    fn target(&self) -> GraphTarget {
        self.target
    }
}

// ---------------------------------------------------------------------------
// Connect
// ---------------------------------------------------------------------------

/// Create a connection between two ports.
#[derive(Debug)]
pub struct ConnectCmd {
    target: GraphTarget,
    from: PortAddr,
    to: PortAddr,
    /// The ID of the created connection (filled on execute).
    created_id: Option<ConnectionId>,
    /// If an existing connection to the same input port was displaced,
    /// it's saved here for undo.
    displaced: Option<Connection>,
}

impl ConnectCmd {
    pub fn new(target: GraphTarget, from: PortAddr, to: PortAddr) -> Self {
        Self {
            target,
            from,
            to,
            created_id: None,
            displaced: None,
        }
    }
}

impl UndoableCommand for ConnectCmd {
    fn execute(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()> {
        // Check for existing connection to the same input port (will be displaced)
        self.displaced = graph
            .connections()
            .iter()
            .find(|c| c.to == self.to)
            .cloned();

        if let Some(id) = graph.add_connection(self.from, self.to) {
            self.created_id = Some(id);
        }
        Ok(())
    }

    fn undo(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()> {
        // Remove the created connection
        if let Some(id) = self.created_id {
            graph.remove_connection(id);
        }
        // Restore the displaced connection
        if let Some(ref conn) = self.displaced {
            graph.force_add_connection(conn.from, conn.to);
        }
        Ok(())
    }

    fn description(&self) -> &str {
        "Connect Ports"
    }

    fn target(&self) -> GraphTarget {
        self.target
    }
}

// ---------------------------------------------------------------------------
// Disconnect
// ---------------------------------------------------------------------------

/// Remove a connection.
#[derive(Debug)]
pub struct DisconnectCmd {
    target: GraphTarget,
    connection_id: ConnectionId,
    /// Snapshot of the removed connection for undo.
    snapshot: Option<Connection>,
}

impl DisconnectCmd {
    pub fn new(target: GraphTarget, connection_id: ConnectionId) -> Self {
        Self {
            target,
            connection_id,
            snapshot: None,
        }
    }
}

impl UndoableCommand for DisconnectCmd {
    fn execute(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()> {
        // Save the connection before removing
        self.snapshot = graph
            .connections()
            .iter()
            .find(|c| c.id == self.connection_id)
            .cloned();
        graph.remove_connection(self.connection_id);
        Ok(())
    }

    fn undo(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()> {
        if let Some(ref conn) = self.snapshot {
            graph.force_add_connection(conn.from, conn.to);
        }
        Ok(())
    }

    fn description(&self) -> &str {
        "Disconnect"
    }

    fn target(&self) -> GraphTarget {
        self.target
    }
}

// ---------------------------------------------------------------------------
// ChangeDefault
// ---------------------------------------------------------------------------

/// Change a node's default value for a specific input port.
#[derive(Debug)]
pub struct ChangeDefaultCmd {
    target: GraphTarget,
    node_id: NodeId,
    port_index: usize,
    old_value: DefaultValue,
    new_value: DefaultValue,
}

impl ChangeDefaultCmd {
    pub fn new(
        target: GraphTarget,
        node_id: NodeId,
        port_index: usize,
        old_value: DefaultValue,
        new_value: DefaultValue,
    ) -> Self {
        Self {
            target,
            node_id,
            port_index,
            old_value,
            new_value,
        }
    }
}

impl UndoableCommand for ChangeDefaultCmd {
    fn execute(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()> {
        if let Some(node) = graph.node_mut(self.node_id)
            && self.port_index < node.defaults.len()
        {
            node.defaults[self.port_index] = self.new_value.clone();
        }
        Ok(())
    }

    fn undo(&mut self, graph: &mut ShaderGraph) -> anyhow::Result<()> {
        if let Some(node) = graph.node_mut(self.node_id)
            && self.port_index < node.defaults.len()
        {
            node.defaults[self.port_index] = self.old_value.clone();
        }
        Ok(())
    }

    fn description(&self) -> &str {
        "Change Default"
    }

    fn target(&self) -> GraphTarget {
        self.target
    }
}
