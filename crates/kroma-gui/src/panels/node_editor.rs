//! Node Editor panel — graph canvas with node palette.
//!
//! This panel wraps `editor::canvas::graph_canvas()` and node palette
//! into the Panel trait. The shader graph state lives in KromaApp and
//! is accessed through AppContext references.

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Border, Element, Fill, Length, Padding, Theme};
use crate::Message;
use crate::editor;
use crate::panels::{AppContext, Panel};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct NodeEditorPanel;

impl NodeEditorPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NodeEditorPanel {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Panel implementation
// ---------------------------------------------------------------------------

impl Panel for NodeEditorPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let palette = build_node_palette(
            ctx.editor_palette_filter,
            Message::EditorAddNode,
        );

        // Determine which graph + canvas to show based on nav path
        let (graph, canvas) = if let Some(&last_id) = ctx.editor_nav_path.last() {
            if let Some((sg, sc)) = ctx.editor_subgraphs.get(&last_id) {
                (sg, sc)
            } else {
                (ctx.shader_graph, ctx.graph_canvas)
            }
        } else {
            (ctx.shader_graph, ctx.graph_canvas)
        };

        let graph_canvas = editor::canvas::graph_canvas(graph, canvas)
            .map(Message::EditorGraph);

        // Build sub-graph tree sidebar if there are sub-graph nodes
        let subgraph_tree = build_subgraph_tree(ctx.shader_graph, ctx.editor_nav_path);

        // Build breadcrumb bar if navigated into sub-graphs
        let mut main_row = row![].width(Fill).height(Fill);

        // Sub-graph tree (left sidebar, only when sub-graph nodes exist)
        if let Some(tree) = subgraph_tree {
            main_row = main_row.push(tree);
        }

        // Palette + canvas
        main_row = main_row.push(palette);
        main_row = main_row.push(graph_canvas);

        if ctx.editor_nav_path.is_empty() {
            main_row.into()
        } else {
            let mut crumbs: Vec<Element<'a, Message>> = Vec::new();
            // Root breadcrumb
            crumbs.push(
                button(text("Root").size(11))
                    .padding(iced::Padding::from([2, 6]))
                    .on_press(Message::EditorGraph(editor::canvas::GraphMessage::ExitSubGraph))
                    .style(|theme: &Theme, _| {
                        let p = theme.extended_palette();
                        button::Style {
                            background: Some(p.background.weak.color.into()),
                            text_color: p.primary.base.color,
                            border: iced::Border::default().rounded(3),
                            ..Default::default()
                        }
                    })
                    .into(),
            );
            for &node_id in ctx.editor_nav_path {
                crumbs.push(text(" \u{25B8} ").size(11).into());
                let label = ctx.shader_graph
                    .node(node_id)
                    .map(|n| n.kind.label())
                    .unwrap_or("SubGraph");
                crumbs.push(
                    text(format!("{} ({})", label, node_id.0)).size(11).into(),
                );
            }

            let breadcrumb_bar = container(row(crumbs).spacing(2).align_y(iced::Alignment::Center))
                .padding(iced::Padding::from([4, 8]))
                .width(Fill)
                .style(|theme: &Theme| {
                    let p = theme.extended_palette();
                    container::Style {
                        background: Some(p.background.strong.color.into()),
                        ..Default::default()
                    }
                });

            column![breadcrumb_bar, main_row]
                .width(Fill)
                .height(Fill)
                .into()
        }
    }
}

// ---------------------------------------------------------------------------
// Sub-graph tree sidebar
// ---------------------------------------------------------------------------

/// Build a tree sidebar showing all sub-graph-capable nodes.
/// Returns None if the graph has no ForLoop/Conditional/CustomFunc nodes.
fn build_subgraph_tree<'a>(
    graph: &'a editor::ShaderGraph,
    nav_path: &[kroma_graph::types::NodeId],
) -> Option<Element<'a, Message>> {
    use editor::NodeKind;
    let subgraph_nodes: Vec<_> = graph.nodes()
        .filter(|n| matches!(n.kind, NodeKind::ForLoop | NodeKind::Conditional | NodeKind::CustomFunc))
        .collect();

    if subgraph_nodes.is_empty() {
        return None;
    }

    let is_at_root = nav_path.is_empty();
    let mut items: Vec<Element<'a, Message>> = Vec::new();

    items.push(text("Graphs").size(12).into());

    // Root entry
    let root_style = if is_at_root { "strong" } else { "normal" };
    items.push(
        button(text(if is_at_root { "\u{25BC} Root" } else { "\u{25B6} Root" }).size(11))
            .width(Fill)
            .padding(iced::Padding::from([3, 6]))
            .on_press(Message::EditorGraph(editor::canvas::GraphMessage::ExitSubGraph))
            .style(move |theme: &Theme, _| {
                let p = theme.extended_palette();
                if root_style == "strong" {
                    button::Style {
                        background: Some(p.primary.weak.color.into()),
                        text_color: p.primary.weak.text,
                        border: Border::default().rounded(3),
                        ..Default::default()
                    }
                } else {
                    button::Style {
                        background: None,
                        text_color: p.background.base.text,
                        border: Border::default().rounded(3),
                        ..Default::default()
                    }
                }
            })
            .into(),
    );

    // Sub-graph node entries
    for node in subgraph_nodes {
        let is_active = nav_path.last() == Some(&node.id);
        let icon = match node.kind {
            NodeKind::ForLoop => "\u{21BB}", // ↻
            NodeKind::Conditional => "\u{2753}", // ❓
            NodeKind::CustomFunc => "\u{0192}", // ƒ
            _ => "\u{25C6}",
        };
        let label_str = format!("  {} {} ({})", icon, node.kind.label(), node.id.0);
        let active_flag = if is_active { "active" } else { "normal" };
        items.push(
            button(text(label_str).size(10))
                .width(Fill)
                .padding(iced::Padding::from([2, 6]))
                .on_press(Message::EditorGraph(
                    editor::canvas::GraphMessage::EnterSubGraph(node.id),
                ))
                .style(move |theme: &Theme, _| {
                    let p = theme.extended_palette();
                    if active_flag == "active" {
                        button::Style {
                            background: Some(p.primary.weak.color.into()),
                            text_color: p.primary.weak.text,
                            border: Border::default().rounded(3),
                            ..Default::default()
                        }
                    } else {
                        button::Style {
                            background: None,
                            text_color: p.background.base.text,
                            border: Border::default().rounded(3),
                            ..Default::default()
                        }
                    }
                })
                .into(),
        );
    }

    Some(
        container(scrollable(column(items).spacing(2)).height(Fill))
            .width(Length::Fixed(130.0))
            .height(Fill)
            .padding(4)
            .style(|theme: &Theme| {
                let p = theme.extended_palette();
                container::Style {
                    background: Some(p.background.strong.color.into()),
                    border: Border::default()
                        .width(1)
                        .color(iced::Color { a: 0.1, ..p.background.base.text }),
                    ..Default::default()
                }
            })
            .into(),
    )
}

// ---------------------------------------------------------------------------
// Shared palette builder
// ---------------------------------------------------------------------------

/// Build the node palette sidebar with searchable filtering.
/// `filter` is the current search text. `on_add` maps a `NodeKind` to a `Message`.
pub fn build_node_palette<'a, F>(filter: &'a str, on_add: F) -> Element<'a, Message>
where
    F: Fn(editor::NodeKind) -> Message + 'a,
{
    let filter_input: Element<'a, Message> = text_input("Search nodes...", filter)
        .size(11)
        .padding(Padding::from([4, 8]))
        .on_input(Message::EditorPaletteFilter)
        .into();

    let filter_lower = filter.to_lowercase();

    let mut palette_col = column![filter_input].spacing(4);

    for (category, kinds) in editor::palette() {
        let filtered: Vec<_> = kinds
            .into_iter()
            .filter(|k| {
                if filter_lower.is_empty() {
                    true
                } else {
                    k.label().to_lowercase().contains(&filter_lower)
                        || category.to_lowercase().contains(&filter_lower)
                }
            })
            .collect();

        if filtered.is_empty() {
            continue;
        }

        let mut cat_col = column![text(category).size(11)].spacing(2);

        for kind in filtered {
            let label = kind.label();
            let msg = on_add(kind);
            let btn = button(text(label).size(10))
                .width(Fill)
                .padding(Padding::from([3, 6]))
                .on_press(msg)
                .style(|theme: &Theme, status| {
                    let p = theme.extended_palette();
                    match status {
                        button::Status::Hovered => button::Style {
                            background: Some(p.primary.weak.color.into()),
                            text_color: p.primary.weak.text,
                            border: Border::default().rounded(4),
                            ..Default::default()
                        },
                        _ => button::Style {
                            background: None,
                            text_color: p.background.base.text,
                            border: Border::default().rounded(4),
                            ..Default::default()
                        },
                    }
                });
            cat_col = cat_col.push(btn);
        }
        palette_col = palette_col.push(cat_col);
        palette_col = palette_col.push(Space::with_height(2));
    }

    container(scrollable(palette_col).height(Fill))
        .width(Length::FillPortion(1))
        .height(Fill)
        .padding(4)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                ..Default::default()
            }
        })
        .into()
}