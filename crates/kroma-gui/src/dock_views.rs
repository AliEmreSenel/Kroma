//! Dock layout rendering — binary split tree views (extracted from main.rs).

use crate::Message;
use crate::dock;
use crate::icons;
use crate::panels;
use crate::panels::Panel;

use iced::widget::rule;
use iced::widget::space::horizontal;
use iced::widget::{Space, button, column, container, row, text};
use iced::{Border, Element, Fill, Length, Padding, Theme};

use super::KromaApp;

impl KromaApp {
    // =======================================================================
    // DOCK LAYOUT — renders the binary split tree
    // =======================================================================

    pub(crate) fn view_docked(&self) -> Element<'_, Message> {
        let ctx = self.build_context();
        let bg_primary = self.theme_tokens.bg_primary;
        let dock_content = self.view_dock_node(&self.dock.root, ctx, &[]);
        let menu_bar = self.view_menu_bar();

        let base: Element<'_, Message> =
            container(column![menu_bar, dock_content].width(Fill).height(Fill))
                .width(Fill)
                .height(Fill)
                .style(move |_theme: &Theme| container::Style {
                    background: Some(bg_primary.into()),
                    ..Default::default()
                })
                .into();

        let toasts = self.view_toasts();

        if self.show_settings {
            let overlay = self.view_settings_overlay();
            iced::widget::stack![base, overlay, toasts]
                .width(Fill)
                .height(Fill)
                .into()
        } else if self.show_import {
            let overlay = self.view_import_overlay();
            iced::widget::stack![base, overlay, toasts]
                .width(Fill)
                .height(Fill)
                .into()
        } else {
            iced::widget::stack![base, toasts]
                .width(Fill)
                .height(Fill)
                .into()
        }
    }

    /// Top menu bar (File, Edit, View, Help).
    fn view_menu_bar(&self) -> Element<'_, Message> {
        let tokens = &self.theme_tokens;
        let bg = tokens.bg_secondary;
        let txt = tokens.text_primary;
        let txt_sec = tokens.text_secondary;
        let hover = tokens.button_hover;
        let radius = tokens.border_radius;

        let menu_btn_style = move |_theme: &Theme, status: button::Status| -> button::Style {
            let bg_color = match status {
                button::Status::Hovered => hover,
                button::Status::Pressed => hover,
                _ => iced::Color::TRANSPARENT,
            };
            button::Style {
                background: Some(iced::Background::Color(bg_color)),
                text_color: txt,
                border: Border {
                    radius: (radius * 0.5).into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        };

        let file_menu = row![
            button(text("New").size(12).color(txt))
                .on_press(Message::ShadeNew)
                .style(menu_btn_style)
                .padding(Padding::from([4, 8])),
            button(text("Open").size(12).color(txt))
                .on_press(Message::ShadeOpen)
                .style(menu_btn_style)
                .padding(Padding::from([4, 8])),
            button(text("Save").size(12).color(txt))
                .on_press(Message::ShadeSave)
                .style(menu_btn_style)
                .padding(Padding::from([4, 8])),
            button(text("Import").size(12).color(txt))
                .on_press(Message::ToggleImportPopup)
                .style(menu_btn_style)
                .padding(Padding::from([4, 8])),
        ]
        .spacing(2);

        let edit_menu = row![
            button(text("Undo").size(12).color(txt))
                .on_press(Message::Undo)
                .style(menu_btn_style)
                .padding(Padding::from([4, 8])),
            button(text("Redo").size(12).color(txt))
                .on_press(Message::Redo)
                .style(menu_btn_style)
                .padding(Padding::from([4, 8])),
        ]
        .spacing(2);

        let view_menu = row![
            button(text("Settings").size(12).color(txt))
                .on_press(Message::ToggleSettings)
                .style(menu_btn_style)
                .padding(Padding::from([4, 8])),
        ]
        .spacing(2);

        let separator = text("|").size(12).color(txt_sec);
        let separator2 = text("|").size(12).color(txt_sec);

        let brand = text("KROMA").size(13).color(tokens.text_accent);

        // Daemon status in menu bar
        let status_color = if self.daemon_connected {
            tokens.success
        } else {
            tokens.error
        };
        let status_label = if self.daemon_connected {
            "Connected"
        } else {
            "Offline"
        };
        let daemon_status = row![
            container(Space::new().width(6).height(6)).style(move |_: &Theme| container::Style {
                background: Some(iced::Background::Color(status_color)),
                border: Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
            text(status_label).size(11).color(txt_sec),
        ]
        .spacing(4)
        .align_y(iced::alignment::Vertical::Center);

        let left_items = row![
            brand,
            Space::new().width(12).height(0),
            file_menu,
            Space::new().width(4).height(0),
            separator,
            Space::new().width(4).height(0),
            edit_menu,
            Space::new().width(4).height(0),
            separator2,
            Space::new().width(4).height(0),
            view_menu,
        ]
        .align_y(iced::alignment::Vertical::Center);

        // Screen tabs (Dashboard / Editor) on the right
        let is_editor = self.screen == crate::AppScreen::Editor;
        let is_dashboard = self.screen == crate::AppScreen::Dashboard;
        let is_designer = self.screen == crate::AppScreen::Designer;
        let tab_active_bg = tokens.tab_active;
        let tab_inactive_bg = tokens.tab_inactive;

        let editor_tab = button(text("Editor").size(12).color(txt))
            .on_press(Message::SwitchToEditor)
            .padding(Padding::from([4, 12]))
            .style(move |_: &Theme, _| {
                let bg_c = if is_editor {
                    tab_active_bg
                } else {
                    tab_inactive_bg
                };
                button::Style {
                    background: Some(iced::Background::Color(bg_c)),
                    text_color: txt,
                    border: Border {
                        radius: (radius * 0.5).into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });

        let dashboard_tab = button(text("Dashboard").size(12).color(txt))
            .on_press(Message::SwitchToDashboard)
            .padding(Padding::from([4, 12]))
            .style(move |_: &Theme, _| {
                let bg_c = if is_dashboard {
                    tab_active_bg
                } else {
                    tab_inactive_bg
                };
                button::Style {
                    background: Some(iced::Background::Color(bg_c)),
                    text_color: txt,
                    border: Border {
                        radius: (radius * 0.5).into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });

        let designer_tab = button(text("Designer").size(12).color(txt))
            .on_press(Message::SwitchToDesigner)
            .padding(Padding::from([4, 12]))
            .style(move |_: &Theme, _| {
                let bg_c = if is_designer {
                    tab_active_bg
                } else {
                    tab_inactive_bg
                };
                button::Style {
                    background: Some(iced::Background::Color(bg_c)),
                    text_color: txt,
                    border: Border {
                        radius: (radius * 0.5).into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });

        let screen_tabs = row![editor_tab, dashboard_tab, designer_tab].spacing(2);

        let bar = row![
            left_items,
            horizontal(),
            screen_tabs,
            Space::new().width(12).height(0),
            daemon_status,
        ]
        .spacing(8)
        .align_y(iced::alignment::Vertical::Center)
        .padding(Padding::from([4, 12]));

        container(bar)
            .width(Fill)
            .style(move |_: &Theme| container::Style {
                background: Some(iced::Background::Color(bg)),
                border: Border {
                    width: 0.0,
                    color: iced::Color::TRANSPARENT,
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    /// Render the toast notification overlay (bottom-right corner).
    fn view_toasts(&self) -> Element<'_, Message> {
        use iced::widget::{Space, column, container, row, text};
        use iced::{Border, Color, Fill, Length, Padding, alignment};

        if self.toasts.is_empty() {
            return Space::new().width(0).height(0).into();
        }

        let tokens = &self.theme_tokens;

        let toast_items: Vec<Element<'_, Message>> = self
            .toasts
            .iter()
            .map(|t| {
                let (icon_char, accent_color) = match t.level {
                    crate::ToastLevel::Info => (crate::icons::INFO, tokens.text_accent),
                    crate::ToastLevel::Success => (crate::icons::CHECK_CIRCLE, tokens.success),
                    crate::ToastLevel::Warning => (crate::icons::WARNING, tokens.warning),
                    crate::ToastLevel::Error => (crate::icons::ERROR, tokens.error),
                };

                let bg = tokens.bg_tertiary;
                let txt = tokens.text_primary;
                let radius = tokens.border_radius;
                let toast_id = t.id;

                let icon_text = text(icon_char)
                    .size(16)
                    .font(crate::icons::ICON_FONT)
                    .color(accent_color);

                let msg_text = text(&t.message).size(12).color(txt);

                let dismiss = button(
                    text(crate::icons::CLOSE)
                        .size(14)
                        .font(crate::icons::ICON_FONT)
                        .color(tokens.text_secondary),
                )
                .on_press(Message::DismissToast(toast_id))
                .padding(Padding::from([2, 4]))
                .style(move |_: &iced::Theme, _| button::Style {
                    background: Some(iced::Background::Color(Color::TRANSPARENT)),
                    ..Default::default()
                });

                let border_color = accent_color;

                container(
                    row![icon_text, msg_text, horizontal(), dismiss]
                        .spacing(8)
                        .align_y(alignment::Vertical::Center)
                        .width(320),
                )
                .padding(Padding::from([8, 12]))
                .style(move |_: &iced::Theme| container::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: Border {
                        radius: radius.into(),
                        width: 1.0,
                        color: border_color,
                    },
                    ..Default::default()
                })
                .into()
            })
            .collect();

        let toast_col = column(toast_items).spacing(4).width(Length::Shrink);

        container(toast_col)
            .width(Fill)
            .height(Fill)
            .align_x(alignment::Horizontal::Right)
            .align_y(alignment::Vertical::Bottom)
            .padding(Padding::from([12, 16]))
            .into()
    }

    /// Full dashboard screen (menu bar + dashboard panel content).
    pub(crate) fn view_dashboard_screen(&self) -> Element<'_, Message> {
        let ctx = self.build_context();
        let bg_primary = self.theme_tokens.bg_primary;
        let menu_bar = self.view_menu_bar();
        let dashboard_content = self.panel_dashboard.view(ctx);

        let base: Element<'_, Message> = container(
            column![menu_bar, dashboard_content]
                .width(Fill)
                .height(Fill),
        )
        .width(Fill)
        .height(Fill)
        .style(move |_theme: &Theme| container::Style {
            background: Some(bg_primary.into()),
            ..Default::default()
        })
        .into();

        let toasts = self.view_toasts();
        iced::widget::stack![base, toasts]
            .width(Fill)
            .height(Fill)
            .into()
    }

    /// Designer screen — simple no-code wallpaper designer.
    pub(crate) fn view_designer_screen(&self) -> Element<'_, Message> {
        let ctx = self.build_context();
        let bg_primary = self.theme_tokens.bg_primary;
        let menu_bar = self.view_menu_bar();
        let designer_content = self.panel_designer.view(ctx);

        let base: Element<'_, Message> =
            container(column![menu_bar, designer_content].width(Fill).height(Fill))
                .width(Fill)
                .height(Fill)
                .style(move |_theme: &Theme| container::Style {
                    background: Some(bg_primary.into()),
                    ..Default::default()
                })
                .into();

        let toasts = self.view_toasts();
        iced::widget::stack![base, toasts]
            .width(Fill)
            .height(Fill)
            .into()
    }

    /// Settings overlay — semi-transparent backdrop with centered settings panel.
    fn view_settings_overlay(&self) -> Element<'_, Message> {
        let ctx = self.build_context();
        let tokens = &self.theme_tokens;
        let bg_overlay = iced::Color::from_rgba(0.0, 0.0, 0.0, 0.5);
        let bg_panel = tokens.bg_secondary;
        let radius = tokens.border_radius;
        let border_col = tokens.border_default;

        // Render the settings panel content
        let settings_content = self.panel_settings.view(ctx);

        // Close button
        let close_btn = button(text("Close").size(13).color(tokens.text_primary))
            .on_press(Message::ToggleSettings)
            .padding(Padding::from([6, 16]))
            .style(move |_: &Theme, status| {
                let bg = match status {
                    button::Status::Hovered => tokens.button_hover,
                    _ => tokens.bg_tertiary,
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    text_color: tokens.text_primary,
                    border: Border::default().rounded(radius),
                    ..Default::default()
                }
            });

        let panel = container(
            column![
                row![
                    text("Settings").size(18).color(tokens.text_primary),
                    horizontal(),
                    close_btn,
                ]
                .align_y(iced::alignment::Vertical::Center)
                .padding(Padding::from([12, 16])),
                rule::horizontal(1),
                settings_content,
            ]
            .width(500)
            .height(Length::FillPortion(3)),
        )
        .style(move |_: &Theme| container::Style {
            background: Some(iced::Background::Color(bg_panel)),
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: border_col,
            },
            ..Default::default()
        });

        // Backdrop — clicking it also closes settings
        let backdrop = button(container(panel).center(Fill).width(Fill).height(Fill))
            .on_press(Message::ToggleSettings)
            .width(Fill)
            .height(Fill)
            .style(move |_: &Theme, _status| button::Style {
                background: Some(iced::Background::Color(bg_overlay)),
                ..Default::default()
            });

        backdrop.into()
    }

    /// Import overlay — semi-transparent backdrop with centered import panel.
    fn view_import_overlay(&self) -> Element<'_, Message> {
        let ctx = self.build_context();
        let tokens = &self.theme_tokens;
        let bg_overlay = iced::Color::from_rgba(0.0, 0.0, 0.0, 0.5);
        let bg_panel = tokens.bg_secondary;
        let radius = tokens.border_radius;
        let border_col = tokens.border_default;

        // Render the import panel content
        let import_content = self.panel_import.view(ctx);

        // Close button
        let close_btn = button(text("Close").size(13).color(tokens.text_primary))
            .on_press(Message::ToggleImportPopup)
            .padding(Padding::from([6, 16]))
            .style(move |_: &Theme, status| {
                let bg = match status {
                    button::Status::Hovered => tokens.button_hover,
                    _ => tokens.bg_tertiary,
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    text_color: tokens.text_primary,
                    border: Border::default().rounded(radius),
                    ..Default::default()
                }
            });

        let panel = container(
            column![
                row![
                    text("Import").size(18).color(tokens.text_primary),
                    horizontal(),
                    close_btn,
                ]
                .align_y(iced::alignment::Vertical::Center)
                .padding(Padding::from([12, 16])),
                rule::horizontal(1),
                import_content,
            ]
            .width(500)
            .height(Length::FillPortion(3)),
        )
        .style(move |_: &Theme| container::Style {
            background: Some(iced::Background::Color(bg_panel)),
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: border_col,
            },
            ..Default::default()
        });

        // Backdrop — clicking it also closes the import popup
        let backdrop = button(container(panel).center(Fill).width(Fill).height(Fill))
            .on_press(Message::ToggleImportPopup)
            .width(Fill)
            .height(Fill)
            .style(move |_: &Theme, _status| button::Style {
                background: Some(iced::Background::Color(bg_overlay)),
                ..Default::default()
            });

        backdrop.into()
    }

    /// Build the shared context that panels read from.
    fn build_context(&self) -> panels::AppContext<'_> {
        panels::AppContext {
            // Theme tokens
            tokens: &self.theme_tokens,
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
            shade_edit_mode: &self.shade_edit_mode,
            shade_graph: &self.shade_graph,
            shade_graph_canvas: &self.shade_graph_canvas,
            shade_subgraphs: &self.shade_subgraphs,
            shade_nav_path: &self.shade_nav_path,
            shade_nav_labels: self
                .shade_nav_path
                .iter()
                .map(|id| {
                    self.shade_graph
                        .node(*id)
                        .map(|n| n.kind.label().to_string())
                        .unwrap_or_else(|| format!("Node {:?}", id))
                })
                .collect(),
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
            drop_hover_active: self.drop_hover_active,
            available_audio_sources: &self.available_audio_sources,
            video_player: self.video_player.as_ref(),
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

                let left_view = self.view_dock_node(left, ctx.clone(), &left_path);
                let right_view = self.view_dock_node(right, ctx, &right_path);

                // Interactive divider: wider hit area (6px), thin visual (2px)
                let divider_path = path.to_vec();
                let divider_axis = *axis;
                let divider_interaction = match axis {
                    dock::tree::SplitAxis::Horizontal => {
                        iced::mouse::Interaction::ResizingHorizontally
                    }
                    dock::tree::SplitAxis::Vertical => iced::mouse::Interaction::ResizingVertically,
                };

                let is_active = self
                    .dragging_divider
                    .as_ref()
                    .is_some_and(|d| d.path == path);

                let divider_color = if is_active {
                    self.theme_tokens.border_focused
                } else {
                    self.theme_tokens.border_default
                };

                let divider_visual: Element<'a, Message> =
                    container(Space::new().width(Fill).height(Fill))
                        .width(Fill)
                        .height(Fill)
                        .style(move |_theme: &Theme| container::Style {
                            background: Some(divider_color.into()),
                            ..Default::default()
                        })
                        .into();

                let divider: Element<'a, Message> = iced::widget::mouse_area(divider_visual)
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
            dock::tree::DockNode::Empty => Space::new().width(Fill).height(Fill).into(),
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

                let tab_label: Element<'_, Message> = text(label_text).size(11).into();

                // Style the container based on state
                let styled_tab: Element<'_, Message> = if dragging_panel == Some(panel_id) {
                    // Highlight the tab being dragged
                    let drag_bg = self.theme_tokens.tab_active;
                    let drag_text = self.theme_tokens.text_primary;
                    let drag_border = self.theme_tokens.border_focused;
                    container(tab_label)
                        .padding(Padding::from([4, 10]))
                        .style(move |_theme: &Theme| container::Style {
                            background: Some(drag_bg.into()),
                            text_color: Some(drag_text),
                            border: Border::default().rounded(4).width(2).color(drag_border),
                            ..Default::default()
                        })
                        .into()
                } else if is_active {
                    let active_bg = self.theme_tokens.tab_active;
                    let active_text = self.theme_tokens.text_accent;
                    container(tab_label)
                        .padding(Padding::from([4, 10]))
                        .style(move |_theme: &Theme| container::Style {
                            background: Some(active_bg.into()),
                            text_color: Some(active_text),
                            border: Border::default().rounded(4),
                            ..Default::default()
                        })
                        .into()
                } else {
                    let inactive_bg = self.theme_tokens.tab_inactive;
                    let inactive_text = self.theme_tokens.text_secondary;
                    container(tab_label)
                        .padding(Padding::from([4, 10]))
                        .style(move |_theme: &Theme| container::Style {
                            background: Some(inactive_bg.into()),
                            text_color: Some(inactive_text),
                            border: Border::default().rounded(4),
                            ..Default::default()
                        })
                        .into()
                };

                // Wrap in mouse_area.  When a drag is active, releasing over
                // a tab reorders into that position (TabDropOnTab).
                // Otherwise, pressing starts a potential drag (TabMouseDown).
                let mut area =
                    iced::widget::mouse_area(styled_tab).on_press(Message::TabMouseDown(panel_id));

                if is_dragging {
                    area = area
                        .on_release(Message::TabDropOnTab(leaf_path_owned.clone(), i))
                        .interaction(iced::mouse::Interaction::Crosshair);
                } else {
                    area = area.interaction(iced::mouse::Interaction::Grab);
                }

                let el: Element<'_, Message> = area.into();
                el
            })
            .collect();

        let mut tab_row = row(tab_buttons).spacing(1);
        if let Some(dragged) = dragging_panel {
            tab_row = tab_row
                .push(horizontal())
                .push(
                    text(format!("Moving: {}", dragged.title()))
                        .size(10)
                        .color(self.theme_tokens.info),
                )
                .push({
                    let cancel_color = self.theme_tokens.error;
                    button(text(icons::CLOSE).font(icons::ICON_FONT).size(9))
                        .padding(Padding::from([2, 6]))
                        .on_press(Message::TabDragCancel)
                        .style(move |_theme: &Theme, _status| button::Style {
                            background: None,
                            text_color: cancel_color,
                            border: Border::default(),
                            ..Default::default()
                        })
                });
        }

        let tab_bar_bg = self.theme_tokens.bg_tertiary;
        let tab_bar = container(tab_row)
            .style(move |_theme: &Theme| container::Style {
                background: Some(tab_bar_bg.into()),
                ..Default::default()
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
            let drop_zone_bg = self.theme_tokens.drop_zone;
            let drop_zone_border = self.theme_tokens.text_accent;
            let drop_zone = |label: &'a str,
                             zone: dock::tree::DropZone,
                             p: Vec<dock::PathDir>|
             -> Element<'a, Message> {
                let visual: Element<'a, Message> =
                    container(text(label).size(11).center().width(Fill).height(Fill))
                        .width(Fill)
                        .height(Fill)
                        .padding(0)
                        .style(move |_theme: &Theme| container::Style {
                            background: Some(drop_zone_bg.into()),
                            border: Border::default().rounded(2).width(1).color(iced::Color {
                                a: 0.2,
                                ..drop_zone_border
                            }),
                            ..Default::default()
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

            let center_zone = drop_zone(
                "Drop Here",
                dock::tree::DropZone::Center,
                path_owned.clone(),
            );
            let left_zone = drop_zone("<", dock::tree::DropZone::Left, path_owned.clone());
            let right_zone = drop_zone(">", dock::tree::DropZone::Right, path_owned.clone());
            let top_zone = drop_zone("^", dock::tree::DropZone::Top, path_owned.clone());
            let bottom_zone = drop_zone("v", dock::tree::DropZone::Bottom, path_owned);

            

            container(
                column![
                    container(top_zone)
                        .height(Length::FillPortion(1))
                        .width(Fill),
                    row![
                        container(left_zone)
                            .width(Length::FillPortion(1))
                            .height(Fill),
                        container(center_zone)
                            .width(Length::FillPortion(2))
                            .height(Fill),
                        container(right_zone)
                            .width(Length::FillPortion(1))
                            .height(Fill),
                    ]
                    .height(Length::FillPortion(2))
                    .width(Fill),
                    container(bottom_zone)
                        .height(Length::FillPortion(1))
                        .width(Fill),
                ]
                .width(Fill)
                .height(Fill),
            )
            .width(Fill)
            .height(Fill)
            .padding(4)
            .into()
        } else {
            let panel_bg = self.theme_tokens.bg_secondary;
            let panel_border = self.theme_tokens.border_default;
            container(content)
                .width(Fill)
                .height(Fill)
                .style(move |_theme: &Theme| container::Style {
                    background: Some(panel_bg.into()),
                    border: Border::default().width(1).color(panel_border),
                    ..Default::default()
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
            Designer => self.panel_designer.view(ctx),
        }
    }
}
