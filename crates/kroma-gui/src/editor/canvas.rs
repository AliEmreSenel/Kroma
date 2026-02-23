//! Interactive canvas for the node-based shader editor.
//!
//! Renders nodes as rounded rectangles with coloured ports, and wires
//! as smooth Bézier curves.  Supports drag-to-move, drag-to-connect,
//! and selection.

use iced::widget::Action;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Stroke, Text};
use iced::widget::text::Alignment;
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, Vector};
use iced::{Event, mouse};
use std::collections::HashSet;

use crate::theme::ThemeTokens;

use super::{
    ConnectionId, DataType, DefaultValue, Node, NodeId, NodeKind, NodeLayout, PortAddr,
    PortDirection, ShaderGraph,
};

// ---------------------------------------------------------------------------
// Public message type for the editor
// ---------------------------------------------------------------------------

/// Messages produced by the graph canvas.
#[derive(Debug, Clone)]
pub enum GraphMessage {
    /// A node was moved by dragging.
    NodeMoved(NodeId, [f32; 2]),
    /// User finished dragging a wire between two ports.
    ConnectionCreated(PortAddr, PortAddr),
    /// User clicked on a connection to delete it.
    ConnectionDeleted(ConnectionId),
    /// User selected / deselected a node. Bool = shift held (additive select).
    NodeSelected(NodeId, bool),
    /// User requested deleting selected nodes (e.g. Delete key).
    DeleteSelected,
    /// User double-clicked on empty space — open palette at position.
    OpenPalette([f32; 2]),
    /// Canvas panned.
    Panned(Vector),
    /// Canvas zoom changed (zoom level, new offset for zoom-towards-cursor).
    Zoomed(f32, Vector),
    /// Default value of a node's input port changed (node, port index, new value).
    DefaultChanged(NodeId, usize, DefaultValue),
    /// Box-select completed — nodes within the selection rectangle.
    BoxSelected(Vec<NodeId>),
    /// User requested copy of selected nodes (Ctrl+C).
    CopySelected,
    /// User requested paste of clipboard (Ctrl+V).
    PasteNodes,
    /// User clicked canvas with a pending node from palette — place it at position.
    PlacePendingNode([f32; 2]),
    /// Add a node at the center of the current viewport (one-click palette add).
    #[allow(dead_code)]
    AddNodeAtCenter(NodeKind),
    /// Set a pending node from palette — will be placed on next canvas click.
    SetPendingNode(NodeKind),
    /// Cancel the pending node placement (Escape while pending).
    CancelPending,
    /// Group selected nodes with a comment frame (Ctrl+G).
    GroupSelected,
    /// User double-clicked a sub-graph node — navigate into it.
    EnterSubGraph(NodeId),
    /// User wants to go back to parent graph (Escape while in sub-graph).
    ExitSubGraph,
    /// Toggle minimap overlay (M key).
    ToggleMinimap,
    /// User double-clicked an unconnected input port value — start inline editing.
    StartEditValue(NodeId, usize),
    /// Text in the inline value editor changed.
    EditValueChanged(String),
    /// Confirm the edited value (Enter or click away).
    CommitEditValue,
    /// Cancel editing without saving (Escape).
    CancelEditValue,
}

// ---------------------------------------------------------------------------
// Comment frames (visual grouping)
// ---------------------------------------------------------------------------

/// A visual comment/group frame around nodes.
#[derive(Debug, Clone)]
pub struct CommentFrame {
    /// World-space bounding rectangle [x, y, w, h].
    pub rect: [f32; 4],
    /// Display label for the group.
    pub label: String,
    /// Frame color (RGBA).
    pub color: [f32; 4],
}

// ---------------------------------------------------------------------------
// Graph canvas state
// ---------------------------------------------------------------------------

pub struct GraphCanvas {
    /// Camera offset (pan).
    pub offset: Vector,
    /// Zoom level (1.0 = 100%).
    pub zoom: f32,
    /// Live values for input nodes (e.g. Time → "3.42").
    pub live_values: std::collections::HashMap<NodeId, String>,
    /// Pending node from palette — will be placed on next canvas click.
    pub pending_node: Option<NodeKind>,
    /// Visual comment/group frames.
    pub comment_frames: Vec<CommentFrame>,
    /// Whether the minimap overlay is visible.
    pub show_minimap: bool,
    /// Currently editing a default value inline: (node_id, port_index, current text).
    pub editing_value: Option<(NodeId, usize, String)>,
}

#[derive(Debug, Clone)]
enum Interaction {
    None,
    /// Dragging a node around.
    DraggingNode {
        node_id: NodeId,
        start_mouse: Point,
        start_pos: [f32; 2],
    },
    /// Drawing a new wire from an output port.
    DraggingWire {
        from: PortAddr,
        from_pos: [f32; 2],
        end: Point,
    },
    /// Box-selecting nodes.
    BoxSelecting {
        start: Point,
        current: Point,
    },
    /// Panning the canvas.
    Panning {
        start: Point,
        start_offset: Vector,
    },
}

impl GraphCanvas {
    pub fn new() -> Self {
        Self {
            offset: Vector::new(0.0, 0.0),
            zoom: 1.0,
            live_values: std::collections::HashMap::new(),
            pending_node: None,
            comment_frames: Vec::new(),
            show_minimap: true,
            editing_value: None,
        }
    }
}

impl Default for GraphCanvas {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

const NODE_ROUNDING: f32 = 8.0;
const PORT_RADIUS: f32 = 5.0;
const HEADER_HEIGHT: f32 = 28.0;
const PORT_SPACING: f32 = 24.0;
const WIRE_THICKNESS: f32 = 2.5;

fn data_type_color(dt: DataType, tokens: &ThemeTokens) -> Color {
    match dt {
        DataType::Float => tokens.wire_float,
        DataType::Vec2 => tokens.wire_vec2,
        DataType::Vec3 => tokens.wire_vec3,
        DataType::Vec4 => tokens.wire_vec4,
    }
}

fn node_bg_color(kind: &NodeKind, tokens: &ThemeTokens) -> Color {
    match kind.category() {
        "Output" => Color::from_rgb(0.35, 0.18, 0.18),
        "Input" | "System" => Color::from_rgb(0.15, 0.22, 0.32),
        "Constants" => Color::from_rgb(0.22, 0.22, 0.15),
        "Math" => Color::from_rgb(0.18, 0.25, 0.18),
        "Vector" => Color::from_rgb(0.2, 0.18, 0.28),
        "Color" => Color::from_rgb(0.28, 0.18, 0.22),
        "Procedural" => Color::from_rgb(0.18, 0.22, 0.25),
        _ => tokens.node_bg,
    }
}

fn node_header_color(kind: &NodeKind, tokens: &ThemeTokens) -> Color {
    match kind.category() {
        "Output" => Color::from_rgb(0.6, 0.2, 0.2),
        "Input" | "System" => Color::from_rgb(0.18, 0.35, 0.55),
        "Constants" => Color::from_rgb(0.45, 0.4, 0.15),
        "Math" => Color::from_rgb(0.2, 0.45, 0.2),
        "Vector" => Color::from_rgb(0.35, 0.22, 0.5),
        "Color" => Color::from_rgb(0.5, 0.2, 0.35),
        "Procedural" => Color::from_rgb(0.2, 0.35, 0.4),
        _ => tokens.node_header,
    }
}

/// Draw a single node onto the frame.
fn draw_node(
    frame: &mut Frame,
    node: &Node,
    offset: Vector,
    zoom: f32,
    live_value: Option<&str>,
    connected_inputs: &HashSet<(NodeId, usize)>,
    tokens: &ThemeTokens,
) {
    let x = node.position[0] * zoom + offset.x;
    let y = node.position[1] * zoom + offset.y;
    let w = node.width() * zoom;
    let h = node.height() * zoom;

    // Extra height for live-value row
    let value_row_h = if live_value.is_some() {
        20.0 * zoom
    } else {
        0.0
    };
    let total_h = h + value_row_h;

    let header_h = HEADER_HEIGHT * zoom;
    let port_spacing = PORT_SPACING * zoom;
    let port_radius = PORT_RADIUS * zoom;
    let font_size = 13.0 * zoom;
    let small_font = 11.0 * zoom;

    // Shadow
    let shadow = Path::rounded_rectangle(
        Point::new(x + 2.0 * zoom, y + 2.0 * zoom),
        Size::new(w, total_h),
        (NODE_ROUNDING * zoom).into(),
    );
    frame.fill(&shadow, Color::from_rgba(0.0, 0.0, 0.0, 0.3));

    // Body
    let body = Path::rounded_rectangle(
        Point::new(x, y),
        Size::new(w, total_h),
        (NODE_ROUNDING * zoom).into(),
    );
    frame.fill(&body, node_bg_color(&node.kind, tokens));

    // Selection highlight
    if node.selected {
        frame.stroke(
            &body,
            Stroke::default()
                .with_color(tokens.node_selected)
                .with_width(2.0),
        );
    } else if matches!(
        node.kind,
        NodeKind::ForLoop | NodeKind::Conditional | NodeKind::CustomFunc
    ) {
        // Subtle border for sub-graph nodes
        frame.stroke(
            &body,
            Stroke::default()
                .with_color(Color::from_rgba(0.6, 0.8, 1.0, 0.3))
                .with_width(1.0),
        );
    }

    // Header
    let header = Path::rectangle(Point::new(x, y), Size::new(w, header_h));
    frame.fill(&header, node_header_color(&node.kind, tokens));

    // Header text
    let label = Text {
        content: node.kind.label().to_string(),
        position: Point::new(x + 8.0 * zoom, y + 6.0 * zoom),
        color: Color::WHITE,
        size: iced::Pixels(font_size),
        ..Text::default()
    };
    frame.fill_text(label);

    // Sub-graph indicator icon (for ForLoop/Conditional/CustomFunc)
    if matches!(
        node.kind,
        NodeKind::ForLoop | NodeKind::Conditional | NodeKind::CustomFunc
    ) {
        let icon = Text {
            content: ">>".to_string(), // double-click hint
            position: Point::new(x + w - 20.0 * zoom, y + 6.0 * zoom),
            color: Color::from_rgba(1.0, 1.0, 1.0, 0.5),
            size: iced::Pixels(10.0 * zoom),
            align_x: Alignment::Right,
            ..Text::default()
        };
        frame.fill_text(icon);
    }

    // Input ports
    for (i, port_def) in node.inputs().iter().enumerate() {
        let py = y + header_h + i as f32 * port_spacing + 12.0 * zoom;
        let port = Path::circle(Point::new(x, py), port_radius);
        frame.fill(&port, data_type_color(port_def.data_type, tokens));

        let is_connected = connected_inputs.contains(&(node.id, i));

        let port_label = Text {
            content: port_def.name.clone(),
            position: Point::new(x + 10.0 * zoom, py - 6.0 * zoom),
            color: Color::from_rgb(0.8, 0.8, 0.8),
            size: iced::Pixels(small_font),
            ..Text::default()
        };
        frame.fill_text(port_label);

        // Show editable default value for unconnected Float inputs
        if !is_connected {
            if let Some(def) = node.defaults.get(i) {
                let val_str = match def {
                    DefaultValue::Float(v) => format!("{:.2}", v),
                    DefaultValue::Vec2(v) => format!("{:.1}, {:.1}", v[0], v[1]),
                    DefaultValue::Vec3(v) => format!("{:.1}, {:.1}, {:.1}", v[0], v[1], v[2]),
                    DefaultValue::Vec4(v) => {
                        format!("{:.1},{:.1},{:.1},{:.1}", v[0], v[1], v[2], v[3])
                    }
                };
                // Background pill for the value
                let val_x = x + 60.0 * zoom;
                let val_w = w - 65.0 * zoom;
                let val_h = 14.0 * zoom;
                let val_y = py - 7.0 * zoom;
                let pill = Path::rounded_rectangle(
                    Point::new(val_x, val_y),
                    Size::new(val_w, val_h),
                    (3.0 * zoom).into(),
                );
                frame.fill(&pill, Color::from_rgba(0.0, 0.0, 0.0, 0.3));
                let value_text = Text {
                    content: val_str,
                    position: Point::new(val_x + 4.0 * zoom, val_y + 1.0 * zoom),
                    color: Color::from_rgb(0.9, 0.85, 0.6),
                    size: iced::Pixels(10.0 * zoom),
                    ..Text::default()
                };
                frame.fill_text(value_text);
            }
        }
    }

    // Output ports
    for (i, port_def) in node.outputs().iter().enumerate() {
        let py = y + header_h + i as f32 * port_spacing + 12.0 * zoom;
        let port = Path::circle(Point::new(x + w, py), port_radius);
        frame.fill(&port, data_type_color(port_def.data_type, tokens));

        let port_label = Text {
            content: port_def.name.clone(),
            position: Point::new(x + w - 10.0 * zoom, py - 6.0 * zoom),
            color: Color::from_rgb(0.8, 0.8, 0.8),
            size: iced::Pixels(small_font),
            align_x: Alignment::Right,
            ..Text::default()
        };
        frame.fill_text(port_label);
    }

    // Live value display (for Input / System nodes)
    if let Some(val) = live_value {
        let vy = y + total_h - value_row_h + 3.0 * zoom;
        let value_text = Text {
            content: val.to_string(),
            position: Point::new(x + 8.0 * zoom, vy),
            color: Color::from_rgb(0.5, 1.0, 0.7),
            size: iced::Pixels(10.0 * zoom),
            ..Text::default()
        };
        frame.fill_text(value_text);
    }
}

/// Draw a Bézier wire between two points.
fn draw_wire(frame: &mut Frame, from: Point, to: Point, color: Color, zoom: f32) {
    let dx = (to.x - from.x).abs() * 0.5;
    let ctrl1 = Point::new(from.x + dx, from.y);
    let ctrl2 = Point::new(to.x - dx, to.y);

    let path = Path::new(|b| {
        b.move_to(from);
        b.bezier_curve_to(ctrl1, ctrl2, to);
    });
    frame.stroke(
        &path,
        Stroke::default()
            .with_color(color)
            .with_width(WIRE_THICKNESS * zoom),
    );
}

// ---------------------------------------------------------------------------
// iced canvas program
// ---------------------------------------------------------------------------

/// Bundle of references the canvas program needs to render and handle events.
pub struct GraphProgram<'a> {
    pub graph: &'a ShaderGraph,
    pub canvas_state: &'a GraphCanvas,
    pub tokens: &'a ThemeTokens,
}

impl<'a> canvas::Program<GraphMessage> for GraphProgram<'a> {
    type State = CanvasInteraction;

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<GraphMessage>> {
        let cursor_pos = cursor.position_in(bounds)?;

        let offset = self.canvas_state.offset;
        let zoom = self.canvas_state.zoom;

        match event {
            // ---- Scroll: adjust port default OR zoom -----------------
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let scroll_y = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => &(y / 80.0),
                };

                // Check if hovering over an input port — adjust default value
                if let Some((node_id, dir, port_idx, _)) =
                    self.hit_test_port(cursor_pos, offset, zoom)
                {
                    if dir == PortDirection::Input {
                        // Check if this port is unconnected
                        let is_connected = self
                            .graph
                            .connections()
                            .iter()
                            .any(|c| c.to.node == node_id && c.to.port == port_idx);
                        if !is_connected {
                            if let Some(node) = self.graph.node(node_id) {
                                if let Some(def) = node.defaults.get(port_idx) {
                                    let step = 0.1 * scroll_y;
                                    let clamp = |x: f32| x.clamp(-1000.0, 1000.0);
                                    let new_def = match def {
                                        DefaultValue::Float(v) => {
                                            DefaultValue::Float(clamp(v + step))
                                        }
                                        DefaultValue::Vec2(v) => DefaultValue::Vec2([
                                            clamp(v[0] + step),
                                            clamp(v[1] + step),
                                        ]),
                                        DefaultValue::Vec3(v) => DefaultValue::Vec3([
                                            clamp(v[0] + step),
                                            clamp(v[1] + step),
                                            clamp(v[2] + step),
                                        ]),
                                        DefaultValue::Vec4(v) => DefaultValue::Vec4([
                                            clamp(v[0] + step),
                                            clamp(v[1] + step),
                                            clamp(v[2] + step),
                                            clamp(v[3] + step),
                                        ]),
                                    };
                                    return Some(
                                        Action::publish(GraphMessage::DefaultChanged(
                                            node_id, port_idx, new_def,
                                        ))
                                        .and_capture(),
                                    );
                                }
                            }
                        }
                    }
                }

                // Otherwise: zoom
                let factor = if *scroll_y > 0.0 { 1.1 } else { 1.0 / 1.1 };
                let new_zoom = (zoom * factor).clamp(0.15, 5.0);
                let world_x = (cursor_pos.x - offset.x) / zoom;
                let world_y = (cursor_pos.y - offset.y) / zoom;
                let new_offset = Vector::new(
                    cursor_pos.x - world_x * new_zoom,
                    cursor_pos.y - world_y * new_zoom,
                );
                Some(Action::publish(GraphMessage::Zoomed(new_zoom, new_offset)).and_capture())
            }

            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // --- Double-click detection for value pill editing ---
                let is_double_click = state.last_value_click.as_ref().is_some_and(|(t, p)| {
                    t.elapsed().as_millis() < 300
                        && (cursor_pos.x - p.x).abs() < 5.0
                        && (cursor_pos.y - p.y).abs() < 5.0
                });
                state.last_value_click = Some((std::time::Instant::now(), cursor_pos));

                if is_double_click {
                    if let Some((node_id, port_idx)) =
                        self.hit_test_value_pill(cursor_pos, offset, zoom)
                    {
                        return Some(
                            Action::publish(GraphMessage::StartEditValue(node_id, port_idx))
                                .and_capture(),
                        );
                    }
                }

                // Check if clicking on a port first
                if let Some((node_id, dir, port_idx, port_pos)) =
                    self.hit_test_port(cursor_pos, offset, zoom)
                {
                    if dir == PortDirection::Output {
                        state.interaction = Interaction::DraggingWire {
                            from: PortAddr {
                                node: node_id,
                                port: port_idx,
                            },
                            from_pos: port_pos,
                            end: cursor_pos,
                        };
                        return Some(Action::capture());
                    }
                }

                // Check if clicking on a wire (connection)
                if let Some(conn_id) = self.hit_test_wire(cursor_pos, offset, zoom) {
                    return Some(
                        Action::publish(GraphMessage::ConnectionDeleted(conn_id)).and_capture(),
                    );
                }

                // Check if clicking on a node
                if let Some(node_id) = self.hit_test_node(cursor_pos, offset, zoom) {
                    let node = self.graph.node(node_id)?;

                    // Double-click detection for sub-graph nodes
                    let is_subgraph_node = matches!(
                        node.kind,
                        NodeKind::ForLoop | NodeKind::Conditional | NodeKind::CustomFunc
                    );
                    if is_subgraph_node {
                        if let Some((last_time, last_id)) = state.last_click {
                            if last_id == node_id && last_time.elapsed().as_millis() < 400 {
                                state.last_click = None;
                                return Some(
                                    Action::publish(GraphMessage::EnterSubGraph(node_id))
                                        .and_capture(),
                                );
                            }
                        }
                        state.last_click = Some((std::time::Instant::now(), node_id));
                    } else {
                        state.last_click = None;
                    }

                    state.interaction = Interaction::DraggingNode {
                        node_id,
                        start_mouse: cursor_pos,
                        start_pos: node.position,
                    };
                    // shift flag = additive select (true means toggle)
                    return Some(
                        Action::publish(GraphMessage::NodeSelected(node_id, state.shift_held))
                            .and_capture(),
                    );
                }

                // Shift+click on empty space — box select
                if state.shift_held {
                    state.interaction = Interaction::BoxSelecting {
                        start: cursor_pos,
                        current: cursor_pos,
                    };
                    return Some(Action::capture());
                }

                // If a pending node from palette, place it here
                if self.canvas_state.pending_node.is_some() {
                    let world_pos = [
                        (cursor_pos.x - offset.x) / zoom,
                        (cursor_pos.y - offset.y) / zoom,
                    ];
                    return Some(
                        Action::publish(GraphMessage::PlacePendingNode(world_pos)).and_capture(),
                    );
                }

                // Clicking empty space — pan
                state.interaction = Interaction::Panning {
                    start: cursor_pos,
                    start_offset: offset,
                };
                Some(Action::capture())
            }

            Event::Mouse(mouse::Event::CursorMoved { .. }) => match &mut state.interaction {
                Interaction::DraggingNode {
                    node_id,
                    start_mouse,
                    start_pos,
                } => {
                    // Convert screen-space delta to world-space delta
                    let delta_x = (cursor_pos.x - start_mouse.x) / zoom;
                    let delta_y = (cursor_pos.y - start_mouse.y) / zoom;
                    let new_pos = [start_pos[0] + delta_x, start_pos[1] + delta_y];

                    Some(Action::publish(GraphMessage::NodeMoved(*node_id, new_pos)).and_capture())
                }
                Interaction::DraggingWire { end, .. } => {
                    *end = cursor_pos;
                    Some(Action::capture())
                }
                Interaction::BoxSelecting { current, .. } => {
                    *current = cursor_pos;
                    Some(Action::capture())
                }
                Interaction::Panning {
                    start,
                    start_offset,
                } => {
                    let dx = cursor_pos.x - start.x;
                    let dy = cursor_pos.y - start.y;
                    let new_offset = Vector::new(start_offset.x + dx, start_offset.y + dy);

                    Some(Action::publish(GraphMessage::Panned(new_offset)).and_capture())
                }
                Interaction::None => None,
            },

            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let msg = match &state.interaction {
                    Interaction::DraggingWire { from, .. } => {
                        // Check if we released on an input port
                        if let Some((node_id, dir, port_idx, _port_pos)) =
                            self.hit_test_port(cursor_pos, offset, zoom)
                        {
                            if dir == PortDirection::Input {
                                Some(GraphMessage::ConnectionCreated(
                                    *from,
                                    PortAddr {
                                        node: node_id,
                                        port: port_idx,
                                    },
                                ))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    Interaction::BoxSelecting { start, current } => {
                        // Select all nodes within the box
                        let world_start =
                            [(start.x - offset.x) / zoom, (start.y - offset.y) / zoom];
                        let world_end =
                            [(current.x - offset.x) / zoom, (current.y - offset.y) / zoom];
                        let min_x = world_start[0].min(world_end[0]);
                        let max_x = world_start[0].max(world_end[0]);
                        let min_y = world_start[1].min(world_end[1]);
                        let max_y = world_start[1].max(world_end[1]);

                        let selected: Vec<NodeId> = self
                            .graph
                            .nodes()
                            .filter(|n| {
                                let [nx, ny] = n.position;
                                let w = n.width();
                                let h = n.height();
                                nx + w >= min_x && nx <= max_x && ny + h >= min_y && ny <= max_y
                            })
                            .map(|n| n.id)
                            .collect();

                        if selected.is_empty() {
                            None
                        } else {
                            Some(GraphMessage::BoxSelected(selected))
                        }
                    }
                    _ => None,
                };
                state.interaction = Interaction::None;
                Some(
                    msg.map(Action::publish)
                        .unwrap_or(Action::capture())
                        .and_capture(),
                )
            }

            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                // Right-click: open palette at cursor (world coords)
                let world_pos = [
                    (cursor_pos.x - offset.x) / zoom,
                    (cursor_pos.y - offset.y) / zoom,
                ];
                Some(Action::publish(GraphMessage::OpenPalette(world_pos)).and_capture())
            }

            // Ctrl+C: copy selected nodes
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Character(c),
                modifiers,
                ..
            }) if modifiers.control() && c.as_ref() == "c" => {
                Some(Action::publish(GraphMessage::CopySelected).and_capture())
            }

            // Ctrl+V: paste nodes
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Character(c),
                modifiers,
                ..
            }) if modifiers.control() && c.as_ref() == "v" => {
                Some(Action::publish(GraphMessage::PasteNodes).and_capture())
            }

            // Ctrl+G: group selected nodes
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Character(c),
                modifiers,
                ..
            }) if modifiers.control() && c.as_ref() == "g" => {
                Some(Action::publish(GraphMessage::GroupSelected))
            }

            // M: toggle minimap overlay
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Character(c),
                modifiers,
                ..
            }) if !modifiers.control() && (c.as_ref() == "m" || c.as_ref() == "M") => {
                Some(Action::publish(GraphMessage::ToggleMinimap).and_capture())
            }

            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Delete),
                ..
            })
            | Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Backspace),
                ..
            }) => Some(Action::publish(GraphMessage::DeleteSelected).and_capture()),

            // Track Shift key state for multi-select
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(mods)) => {
                // This is a mutable self issue — but GraphProgram borrows canvas_state
                // immutably.  We'll track shift via the CanvasInteraction state instead.
                state.shift_held = mods.shift();
                None
            }

            // Escape: cancel pending node / cancel editing / exit sub-graph
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            }) => {
                if self.canvas_state.pending_node.is_some() {
                    Some(Action::publish(GraphMessage::CancelPending).and_capture())
                } else if self.canvas_state.editing_value.is_some() {
                    Some(Action::publish(GraphMessage::CancelEditValue).and_capture())
                } else {
                    Some(Action::publish(GraphMessage::ExitSubGraph).and_capture())
                }
            }

            _ => None,
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let offset = self.canvas_state.offset;
        let zoom = self.canvas_state.zoom;

        let mut frame = Frame::new(renderer, bounds.size());

        // Background grid
        draw_grid(&mut frame, bounds.size(), offset, zoom);

        // Draw comment frames (behind everything)
        for cf in &self.canvas_state.comment_frames {
            let x = cf.rect[0] * zoom + offset.x;
            let y = cf.rect[1] * zoom + offset.y;
            let w = cf.rect[2] * zoom;
            let h = cf.rect[3] * zoom;
            let fill_color =
                Color::from_rgba(cf.color[0], cf.color[1], cf.color[2], cf.color[3] * 0.15);
            let border_color =
                Color::from_rgba(cf.color[0], cf.color[1], cf.color[2], cf.color[3] * 0.5);
            let rect =
                Path::rounded_rectangle(Point::new(x, y), Size::new(w, h), (6.0 * zoom).into());
            frame.fill(&rect, fill_color);
            frame.stroke(
                &rect,
                Stroke::default().with_color(border_color).with_width(1.5),
            );
            // Label at top-left
            if !cf.label.is_empty() {
                let label = Text {
                    content: cf.label.clone(),
                    position: Point::new(x + 8.0 * zoom, y + 4.0 * zoom),
                    color: Color::from_rgba(cf.color[0], cf.color[1], cf.color[2], 0.8),
                    size: iced::Pixels(12.0 * zoom),
                    ..Text::default()
                };
                frame.fill_text(label);
            }
        }

        // Draw connections first (behind nodes)
        for conn in self.graph.connections() {
            if let (Some(from_node), Some(to_node)) = (
                self.graph.node(conn.from.node),
                self.graph.node(conn.to.node),
            ) {
                let from_pos = from_node.port_position(PortDirection::Output, conn.from.port);
                let to_pos = to_node.port_position(PortDirection::Input, conn.to.port);

                let from_pt =
                    Point::new(from_pos[0] * zoom + offset.x, from_pos[1] * zoom + offset.y);
                let to_pt = Point::new(to_pos[0] * zoom + offset.x, to_pos[1] * zoom + offset.y);

                let color = from_node
                    .outputs()
                    .get(conn.from.port)
                    .map(|p| data_type_color(p.data_type, self.tokens))
                    .unwrap_or(Color::from_rgb(0.5, 0.5, 0.5));

                draw_wire(&mut frame, from_pt, to_pt, color, zoom);

                // Type coercion indicator: show when Float → Vec2/Vec3/Vec4
                if let (Some(from_port), Some(to_port)) = (
                    from_node.outputs().get(conn.from.port),
                    to_node.inputs().get(conn.to.port),
                ) {
                    if from_port.data_type == DataType::Float
                        && from_port.data_type != to_port.data_type
                    {
                        let to_color = data_type_color(to_port.data_type, self.tokens);
                        // Diamond indicator near the target port
                        let dx = to_pt.x - 14.0 * zoom;
                        let dy = to_pt.y;
                        let ds = 4.0 * zoom;
                        let diamond = Path::new(|b| {
                            b.move_to(Point::new(dx, dy - ds));
                            b.line_to(Point::new(dx + ds, dy));
                            b.line_to(Point::new(dx, dy + ds));
                            b.line_to(Point::new(dx - ds, dy));
                            b.close();
                        });
                        frame.fill(&diamond, to_color);
                        frame.stroke(
                            &diamond,
                            Stroke::default()
                                .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.6))
                                .with_width(1.0),
                        );
                        // Small type label
                        let type_label = Text {
                            content: to_port.data_type.glsl_type().to_string(),
                            position: Point::new(dx - 16.0 * zoom, dy - 8.0 * zoom),
                            color: Color::from_rgba(to_color.r, to_color.g, to_color.b, 0.7),
                            size: iced::Pixels(9.0 * zoom),
                            ..Text::default()
                        };
                        frame.fill_text(type_label);
                    }
                }
            }
        }

        // Draw in-progress wire
        if let Interaction::DraggingWire { from_pos, end, .. } = &state.interaction {
            let from_pt = Point::new(from_pos[0] * zoom + offset.x, from_pos[1] * zoom + offset.y);
            draw_wire(&mut frame, from_pt, *end, self.tokens.wire_color, zoom);
        }

        // Build set of connected input ports
        let mut connected_inputs: HashSet<(NodeId, usize)> = HashSet::new();
        for conn in self.graph.connections() {
            connected_inputs.insert((conn.to.node, conn.to.port));
        }

        // Draw nodes
        for node in self.graph.nodes() {
            let live_val = self
                .canvas_state
                .live_values
                .get(&node.id)
                .map(|s| s.as_str());
            draw_node(
                &mut frame,
                node,
                offset,
                zoom,
                live_val,
                &connected_inputs,
                self.tokens,
            );
        }

        // Draw box selection rectangle
        if let Interaction::BoxSelecting { start, current } = &state.interaction {
            let x = start.x.min(current.x);
            let y = start.y.min(current.y);
            let w = (start.x - current.x).abs();
            let h = (start.y - current.y).abs();
            let rect = Path::rectangle(Point::new(x, y), Size::new(w, h));
            frame.fill(&rect, Color::from_rgba(0.3, 0.5, 1.0, 0.1));
            frame.stroke(
                &rect,
                Stroke::default()
                    .with_color(Color::from_rgba(0.3, 0.5, 1.0, 0.5))
                    .with_width(1.0),
            );
        }

        // Zoom indicator (bottom-right)
        let zoom_pct = format!("{:.0}%", zoom * 100.0);
        let zoom_label = Text {
            content: zoom_pct,
            position: Point::new(bounds.width - 60.0, bounds.height - 20.0),
            color: Color::from_rgba(1.0, 1.0, 1.0, 0.4),
            size: iced::Pixels(12.0),
            ..Text::default()
        };
        frame.fill_text(zoom_label);

        // Pending node placement indicator + ghost at cursor
        if let Some(kind) = &self.canvas_state.pending_node {
            let hint = format!("Click to place: {}  (Esc to cancel)", kind.label());
            let hint_label = Text {
                content: hint,
                position: Point::new(bounds.width / 2.0 - 100.0, 10.0),
                color: Color::from_rgba(1.0, 0.9, 0.3, 0.9),
                size: iced::Pixels(14.0),
                ..Text::default()
            };
            frame.fill_text(hint_label);

            // Draw a ghost node at cursor position
            if let Some(cursor_pos) = cursor.position_in(bounds) {
                let ghost_rect = Path::rounded_rectangle(
                    Point::new(cursor_pos.x + 10.0, cursor_pos.y + 10.0),
                    Size::new(120.0, 30.0),
                    4.0.into(),
                );
                frame.fill(&ghost_rect, Color::from_rgba(0.3, 0.5, 0.8, 0.2));
                frame.stroke(
                    &ghost_rect,
                    Stroke::default()
                        .with_color(Color::from_rgba(0.4, 0.6, 1.0, 0.5))
                        .with_width(1.0),
                );
                let ghost_text = Text {
                    content: kind.label().to_string(),
                    position: Point::new(cursor_pos.x + 15.0, cursor_pos.y + 15.0),
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.6),
                    size: iced::Pixels(13.0),
                    ..Text::default()
                };
                frame.fill_text(ghost_text);
            }
        }

        // Minimap overlay (bottom-left)
        if self.canvas_state.show_minimap {
            let nodes: Vec<_> = self.graph.nodes().collect();
            if !nodes.is_empty() {
                let minimap_w: f32 = 160.0;
                let minimap_h: f32 = 110.0;
                let minimap_margin: f32 = 10.0;
                let minimap_x = minimap_margin;
                let minimap_y = bounds.height - minimap_h - minimap_margin;

                // Compute world-space bounding box of all nodes
                let mut min_x = f32::MAX;
                let mut min_y = f32::MAX;
                let mut max_x = f32::MIN;
                let mut max_y = f32::MIN;
                for node in &nodes {
                    let [nx, ny] = node.position;
                    min_x = min_x.min(nx);
                    min_y = min_y.min(ny);
                    max_x = max_x.max(nx + node.width());
                    max_y = max_y.max(ny + node.height());
                }
                // Add padding
                let pad = 100.0;
                min_x -= pad;
                min_y -= pad;
                max_x += pad;
                max_y += pad;
                let world_w = (max_x - min_x).max(1.0);
                let world_h = (max_y - min_y).max(1.0);

                // Scale to fit minimap
                let scale = (minimap_w / world_w).min(minimap_h / world_h);

                // Minimap background
                let bg = Path::rectangle(
                    Point::new(minimap_x, minimap_y),
                    Size::new(minimap_w, minimap_h),
                );
                frame.fill(&bg, Color::from_rgba(0.05, 0.05, 0.1, 0.75));
                frame.stroke(
                    &bg,
                    Stroke::default()
                        .with_color(Color::from_rgba(0.4, 0.4, 0.5, 0.6))
                        .with_width(1.0),
                );

                // Draw node rectangles on minimap
                for node in &nodes {
                    let [nx, ny] = node.position;
                    let nw = node.width();
                    let nh = node.height();
                    let rx = minimap_x + (nx - min_x) * scale;
                    let ry = minimap_y + (ny - min_y) * scale;
                    let rw = (nw * scale).max(2.0);
                    let rh = (nh * scale).max(2.0);
                    let color = if node.selected {
                        Color::from_rgba(0.3, 0.7, 1.0, 0.9)
                    } else {
                        Color::from_rgba(0.6, 0.6, 0.7, 0.7)
                    };
                    let r = Path::rectangle(Point::new(rx, ry), Size::new(rw, rh));
                    frame.fill(&r, color);
                }

                // Viewport indicator — clamped to minimap bounds
                let vp_x_raw = (-offset.x / zoom - min_x) * scale + minimap_x;
                let vp_y_raw = (-offset.y / zoom - min_y) * scale + minimap_y;
                let vp_w_raw = (bounds.width / zoom) * scale;
                let vp_h_raw = (bounds.height / zoom) * scale;
                // Clamp to minimap rectangle
                let vp_x = vp_x_raw.max(minimap_x).min(minimap_x + minimap_w);
                let vp_y = vp_y_raw.max(minimap_y).min(minimap_y + minimap_h);
                let vp_w = vp_w_raw.min(minimap_x + minimap_w - vp_x).max(0.0);
                let vp_h = vp_h_raw.min(minimap_y + minimap_h - vp_y).max(0.0);
                if vp_w > 0.0 && vp_h > 0.0 {
                    let vp = Path::rectangle(Point::new(vp_x, vp_y), Size::new(vp_w, vp_h));
                    frame.stroke(
                        &vp,
                        Stroke::default()
                            .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.5))
                            .with_width(1.0),
                    );
                }
            }
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match &state.interaction {
            Interaction::DraggingNode { .. } => mouse::Interaction::Grabbing,
            Interaction::DraggingWire { .. } => mouse::Interaction::Crosshair,
            Interaction::Panning { .. } => mouse::Interaction::Grabbing,
            Interaction::BoxSelecting { .. } => mouse::Interaction::Crosshair,
            Interaction::None => {
                // Show crosshair when pending node placement
                if self.canvas_state.pending_node.is_some() {
                    return mouse::Interaction::Crosshair;
                }
                if let Some(pos) = cursor.position_in(bounds) {
                    let offset = self.canvas_state.offset;
                    let zoom = self.canvas_state.zoom;
                    if self.hit_test_port(pos, offset, zoom).is_some() {
                        mouse::Interaction::Pointer
                    } else if self.hit_test_node(pos, offset, zoom).is_some() {
                        mouse::Interaction::Grab
                    } else {
                        mouse::Interaction::default()
                    }
                } else {
                    mouse::Interaction::default()
                }
            }
        }
    }
}

impl<'a> GraphProgram<'a> {
    /// Check if `pos` (in canvas coords) hits a port.
    /// Returns (node_id, direction, port_index, world_position).
    fn hit_test_port(
        &self,
        pos: Point,
        offset: Vector,
        zoom: f32,
    ) -> Option<(NodeId, PortDirection, usize, [f32; 2])> {
        let hit_radius = (PORT_RADIUS + 4.0) * zoom;
        let hit_r2 = hit_radius.powi(2);
        let mut best: Option<(NodeId, PortDirection, usize, [f32; 2], f32)> = None;
        for node in self.graph.nodes() {
            // Input ports
            for (i, _) in node.inputs().iter().enumerate() {
                let [px, py] = node.port_position(PortDirection::Input, i);
                let sp = Point::new(px * zoom + offset.x, py * zoom + offset.y);
                let d2 = (pos.x - sp.x).powi(2) + (pos.y - sp.y).powi(2);
                if d2 < hit_r2 && best.as_ref().is_none_or(|b| d2 < b.4) {
                    best = Some((node.id, PortDirection::Input, i, [px, py], d2));
                }
            }
            // Output ports
            for (i, _) in node.outputs().iter().enumerate() {
                let [px, py] = node.port_position(PortDirection::Output, i);
                let sp = Point::new(px * zoom + offset.x, py * zoom + offset.y);
                let d2 = (pos.x - sp.x).powi(2) + (pos.y - sp.y).powi(2);
                if d2 < hit_r2 && best.as_ref().is_none_or(|b| d2 < b.4) {
                    best = Some((node.id, PortDirection::Output, i, [px, py], d2));
                }
            }
        }
        best.map(|(id, dir, idx, pos, _)| (id, dir, idx, pos))
    }

    /// Check if `pos` hits a node body. Returns the topmost node ID.
    fn hit_test_node(&self, pos: Point, offset: Vector, zoom: f32) -> Option<NodeId> {
        // We want the last-drawn (topmost) node, so keep iterating
        // and return the last match found.
        let mut best: Option<NodeId> = None;
        for node in self.graph.nodes() {
            let x = node.position[0] * zoom + offset.x;
            let y = node.position[1] * zoom + offset.y;
            let w = node.width() * zoom;
            let h = node.height() * zoom;
            if pos.x >= x && pos.x <= x + w && pos.y >= y && pos.y <= y + h {
                // HashMap iteration order is arbitrary, but we still
                // take the last match (which at least is deterministic
                // per-session).  A proper z-order list would be ideal.
                best = Some(node.id);
            }
        }
        best
    }

    /// Check if `pos` (screen coords) hits a value pill on an unconnected input port.
    /// Returns (node_id, port_index) of the hit pill, or None.
    fn hit_test_value_pill(
        &self,
        pos: Point,
        offset: Vector,
        zoom: f32,
    ) -> Option<(NodeId, usize)> {
        let connected_inputs: HashSet<(NodeId, usize)> = self
            .graph
            .connections()
            .iter()
            .map(|c| (c.to.node, c.to.port))
            .collect();

        for node in self.graph.nodes() {
            let x = node.position[0] * zoom + offset.x;
            let y = node.position[1] * zoom + offset.y;
            let w = node.width() * zoom;
            let header_h = HEADER_HEIGHT * zoom;
            let port_spacing = PORT_SPACING * zoom;

            for (i, _) in node.inputs().iter().enumerate() {
                if connected_inputs.contains(&(node.id, i)) {
                    continue;
                }
                if node.defaults.get(i).is_none() {
                    continue;
                }
                let py = y + header_h + i as f32 * port_spacing + 12.0 * zoom;
                let val_x = x + 60.0 * zoom;
                let val_w = w - 65.0 * zoom;
                let val_h = 14.0 * zoom;
                let val_y = py - 7.0 * zoom;

                if pos.x >= val_x
                    && pos.x <= val_x + val_w
                    && pos.y >= val_y
                    && pos.y <= val_y + val_h
                {
                    return Some((node.id, i));
                }
            }
        }
        None
    }

    /// Check if `pos` (screen coords) is close to a wire (connection).
    /// Returns the ConnectionId of the nearest hit wire, or None.
    fn hit_test_wire(&self, pos: Point, offset: Vector, zoom: f32) -> Option<ConnectionId> {
        let threshold = 6.0_f32;
        let mut best: Option<(ConnectionId, f32)> = None;

        for conn in self.graph.connections() {
            let (from_node, to_node) = match (
                self.graph.node(conn.from.node),
                self.graph.node(conn.to.node),
            ) {
                (Some(f), Some(t)) => (f, t),
                _ => continue,
            };

            let [fx, fy] = from_node.port_position(PortDirection::Output, conn.from.port);
            let [tx, ty] = to_node.port_position(PortDirection::Input, conn.to.port);

            let from_screen = Point::new(fx * zoom + offset.x, fy * zoom + offset.y);
            let to_screen = Point::new(tx * zoom + offset.x, ty * zoom + offset.y);

            // Sample the Bézier curve and find minimum distance
            let dist = point_to_bezier_distance(pos, from_screen, to_screen);
            if dist < threshold && best.is_none_or(|(_, d)| dist < d) {
                best = Some((conn.id, dist));
            }
        }

        best.map(|(id, _)| id)
    }
}

/// Mutable interaction state stored by the canvas widget between events.
pub struct CanvasInteraction {
    interaction: Interaction,
    /// Whether the Shift key is currently held.
    pub shift_held: bool,
    /// Last click time + node for double-click detection (sub-graph nodes).
    last_click: Option<(std::time::Instant, NodeId)>,
    /// Last click time + position for double-click detection (value pills).
    last_value_click: Option<(std::time::Instant, Point)>,
}

impl Default for CanvasInteraction {
    fn default() -> Self {
        Self {
            interaction: Interaction::None,
            shift_held: false,
            last_click: None,
            last_value_click: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Background grid
// ---------------------------------------------------------------------------

/// Approximate minimum distance from a point to a cubic Bézier curve
/// by sampling.  Uses the same control-point formula as `draw_wire`.
fn point_to_bezier_distance(p: Point, from: Point, to: Point) -> f32 {
    let dx = (to.x - from.x).abs() * 0.5;
    let ctrl1 = Point::new(from.x + dx, from.y);
    let ctrl2 = Point::new(to.x - dx, to.y);

    let samples = 20;
    let mut min_dist = f32::MAX;
    for i in 0..=samples {
        let t = i as f32 / samples as f32;
        let it = 1.0 - t;
        let bx = it * it * it * from.x
            + 3.0 * it * it * t * ctrl1.x
            + 3.0 * it * t * t * ctrl2.x
            + t * t * t * to.x;
        let by = it * it * it * from.y
            + 3.0 * it * it * t * ctrl1.y
            + 3.0 * it * t * t * ctrl2.y
            + t * t * t * to.y;
        let dx2 = p.x - bx;
        let dy2 = p.y - by;
        let dist = (dx2 * dx2 + dy2 * dy2).sqrt();
        if dist < min_dist {
            min_dist = dist;
        }
    }
    min_dist
}

fn draw_grid(frame: &mut Frame, size: Size, offset: Vector, zoom: f32) {
    let grid_size = 30.0f32 * zoom;
    let dot_color = Color::from_rgba(1.0, 1.0, 1.0, 0.06);

    // Skip grid if zoom is too small (dots would be invisible)
    if grid_size < 5.0 {
        return;
    }

    let start_x = (offset.x % grid_size) - grid_size;
    let start_y = (offset.y % grid_size) - grid_size;

    let dot_radius = (1.0 * zoom).max(0.5);

    let mut x = start_x;
    while x < size.width + grid_size {
        let mut y = start_y;
        while y < size.height + grid_size {
            let dot = Path::circle(Point::new(x, y), dot_radius);
            frame.fill(&dot, dot_color);
            y += grid_size;
        }
        x += grid_size;
    }
}

// ---------------------------------------------------------------------------
// Public widget constructor
// ---------------------------------------------------------------------------

/// Build the graph canvas widget.
pub fn graph_canvas<'a>(
    graph: &'a ShaderGraph,
    canvas_state: &'a GraphCanvas,
    tokens: &'a ThemeTokens,
) -> Element<'a, GraphMessage> {
    use iced::Padding;
    use iced::widget::{Space, column, row, stack, text_input};

    let canvas_elem: Element<'a, GraphMessage> = Canvas::new(GraphProgram {
        graph,
        canvas_state,
        tokens,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into();

    if let Some((node_id, port_idx, ref edit_text)) = canvas_state.editing_value {
        if let Some(node) = graph.node(node_id) {
            let zoom = canvas_state.zoom;
            let offset = canvas_state.offset;

            let pill_x = node.position[0] * zoom + offset.x + 60.0 * zoom;
            let pill_y = node.position[1] * zoom
                + offset.y
                + HEADER_HEIGHT * zoom
                + port_idx as f32 * PORT_SPACING * zoom
                + 5.0 * zoom;
            let pill_w = (node.width() - 65.0) * zoom;

            let px = pill_x.max(0.0);
            let py = pill_y.max(0.0);
            let pw = pill_w.max(40.0);

            let bg = tokens.bg_tertiary;
            let txt_color = tokens.text_primary;
            let accent = tokens.text_accent;

            let input = text_input("value...", edit_text)
                .on_input(GraphMessage::EditValueChanged)
                .on_submit(GraphMessage::CommitEditValue)
                .size(11.0 * zoom.max(0.5))
                .width(pw)
                .padding(Padding::from([2, 4]))
                .style(move |_theme: &Theme, _status| text_input::Style {
                    background: iced::Background::Color(bg),
                    border: iced::Border {
                        color: accent,
                        width: 1.5,
                        radius: (3.0).into(),
                    },
                    icon: txt_color,
                    placeholder: Color::from_rgba(0.5, 0.5, 0.5, 0.6),
                    value: txt_color,
                    selection: Color::from_rgba(0.3, 0.5, 0.8, 0.4),
                });

            let overlay: Element<'a, GraphMessage> = column![
                Space::new().width(0).height(py),
                row![Space::new().width(px).height(0), input,],
            ]
            .into();

            stack![canvas_elem, overlay]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            canvas_elem
        }
    } else {
        canvas_elem
    }
}
