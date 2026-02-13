//! Graph message handler and daemon communication (extracted from main.rs).

use crate::commands;
use crate::editor;
use crate::sysinfo;
use crate::utils;

use super::{ClipboardData, KromaApp};

impl KromaApp {
    /// Unified handler for graph messages (EditorGraph and ShadeGraphMsg).
    /// This avoids duplicating ~170 lines of near-identical code.
    pub(crate) fn handle_graph_msg(
        &mut self,
        target: commands::GraphTarget,
        graph_msg: editor::canvas::GraphMessage,
    ) {
        use editor::canvas::GraphMessage;

        // Helper macro to access the correct graph/canvas/flags for the target.
        macro_rules! graph {
            () => {
                match target {
                    commands::GraphTarget::Editor => &self.shader_graph,
                    commands::GraphTarget::Shade => &self.shade_graph,
                }
            };
        }
        macro_rules! graph_mut {
            () => {
                match target {
                    commands::GraphTarget::Editor => &mut self.shader_graph,
                    commands::GraphTarget::Shade => &mut self.shade_graph,
                }
            };
        }
        macro_rules! canvas_mut {
            () => {
                match target {
                    commands::GraphTarget::Editor => &mut self.graph_canvas,
                    commands::GraphTarget::Shade => &mut self.shade_graph_canvas,
                }
            };
        }
        macro_rules! mark_dirty {
            () => {
                match target {
                    commands::GraphTarget::Editor => {
                        self.editor_graph_dirty = true;
                        self.editor_graph_last_edit = std::time::Instant::now();
                    }
                    commands::GraphTarget::Shade => {
                        self.shade_graph_dirty = true;
                        self.shade_graph_last_edit = std::time::Instant::now();
                    }
                }
            };
        }

        match graph_msg {
            GraphMessage::NodeMoved(id, new_pos) => {
                let old_pos = graph!().node(id).map(|n| n.position).unwrap_or(new_pos);
                let dx = new_pos[0] - old_pos[0];
                let dy = new_pos[1] - old_pos[1];
                let other_moves: Vec<_> = graph!()
                    .nodes()
                    .filter(|n| n.selected && n.id != id)
                    .map(|n| (n.id, n.position, [n.position[0] + dx, n.position[1] + dy]))
                    .collect();
                for &(other_id, _, ref new_p) in &other_moves {
                    if let Some(n) = graph_mut!().node_mut(other_id) {
                        n.position = *new_p;
                    }
                }
                let cmd = commands::MoveNodeCmd::new_multi(
                    target,
                    id,
                    old_pos,
                    new_pos,
                    other_moves.iter().map(|&(nid, old, new)| (nid, old, new)).collect(),
                );
                let _ = self.command_history.execute(
                    Box::new(cmd),
                    &mut self.shader_graph,
                    &mut self.shade_graph,
                );
            }
            GraphMessage::ConnectionCreated(from, to) => {
                let cmd = commands::ConnectCmd::new(target, from, to);
                let _ = self.command_history.execute(
                    Box::new(cmd),
                    &mut self.shader_graph,
                    &mut self.shade_graph,
                );
                mark_dirty!();
            }
            GraphMessage::ConnectionDeleted(id) => {
                let cmd = commands::DisconnectCmd::new(target, id);
                let _ = self.command_history.execute(
                    Box::new(cmd),
                    &mut self.shader_graph,
                    &mut self.shade_graph,
                );
                mark_dirty!();
            }
            GraphMessage::NodeSelected(id, shift) => {
                if shift {
                    if let Some(n) = graph_mut!().node_mut(id) {
                        n.selected = !n.selected;
                    }
                } else {
                    let already_selected =
                        graph!().node(id).map(|n| n.selected).unwrap_or(false);
                    if !already_selected {
                        let ids: Vec<_> = graph!().nodes().map(|n| n.id).collect();
                        for nid in ids {
                            if let Some(n) = graph_mut!().node_mut(nid) {
                                n.selected = nid == id;
                            }
                        }
                    }
                }
            }
            GraphMessage::BoxSelected(ids) => {
                for node in graph_mut!().nodes_mut() {
                    if ids.contains(&node.id) {
                        node.selected = true;
                    }
                }
            }
            GraphMessage::DeleteSelected => {
                let to_delete: Vec<_> = graph!()
                    .nodes()
                    .filter(|n| n.selected)
                    .map(|n| n.id)
                    .collect();
                if !to_delete.is_empty() {
                    let cmd = commands::RemoveNodesCmd::new(target, to_delete);
                    let _ = self.command_history.execute(
                        Box::new(cmd),
                        &mut self.shader_graph,
                        &mut self.shade_graph,
                    );
                    mark_dirty!();
                }
            }
            GraphMessage::OpenPalette(pos) => {
                if matches!(target, commands::GraphTarget::Editor) {
                    self.editor_palette_open = true;
                    self.editor_palette_pos = pos;
                    self.editor_palette_filter.clear();
                }
            }
            GraphMessage::Panned(offset) => {
                canvas_mut!().offset = offset;
            }
            GraphMessage::Zoomed(zoom, offset) => {
                canvas_mut!().zoom = zoom;
                canvas_mut!().offset = offset;
            }
            GraphMessage::DefaultChanged(node_id, port_idx, new_val) => {
                let old_val = graph!()
                    .node(node_id)
                    .and_then(|n| n.defaults.get(port_idx).cloned())
                    .unwrap_or(new_val.clone());
                let cmd = commands::ChangeDefaultCmd::new(
                    target, node_id, port_idx, old_val, new_val,
                );
                let _ = self.command_history.execute(
                    Box::new(cmd),
                    &mut self.shader_graph,
                    &mut self.shade_graph,
                );
                mark_dirty!();
            }
            GraphMessage::CopySelected => {
                let selected: Vec<_> = graph!()
                    .nodes()
                    .filter(|n| n.selected && n.kind != editor::NodeKind::Output)
                    .collect();
                if !selected.is_empty() {
                    let id_to_idx: std::collections::HashMap<kroma_graph::types::NodeId, usize> =
                        selected.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
                    let cx = selected.iter().map(|n| n.position[0]).sum::<f32>()
                        / selected.len() as f32;
                    let cy = selected.iter().map(|n| n.position[1]).sum::<f32>()
                        / selected.len() as f32;
                    let nodes: Vec<_> = selected
                        .iter()
                        .map(|n| {
                            (
                                n.kind.clone(),
                                [n.position[0] - cx, n.position[1] - cy],
                                n.defaults.clone(),
                            )
                        })
                        .collect();
                    let connections: Vec<_> = graph!()
                        .connections()
                        .iter()
                        .filter_map(|c| {
                            let fi = id_to_idx.get(&c.from.node)?;
                            let ti = id_to_idx.get(&c.to.node)?;
                            Some((*fi, c.from.port, *ti, c.to.port))
                        })
                        .collect();
                    self.clipboard = Some(ClipboardData { nodes, connections });
                }
            }
            GraphMessage::PasteNodes => {
                if let Some(clip) = self.clipboard.clone() {
                    let base = match target {
                        commands::GraphTarget::Editor => self.editor_palette_pos,
                        commands::GraphTarget::Shade => [300.0, 250.0],
                    };
                    let mut new_ids = Vec::with_capacity(clip.nodes.len());
                    let all_ids: Vec<_> = graph!().nodes().map(|n| n.id).collect();
                    for nid in &all_ids {
                        if let Some(n) = graph_mut!().node_mut(*nid) {
                            n.selected = false;
                        }
                    }
                    for (kind, rel_pos, defaults) in &clip.nodes {
                        let pos = [base[0] + rel_pos[0], base[1] + rel_pos[1]];
                        let id = graph_mut!().add_node(kind.clone(), pos);
                        if let Some(node) = graph_mut!().node_mut(id) {
                            node.defaults = defaults.clone();
                            node.selected = true;
                        }
                        new_ids.push(id);
                    }
                    for &(fi, fp, ti, tp) in &clip.connections {
                        if fi < new_ids.len() && ti < new_ids.len() {
                            let from = kroma_graph::types::PortAddr {
                                node: new_ids[fi],
                                port: fp,
                            };
                            let to = kroma_graph::types::PortAddr {
                                node: new_ids[ti],
                                port: tp,
                            };
                            graph_mut!().add_connection(from, to);
                        }
                    }
                    mark_dirty!();
                }
            }
            GraphMessage::PlacePendingNode(pos) => {
                if canvas_mut!().pending_node.is_some() {
                    let kind = canvas_mut!().pending_node.take().unwrap();
                    let cmd = commands::AddNodeCmd::new(target, kind, pos);
                    let _ = self.command_history.execute(
                        Box::new(cmd),
                        &mut self.shader_graph,
                        &mut self.shade_graph,
                    );
                    mark_dirty!();
                }
            }
            GraphMessage::GroupSelected => {
                let (graph, frames) = match target {
                    commands::GraphTarget::Editor => {
                        (&self.shader_graph, &mut self.graph_canvas.comment_frames)
                    }
                    commands::GraphTarget::Shade => {
                        (&self.shade_graph, &mut self.shade_graph_canvas.comment_frames)
                    }
                };
                Self::create_comment_frame(graph, frames);
            }
            GraphMessage::EnterSubGraph(node_id) => {
                let (subgraphs, nav_path) = match target {
                    commands::GraphTarget::Editor => {
                        (&mut self.editor_subgraphs, &mut self.editor_nav_path)
                    }
                    commands::GraphTarget::Shade => {
                        (&mut self.shade_subgraphs, &mut self.shade_nav_path)
                    }
                };
                subgraphs.entry(node_id).or_insert_with(|| {
                    (editor::ShaderGraph::new(), editor::canvas::GraphCanvas::new())
                });
                nav_path.push(node_id);
            }
            GraphMessage::ExitSubGraph => {
                match target {
                    commands::GraphTarget::Editor => { self.editor_nav_path.pop(); }
                    commands::GraphTarget::Shade => { self.shade_nav_path.pop(); }
                }
            }
            GraphMessage::ToggleMinimap => {
                let canvas = canvas_mut!();
                canvas.show_minimap = !canvas.show_minimap;
            }
        }
    }

    /// Create a comment frame around selected nodes in the given graph.
    fn create_comment_frame(
        graph: &editor::ShaderGraph,
        frames: &mut Vec<editor::canvas::CommentFrame>,
    ) {
        let selected: Vec<_> = graph.nodes().filter(|n| n.selected).collect();
        if selected.is_empty() {
            return;
        }
        use crate::editor::NodeLayout;
        let padding = 30.0;
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for n in &selected {
            min_x = min_x.min(n.position[0]);
            min_y = min_y.min(n.position[1]);
            max_x = max_x.max(n.position[0] + n.width());
            max_y = max_y.max(n.position[1] + n.height());
        }
        // Random-ish color from frame count
        let hue = (frames.len() as f32 * 0.618) % 1.0;
        let (r, g, b) = utils::hsv_to_rgb(hue, 0.5, 0.9);
        frames.push(editor::canvas::CommentFrame {
            rect: [
                min_x - padding,
                min_y - padding - 20.0, // extra space for label
                (max_x - min_x) + padding * 2.0,
                (max_y - min_y) + padding * 2.0 + 20.0,
            ],
            label: format!("Group {}", frames.len() + 1),
            color: [r, g, b, 1.0],
        });
    }

    /// Update live value display strings for Input/System nodes.
    pub(crate) fn update_live_values(&mut self) {
        let elapsed = self.app_start.elapsed().as_secs_f32();
        let frame_count = (elapsed * 60.0) as u32; // approximate

        let cpu_pct = sysinfo::read_cpu_usage_fast();
        let (used, total) = sysinfo::read_mem_info();
        let ram_pct = if total > 0 { (used as f32 / total as f32) * 100.0 } else { 0.0 };
        let battery_pct = sysinfo::read_battery_pct();
        let audio_level = 0.0_f32;
        let cursor_x = 0.0_f32;
        let cursor_y = 0.0_f32;

        let cursor_str = format!("{:.0}, {:.0}", cursor_x, cursor_y);
        let audio_str = format!("{:.2}", audio_level);

        struct LiveCtx<'a> {
            elapsed: f32,
            frame: u32,
            fps: f32,
            cpu: f32,
            ram: f32,
            battery: Option<f32>,
            cursor: &'a str,
            audio: &'a str,
        }

        fn value_for(kind: &editor::NodeKind, ctx: &LiveCtx<'_>) -> Option<String> {
            use editor::NodeKind;
            match kind {
                NodeKind::Time => Some(format!("{:.2}s", ctx.elapsed)),
                NodeKind::DeltaTime => Some(format!("{:.4}", if ctx.fps > 0.0 { 1.0 / ctx.fps } else { 0.016 })),
                NodeKind::Frame => Some(format!("{}", ctx.frame)),
                NodeKind::Resolution => Some("1920x1080".into()),
                NodeKind::Mouse => Some(ctx.cursor.to_string()),
                NodeKind::UV => Some("0..1".into()),
                NodeKind::CpuUsage => Some(format!("{:.0}%", ctx.cpu)),
                NodeKind::RamUsage => Some(format!("{:.0}%", ctx.ram)),
                NodeKind::Battery => Some(ctx.battery.map(|b| format!("{:.0}%", b)).unwrap_or_else(|| "N/A".into())),
                NodeKind::AudioLevel => Some(ctx.audio.to_string()),
                NodeKind::FloatConst => Some("0.0".into()),
                _ => None,
            }
        }

        let live_ctx = LiveCtx {
            elapsed,
            frame: frame_count,
            fps: self.fps,
            cpu: cpu_pct,
            ram: ram_pct,
            battery: battery_pct,
            cursor: &cursor_str,
            audio: &audio_str,
        };

        // Editor graph
        let vals: Vec<_> = self.shader_graph.nodes()
            .filter_map(|n| value_for(&n.kind, &live_ctx).map(|v| (n.id, v)))
            .collect();
        self.graph_canvas.live_values.clear();
        for (id, val) in vals {
            self.graph_canvas.live_values.insert(id, val);
        }

        // Shade graph
        let vals: Vec<_> = self.shade_graph.nodes()
            .filter_map(|n| value_for(&n.kind, &live_ctx).map(|v| (n.id, v)))
            .collect();
        self.shade_graph_canvas.live_values.clear();
        for (id, val) in vals {
            self.shade_graph_canvas.live_values.insert(id, val);
        }
    }

    /// Send GLSL source to the daemon via async IPC (non-blocking).
    pub(crate) fn send_to_daemon(&mut self, glsl_source: &str) {
        if let Some(ref handle) = self.ipc_handle {
            if handle.send(kroma_shared::ipc::DaemonCommand::LiveReload {
                glsl_source: glsl_source.to_string(),
            }) {
                self.log_msg("Shader sent to daemon".into());
            } else {
                self.compile_success = Some(false);
                self.log_msg("Failed to send shader — IPC channel closed".into());
            }
        } else {
            self.compile_success = Some(false);
            self.compile_errors = vec![kroma_shared::ipc::CompileError {
                message: "Not connected to daemon".into(),
                line: None,
                column: None,
            }];
            self.compile_warnings.clear();
            self.log_msg("Not connected to daemon".into());
        }
    }

    /// Process a `DaemonEvent` compile result and update error state.
    pub(crate) fn handle_compile_result(&mut self, event: kroma_shared::ipc::DaemonEvent) {
        use kroma_shared::ipc::DaemonEvent;
        match event {
            DaemonEvent::CompileResult { success, errors, warnings } => {
                self.compile_success = Some(success);

                if success {
                    self.compile_errors.clear();
                    self.compile_warnings = warnings.clone();
                    self.log_msg("Daemon: shader compiled successfully".into());
                    for w in &warnings {
                        self.log_msg(format!("Warning: {}", w));
                    }
                } else {
                    let msgs: Vec<String> = errors.iter().map(|err| {
                        let loc = match (err.line, err.column) {
                            (Some(l), Some(c)) => format!(" (line {}, col {})", l, c),
                            (Some(l), None) => format!(" (line {})", l),
                            _ => String::new(),
                        };
                        format!("Compile error{}: {}", loc, err.message)
                    }).collect();
                    self.compile_errors = errors;
                    self.compile_warnings = warnings;
                    for msg in msgs {
                        self.log_msg(msg);
                    }
                }
            }
            DaemonEvent::Error { message } => {
                self.compile_success = Some(false);
                self.compile_errors = vec![kroma_shared::ipc::CompileError {
                    message: message.clone(),
                    line: None,
                    column: None,
                }];
                self.compile_warnings.clear();
                self.log_msg(format!("Daemon error: {}", message));
            }
            other => {
                self.log_msg(format!("Unexpected daemon response: {:?}", other));
            }
        }
    }
}
