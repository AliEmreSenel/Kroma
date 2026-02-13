//! Docking system — IDE-style panel layout with binary split tree.
//!
//! The dock state is a binary tree where each leaf holds one or more panels
//! as tabs, and internal nodes represent horizontal or vertical splits with
//! a draggable ratio.

pub mod tree;

pub use tree::{DockNode, DropZone};

use crate::panels::PanelId;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Dock messages
// ---------------------------------------------------------------------------

/// Messages produced by dock interactions (divider drag, tab switch, etc.).
#[derive(Debug, Clone)]
pub enum DockMessage {
    /// User dropped a panel onto a drop zone.
    PanelDropped {
        panel_id: PanelId,
        target_path: Vec<PathDir>,
        zone: DropZone,
    },
}

/// Direction taken at a split node when navigating the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PathDir {
    Left,
    Right,
}

// ---------------------------------------------------------------------------
// DockState — top-level dock manager
// ---------------------------------------------------------------------------

/// The persistent state for the docking system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockState {
    /// Root of the dock tree.
    pub root: DockNode,
}

impl DockState {
    /// Create a new dock state with the default layout.
    pub fn default_layout() -> Self {
        Self {
            root: DockNode::default_layout(),
        }
    }

    /// Handle a dock message, mutating the tree in place.
    pub fn update(&mut self, msg: DockMessage) {
        let DockMessage::PanelDropped {
            panel_id,
            target_path,
            zone,
        } = msg;
        // Remove panel from its current location, then insert at the
        // target — but do NOT cleanup() between remove and insert.
        //
        // cleanup() collapses empty leaves and trivial splits, which
        // restructures the tree and invalidates `target_path` (it was
        // captured at render time, before the remove).  By deferring
        // cleanup to after the insert, the tree shape is unchanged
        // while `target_path` is resolved.
        self.root.remove_panel(panel_id);
        self.root.insert_panel(panel_id, &target_path, zone);
        self.root.cleanup();
    }
}

#[cfg(test)]
impl DockState {
    /// Collect all panel IDs that are present in the tree.
    pub fn visible_panels(&self) -> Vec<PanelId> {
        let mut out = Vec::new();
        self.root.collect_panels(&mut out);
        out
    }
}
