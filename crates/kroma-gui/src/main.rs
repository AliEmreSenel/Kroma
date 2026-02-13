//! Kroma GUI — modular docking-based desktop application for the Kroma wallpaper engine.
//!
//! Features a full IDE-style docking layout with panels for node editing,
//! code editing, asset management, live preview, and more.
//! Communicates with `kroma-daemon` over Unix IPC.
//!
//! CLI fallback: pass any subcommand (import, download, load, pause, …).

mod commands;
mod dock;
mod dock_views;
mod editor;
mod editor_handler;
mod graph_handler;
mod importer;
mod ipc;
mod ipc_client;
mod legacy_views;
mod message;
mod panels;
mod shade_handler;
mod shade_views;
mod sysinfo;
mod theme;
mod utils;
mod widgets;

use std::path::PathBuf;

use anyhow::Result;
use iced::widget::{container, row, text_editor};
use iced::{Element, Fill, Subscription, Task, Theme};
use log::info;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 && args[1] != "--gui" {
        return cli_main(&args);
    }

    info!("Kroma GUI v{}", env!("CARGO_PKG_VERSION"));
    iced::application("Kroma", KromaApp::update, KromaApp::view)
        .theme(KromaApp::theme)
        .subscription(KromaApp::subscription)
        .window_size((1280.0, 800.0))
        .run_with(KromaApp::new)
        .map_err(|e| anyhow::anyhow!("GUI error: {}", e))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// CLI mode
// ---------------------------------------------------------------------------

fn cli_main(args: &[String]) -> Result<()> {
    info!("Kroma GUI v{} (CLI mode)", env!("CARGO_PKG_VERSION"));

    match args.get(1).map(|s| s.as_str()) {
        Some("import") => {
            let source_path = args.get(2).map(PathBuf::from);
            let name = args.get(3).map(|s| s.as_str()).unwrap_or("Imported Shader");
            let author = args.get(4).map(|s| s.as_str()).unwrap_or("Unknown");
            if let Some(path) = source_path {
                info!("Importing shader from: {}", path.display());
                let output = importer::import_shadertoy_file(&path, name, author)?;
                info!("Created shade package: {}", output.display());
            } else {
                eprintln!("Usage: kroma-gui import <shader.glsl> [name] [author]");
            }
        }
        Some("load") => {
            let shade_path = args.get(2).map(|s| s.as_str()).unwrap_or("");
            if shade_path.is_empty() {
                eprintln!("Usage: kroma-gui load <path.shade>");
            } else {
                ipc_client::send_load(shade_path)?;
                info!("Sent load command to daemon: {}", shade_path);
            }
        }
        Some("pause") => {
            ipc_client::send_pause()?;
            info!("Sent pause command");
        }
        Some("resume") => {
            ipc_client::send_resume()?;
            info!("Sent resume command");
        }
        Some("shutdown") => {
            ipc_client::send_shutdown()?;
            info!("Sent shutdown command");
        }
        Some("status") => match ipc_client::query_status() {
            Ok(response) => println!("{}", response),
            Err(e) => eprintln!("Failed: {}", e),
        },
        Some("download") => {
            let url_or_id = args.get(2).map(|s| s.as_str()).unwrap_or("");
            if url_or_id.is_empty() {
                eprintln!("Usage: kroma-gui download <shadertoy-url-or-id> [output-dir]");
            } else {
                let output_dir = args
                    .get(3)
                    .map(PathBuf::from)
                    .unwrap_or_else(utils::dirs_output_dir);
                let rt = tokio::runtime::Runtime::new()?;
                match rt.block_on(importer::download_shadertoy(url_or_id, &output_dir, None)) {
                    Ok((shade_path, name)) => {
                        info!("Downloaded '{}': {}", name, shade_path.display())
                    }
                    Err(e) => eprintln!("Download failed: {}", e),
                }
            }
        }
        _ => {
            println!("Kroma GUI v{}", env!("CARGO_PKG_VERSION"));
            println!();
            println!("Usage:");
            println!(
                "  kroma-gui                                           Launch graphical interface"
            );
            println!(
                "  kroma-gui import <shader.glsl> [name] [author]      Import Shadertoy shader"
            );
            println!(
                "  kroma-gui download <url-or-id> [output-dir]         Download from Shadertoy"
            );
            println!(
                "  kroma-gui load <path.shade>                         Load shade package"
            );
            println!(
                "  kroma-gui pause / resume / shutdown / status        Control daemon"
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Dashboard,
    Import,
    Editor,
    ShadeEdit,
    Settings,
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// Tracks the state of a divider drag operation.
#[derive(Debug, Clone)]
struct DividerDrag {
    /// Path to the split node whose divider is being dragged.
    path: Vec<dock::PathDir>,
    /// Axis of the split (determines horizontal vs vertical drag).
    axis: dock::tree::SplitAxis,
}

struct KromaApp {
    active_tab: Tab,
    loaded_shade: Option<String>,
    status_text: String,
    daemon_connected: bool,
    paused: bool,
    fps: f32,
    target_fps: u32,
    // --- Dock system ---
    dock: dock::DockState,
    // --- Panel instances ---
    panel_dashboard: panels::dashboard::DashboardPanel,
    panel_node_editor: panels::node_editor::NodeEditorPanel,
    panel_code_editor: panels::code_editor::CodeEditorPanel,
    panel_asset_browser: panels::asset_browser::AssetBrowserPanel,
    panel_asset_preview: panels::asset_preview::AssetPreviewPanel,
    panel_properties: panels::properties::PropertiesPanel,
    panel_library: panels::library::LibraryPanel,
    panel_live_preview: panels::live_preview::LivePreviewPanel,
    panel_import: panels::import::ImportPanel,
    panel_error_log: panels::error_log::ErrorLogPanel,
    panel_settings: panels::settings::SettingsPanel,
    /// Whether to use the new dock layout (true) or legacy sidebar (false).
    use_dock_layout: bool,
    /// Command history for undo/redo.
    command_history: commands::CommandHistory,
    /// Active UI theme.
    active_theme: theme::KromaThemeId,
    /// Divider drag state: which split is being dragged.
    dragging_divider: Option<DividerDrag>,
    /// Tab being dragged for dock rearrangement.
    dragging_tab: Option<panels::PanelId>,
    /// Pending tab press (before drag threshold is met).
    /// Stores (panel_id, cursor_position_at_press).
    tab_press_origin: Option<(panels::PanelId, iced::Point)>,
    /// Last known cursor position (for drag threshold computation).
    last_cursor_pos: iced::Point,
    /// Window size for ratio computation during divider drag.
    window_size: (f32, f32),
    // Import fields
    import_path: String,
    import_name: String,
    import_author: String,
    import_result: String,
    // Shadertoy download
    shadertoy_url: String,
    download_result: String,
    downloading: bool,
    // Settings
    api_key: String,
    // Shader editor
    shader_graph: editor::ShaderGraph,
    graph_canvas: editor::canvas::GraphCanvas,
    editor_palette_open: bool,
    editor_palette_pos: [f32; 2],
    editor_palette_filter: String,
    editor_glsl_preview: String,
    /// Raw GLSL text editor content (for text editing mode).
    editor_glsl_content: text_editor::Content,
    /// Whether the editor is in text mode (true) or node mode (false).
    editor_text_mode: bool,
    /// Whether live preview is active (auto-send to daemon on compile).
    editor_live_preview: bool,
    /// Auto-compile: mark dirty when text changes, debounce before compiling.
    editor_dirty: bool,
    /// Timestamp of the last text edit for debounce timing.
    editor_last_edit: std::time::Instant,
    /// Auto-compile: mark dirty when the editor node graph structure changes.
    editor_graph_dirty: bool,
    /// Timestamp of the last editor graph mutation for debounce.
    editor_graph_last_edit: std::time::Instant,
    /// Auto-compile: mark dirty when the shade node graph structure changes.
    shade_graph_dirty: bool,
    /// Timestamp of the last shade graph mutation for debounce.
    shade_graph_last_edit: std::time::Instant,
    // Sub-graph navigation
    /// Sub-graphs for the editor graph: NodeId → (sub-graph, canvas state).
    editor_subgraphs: std::collections::HashMap<kroma_graph::types::NodeId, (editor::ShaderGraph, editor::canvas::GraphCanvas)>,
    /// Navigation path from root into sub-graphs (stack of NodeIds).
    editor_nav_path: Vec<kroma_graph::types::NodeId>,
    /// Sub-graphs for the shade graph.
    shade_subgraphs: std::collections::HashMap<kroma_graph::types::NodeId, (editor::ShaderGraph, editor::canvas::GraphCanvas)>,
    /// Navigation path for shade graph.
    shade_nav_path: Vec<kroma_graph::types::NodeId>,
    // Shade package editing
    shade_config: kroma_shared::types::ShadeConfig,
    shade_config_toml: text_editor::Content,
    shade_loaded_path: Option<PathBuf>,
    shade_new_uniform_name: String,
    shade_new_uniform_type: String,
    shade_new_texture_name: String,
    shade_new_texture_type: String,
    /// The loaded shade package for editing (full package including assets).
    shade_package: Option<kroma_shared::shade::ShadePackage>,
    /// Currently selected file in the project file tree.
    shade_selected_file: Option<String>,
    /// The shader source GLSL content for the shade package.
    shade_shader_content: text_editor::Content,
    /// Edit mode for shade shader: "code", "settings", "preview", or "nodes"
    shade_edit_mode: String,
    /// Node graph for the shade editor (independent of the Editor tab's graph).
    shade_graph: editor::ShaderGraph,
    /// Canvas state for the shade editor's node graph.
    shade_graph_canvas: editor::canvas::GraphCanvas,
    // Compile errors from daemon
    /// Last compile errors returned by the daemon.
    compile_errors: Vec<kroma_shared::ipc::CompileError>,
    /// Last compile warnings returned by the daemon.
    compile_warnings: Vec<String>,
    /// Whether the last compile was successful.
    compile_success: Option<bool>,
    // Log
    log_messages: std::collections::VecDeque<String>,
    /// Application start time for simulating runtime values.
    app_start: std::time::Instant,
    /// Clipboard buffer for node copy/paste.
    clipboard: Option<ClipboardData>,
    // --- Async IPC ---
    /// Handle for sending commands to the daemon via async IPC.
    ipc_handle: Option<ipc::IpcHandle>,
    /// Latest preview frame from the daemon (RGBA pixel data).
    preview_frame: Option<PreviewFrameData>,
    /// Whether preview streaming is active.
    preview_streaming: bool,
}

/// Serializable clipboard data for copy-paste of nodes.
#[derive(Debug, Clone)]
struct ClipboardData {
    /// (NodeKind, relative_position, defaults) for each node.
    nodes: Vec<(editor::NodeKind, [f32; 2], Vec<kroma_graph::types::DefaultValue>)>,
    /// (from_node_index, from_port, to_node_index, to_port) — indices into `nodes`.
    connections: Vec<(usize, usize, usize, usize)>,
}

/// Decoded preview frame data from daemon.
#[derive(Debug, Clone)]
struct PreviewFrameData {
    /// RGBA pixel data.
    pixels: Vec<u8>,
    /// Frame width.
    width: u32,
    /// Frame height.
    height: u32,
}

#[derive(Debug, Clone)]
enum Message {
    TabSelected(Tab),
    // Shade management
    LoadShadeClicked,
    ShadeFileSelected(Option<PathBuf>),
    UnloadShadeClicked,
    ClearLog,
    // Daemon control
    PauseClicked,
    ResumeClicked,
    ShutdownClicked,
    RefreshStatus,
    // Import
    ImportPathChanged(String),
    ImportNameChanged(String),
    ImportAuthorChanged(String),
    ImportClicked,
    BrowseImportClicked,
    ImportFileSelected(Option<PathBuf>),
    ImportResult(String),
    // Shadertoy download
    ShadertoyUrlChanged(String),
    DownloadShadertoyClicked,
    DownloadResult(String),
    // Settings
    ApiKeyChanged(String),
    // Editor
    EditorGraph(editor::canvas::GraphMessage),
    EditorAddNode(editor::NodeKind),
    EditorCompile,
    EditorExport,
    #[allow(dead_code)]
    EditorPaletteFilter(String),
    #[allow(dead_code)]
    EditorClosePalette,
    EditorToggleTextMode,
    EditorGlslSourceChanged(text_editor::Action),
    EditorLivePreview,
    EditorToggleLive,
    EditorLoadGlsl,
    EditorLoadGlslResult(Option<PathBuf>),
    // Shade editing
    ShadeNew,
    ShadeOpen,
    ShadeOpenResult(Option<PathBuf>),
    ShadeSave,
    ShadeMetaName(String),
    ShadeMetaAuthor(String),
    ShadeMetaVersion(String),
    ShadeMetaDescription(String),
    ShadeTargetFps(String),
    ShadePauseOffscreen(bool),
    ShadePauseFullscreen(bool),
    ShadeAudioEnabled(bool),
    ShadeAudioSource(String),
    ShadeConfigToml(text_editor::Action),
    ShadeAddUniform,
    ShadeRemoveUniform(String),
    ShadeNewUniformName(String),
    ShadeNewUniformType(String),
    ShadeAddTexture,
    ShadeRemoveTexture(String),
    ShadeNewTextureName(String),
    ShadeNewTextureType(String),
    ShadeSelectFile(String),
    ShadeAddAsset,
    ShadeAddAssetResult(Option<PathBuf>),
    ShadeRemoveAsset(String),
    ShadeEditMode(String),
    ShadeShaderChanged(text_editor::Action),
    /// Node graph message for the shade editor graph.
    ShadeGraphMsg(editor::canvas::GraphMessage),
    /// Add a node in the shade editor graph.
    ShadeAddNode(editor::NodeKind),
    /// Compile the shade node graph to GLSL.
    ShadeCompileGraph,
    /// Parse GLSL text into the shade node graph.
    ShadeParseToNodes,
    /// Send the shade shader to the daemon for live preview.
    ShadeSendToDaemon,
    ShadeAddGlslFile,
    ShadeAddGlslFileResult(Option<PathBuf>),
    // --- Async IPC ---
    /// Received an event from the async IPC subscription.
    IpcEvent(ipc::IpcEvent, Option<ipc::IpcHandle>),
    /// Toggle preview frame streaming on/off.
    TogglePreviewStream,
    // Tick
    Tick,
    /// Auto-compile debounce tick (fires every 500ms).
    AutoCompileTick,
    /// Keyboard shortcut: Undo (Ctrl+Z).
    Undo,
    /// Keyboard shortcut: Redo (Ctrl+Y / Ctrl+Shift+Z).
    Redo,
    /// Theme changed by user.
    ThemeChanged(theme::KromaThemeId),
    /// Divider drag started on a split node.
    DividerDragStart(Vec<dock::PathDir>, dock::tree::SplitAxis),
    /// Mouse moved during divider drag (absolute cursor position).
    DividerMouseMoved(iced::Point),
    /// Divider drag ended (mouse released).
    DividerDragEnd,
    /// Window resized.
    WindowResized(iced::Size),
    /// User pressed down on a tab (potential drag start).
    TabMouseDown(panels::PanelId),
    /// Tab dropped on a leaf pane's drop zone.
    TabDrop(Vec<dock::PathDir>, dock::tree::DropZone),
    /// Tab dropped onto another tab — reorder within (or move to) a leaf.
    TabDropOnTab(Vec<dock::PathDir>, usize),
    /// Tab drag cancelled (Escape or cancel button).
    TabDragCancel,
}

// ---------------------------------------------------------------------------
// Application logic
// ---------------------------------------------------------------------------

impl KromaApp {
    fn new() -> (Self, Task<Message>) {
        let api_key = std::env::var("SHADERTOY_API_KEY").unwrap_or_default();
        let app = Self {
            active_tab: Tab::Dashboard,
            loaded_shade: None,
            status_text: "Connecting...".into(),
            daemon_connected: false,
            paused: false,
            fps: 0.0,
            target_fps: 60,
            // Dock system
            dock: dock::DockState::default_layout(),
            // Panel instances
            panel_dashboard: panels::dashboard::DashboardPanel::new(),
            panel_node_editor: panels::node_editor::NodeEditorPanel::new(),
            panel_code_editor: panels::code_editor::CodeEditorPanel::new(),
            panel_asset_browser: panels::asset_browser::AssetBrowserPanel::new(),
            panel_asset_preview: panels::asset_preview::AssetPreviewPanel::new(),
            panel_properties: panels::properties::PropertiesPanel::new(),
            panel_library: panels::library::LibraryPanel::new(),
            panel_live_preview: panels::live_preview::LivePreviewPanel::new(),
            panel_import: panels::import::ImportPanel::new(),
            panel_error_log: panels::error_log::ErrorLogPanel::new(),
            panel_settings: panels::settings::SettingsPanel::new(),
            use_dock_layout: true,
            command_history: commands::CommandHistory::default(),
            active_theme: theme::KromaThemeId::TokyoNight,
            dragging_divider: None,
            dragging_tab: None,
            tab_press_origin: None,
            last_cursor_pos: iced::Point::ORIGIN,
            window_size: (1280.0, 800.0),
            import_path: String::new(),
            import_name: "Imported Shader".into(),
            import_author: "Unknown".into(),
            import_result: String::new(),
            shadertoy_url: String::new(),
            download_result: String::new(),
            downloading: false,
            api_key,
            shader_graph: editor::ShaderGraph::new(),
            graph_canvas: editor::canvas::GraphCanvas::new(),
            editor_palette_open: false,
            editor_palette_pos: [300.0, 300.0],
            editor_palette_filter: String::new(),
            editor_glsl_preview: String::new(),
            editor_glsl_content: text_editor::Content::new(),
            editor_text_mode: false,
            editor_live_preview: false,
            editor_dirty: false,
            editor_last_edit: std::time::Instant::now(),
            editor_graph_dirty: false,
            editor_graph_last_edit: std::time::Instant::now(),
            shade_graph_dirty: false,
            shade_graph_last_edit: std::time::Instant::now(),
            editor_subgraphs: std::collections::HashMap::new(),
            editor_nav_path: Vec::new(),
            shade_subgraphs: std::collections::HashMap::new(),
            shade_nav_path: Vec::new(),
            shade_config: kroma_shared::types::ShadeConfig {
                meta: kroma_shared::types::ShadeMeta {
                    name: "New Shader".into(),
                    author: "Kroma User".into(),
                    version: "1.0".into(),
                    description: String::new(),
                    tags: Vec::new(),
                },
                mode: Default::default(),
                rendering: Default::default(),
                audio: Default::default(),
                uniforms: Default::default(),
                textures: Default::default(),
                slideshow: Default::default(),
                fonts: Default::default(),
            },
            shade_config_toml: text_editor::Content::new(),
            shade_loaded_path: None,
            shade_new_uniform_name: String::new(),
            shade_new_uniform_type: "float".into(),
            shade_new_texture_name: String::new(),
            shade_new_texture_type: "image".into(),
            shade_package: None,
            shade_selected_file: None,
            shade_shader_content: text_editor::Content::new(),
            shade_edit_mode: "settings".into(),
            shade_graph: editor::ShaderGraph::new(),
            shade_graph_canvas: editor::canvas::GraphCanvas::default(),
            compile_errors: Vec::new(),
            compile_warnings: Vec::new(),
            compile_success: None,
            log_messages: std::collections::VecDeque::new(),
            app_start: std::time::Instant::now(),
            clipboard: None,
            ipc_handle: None,
            preview_frame: None,
            preview_streaming: false,
        };
        (
            app,
            Task::none(),
        )
    }

    fn theme(&self) -> Theme {
        self.active_theme.iced_theme()
    }

    fn subscription(&self) -> Subscription<Message> {
        // Fast tick for auto-compile debounce
        let fast_tick = iced::time::every(std::time::Duration::from_millis(500))
            .map(|_| Message::AutoCompileTick);

        // Status polling fallback (used when async IPC is connected to
        // periodically query status — fires the StatusQuery command)
        let status_tick = iced::time::every(std::time::Duration::from_secs(3))
            .map(|_| Message::Tick);

        // Async IPC subscription — persistent connection with auto-reconnect
        let ipc_sub = ipc::ipc_subscription()
            .map(|(event, handle)| Message::IpcEvent(event, handle));

        // Keyboard shortcuts for undo/redo
        let keyboard = iced::keyboard::on_key_press(|key, modifiers| {
            use iced::keyboard::Key;
            if modifiers.control() {
                match key {
                    Key::Character(c) if c.as_ref() == "z" && !modifiers.shift() => {
                        Some(Message::Undo)
                    }
                    Key::Character(c) if c.as_ref() == "y" => {
                        Some(Message::Redo)
                    }
                    Key::Character(c) if c.as_ref() == "z" && modifiers.shift() => {
                        Some(Message::Redo)
                    }
                    _ => None,
                }
            } else {
                match key {
                    Key::Named(iced::keyboard::key::Named::Escape) => Some(Message::TabDragCancel),
                    _ => None,
                }
            }
        });

        let mut subs = vec![fast_tick, status_tick, keyboard, ipc_sub];

        // Listen for mouse events (divider drag) + window resize.
        // DividerMouseMoved / DividerDragEnd are ignored by update() when
        // not dragging, so the overhead of always subscribing is negligible.
        subs.push(iced::event::listen_with(|event, _status, _id| {
            match event {
                iced::Event::Window(iced::window::Event::Resized(size)) => {
                    Some(Message::WindowResized(size))
                }
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Message::DividerMouseMoved(position))
                }
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                    iced::mouse::Button::Left,
                )) => Some(Message::DividerDragEnd),
                _ => None,
            }
        }));

        Subscription::batch(subs)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TabSelected(tab) => self.active_tab = tab,

            Message::LoadShadeClicked => {
                return Task::perform(
                    async {
                        let h = rfd::AsyncFileDialog::new()
                            .set_title("Select Shade Package")
                            .add_filter("Shade Package", &["shade"])
                            .pick_file()
                            .await;
                        h.map(|f| f.path().to_path_buf())
                    },
                    Message::ShadeFileSelected,
                );
            }
            Message::ShadeFileSelected(Some(path)) => {
                let s = path.to_string_lossy().to_string();
                if let Some(ref handle) = self.ipc_handle {
                    handle.send(kroma_shared::ipc::DaemonCommand::LoadShade {
                        path: s.clone(),
                    });
                    self.loaded_shade = Some(s.clone());
                    self.log_msg(format!("Loading: {}", s));
                } else {
                    self.log_msg("Not connected to daemon".into());
                }
            }
            Message::ShadeFileSelected(None) => {}
            Message::UnloadShadeClicked => {
                self.loaded_shade = None;
                // Notify daemon to stop rendering the current shade
                if let Some(ref handle) = self.ipc_handle {
                    handle.send(kroma_shared::ipc::DaemonCommand::Pause);
                }
                self.log_msg("Shade unloaded".into());
            }
            Message::ClearLog => {
                self.log_messages.clear();
            }

            Message::PauseClicked => {
                if let Some(ref handle) = self.ipc_handle {
                    handle.send(kroma_shared::ipc::DaemonCommand::Pause);
                    self.paused = true;
                    self.log_msg("Pause sent".into());
                } else {
                    self.log_msg("Not connected to daemon".into());
                }
            }
            Message::ResumeClicked => {
                if let Some(ref handle) = self.ipc_handle {
                    handle.send(kroma_shared::ipc::DaemonCommand::Resume);
                    self.paused = false;
                    self.log_msg("Resume sent".into());
                } else {
                    self.log_msg("Not connected to daemon".into());
                }
            }
            Message::ShutdownClicked => {
                if let Some(ref handle) = self.ipc_handle {
                    handle.send(kroma_shared::ipc::DaemonCommand::Shutdown);
                    self.daemon_connected = false;
                    self.status_text = "Daemon shut down".into();
                    self.log_msg("Daemon shutdown sent".into());
                } else {
                    self.log_msg("Not connected to daemon".into());
                }
            },

            Message::RefreshStatus | Message::Tick => {
                // Send status query via async IPC handle
                if let Some(ref handle) = self.ipc_handle {
                    handle.send(kroma_shared::ipc::DaemonCommand::StatusQuery);
                }
            }
            Message::AutoCompileTick => {
                let debounce = std::time::Duration::from_millis(800);

                // Auto-compile: text editor dirty (text mode).
                if self.editor_dirty
                    && self.editor_text_mode
                    && self.editor_live_preview
                    && self.editor_last_edit.elapsed() >= debounce
                {
                    self.editor_dirty = false;
                    let src = self.editor_glsl_content.text();
                    if !src.trim().is_empty() {
                        self.editor_glsl_preview = src.clone();
                        self.send_to_daemon(&src);
                        // Keep node graph in sync
                        self.shader_graph = editor::parse_glsl_to_graph(&src);
                    }
                }

                // Auto-compile: editor node graph dirty (node mode).
                if self.editor_graph_dirty
                    && !self.editor_text_mode
                    && self.editor_graph_last_edit.elapsed() >= debounce
                {
                    self.editor_graph_dirty = false;
                    match self.shader_graph.compile_glsl() {
                        Ok(glsl) => {
                            self.editor_glsl_preview = glsl.clone();
                            self.editor_glsl_content = text_editor::Content::with_text(&glsl);
                            if self.editor_live_preview {
                                self.send_to_daemon(&glsl);
                            }
                        }
                        Err(e) => self.log_msg(format!("Graph auto-compile: {}", e)),
                    }
                }

                // Auto-compile: shade node graph dirty.
                if self.shade_graph_dirty
                    && self.shade_graph_last_edit.elapsed() >= debounce
                {
                    self.shade_graph_dirty = false;
                    match self.shade_graph.compile_glsl() {
                        Ok(glsl) => {
                            self.shade_shader_content = text_editor::Content::with_text(&glsl);
                            if let Some(ref mut pkg) = self.shade_package {
                                pkg.shader_source = Some(glsl);
                            }
                        }
                        Err(e) => self.log_msg(format!("Shade graph auto-compile: {}", e)),
                    }
                }

                // Update live values for input/system nodes on both canvases
                self.update_live_values();
            }

            Message::Undo => {
                match self.command_history.undo(
                    &mut self.shader_graph,
                    &mut self.shade_graph,
                ) {
                    Ok(true) => {
                        let desc = self.command_history.redo_description()
                            .unwrap_or("?").to_string();
                        self.log_msg(format!("Undo: {}", desc));
                        // Re-compile both graphs since undo may affect either.
                        let now = std::time::Instant::now();
                        self.editor_graph_dirty = true;
                        self.editor_graph_last_edit = now;
                        self.shade_graph_dirty = true;
                        self.shade_graph_last_edit = now;
                    }
                    Ok(false) => self.log_msg("Nothing to undo".into()),
                    Err(e) => self.log_msg(format!("Undo failed: {}", e)),
                }
            }
            Message::Redo => {
                match self.command_history.redo(
                    &mut self.shader_graph,
                    &mut self.shade_graph,
                ) {
                    Ok(true) => {
                        let desc = self.command_history.undo_description()
                            .unwrap_or("?").to_string();
                        self.log_msg(format!("Redo: {}", desc));
                        let now = std::time::Instant::now();
                        self.editor_graph_dirty = true;
                        self.editor_graph_last_edit = now;
                        self.shade_graph_dirty = true;
                        self.shade_graph_last_edit = now;
                    }
                    Ok(false) => self.log_msg("Nothing to redo".into()),
                    Err(e) => self.log_msg(format!("Redo failed: {}", e)),
                }
            }

            Message::ThemeChanged(theme_id) => {
                self.active_theme = theme_id;
                self.log_msg(format!("Theme changed to {}", theme_id.name()));
            }

            Message::DividerDragStart(path, axis) => {
                self.dragging_divider = Some(DividerDrag { path, axis });
            }
            Message::DividerMouseMoved(cursor) => {
                self.last_cursor_pos = cursor;

                // Check drag threshold: if a tab press is pending and cursor
                // moved more than 8 px, promote to a full drag.
                if let Some((panel_id, origin)) = self.tab_press_origin {
                    let dx = cursor.x - origin.x;
                    let dy = cursor.y - origin.y;
                    if dx * dx + dy * dy > 64.0 {
                        self.dragging_tab = Some(panel_id);
                        self.tab_press_origin = None;
                        self.dock.root.activate_panel(panel_id);
                    }
                }

                if let Some(ref drag) = self.dragging_divider {
                    let (start, size) = utils::compute_split_bounds(
                        &self.dock.root,
                        &drag.path,
                        drag.axis,
                        self.window_size.0,
                        self.window_size.1,
                    );
                    let cursor_pos = match drag.axis {
                        dock::tree::SplitAxis::Horizontal => cursor.x,
                        dock::tree::SplitAxis::Vertical => cursor.y,
                    };
                    if size > 0.0 {
                        let new_ratio = ((cursor_pos - start) / size).clamp(0.1, 0.9);
                        self.dock.root.set_ratio(&drag.path, new_ratio);
                    }
                }
            }
            Message::DividerDragEnd => {
                self.dragging_divider = None;
                // Clear pending tab press (it was just a quick click, not a drag).
                self.tab_press_origin = None;
                // If a tab drag is in progress but didn't land on a drop zone,
                // cancel it so the drag overlay doesn't stay visible.
                if self.dragging_tab.is_some() {
                    self.dragging_tab = None;
                }
            }
            Message::WindowResized(size) => {
                self.window_size = (size.width, size.height);
            }
            Message::TabMouseDown(panel_id) => {
                // Record the press origin; drag mode activates only after
                // the cursor moves beyond the threshold (see DividerMouseMoved).
                self.tab_press_origin = Some((panel_id, self.last_cursor_pos));
                // Activate the tab immediately so the user sees it.
                self.dock.root.activate_panel(panel_id);
            }
            Message::TabDrop(target_path, zone) => {
                if let Some(panel_id) = self.dragging_tab.take() {
                    self.dock.update(dock::DockMessage::PanelDropped {
                        panel_id,
                        target_path,
                        zone,
                    });
                }
            }
            Message::TabDropOnTab(target_path, target_index) => {
                if let Some(panel_id) = self.dragging_tab.take() {
                    // Remove from old location, insert at target position
                    self.dock.root.remove_panel(panel_id);
                    self.dock.root.insert_panel_at_index(panel_id, &target_path, target_index);
                    self.dock.root.cleanup();
                }
                self.tab_press_origin = None;
            }
            Message::TabDragCancel => {
                self.dragging_tab = None;
                self.tab_press_origin = None;
                // Also cancel any pending palette node placement
                self.graph_canvas.pending_node = None;
                self.shade_graph_canvas.pending_node = None;
            }

            // --- Async IPC events ---
            Message::IpcEvent(event, handle) => {
                // Store handle on first connection
                if let Some(h) = handle {
                    self.ipc_handle = Some(h);
                }
                match event {
                    ipc::IpcEvent::Connected => {
                        self.daemon_connected = true;
                        self.log_msg("Connected to daemon".into());
                        // Query status immediately on connect
                        if let Some(ref h) = self.ipc_handle {
                            h.send(kroma_shared::ipc::DaemonCommand::StatusQuery);
                        }
                    }
                    ipc::IpcEvent::Disconnected(reason) => {
                        self.daemon_connected = false;
                        self.status_text = "Offline".into();
                        self.preview_streaming = false;
                        self.log_msg(format!("Daemon disconnected: {}", reason));
                    }
                    ipc::IpcEvent::Reconnecting { attempt } => {
                        self.status_text = format!("Reconnecting (attempt {})...", attempt);
                    }
                    ipc::IpcEvent::Event(daemon_event) => {
                        use kroma_shared::ipc::DaemonEvent;
                        match daemon_event {
                            DaemonEvent::Status { fps, paused, loaded_shade } => {
                                self.daemon_connected = true;
                                self.fps = fps;
                                self.paused = paused;
                                self.loaded_shade = loaded_shade;
                                self.status_text = format!("{:.1} FPS", fps);
                            }
                            DaemonEvent::CompileResult { success, errors, warnings } => {
                                self.handle_compile_result(
                                    DaemonEvent::CompileResult { success, errors, warnings },
                                );
                            }
                            DaemonEvent::PreviewFrame { jpeg_base64, width, height } => {
                                // Decode base64 JPEG → RGBA pixels
                                if let Ok(jpeg_data) = base64::Engine::decode(
                                    &base64::engine::general_purpose::STANDARD,
                                    &jpeg_base64,
                                ) {
                                    if let Ok(img) = image::load_from_memory_with_format(
                                        &jpeg_data,
                                        image::ImageFormat::Jpeg,
                                    ) {
                                        let rgba = img.to_rgba8();
                                        self.preview_frame = Some(PreviewFrameData {
                                            pixels: rgba.into_raw(),
                                            width,
                                            height,
                                        });
                                    }
                                }
                            }
                            DaemonEvent::ShadeLoaded { name } => {
                                self.loaded_shade = Some(name.clone());
                                self.log_msg(format!("Shade loaded: {}", name));
                            }
                            DaemonEvent::Error { message } => {
                                self.log_msg(format!("Daemon error: {}", message));
                            }
                            DaemonEvent::Ready => {}
                            DaemonEvent::SystemInfo { cpu_usage, ram_usage, .. } => {
                                self.status_text = format!(
                                    "{:.1} FPS | CPU {:.0}% | RAM {:.0}%",
                                    self.fps, cpu_usage, ram_usage
                                );
                            }
                        }
                    }
                }
            }

            Message::TogglePreviewStream => {
                if self.preview_streaming {
                    // Stop streaming
                    if let Some(ref handle) = self.ipc_handle {
                        handle.send(kroma_shared::ipc::DaemonCommand::StopPreviewStream);
                    }
                    self.preview_streaming = false;
                    self.log_msg("Preview stream stopped".into());
                } else {
                    // Start streaming at 1/4 res, ~15fps
                    if let Some(ref handle) = self.ipc_handle {
                        handle.send(kroma_shared::ipc::DaemonCommand::StartPreviewStream {
                            width: 480,
                            height: 270,
                            target_fps: 15,
                        });
                        self.preview_streaming = true;
                        self.log_msg("Preview stream started".into());
                    } else {
                        self.log_msg("Not connected to daemon".into());
                    }
                }
            }

            Message::ImportPathChanged(s) => self.import_path = s,
            Message::ImportNameChanged(s) => self.import_name = s,
            Message::ImportAuthorChanged(s) => self.import_author = s,
            Message::BrowseImportClicked => {
                return Task::perform(
                    async {
                        let h = rfd::AsyncFileDialog::new()
                            .set_title("Select GLSL file")
                            .add_filter("GLSL", &["glsl", "frag", "txt"])
                            .pick_file()
                            .await;
                        h.map(|f| f.path().to_path_buf())
                    },
                    Message::ImportFileSelected,
                );
            }
            Message::ImportFileSelected(Some(p)) => {
                self.import_path = p.to_string_lossy().to_string();
            }
            Message::ImportFileSelected(None) => {}
            Message::ImportClicked => {
                let (path, name, author) = (
                    self.import_path.clone(),
                    self.import_name.clone(),
                    self.import_author.clone(),
                );
                return Task::perform(
                    async move { utils::do_import(&path, &name, &author) },
                    Message::ImportResult,
                );
            }
            Message::ImportResult(msg) => {
                self.import_result = msg.clone();
                self.log_msg(msg);
            }

            Message::ShadertoyUrlChanged(s) => self.shadertoy_url = s,
            Message::DownloadShadertoyClicked => {
                if self.shadertoy_url.is_empty() {
                    self.download_result = "Enter a Shadertoy URL or shader ID".into();
                } else {
                    self.downloading = true;
                    self.download_result = "Downloading...".into();
                    let url = self.shadertoy_url.clone();
                    return Task::perform(
                        async move { utils::do_download(&url).await },
                        Message::DownloadResult,
                    );
                }
            }
            Message::DownloadResult(msg) => {
                self.downloading = false;
                self.download_result = msg.clone();
                self.log_msg(msg);
            }

            Message::ApiKeyChanged(s) => {
                self.api_key = s.clone();
                // Store the API key in app state — importer will read it
                // from self.api_key rather than env var.
            }

            // --- Editor messages (delegated to editor_handler.rs) ---
            msg @ (Message::EditorGraph(_)
            | Message::EditorAddNode(_)
            | Message::EditorCompile
            | Message::EditorExport
            | Message::EditorPaletteFilter(_)
            | Message::EditorClosePalette
            | Message::EditorToggleTextMode
            | Message::EditorGlslSourceChanged(_)
            | Message::EditorLivePreview
            | Message::EditorToggleLive
            | Message::EditorLoadGlsl
            | Message::EditorLoadGlslResult(_)) => {
                return self.handle_editor_msg(msg);
            }

            // --- Shade messages (delegated to shade_handler.rs) ---
            msg @ (Message::ShadeNew
            | Message::ShadeOpen
            | Message::ShadeOpenResult(_)
            | Message::ShadeSave
            | Message::ShadeMetaName(_)
            | Message::ShadeMetaAuthor(_)
            | Message::ShadeMetaVersion(_)
            | Message::ShadeMetaDescription(_)
            | Message::ShadeTargetFps(_)
            | Message::ShadePauseOffscreen(_)
            | Message::ShadePauseFullscreen(_)
            | Message::ShadeAudioEnabled(_)
            | Message::ShadeAudioSource(_)
            | Message::ShadeConfigToml(_)
            | Message::ShadeAddUniform
            | Message::ShadeRemoveUniform(_)
            | Message::ShadeNewUniformName(_)
            | Message::ShadeNewUniformType(_)
            | Message::ShadeAddTexture
            | Message::ShadeRemoveTexture(_)
            | Message::ShadeNewTextureName(_)
            | Message::ShadeNewTextureType(_)
            | Message::ShadeSelectFile(_)
            | Message::ShadeEditMode(_)
            | Message::ShadeShaderChanged(_)
            | Message::ShadeGraphMsg(_)
            | Message::ShadeAddNode(_)
            | Message::ShadeCompileGraph
            | Message::ShadeParseToNodes
            | Message::ShadeSendToDaemon
            | Message::ShadeAddAsset
            | Message::ShadeAddAssetResult(_)
            | Message::ShadeRemoveAsset(_)
            | Message::ShadeAddGlslFile
            | Message::ShadeAddGlslFileResult(_)) => {
                return self.handle_shade_msg(msg);
            }
        }
        Task::none()
    }

    /// Sync the shade_config_toml text editor content from shade_config.
    fn sync_shade_toml(&mut self) {
        if let Ok(toml_str) = toml::to_string_pretty(&self.shade_config) {
            self.shade_config_toml = text_editor::Content::with_text(&toml_str);
        }
    }

    fn view(&self) -> Element<'_, Message> {
        if self.use_dock_layout {
            return self.view_docked();
        }
        // Legacy sidebar layout
        let sidebar = self.view_sidebar();
        let content: Element<'_, Message> = match self.active_tab {
            Tab::Dashboard => self.view_dashboard(),
            Tab::Import => self.view_import(),
            Tab::Editor => self.view_editor(),
            Tab::ShadeEdit => self.view_shade_edit(),
            Tab::Settings => self.view_settings(),
        };
        row![
            sidebar,
            container(content).width(Fill).height(Fill).padding(24),
        ]
        .width(Fill)
        .height(Fill)
        .into()
    }

    fn log_msg(&mut self, msg: String) {
        let ts = utils::chrono_now();
        self.log_messages.push_back(format!("[{}] {}", ts, msg));
        if self.log_messages.len() > 100 {
            self.log_messages.pop_front();
        }
    }
}
