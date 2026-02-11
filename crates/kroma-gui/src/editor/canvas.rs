//! Interactive canvas for the node-based shader editor.
//!
//! Renders nodes as rounded rectangles with coloured ports, and wires
//! as smooth Bézier curves.  Supports drag-to-move, drag-to-connect,
//! and selection.

use iced::mouse;
use iced::widget::canvas::{self, Canvas, Event, Frame, Geometry, Path, Stroke, Text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, Vector};
use std::collections::HashSet;

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
    #[allow(dead_code)]
    ConnectionDeleted(ConnectionId),
    /// User selected / deselected a node.
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
}

// ---------------------------------------------------------------------------
// Graph canvas state
// ---------------------------------------------------------------------------

pub struct GraphCanvas {
    /// Camera offset (pan).
    pub offset: Vector,
    /// Zoom level (1.0 = 100%).
    pub zoom: f32,
    /// Current interaction.
    interaction: Interaction,
    /// Live values for input nodes (e.g. Time → "3.42").
    pub live_values: std::collections::HashMap<NodeId, String>,
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
            interaction: Interaction::None,
            live_values: std::collections::HashMap::new(),
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

fn data_type_color(dt: DataType) -> Color {
    let [r, g, b] = dt.color();
    Color::from_rgb(r, g, b)
}

fn node_bg_color(kind: &NodeKind) -> Color {
    match kind.category() {
        "Output" => Color::from_rgb(0.35, 0.18, 0.18),
        "Input" | "System" => Color::from_rgb(0.15, 0.22, 0.32),
        "Constants" => Color::from_rgb(0.22, 0.22, 0.15),
        "Math" => Color::from_rgb(0.18, 0.25, 0.18),
        "Vector" => Color::from_rgb(0.2, 0.18, 0.28),
        "Color" => Color::from_rgb(0.28, 0.18, 0.22),
        "Procedural" => Color::from_rgb(0.18, 0.22, 0.25),
        _ => Color::from_rgb(0.18, 0.18, 0.18),
    }
}

fn node_header_color(kind: &NodeKind) -> Color {
    match kind.category() {
        "Output" => Color::from_rgb(0.6, 0.2, 0.2),
        "Input" | "System" => Color::from_rgb(0.18, 0.35, 0.55),
        "Constants" => Color::from_rgb(0.45, 0.4, 0.15),
        "Math" => Color::from_rgb(0.2, 0.45, 0.2),
        "Vector" => Color::from_rgb(0.35, 0.22, 0.5),
        "Color" => Color::from_rgb(0.5, 0.2, 0.35),
        "Procedural" => Color::from_rgb(0.2, 0.35, 0.4),
        _ => Color::from_rgb(0.25, 0.25, 0.25),
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
) {
    let x = node.position[0] * zoom + offset.x;
    let y = node.position[1] * zoom + offset.y;
    let w = node.width() * zoom;
    let h = node.height() * zoom;

    // Extra height for live-value row
    let value_row_h = if live_value.is_some() { 20.0 * zoom } else { 0.0 };
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
    frame.fill(&body, node_bg_color(&node.kind));

    // Selection highlight
    if node.selected {
        frame.stroke(
            &body,
            Stroke::default()
                .with_color(Color::from_rgb(0.4, 0.7, 1.0))
                .with_width(2.0),
        );
    }

    // Header
    let header = Path::rectangle(
        Point::new(x, y),
        Size::new(w, header_h),
    );
    frame.fill(&header, node_header_color(&node.kind));

    // Header text
    let label = Text {
        content: node.kind.label().to_string(),
        position: Point::new(x + 8.0 * zoom, y + 6.0 * zoom),
        color: Color::WHITE,
        size: iced::Pixels(font_size),
        ..Text::default()
    };
    frame.fill_text(label);

    // Input ports
    for (i, port_def) in node.inputs().iter().enumerate() {
        let py = y + header_h + i as f32 * port_spacing + 12.0 * zoom;
        let port = Path::circle(Point::new(x, py), port_radius);
        frame.fill(&port, data_type_color(port_def.data_type));

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
                    DefaultValue::Vec4(v) => format!("{:.1},{:.1},{:.1},{:.1}", v[0], v[1], v[2], v[3]),
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
        frame.fill(&port, data_type_color(port_def.data_type));

        let port_label = Text {
            content: port_def.name.clone(),
            position: Point::new(x + w - 10.0 * zoom, py - 6.0 * zoom),
            color: Color::from_rgb(0.8, 0.8, 0.8),
            size: iced::Pixels(small_font),
            horizontal_alignment: iced::alignment::Horizontal::Right,
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
}

impl<'a> canvas::Program<GraphMessage> for GraphProgram<'a> {
    type State = CanvasInteraction;

    fn update(
        &self,
        state: &mut Self::State,
        event: Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<GraphMessage>) {
        let Some(cursor_pos) = cursor.position_in(bounds) else {
            return (canvas::event::Status::Ignored, None);
        };

        let offset = self.canvas_state.offset;
        let zoom = self.canvas_state.zoom;

        match event {
            // ---- Scroll: adjust port default OR zoom -----------------
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let scroll_y = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 80.0,
                };

                // Check if hovering over an input port — adjust default value
                if let Some((node_id, dir, port_idx, _)) =
                    self.hit_test_port(cursor_pos, offset, zoom)
                {
                    if dir == PortDirection::Input {
                        // Check if this port is unconnected
                        let is_connected = self.graph.connections().iter()
                            .any(|c| c.to.node == node_id && c.to.port == port_idx);
                        if !is_connected {
                            if let Some(node) = self.graph.node(node_id) {
                                if let Some(def) = node.defaults.get(port_idx) {
                                    let step = 0.1 * scroll_y;
                                    let new_def = match def {
                                        DefaultValue::Float(v) => DefaultValue::Float(v + step),
                                        DefaultValue::Vec2(v) => DefaultValue::Vec2([v[0] + step, v[1] + step]),
                                        DefaultValue::Vec3(v) => DefaultValue::Vec3([v[0] + step, v[1] + step, v[2] + step]),
                                        DefaultValue::Vec4(v) => DefaultValue::Vec4([v[0] + step, v[1] + step, v[2] + step, v[3] + step]),
                                    };
                                    return (
                                        canvas::event::Status::Captured,
                                        Some(GraphMessage::DefaultChanged(node_id, port_idx, new_def)),
                                    );
                                }
                            }
                        }
                    }
                }

                // Otherwise: zoom
                let factor = if scroll_y > 0.0 { 1.1 } else { 1.0 / 1.1 };
                let new_zoom = (zoom * factor).clamp(0.15, 5.0);
                let world_x = (cursor_pos.x - offset.x) / zoom;
                let world_y = (cursor_pos.y - offset.y) / zoom;
                let new_offset = Vector::new(
                    cursor_pos.x - world_x * new_zoom,
                    cursor_pos.y - world_y * new_zoom,
                );
                return (
                    canvas::event::Status::Captured,
                    Some(GraphMessage::Zoomed(new_zoom, new_offset)),
                );
            }

            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
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
                        return (canvas::event::Status::Captured, None);
                    }
                }

                // Check if clicking on a node
                if let Some(node_id) = self.hit_test_node(cursor_pos, offset, zoom) {
                    let node = self.graph.node(node_id).unwrap();
                    state.interaction = Interaction::DraggingNode {
                        node_id,
                        start_mouse: cursor_pos,
                        start_pos: node.position,
                    };
                    return (
                        canvas::event::Status::Captured,
                        Some(GraphMessage::NodeSelected(node_id, true)),
                    );
                }

                // Clicking empty space — pan
                state.interaction = Interaction::Panning {
                    start: cursor_pos,
                    start_offset: offset,
                };
                (canvas::event::Status::Captured, None)
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
                    (
                        canvas::event::Status::Captured,
                        Some(GraphMessage::NodeMoved(*node_id, new_pos)),
                    )
                }
                Interaction::DraggingWire { end, .. } => {
                    *end = cursor_pos;
                    (canvas::event::Status::Captured, None)
                }
                Interaction::Panning {
                    start,
                    start_offset,
                } => {
                    let dx = cursor_pos.x - start.x;
                    let dy = cursor_pos.y - start.y;
                    let new_offset = Vector::new(start_offset.x + dx, start_offset.y + dy);
                    (
                        canvas::event::Status::Captured,
                        Some(GraphMessage::Panned(new_offset)),
                    )
                }
                Interaction::None => (canvas::event::Status::Ignored, None),
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
                    _ => None,
                };
                state.interaction = Interaction::None;
                (canvas::event::Status::Captured, msg)
            }

            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                // Right-click: open palette at cursor (world coords)
                let world_pos = [
                    (cursor_pos.x - offset.x) / zoom,
                    (cursor_pos.y - offset.y) / zoom,
                ];
                (
                    canvas::event::Status::Captured,
                    Some(GraphMessage::OpenPalette(world_pos)),
                )
            }

            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Delete),
                ..
            }) => (
                canvas::event::Status::Captured,
                Some(GraphMessage::DeleteSelected),
            ),

            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let offset = self.canvas_state.offset;
        let zoom = self.canvas_state.zoom;

        let mut frame = Frame::new(renderer, bounds.size());

        // Background grid
        draw_grid(&mut frame, bounds.size(), offset, zoom);

        // Draw connections first (behind nodes)
        for conn in self.graph.connections() {
            if let (Some(from_node), Some(to_node)) =
                (self.graph.node(conn.from.node), self.graph.node(conn.to.node))
            {
                let from_pos = from_node.port_position(PortDirection::Output, conn.from.port);
                let to_pos = to_node.port_position(PortDirection::Input, conn.to.port);

                let from_pt = Point::new(
                    from_pos[0] * zoom + offset.x,
                    from_pos[1] * zoom + offset.y,
                );
                let to_pt = Point::new(
                    to_pos[0] * zoom + offset.x,
                    to_pos[1] * zoom + offset.y,
                );

                let color = from_node
                    .outputs()
                    .get(conn.from.port)
                    .map(|p| data_type_color(p.data_type))
                    .unwrap_or(Color::from_rgb(0.5, 0.5, 0.5));

                draw_wire(&mut frame, from_pt, to_pt, color, zoom);
            }
        }

        // Draw in-progress wire
        if let Interaction::DraggingWire {
            from_pos, end, ..
        } = &self.canvas_state.interaction
        {
            let from_pt = Point::new(
                from_pos[0] * zoom + offset.x,
                from_pos[1] * zoom + offset.y,
            );
            draw_wire(
                &mut frame,
                from_pt,
                *end,
                Color::from_rgba(1.0, 1.0, 1.0, 0.5),
                zoom,
            );
        }

        // Build set of connected input ports
        let mut connected_inputs: HashSet<(NodeId, usize)> = HashSet::new();
        for conn in self.graph.connections() {
            connected_inputs.insert((conn.to.node, conn.to.port));
        }

        // Draw nodes
        for node in self.graph.nodes() {
            let live_val = self.canvas_state.live_values.get(&node.id).map(|s| s.as_str());
            draw_node(&mut frame, node, offset, zoom, live_val, &connected_inputs);
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
            Interaction::None => {
                if let Some(pos) = cursor.position_in(bounds) {
                    let offset = self.canvas_state.offset;
                    let zoom = self.canvas_state.zoom;
                    if self
                        .hit_test_port(pos, offset, zoom)
                        .is_some()
                    {
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
        for node in self.graph.nodes() {
            // Input ports
            for (i, _) in node.inputs().iter().enumerate() {
                let [px, py] = node.port_position(PortDirection::Input, i);
                let sp = Point::new(px * zoom + offset.x, py * zoom + offset.y);
                if (pos.x - sp.x).powi(2) + (pos.y - sp.y).powi(2) < hit_radius.powi(2)
                {
                    return Some((node.id, PortDirection::Input, i, [px, py]));
                }
            }
            // Output ports
            for (i, _) in node.outputs().iter().enumerate() {
                let [px, py] = node.port_position(PortDirection::Output, i);
                let sp = Point::new(px * zoom + offset.x, py * zoom + offset.y);
                if (pos.x - sp.x).powi(2) + (pos.y - sp.y).powi(2) < hit_radius.powi(2)
                {
                    return Some((node.id, PortDirection::Output, i, [px, py]));
                }
            }
        }
        None
    }

    /// Check if `pos` hits a node body. Returns the topmost node ID.
    fn hit_test_node(&self, pos: Point, offset: Vector, zoom: f32) -> Option<NodeId> {
        // Iterate in reverse to hit topmost nodes first
        let mut best: Option<NodeId> = None;
        for node in self.graph.nodes() {
            let x = node.position[0] * zoom + offset.x;
            let y = node.position[1] * zoom + offset.y;
            let w = node.width() * zoom;
            let h = node.height() * zoom;
            if pos.x >= x && pos.x <= x + w && pos.y >= y && pos.y <= y + h
            {
                best = Some(node.id);
            }
        }
        best
    }
}

/// Mutable interaction state stored by the canvas widget between events.
pub struct CanvasInteraction {
    interaction: Interaction,
}

impl Default for CanvasInteraction {
    fn default() -> Self {
        Self {
            interaction: Interaction::None,
        }
    }
}

// ---------------------------------------------------------------------------
// Background grid
// ---------------------------------------------------------------------------

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
) -> Element<'a, GraphMessage> {
    Canvas::new(GraphProgram {
        graph,
        canvas_state,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
