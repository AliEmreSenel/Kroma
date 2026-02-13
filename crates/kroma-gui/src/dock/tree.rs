//! Binary split tree for the docking layout.
//!
//! Each node is either a [`Split`](DockNode::Split) (two children separated
//! by a draggable divider) or a [`Leaf`](DockNode::Leaf) (one or more tabbed
//! panels).

use super::PathDir;
use crate::panels::PanelId;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Axis of a split — determines whether children are side-by-side or stacked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitAxis {
    /// Left | Right
    Horizontal,
    /// Top / Bottom
    Vertical,
}

/// Where a panel can be dropped relative to an existing leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    /// Add as a new tab in the target leaf.
    Center,
    /// Split the target leaf and place panel on the left/top.
    Left,
    Right,
    Top,
    Bottom,
}

/// A node in the binary dock tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DockNode {
    /// Two children separated by a divider.
    Split {
        axis: SplitAxis,
        /// Fraction (0.0–1.0) of space allocated to the first child.
        ratio: f32,
        left: Box<DockNode>,
        right: Box<DockNode>,
    },
    /// One or more panels shown as tabs.
    Leaf {
        tabs: Vec<PanelId>,
        /// Index of the currently active tab.
        active: usize,
    },
    /// An empty placeholder (used temporarily during tree surgery).
    Empty,
}

impl DockNode {
    // -----------------------------------------------------------------------
    // Construction helpers
    // -----------------------------------------------------------------------

    /// Create a leaf with a single panel.
    pub fn leaf(panel: PanelId) -> Self {
        Self::Leaf {
            tabs: vec![panel],
            active: 0,
        }
    }

    /// Create a horizontal split (left | right).
    pub fn hsplit(ratio: f32, left: DockNode, right: DockNode) -> Self {
        Self::Split {
            axis: SplitAxis::Horizontal,
            ratio,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Create a vertical split (top / bottom).
    pub fn vsplit(ratio: f32, top: DockNode, bottom: DockNode) -> Self {
        Self::Split {
            axis: SplitAxis::Vertical,
            ratio,
            left: Box::new(top),
            right: Box::new(bottom),
        }
    }

    /// The default layout preset:
    /// ```text
    /// ┌──────────┬─────────────────┬───────────┐
    /// │ Library  │   Node Editor   │Properties │
    /// │          │                 │           │
    /// │          │                 │           │
    /// │          ├─────────────────┤           │
    /// │          │ Code/Preview    │           │
    /// ├──────────┴─────────────────┴───────────┤
    /// │  Error Log  │  Dashboard  │  Import   │
    /// └──────────────────────────────────────────┘
    /// ```
    pub fn default_layout() -> Self {
        // Bottom bar: Error Log + Dashboard + Import as tabs
        let bottom = DockNode::Leaf {
            tabs: vec![PanelId::ErrorLog, PanelId::Dashboard, PanelId::Import],
            active: 0,
        };

        // Left sidebar: Library + Asset Browser as tabs
        let left_sidebar = DockNode::Leaf {
            tabs: vec![PanelId::Library, PanelId::AssetBrowser],
            active: 0,
        };

        // Right sidebar: Properties + Settings as tabs
        let right_sidebar = DockNode::Leaf {
            tabs: vec![PanelId::Properties, PanelId::Settings],
            active: 0,
        };

        // Center top: Node Editor
        let center_top = DockNode::leaf(PanelId::NodeEditor);

        // Center bottom: Code Editor + Live Preview + Asset Preview as tabs
        let center_bottom = DockNode::Leaf {
            tabs: vec![
                PanelId::CodeEditor,
                PanelId::LivePreview,
                PanelId::AssetPreview,
            ],
            active: 0,
        };

        // Center area: top (node editor) / bottom (code + preview)
        let center = DockNode::vsplit(0.6, center_top, center_bottom);

        // Main area: left sidebar | center | right sidebar
        let main_area = DockNode::hsplit(
            0.18,
            left_sidebar,
            DockNode::hsplit(0.75, center, right_sidebar),
        );

        // Root: main area / bottom bar
        DockNode::vsplit(0.75, main_area, bottom)
    }

    // -----------------------------------------------------------------------
    // Tree navigation
    // -----------------------------------------------------------------------

    /// Search the tree for a leaf containing `panel_id` and make it the active tab.
    pub fn activate_panel(&mut self, panel_id: PanelId) {
        match self {
            DockNode::Leaf { tabs, active, .. } => {
                if let Some(idx) = tabs.iter().position(|&p| p == panel_id) {
                    *active = idx;
                }
            }
            DockNode::Split { left, right, .. } => {
                left.activate_panel(panel_id);
                right.activate_panel(panel_id);
            }
            DockNode::Empty => {}
        }
    }

    /// Set the split ratio at the given path.
    pub fn set_ratio(&mut self, path: &[PathDir], new_ratio: f32) {
        match (self, path.first()) {
            (DockNode::Split { ratio, .. }, None) => {
                *ratio = new_ratio;
            }
            (DockNode::Split { left, right, .. }, Some(&dir)) => {
                let child = match dir {
                    PathDir::Left => left,
                    PathDir::Right => right,
                };
                child.set_ratio(&path[1..], new_ratio);
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Tree mutation
    // -----------------------------------------------------------------------

    /// Remove a specific panel from anywhere in the tree.
    pub fn remove_panel(&mut self, panel_id: PanelId) {
        match self {
            DockNode::Leaf { tabs, active, .. } => {
                if let Some(pos) = tabs.iter().position(|&id| id == panel_id) {
                    tabs.remove(pos);
                    if *active >= tabs.len() && !tabs.is_empty() {
                        *active = tabs.len() - 1;
                    }
                }
            }
            DockNode::Split { left, right, .. } => {
                left.remove_panel(panel_id);
                right.remove_panel(panel_id);
            }
            DockNode::Empty => {}
        }
    }

    /// Insert a panel at a target path with a drop zone.
    pub fn insert_panel(&mut self, panel_id: PanelId, path: &[PathDir], zone: DropZone) {
        match (self, path.first()) {
            (node @ DockNode::Leaf { .. }, None) => {
                match zone {
                    DropZone::Center => {
                        if let DockNode::Leaf { tabs, active, .. } = node {
                            tabs.push(panel_id);
                            *active = tabs.len() - 1;
                        }
                    }
                    DropZone::Left => {
                        let existing = std::mem::replace(node, DockNode::Empty);
                        *node = DockNode::hsplit(0.3, DockNode::leaf(panel_id), existing);
                    }
                    DropZone::Right => {
                        let existing = std::mem::replace(node, DockNode::Empty);
                        *node = DockNode::hsplit(0.7, existing, DockNode::leaf(panel_id));
                    }
                    DropZone::Top => {
                        let existing = std::mem::replace(node, DockNode::Empty);
                        *node = DockNode::vsplit(0.3, DockNode::leaf(panel_id), existing);
                    }
                    DropZone::Bottom => {
                        let existing = std::mem::replace(node, DockNode::Empty);
                        *node = DockNode::vsplit(0.7, existing, DockNode::leaf(panel_id));
                    }
                }
            }
            (DockNode::Split { left, right, .. }, Some(&dir)) => {
                let child = match dir {
                    PathDir::Left => left.as_mut(),
                    PathDir::Right => right.as_mut(),
                };
                child.insert_panel(panel_id, &path[1..], zone);
            }
            (node @ DockNode::Empty, None) => {
                *node = DockNode::leaf(panel_id);
            }
            _ => {}
        }
    }

    /// Insert a panel into a leaf at a specific tab index (for reordering).
    ///
    /// Navigates to the leaf at `path` and inserts `panel_id` at `index`.
    /// If `index` is out of range, appends at the end.
    pub fn insert_panel_at_index(&mut self, panel_id: PanelId, path: &[PathDir], index: usize) {
        match (self, path.first()) {
            (DockNode::Leaf { tabs, active, .. }, None) => {
                let idx = index.min(tabs.len());
                tabs.insert(idx, panel_id);
                *active = idx;
            }
            (DockNode::Split { left, right, .. }, Some(&dir)) => {
                let child = match dir {
                    PathDir::Left => left.as_mut(),
                    PathDir::Right => right.as_mut(),
                };
                child.insert_panel_at_index(panel_id, &path[1..], index);
            }
            (node @ DockNode::Empty, None) => {
                *node = DockNode::leaf(panel_id);
            }
            _ => {}
        }
    }

    /// Clean up the tree: collapse empty leaves and trivial splits.
    pub fn cleanup(&mut self) {
        match self {
            DockNode::Split { left, right, .. } => {
                left.cleanup();
                right.cleanup();

                // If one child is empty, replace self with the other
                let left_empty = matches!(left.as_ref(), DockNode::Empty)
                    || matches!(left.as_ref(), DockNode::Leaf { tabs, .. } if tabs.is_empty());
                let right_empty = matches!(right.as_ref(), DockNode::Empty)
                    || matches!(right.as_ref(), DockNode::Leaf { tabs, .. } if tabs.is_empty());

                if left_empty && right_empty {
                    *self = DockNode::Empty;
                } else if left_empty {
                    let replacement = std::mem::replace(right.as_mut(), DockNode::Empty);
                    *self = replacement;
                } else if right_empty {
                    let replacement = std::mem::replace(left.as_mut(), DockNode::Empty);
                    *self = replacement;
                }
            }
            DockNode::Leaf { tabs, .. } if tabs.is_empty() => {
                *self = DockNode::Empty;
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Test-only helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
impl DockNode {
    /// Get the active tab index at a leaf.
    pub fn active_tab(&self, path: &[PathDir]) -> Option<usize> {
        match (self, path.first()) {
            (DockNode::Leaf { active, .. }, None) => Some(*active),
            (DockNode::Split { left, right, .. }, Some(&dir)) => {
                let child = match dir {
                    PathDir::Left => left,
                    PathDir::Right => right,
                };
                child.active_tab(&path[1..])
            }
            _ => None,
        }
    }

    /// Set the active tab at a leaf.
    pub fn set_active_tab(&mut self, path: &[PathDir], index: usize) {
        match (self, path.first()) {
            (DockNode::Leaf { active, tabs, .. }, None) => {
                if index < tabs.len() {
                    *active = index;
                }
            }
            (DockNode::Split { left, right, .. }, Some(&dir)) => {
                let child = match dir {
                    PathDir::Left => left,
                    PathDir::Right => right,
                };
                child.set_active_tab(&path[1..], index);
            }
            _ => {}
        }
    }

    /// Close a tab at the given leaf path.
    pub fn close_tab(&mut self, path: &[PathDir], tab_index: usize) {
        match (self, path.first()) {
            (DockNode::Leaf { tabs, active, .. }, None) => {
                if tab_index < tabs.len() {
                    tabs.remove(tab_index);
                    if *active >= tabs.len() && !tabs.is_empty() {
                        *active = tabs.len() - 1;
                    }
                }
            }
            (DockNode::Split { left, right, .. }, Some(&dir)) => {
                let child = match dir {
                    PathDir::Left => left,
                    PathDir::Right => right,
                };
                child.close_tab(&path[1..], tab_index);
            }
            _ => {}
        }
    }

    /// Collect all panel IDs in the tree.
    pub fn collect_panels(&self, out: &mut Vec<PanelId>) {
        match self {
            DockNode::Split { left, right, .. } => {
                left.collect_panels(out);
                right.collect_panels(out);
            }
            DockNode::Leaf { tabs, .. } => {
                out.extend(tabs);
            }
            DockNode::Empty => {}
        }
    }

    /// Find the path to a panel in the tree.
    pub fn find_panel(&self, target: PanelId, path: &mut Vec<PathDir>) -> Option<Vec<PathDir>> {
        match self {
            DockNode::Leaf { tabs, .. } => {
                if tabs.contains(&target) {
                    Some(path.clone())
                } else {
                    None
                }
            }
            DockNode::Split { left, right, .. } => {
                path.push(PathDir::Left);
                if let Some(result) = left.find_panel(target, path) {
                    return Some(result);
                }
                path.pop();

                path.push(PathDir::Right);
                if let Some(result) = right.find_panel(target, path) {
                    return Some(result);
                }
                path.pop();

                None
            }
            DockNode::Empty => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_has_all_panels() {
        let tree = DockNode::default_layout();
        let mut panels = Vec::new();
        tree.collect_panels(&mut panels);

        // Should contain all standard panels
        assert!(panels.contains(&PanelId::Dashboard));
        assert!(panels.contains(&PanelId::NodeEditor));
        assert!(panels.contains(&PanelId::CodeEditor));
        assert!(panels.contains(&PanelId::AssetBrowser));
        assert!(panels.contains(&PanelId::AssetPreview));
        assert!(panels.contains(&PanelId::Properties));
        assert!(panels.contains(&PanelId::Library));
        assert!(panels.contains(&PanelId::LivePreview));
        assert!(panels.contains(&PanelId::Import));
        assert!(panels.contains(&PanelId::ErrorLog));
        assert!(panels.contains(&PanelId::Settings));
    }

    #[test]
    fn tab_selection() {
        let mut state = DockNode::Leaf {
            tabs: vec![PanelId::Dashboard, PanelId::ErrorLog, PanelId::Import],
            active: 0,
        };
        state.set_active_tab(&[], 2);
        assert_eq!(state.active_tab(&[]), Some(2));
    }

    #[test]
    fn close_tab_adjusts_active() {
        let mut state = DockNode::Leaf {
            tabs: vec![PanelId::Dashboard, PanelId::ErrorLog, PanelId::Import],
            active: 2,
        };
        state.close_tab(&[], 2);
        assert_eq!(state.active_tab(&[]), Some(1));
        if let DockNode::Leaf { tabs, .. } = &state {
            assert_eq!(tabs.len(), 2);
        }
    }

    #[test]
    fn remove_and_cleanup() {
        let mut tree = DockNode::hsplit(
            0.5,
            DockNode::leaf(PanelId::Dashboard),
            DockNode::leaf(PanelId::Settings),
        );
        tree.remove_panel(PanelId::Dashboard);
        tree.cleanup();
        // Should collapse to just the Settings leaf
        if let DockNode::Leaf { tabs, .. } = &tree {
            assert_eq!(tabs, &[PanelId::Settings]);
        } else {
            panic!("Expected Leaf after cleanup, got {:?}", tree);
        }
    }

    #[test]
    fn insert_panel_center() {
        let mut tree = DockNode::leaf(PanelId::Dashboard);
        tree.insert_panel(PanelId::Settings, &[], DropZone::Center);
        if let DockNode::Leaf { tabs, active, .. } = &tree {
            assert_eq!(tabs, &[PanelId::Dashboard, PanelId::Settings]);
            assert_eq!(*active, 1);
        } else {
            panic!("Expected Leaf");
        }
    }

    #[test]
    fn insert_panel_split() {
        let mut tree = DockNode::leaf(PanelId::Dashboard);
        tree.insert_panel(PanelId::Settings, &[], DropZone::Right);
        if let DockNode::Split { axis, left, right, .. } = &tree {
            assert_eq!(*axis, SplitAxis::Horizontal);
            assert!(matches!(left.as_ref(), DockNode::Leaf { tabs, .. } if tabs == &[PanelId::Dashboard]));
            assert!(matches!(right.as_ref(), DockNode::Leaf { tabs, .. } if tabs == &[PanelId::Settings]));
        } else {
            panic!("Expected Split");
        }
    }

    #[test]
    fn find_panel_in_tree() {
        let tree = DockNode::default_layout();
        let path = tree.find_panel(PanelId::NodeEditor, &mut Vec::new());
        assert!(path.is_some(), "NodeEditor should be findable in default layout");
    }

    /// Regression test: dropping a panel from the left child of a split onto
    /// the right child must not lose the panel. Previously, `remove_panel` +
    /// `cleanup()` was called *before* `insert_panel`, which collapsed the
    /// tree and made `target_path` stale.
    #[test]
    fn panel_drop_does_not_lose_panel_after_tree_collapse() {
        use super::super::DockState;

        // Build: Split( L=A, R=Split( L=B, R=C ) )
        // Panel A path = [Left], panel C path = [Right, Right]
        let mut state = DockState {
            root: DockNode::hsplit(
                0.5,
                DockNode::leaf(PanelId::Dashboard),        // A at [Left]
                DockNode::hsplit(
                    0.5,
                    DockNode::leaf(PanelId::Settings),      // B at [Right, Left]
                    DockNode::leaf(PanelId::ErrorLog),      // C at [Right, Right]
                ),
            ),
        };

        // Simulate: drag Dashboard (A) and drop onto ErrorLog's leaf (C)
        // as a Center drop (add as tab).
        // target_path = [Right, Right] — captured at render time before the drag.
        state.update(super::super::DockMessage::PanelDropped {
            panel_id: PanelId::Dashboard,
            target_path: vec![super::PathDir::Right, super::PathDir::Right],
            zone: DropZone::Center,
        });

        let panels = state.visible_panels();
        assert!(
            panels.contains(&PanelId::Dashboard),
            "Dashboard must still be in the tree after drop, got: {:?}",
            panels,
        );
        assert!(
            panels.contains(&PanelId::Settings),
            "Settings must still be in the tree after drop",
        );
        assert!(
            panels.contains(&PanelId::ErrorLog),
            "ErrorLog must still be in the tree after drop",
        );
    }

    #[test]
    fn insert_panel_at_index_reorder() {
        // Start with [Dashboard, ErrorLog, Import], insert Settings at index 1
        let mut tree = DockNode::Leaf {
            tabs: vec![PanelId::Dashboard, PanelId::ErrorLog, PanelId::Import],
            active: 0,
        };
        tree.insert_panel_at_index(PanelId::Settings, &[], 1);
        if let DockNode::Leaf { tabs, active, .. } = &tree {
            assert_eq!(tabs, &[PanelId::Dashboard, PanelId::Settings, PanelId::ErrorLog, PanelId::Import]);
            assert_eq!(*active, 1);
        } else {
            panic!("Expected Leaf");
        }
    }

    #[test]
    fn insert_panel_at_index_end() {
        // Insert at index beyond length → appends
        let mut tree = DockNode::Leaf {
            tabs: vec![PanelId::Dashboard],
            active: 0,
        };
        tree.insert_panel_at_index(PanelId::Settings, &[], 99);
        if let DockNode::Leaf { tabs, active, .. } = &tree {
            assert_eq!(tabs, &[PanelId::Dashboard, PanelId::Settings]);
            assert_eq!(*active, 1);
        } else {
            panic!("Expected Leaf");
        }
    }
}
