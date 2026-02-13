//! Dock layout rendering — binary split tree views (extracted from main.rs).

use crate::dock;
use crate::panels;
use crate::Message;

use iced::widget::{button, column, container, row, text, Space};
use iced::{
    Border, Element, Fill, Length, Padding, Theme,
};

use super::KromaApp;

impl KromaApp {
    // =======================================================================
    // DOCK LAYOUT — renders the binary split tree
    // =======================================================================

    pub(crate) fn view_docked(&self) -> Element<'_, Message> {
        let ctx = self.build_context();
        self.view_dock_node(&self.dock.root, ctx, &[])
    }

    /// Build the shared context that panels read from.
    fn build_context(&self) -> panels::AppContext<'_> {
        panels::AppContext {
            // Core
            daemon_connected: self.daemon_connected,
            loaded_shade: self.loaded_shade.as_deref(),
            fps: self.fps,
            paused: self.paused,
            status_text: &self.status_text,
            log_messages: &self.log_messages,
            compile_errors: &self.compile_errors,
            compile_warnings: &self.compile_warnings,
            compile_success: self.compile_success,
            // Import
            import_path: &self.import_path,
            import_name: &self.import_name,
            import_author: &self.import_author,
            import_result: &self.import_result,
            shadertoy_url: &self.shadertoy_url,
            download_result: &self.download_result,
            downloading: self.downloading,
            // Shade project
            shade_config: &self.shade_config,
            shade_package: self.shade_package.as_ref(),
            shade_selected_file: self.shade_selected_file.as_deref(),
            shade_config_toml: &self.shade_config_toml,
            shade_shader_content: &self.shade_shader_content,
            shade_loaded_path: self.shade_loaded_path.as_deref(),
            shade_new_uniform_name: &self.shade_new_uniform_name,
            shade_new_uniform_type: &self.shade_new_uniform_type,
            shade_new_texture_name: &self.shade_new_texture_name,
            shade_new_texture_type: &self.shade_new_texture_type,
            // Editor
            editor_glsl_content: &self.editor_glsl_content,
            editor_glsl_preview: &self.editor_glsl_preview,
            editor_text_mode: self.editor_text_mode,
            editor_live_preview: self.editor_live_preview,
            shader_graph: &self.shader_graph,
            graph_canvas: &self.graph_canvas,
            editor_palette_filter: &self.editor_palette_filter,
            editor_nav_path: &self.editor_nav_path,
            editor_subgraphs: &self.editor_subgraphs,
            // Live Preview
            preview_frame: self.preview_frame.as_ref(),
            preview_streaming: self.preview_streaming,
            // Settings
            api_key: &self.api_key,
        }
    }

    /// Recursively render a dock tree node.
    ///
    /// `path` tracks the route from the root to the current node, used for
    /// divider drag interaction.
    fn view_dock_node<'a>(
        &'a self,
        node: &'a dock::tree::DockNode,
        ctx: panels::AppContext<'a>,
        path: &[dock::PathDir],
    ) -> Element<'a, Message> {
        match node {
            dock::tree::DockNode::Split {
                axis,
                ratio,
                left,
                right,
            } => {
                let left_portion = (*ratio * 1000.0) as u16;
                let right_portion = 1000 - left_portion;

                let mut left_path = path.to_vec();
                left_path.push(dock::PathDir::Left);
                let mut right_path = path.to_vec();
                right_path.push(dock::PathDir::Right);

                let left_view = self.view_dock_node(left, ctx, &left_path);
                let right_view = self.view_dock_node(right, ctx, &right_path);

                // Interactive divider: wider hit area (6px), thin visual (2px)
                let divider_path = path.to_vec();
                let divider_axis = *axis;
                let divider_interaction = match axis {
                    dock::tree::SplitAxis::Horizontal => {
                        iced::mouse::Interaction::ResizingHorizontally
                    }
                    dock::tree::SplitAxis::Vertical => {
                        iced::mouse::Interaction::ResizingVertically
                    }
                };

                let is_active = self
                    .dragging_divider
                    .as_ref()
                    .is_some_and(|d| d.path == path);

                let divider_visual: Element<'a, Message> =
                    container(Space::new(Fill, Fill))
                        .width(Fill)
                        .height(Fill)
                        .style(move |theme: &Theme| {
                            let p = theme.extended_palette();
                            let alpha = if is_active { 0.6 } else { 0.3 };
                            container::Style {
                                background: Some(
                                    iced::Color {
                                        a: alpha,
                                        ..p.background.base.text
                                    }
                                    .into(),
                                ),
                                ..Default::default()
                            }
                        })
                        .into();

                let divider: Element<'a, Message> =
                    iced::widget::mouse_area(divider_visual)
                        .on_press(Message::DividerDragStart(divider_path, divider_axis))
                        .interaction(divider_interaction)
                        .into();

                match axis {
                    dock::tree::SplitAxis::Horizontal => row![
                        container(left_view)
                            .width(Length::FillPortion(left_portion))
                            .height(Fill),
                        container(divider)
                            .width(Length::Fixed(6.0))
                            .height(Fill)
                            .center_x(Fill),
                        container(right_view)
                            .width(Length::FillPortion(right_portion))
                            .height(Fill),
                    ]
                    .width(Fill)
                    .height(Fill)
                    .into(),
                    dock::tree::SplitAxis::Vertical => column![
                        container(left_view)
                            .height(Length::FillPortion(left_portion))
                            .width(Fill),
                        container(divider)
                            .height(Length::Fixed(6.0))
                            .width(Fill)
                            .center_y(Fill),
                        container(right_view)
                            .height(Length::FillPortion(right_portion))
                            .width(Fill),
                    ]
                    .width(Fill)
                    .height(Fill)
                    .into(),
                }
            }
            dock::tree::DockNode::Leaf { tabs, active } => {
                self.view_dock_leaf(tabs, *active, ctx, path)
            }
            dock::tree::DockNode::Empty => Space::new(Fill, Fill).into(),
        }
    }

    /// Render a leaf node: tab bar at top + active panel content below.
    fn view_dock_leaf<'a>(
        &'a self,
        tabs: &'a [panels::PanelId],
        active: usize,
        ctx: panels::AppContext<'a>,
        leaf_path: &[dock::PathDir],
    ) -> Element<'a, Message> {
        let is_dragging = self.dragging_tab.is_some();
        let dragging_panel = self.dragging_tab;

        // Tab bar — press-and-hold any tab to start dragging it.
        // Uses container (not button) so the parent mouse_area receives
        // the press event.  Buttons capture press events in iced, which
        // would prevent mouse_area.on_press from firing.
        let leaf_path_owned = leaf_path.to_vec();
        let tab_buttons: Vec<Element<'_, Message>> = tabs
            .iter()
            .enumerate()
            .map(|(i, &panel_id)| {
                let is_active = i == active;
                let label_text = panel_id.title();

                let tab_label: Element<'_, Message> =
                    text(label_text).size(11).into();

                // Style the container based on state
                let styled_tab: Element<'_, Message> = if dragging_panel == Some(panel_id) {
                    // Highlight the tab being dragged
                    container(tab_label)
                        .padding(Padding::from([4, 10]))
                        .style(|theme: &Theme| {
                            let p = theme.extended_palette();
                            container::Style {
                                background: Some(p.primary.weak.color.into()),
                                text_color: Some(p.primary.base.text),
                                border: Border::default().rounded(4).width(2).color(p.primary.base.color),
                                ..Default::default()
                            }
                        })
                        .into()
                } else if is_active {
                    container(tab_label)
                        .padding(Padding::from([4, 10]))
                        .style(|theme: &Theme| {
                            let p = theme.extended_palette();
                            container::Style {
                                background: Some(p.background.weak.color.into()),
                                text_color: Some(p.primary.base.color),
                                border: Border::default().rounded(4),
                                ..Default::default()
                            }
                        })
                        .into()
                } else {
                    container(tab_label)
                        .padding(Padding::from([4, 10]))
                        .style(|theme: &Theme| {
                            let p = theme.extended_palette();
                            container::Style {
                                background: Some(p.background.strong.color.into()),
                                text_color: Some(p.background.base.text),
                                border: Border::default().rounded(4),
                                ..Default::default()
                            }
                        })
                        .into()
                };

                // Wrap in mouse_area.  When a drag is active, releasing over
                // a tab reorders into that position (TabDropOnTab).
                // Otherwise, pressing starts a potential drag (TabMouseDown).
                let mut area = iced::widget::mouse_area(styled_tab)
                    .on_press(Message::TabMouseDown(panel_id));

                if is_dragging {
                    area = area
                        .on_release(Message::TabDropOnTab(leaf_path_owned.clone(), i))
                        .interaction(iced::mouse::Interaction::Crosshair);
                } else {
                    area = area
                        .interaction(iced::mouse::Interaction::Grab);
                }

                let el: Element<'_, Message> = area.into();
                el
            })
            .collect();

        let mut tab_row = row(tab_buttons).spacing(1);
        if let Some(dragged) = dragging_panel {
            tab_row = tab_row
                .push(iced::widget::horizontal_space())
                .push(
                    text(format!("Moving: {}", dragged.title()))
                        .size(10)
                        .color(iced::Color::from_rgb(0.4, 0.7, 1.0)),
                )
                .push(
                    button(text("\u{2716}").size(9))
                        .padding(Padding::from([2, 6]))
                        .on_press(Message::TabDragCancel)
                        .style(|_theme: &Theme, _status| button::Style {
                            background: None,
                            text_color: iced::Color::from_rgb(1.0, 0.4, 0.4),
                            border: Border::default(),
                            ..Default::default()
                        }),
                );
        }

        let tab_bar = container(tab_row)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                ..Default::default()
            }
        })
        .width(Fill);

        // Active panel content
        let active_panel_id = tabs.get(active).copied();
        let content = if let Some(panel_id) = active_panel_id {
            self.view_panel(panel_id, ctx)
        } else {
            text("Empty panel").size(12).into()
        };

        let panel_content: Element<'a, Message> = if is_dragging {
            // Show drop zone overlay when a tab is being dragged.
            // Each zone uses mouse_area so releasing the mouse button over a
            // zone completes the drop (press-hold-move-release gesture).
            let path_owned = leaf_path.to_vec();
            let drop_zone = |label: &'a str, zone: dock::tree::DropZone, p: Vec<dock::PathDir>| -> Element<'a, Message> {
                let visual: Element<'a, Message> = container(
                    text(label)
                        .size(11)
                        .center()
                        .width(Fill)
                        .height(Fill),
                )
                .width(Fill)
                .height(Fill)
                .padding(0)
                .style(|theme: &Theme| {
                    let pal = theme.extended_palette();
                    container::Style {
                        background: Some(iced::Color { a: 0.08, ..pal.primary.base.color }.into()),
                        border: Border::default().rounded(2).width(1).color(
                            iced::Color { a: 0.2, ..pal.primary.base.color },
                        ),
                        ..Default::default()
                    }
                })
                .into();

                // mouse_area: release over this zone completes the drop.
                // We also keep on_press as a fallback for the click-to-drop
                // workflow (in case user prefers clicking after entering drag
                // mode via keyboard or other means).
                iced::widget::mouse_area(visual)
                    .on_release(Message::TabDrop(p.clone(), zone))
                    .on_press(Message::TabDrop(p, zone))
                    .interaction(iced::mouse::Interaction::Crosshair)
                    .into()
            };

            let center_zone = drop_zone("Drop Here", dock::tree::DropZone::Center, path_owned.clone());
            let left_zone = drop_zone("\u{25C0}", dock::tree::DropZone::Left, path_owned.clone());
            let right_zone = drop_zone("\u{25B6}", dock::tree::DropZone::Right, path_owned.clone());
            let top_zone = drop_zone("\u{25B2}", dock::tree::DropZone::Top, path_owned.clone());
            let bottom_zone = drop_zone("\u{25BC}", dock::tree::DropZone::Bottom, path_owned);

            let overlay = container(
                column![
                    container(top_zone).height(Length::FillPortion(1)).width(Fill),
                    row![
                        container(left_zone).width(Length::FillPortion(1)).height(Fill),
                        container(center_zone).width(Length::FillPortion(2)).height(Fill),
                        container(right_zone).width(Length::FillPortion(1)).height(Fill),
                    ]
                    .height(Length::FillPortion(2))
                    .width(Fill),
                    container(bottom_zone).height(Length::FillPortion(1)).width(Fill),
                ]
                .width(Fill)
                .height(Fill),
            )
            .width(Fill)
            .height(Fill)
            .padding(4)
            .into();

            overlay
        } else {
            container(content)
                .width(Fill)
                .height(Fill)
                .style(|theme: &Theme| {
                    let p = theme.extended_palette();
                    container::Style {
                        background: Some(p.background.weak.color.into()),
                        border: Border::default()
                            .width(1)
                            .color(iced::Color { a: 0.1, ..p.background.base.text }),
                        ..Default::default()
                    }
                })
                .into()
        };

        column![tab_bar, panel_content]
            .width(Fill)
            .height(Fill)
            .into()
    }

    /// Dispatch view rendering to the correct panel.
    fn view_panel<'a>(
        &'a self,
        panel_id: panels::PanelId,
        ctx: panels::AppContext<'a>,
    ) -> Element<'a, Message> {
        use panels::Panel;
        use panels::PanelId::*;
        match panel_id {
            Dashboard => self.panel_dashboard.view(ctx),
            NodeEditor => self.panel_node_editor.view(ctx),
            CodeEditor => self.panel_code_editor.view(ctx),
            AssetBrowser => self.panel_asset_browser.view(ctx),
            AssetPreview => self.panel_asset_preview.view(ctx),
            Properties => self.panel_properties.view(ctx),
            Library => self.panel_library.view(ctx),
            LivePreview => self.panel_live_preview.view(ctx),
            Import => self.panel_import.view(ctx),
            ErrorLog => self.panel_error_log.view(ctx),
            Settings => self.panel_settings.view(ctx),
        }
    }
}
