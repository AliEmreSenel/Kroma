//! Kroma GUI — polished desktop application for the Kroma wallpaper engine.
//!
//! Features a sidebar-navigated interface with Dashboard, Import, and Settings
//! tabs. Communicates with `kroma-daemon` over Unix IPC.
//!
//! CLI fallback: pass any subcommand (import, download, load, pause, …).

mod editor;
mod importer;
mod ipc_client;

use std::path::PathBuf;

use anyhow::Result;
use iced::widget::{
    button, column, container, horizontal_rule, horizontal_space, progress_bar, row, scrollable,
    text, text_editor, text_input, Space,
};
use iced::{color, Border, Element, Fill, Length, Padding, Subscription, Task, Theme};
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
        .window_size((780.0, 680.0))
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
                    .unwrap_or_else(dirs_output_dir);
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

struct KromaApp {
    active_tab: Tab,
    loaded_shade: Option<String>,
    status_text: String,
    daemon_connected: bool,
    paused: bool,
    fps: f32,
    target_fps: u32,
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
    log_messages: Vec<String>,
    /// Application start time for simulating runtime values.
    app_start: std::time::Instant,
}

#[derive(Debug, Clone)]
enum Message {
    TabSelected(Tab),
    // Shade management
    LoadShadeClicked,
    ShadeFileSelected(Option<PathBuf>),
    UnloadShadeClicked,
    // Daemon control
    PauseClicked,
    ResumeClicked,
    ShutdownClicked,
    RefreshStatus,
    StatusReceived(String),
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
    // Tick
    Tick,
    /// Auto-compile debounce tick (fires every 500ms).
    AutoCompileTick,
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
            log_messages: Vec::new(),
            app_start: std::time::Instant::now(),
        };
        (
            app,
            Task::perform(async { query_daemon_status() }, Message::StatusReceived),
        )
    }

    fn theme(&self) -> Theme {
        Theme::TokyoNight
    }

    fn subscription(&self) -> Subscription<Message> {
        // Fast tick for auto-compile debounce + slower tick for status polling
        let fast_tick = iced::time::every(std::time::Duration::from_millis(500))
            .map(|_| Message::AutoCompileTick);
        let status_tick = iced::time::every(std::time::Duration::from_secs(3))
            .map(|_| Message::Tick);
        Subscription::batch([fast_tick, status_tick])
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
                match ipc_client::send_load(&s) {
                    Ok(()) => {
                        self.loaded_shade = Some(s.clone());
                        self.log_msg(format!("Loaded: {}", s));
                    }
                    Err(e) => self.log_msg(format!("Load failed: {}", e)),
                }
            }
            Message::ShadeFileSelected(None) => {}
            Message::UnloadShadeClicked => {
                self.loaded_shade = None;
                self.log_msg("Shade unloaded".into());
            }

            Message::PauseClicked => match ipc_client::send_pause() {
                Ok(()) => {
                    self.paused = true;
                    self.log_msg("Paused".into());
                }
                Err(e) => self.log_msg(format!("Pause failed: {}", e)),
            },
            Message::ResumeClicked => match ipc_client::send_resume() {
                Ok(()) => {
                    self.paused = false;
                    self.log_msg("Resumed".into());
                }
                Err(e) => self.log_msg(format!("Resume failed: {}", e)),
            },
            Message::ShutdownClicked => match ipc_client::send_shutdown() {
                Ok(()) => {
                    self.daemon_connected = false;
                    self.status_text = "Daemon shut down".into();
                    self.log_msg("Daemon shutdown sent".into());
                }
                Err(e) => self.log_msg(format!("Shutdown failed: {}", e)),
            },

            Message::RefreshStatus | Message::Tick => {
                return Task::perform(
                    async { query_daemon_status() },
                    Message::StatusReceived,
                );
            }
            Message::AutoCompileTick => {
                // Auto-compile: if editor is dirty and enough time has passed since
                // the last keystroke (debounce ~800ms), compile and optionally send.
                if self.editor_dirty
                    && self.editor_text_mode
                    && self.editor_live_preview
                    && self.editor_last_edit.elapsed() >= std::time::Duration::from_millis(800)
                {
                    self.editor_dirty = false;
                    let src = self.editor_glsl_content.text();
                    if !src.trim().is_empty() {
                        self.editor_glsl_preview = src.clone();
                        self.send_to_daemon(&src);
                    }
                }
                // Update live values for input/system nodes on both canvases
                self.update_live_values();
            }
            Message::StatusReceived(status) => {
                if status.starts_with('{') {
                    self.daemon_connected = true;
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&status) {
                        if let Some(fps) = parsed.get("fps").and_then(|v| v.as_f64()) {
                            self.fps = fps as f32;
                        }
                        if let Some(p) = parsed.get("paused").and_then(|v| v.as_bool()) {
                            self.paused = p;
                        }
                        if let Some(s) = parsed.get("loaded_shade").and_then(|v| v.as_str()) {
                            self.loaded_shade = Some(s.to_string());
                        }
                        self.status_text = format!("{:.1} FPS", self.fps);
                    }
                } else if status.contains("error") || status.contains("Could not") {
                    self.daemon_connected = false;
                    self.status_text = "Offline".into();
                } else {
                    self.status_text = status;
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
                    async move { do_import(&path, &name, &author) },
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
                        async move { do_download(&url).await },
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
                // Store for this session (importer reads env var)
                std::env::set_var("SHADERTOY_API_KEY", &s);
            }

            // --- Editor messages ---
            Message::EditorGraph(graph_msg) => {
                use editor::canvas::GraphMessage;
                match graph_msg {
                    GraphMessage::NodeMoved(id, pos) => {
                        if let Some(node) = self.shader_graph.node_mut(id) {
                            node.position = pos;
                        }
                    }
                    GraphMessage::ConnectionCreated(from, to) => {
                        self.shader_graph.add_connection(from, to);
                    }
                    GraphMessage::ConnectionDeleted(id) => {
                        self.shader_graph.remove_connection(id);
                    }
                    GraphMessage::NodeSelected(id, selected) => {
                        // Deselect all, then select the target
                        let ids: Vec<_> = self.shader_graph.nodes().map(|n| n.id).collect();
                        for nid in ids {
                            if let Some(n) = self.shader_graph.node_mut(nid) {
                                n.selected = nid == id && selected;
                            }
                        }
                    }
                    GraphMessage::DeleteSelected => {
                        let to_delete: Vec<_> = self
                            .shader_graph
                            .nodes()
                            .filter(|n| n.selected)
                            .map(|n| n.id)
                            .collect();
                        for id in to_delete {
                            self.shader_graph.remove_node(id);
                        }
                    }
                    GraphMessage::OpenPalette(pos) => {
                        self.editor_palette_open = true;
                        self.editor_palette_pos = pos;
                        self.editor_palette_filter.clear();
                    }
                    GraphMessage::Panned(offset) => {
                        self.graph_canvas.offset = offset;
                    }
                    GraphMessage::Zoomed(zoom, offset) => {
                        self.graph_canvas.zoom = zoom;
                        self.graph_canvas.offset = offset;
                    }
                    GraphMessage::DefaultChanged(node_id, port_idx, new_val) => {
                        if let Some(node) = self.shader_graph.node_mut(node_id) {
                            if port_idx < node.defaults.len() {
                                node.defaults[port_idx] = new_val;
                            }
                        }
                    }
                }
            }
            Message::EditorAddNode(kind) => {
                self.shader_graph
                    .add_node(kind, self.editor_palette_pos);
                self.editor_palette_open = false;
            }
            Message::EditorCompile => {
                let glsl = if self.editor_text_mode {
                    let src = self.editor_glsl_content.text();
                    if src.trim().is_empty() {
                        Err("No GLSL source to compile".to_string())
                    } else {
                        Ok(src)
                    }
                } else {
                    self.shader_graph.compile_glsl()
                };
                match glsl {
                    Ok(src) => {
                        self.editor_glsl_preview = src.clone();
                        self.log_msg("Shader compiled successfully".into());
                        if self.editor_live_preview {
                            self.send_to_daemon(&src);
                        }
                    }
                    Err(e) => {
                        self.editor_glsl_preview = format!("// Error: {}", e);
                        self.log_msg(format!("Compile error: {}", e));
                    }
                }
            }
            Message::EditorExport => {
                let glsl = if self.editor_text_mode {
                    let src = self.editor_glsl_content.text();
                    if src.trim().is_empty() {
                        Err("No GLSL source".to_string())
                    } else {
                        Ok(src)
                    }
                } else {
                    self.shader_graph.compile_glsl()
                };
                match glsl {
                    Ok(src) => {
                        let dir = dirs_output_dir();
                        std::fs::create_dir_all(&dir).ok();
                        let glsl_path = dir.join("editor_shader.glsl");
                        if let Err(e) = std::fs::write(&glsl_path, &src) {
                            self.log_msg(format!("Write failed: {}", e));
                        } else {
                            match importer::import_shadertoy_file(
                                &glsl_path,
                                "Editor Shader",
                                "Kroma Editor",
                            ) {
                                Ok(shade) => self.log_msg(format!("Exported: {}", shade.display())),
                                Err(e) => self.log_msg(format!("Export failed: {}", e)),
                            }
                        }
                    }
                    Err(e) => self.log_msg(format!("Compile error: {}", e)),
                }
            }
            Message::EditorPaletteFilter(s) => self.editor_palette_filter = s,
            Message::EditorClosePalette => self.editor_palette_open = false,
            Message::EditorToggleTextMode => {
                self.editor_text_mode = !self.editor_text_mode;
                // If switching to text mode, compile graph GLSL as starting point
                if self.editor_text_mode && self.editor_glsl_content.text().trim().is_empty() {
                    if let Ok(glsl) = self.shader_graph.compile_glsl() {
                        self.editor_glsl_content = text_editor::Content::with_text(&glsl);
                    }
                }
            }
            Message::EditorGlslSourceChanged(action) => {
                self.editor_glsl_content.perform(action);
                // Mark dirty for auto-compile debounce
                self.editor_dirty = true;
                self.editor_last_edit = std::time::Instant::now();
            }
            Message::EditorLivePreview => {
                // Send current GLSL to daemon immediately
                let src = if self.editor_text_mode {
                    self.editor_glsl_content.text()
                } else {
                    self.editor_glsl_preview.clone()
                };
                if src.is_empty() {
                    self.log_msg("Nothing to preview — compile first".into());
                } else {
                    self.send_to_daemon(&src);
                }
            }
            Message::EditorToggleLive => {
                self.editor_live_preview = !self.editor_live_preview;
                if self.editor_live_preview {
                    self.log_msg("Live preview enabled — shaders auto-sent on compile".into());
                } else {
                    self.log_msg("Live preview disabled".into());
                }
            }
            Message::EditorLoadGlsl => {
                return Task::perform(
                    async {
                        let h = rfd::AsyncFileDialog::new()
                            .set_title("Open GLSL Shader")
                            .add_filter("GLSL", &["glsl", "frag", "txt"])
                            .pick_file()
                            .await;
                        h.map(|f| f.path().to_path_buf())
                    },
                    Message::EditorLoadGlslResult,
                );
            }
            Message::EditorLoadGlslResult(Some(path)) => {
                match std::fs::read_to_string(&path) {
                    Ok(source) => {
                        self.editor_glsl_content = text_editor::Content::with_text(&source);
                        self.editor_text_mode = true;
                        self.log_msg(format!("Loaded: {}", path.display()));
                    }
                    Err(e) => self.log_msg(format!("Failed to read: {}", e)),
                }
            }
            Message::EditorLoadGlslResult(None) => {}

            // ------ Shade editing ------
            Message::ShadeNew => {
                let config = kroma_shared::types::ShadeConfig {
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
                };
                let default_frag = "// Kroma shader\nvoid mainImage(out vec4 fragColor, in vec2 fragCoord) {\n    vec2 uv = fragCoord / u_resolution;\n    fragColor = vec4(uv, 0.5 + 0.5 * sin(u_time), 1.0);\n}\n";
                self.shade_config = config.clone();
                self.shade_package = Some(kroma_shared::shade::ShadePackage {
                    config,
                    shader_source: Some(default_frag.into()),
                    preview: None,
                    assets: Vec::new(),
                });
                self.shade_shader_content = text_editor::Content::with_text(default_frag);
                self.shade_loaded_path = None;
                self.shade_selected_file = Some("config.toml".into());
                self.shade_edit_mode = "settings".into();
                self.sync_shade_toml();
                self.log_msg("New shade project created".into());
            }
            Message::ShadeOpen => {
                return Task::perform(
                    async {
                        let h = rfd::AsyncFileDialog::new()
                            .set_title("Open .shade or config.toml")
                            .add_filter("Shade/TOML", &["shade", "toml"])
                            .pick_file()
                            .await;
                        h.map(|f| f.path().to_path_buf())
                    },
                    Message::ShadeOpenResult,
                );
            }
            Message::ShadeOpenResult(Some(path)) => {
                if path.extension().map(|e| e == "shade").unwrap_or(false) {
                    // Load full ShadePackage from .shade ZIP
                    match kroma_shared::shade::ShadePackage::load(&path) {
                        Ok(pkg) => {
                            self.shade_config = pkg.config.clone();
                            self.shade_shader_content = text_editor::Content::with_text(
                                pkg.shader_source.as_deref().unwrap_or(""),
                            );
                            self.shade_package = Some(pkg);
                            self.shade_loaded_path = Some(path.clone());
                            self.shade_selected_file = Some("config.toml".into());
                            self.shade_edit_mode = "settings".into();
                            self.sync_shade_toml();
                            self.log_msg(format!("Opened: {}", path.display()));
                        }
                        Err(e) => self.log_msg(format!("Failed to open shade: {}", e)),
                    }
                } else {
                    // Direct TOML file
                    if let Some(toml_str) = std::fs::read_to_string(&path).ok() {
                        match toml::from_str::<kroma_shared::types::ShadeConfig>(&toml_str) {
                            Ok(config) => {
                                self.shade_config = config.clone();
                                self.shade_package = Some(kroma_shared::shade::ShadePackage {
                                    config,
                                    shader_source: None,
                                    preview: None,
                                    assets: Vec::new(),
                                });
                                self.shade_loaded_path = Some(path.clone());
                                self.shade_selected_file = Some("config.toml".into());
                                self.shade_edit_mode = "settings".into();
                                self.sync_shade_toml();
                                self.log_msg(format!("Opened: {}", path.display()));
                            }
                            Err(e) => self.log_msg(format!("Invalid config: {}", e)),
                        }
                    }
                }
            }
            Message::ShadeOpenResult(None) => {}
            Message::ShadeSave => {
                // Sync shader source from text editor back into the package
                let shader_src = self.shade_shader_content.text();
                let pkg = kroma_shared::shade::ShadePackage {
                    config: self.shade_config.clone(),
                    shader_source: Some(shader_src),
                    preview: self.shade_package.as_ref().and_then(|p| p.preview.clone()),
                    assets: self.shade_package.as_ref().map(|p| p.assets.clone()).unwrap_or_default(),
                };
                let dir = dirs_output_dir();
                std::fs::create_dir_all(&dir).ok();
                let name = self.shade_config.meta.name.replace(' ', "_").to_lowercase();
                let shade_path = dir.join(format!("{}.shade", name));
                match pkg.save(&shade_path) {
                    Ok(()) => {
                        self.shade_package = Some(pkg);
                        self.shade_loaded_path = Some(shade_path.clone());
                        self.log_msg(format!("Saved: {}", shade_path.display()));
                    }
                    Err(e) => self.log_msg(format!("Save failed: {}", e)),
                }
            }
            Message::ShadeMetaName(s) => self.shade_config.meta.name = s,
            Message::ShadeMetaAuthor(s) => self.shade_config.meta.author = s,
            Message::ShadeMetaVersion(s) => self.shade_config.meta.version = s,
            Message::ShadeMetaDescription(s) => self.shade_config.meta.description = s,
            Message::ShadeTargetFps(s) => {
                if let Ok(v) = s.parse::<u32>() {
                    self.shade_config.rendering.target_fps = v;
                }
            }
            Message::ShadePauseOffscreen(b) => self.shade_config.rendering.pause_offscreen = b,
            Message::ShadePauseFullscreen(b) => self.shade_config.rendering.pause_fullscreen = b,
            Message::ShadeAudioEnabled(b) => self.shade_config.audio.enabled = b,
            Message::ShadeAudioSource(s) => self.shade_config.audio.source = s,
            Message::ShadeConfigToml(action) => self.shade_config_toml.perform(action),
            Message::ShadeAddUniform => {
                if !self.shade_new_uniform_name.is_empty() {
                    self.shade_config.uniforms.insert(
                        self.shade_new_uniform_name.clone(),
                        kroma_shared::types::UniformDef {
                            ty: self.shade_new_uniform_type.clone(),
                            min: Some(0.0),
                            max: Some(1.0),
                            default: Some(toml::Value::Float(0.5)),
                        },
                    );
                    self.shade_new_uniform_name.clear();
                    self.sync_shade_toml();
                }
            }
            Message::ShadeRemoveUniform(name) => {
                self.shade_config.uniforms.remove(&name);
                self.sync_shade_toml();
            }
            Message::ShadeNewUniformName(s) => self.shade_new_uniform_name = s,
            Message::ShadeNewUniformType(s) => self.shade_new_uniform_type = s,
            Message::ShadeAddTexture => {
                if !self.shade_new_texture_name.is_empty() {
                    self.shade_config.textures.insert(
                        self.shade_new_texture_name.clone(),
                        kroma_shared::types::TextureDef {
                            ty: self.shade_new_texture_type.clone(),
                            source: None,
                            looping: false,
                            filter: Default::default(),
                            wrap: Default::default(),
                            binding: None,
                        },
                    );
                    self.shade_new_texture_name.clear();
                    self.sync_shade_toml();
                }
            }
            Message::ShadeRemoveTexture(name) => {
                self.shade_config.textures.remove(&name);
                self.sync_shade_toml();
            }
            Message::ShadeNewTextureName(s) => self.shade_new_texture_name = s,
            Message::ShadeNewTextureType(s) => self.shade_new_texture_type = s,

            // --- Shade project file messages ---
            Message::ShadeSelectFile(file) => {
                self.shade_selected_file = Some(file);
            }
            Message::ShadeEditMode(mode) => {
                // When switching from nodes → code, auto-compile the graph
                if self.shade_edit_mode == "nodes" && mode == "code" {
                    if let Ok(glsl) = self.shade_graph.compile_glsl() {
                        self.shade_shader_content = text_editor::Content::with_text(&glsl);
                        if let Some(ref mut pkg) = self.shade_package {
                            pkg.shader_source = Some(glsl);
                        }
                    }
                }
                // When switching from code → nodes, auto-parse the GLSL
                if self.shade_edit_mode == "code" && mode == "nodes" {
                    let glsl = self.shade_shader_content.text();
                    if !glsl.trim().is_empty() {
                        eprintln!("[kroma-gui] Parsing GLSL to nodes (code→nodes switch), {} chars", glsl.len());
                        self.shade_graph = editor::parse_glsl_to_graph(&glsl);
                        let n = self.shade_graph.nodes().count();
                        let c = self.shade_graph.connections().len();
                        self.log_msg(format!("Parsed GLSL → {} nodes, {} connections", n, c));
                        self.shade_graph_canvas = editor::canvas::GraphCanvas::default();
                    }
                }
                self.shade_edit_mode = mode;
            }
            Message::ShadeShaderChanged(action) => {
                self.shade_shader_content.perform(action);
                // Sync changes back to shade_package if loaded
                if let Some(ref mut pkg) = self.shade_package {
                    pkg.shader_source = Some(self.shade_shader_content.text());
                }
            }
            Message::ShadeGraphMsg(msg) => {
                use editor::canvas::GraphMessage;
                match msg {
                    GraphMessage::NodeMoved(id, pos) => {
                        if let Some(node) = self.shade_graph.node_mut(id) {
                            node.position = pos;
                        }
                    }
                    GraphMessage::ConnectionCreated(from, to) => {
                        self.shade_graph.add_connection(from, to);
                    }
                    GraphMessage::NodeSelected(id, _selected) => {
                        for node in self.shade_graph.nodes_mut() {
                            node.selected = node.id == id;
                        }
                    }
                    GraphMessage::DeleteSelected => {
                        let to_remove: Vec<_> = self.shade_graph.nodes()
                            .filter(|n| n.selected)
                            .map(|n| n.id)
                            .collect();
                        for id in to_remove {
                            self.shade_graph.remove_node(id);
                        }
                    }
                    GraphMessage::ConnectionDeleted(id) => {
                        self.shade_graph.remove_connection(id);
                    }
                    GraphMessage::Panned(offset) => {
                        self.shade_graph_canvas.offset = offset;
                    }
                    GraphMessage::Zoomed(zoom, offset) => {
                        self.shade_graph_canvas.zoom = zoom;
                        self.shade_graph_canvas.offset = offset;
                    }
                    GraphMessage::DefaultChanged(node_id, port_idx, new_val) => {
                        if let Some(node) = self.shade_graph.node_mut(node_id) {
                            if port_idx < node.defaults.len() {
                                node.defaults[port_idx] = new_val;
                            }
                        }
                    }
                    GraphMessage::OpenPalette(_pos) => {
                        // Ignored for now — palette is always visible
                    }
                }
            }
            Message::ShadeAddNode(kind) => {
                self.shade_graph.add_node(kind, [300.0, 250.0]);
            }
            Message::ShadeCompileGraph => {
                match self.shade_graph.compile_glsl() {
                    Ok(glsl) => {
                        self.shade_shader_content = text_editor::Content::with_text(&glsl);
                        if let Some(ref mut pkg) = self.shade_package {
                            pkg.shader_source = Some(glsl);
                        }
                        self.log_msg("Node graph compiled to GLSL".into());
                    }
                    Err(e) => self.log_msg(format!("Graph compile error: {}", e)),
                }
            }
            Message::ShadeParseToNodes => {
                let glsl = self.shade_shader_content.text();
                if glsl.trim().is_empty() {
                    self.log_msg("No GLSL source to parse into nodes".into());
                } else {
                    eprintln!("[kroma-gui] ShadeParseToNodes: parsing {} chars", glsl.len());
                    self.shade_graph = editor::parse_glsl_to_graph(&glsl);
                    self.shade_graph_canvas = editor::canvas::GraphCanvas::default();
                    let n = self.shade_graph.nodes().count();
                    let c = self.shade_graph.connections().len();
                    self.log_msg(format!("GLSL parsed → {} nodes, {} connections", n, c));
                }
            }
            Message::ShadeSendToDaemon => {
                let src = self.shade_shader_content.text();
                if src.trim().is_empty() {
                    self.log_msg("No shader source to send".into());
                } else {
                    self.send_to_daemon(&src);
                }
            }
            Message::ShadeAddAsset => {
                return Task::perform(
                    async {
                        let h = rfd::AsyncFileDialog::new()
                            .set_title("Add Asset File")
                            .pick_file()
                            .await;
                        h.map(|f| f.path().to_path_buf())
                    },
                    Message::ShadeAddAssetResult,
                );
            }
            Message::ShadeAddAssetResult(Some(path)) => {
                if let Ok(data) = std::fs::read(&path) {
                    let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    let asset_path = format!("assets/{}", filename);
                    if let Some(ref mut pkg) = self.shade_package {
                        pkg.assets.retain(|(n, _)| n != &asset_path);
                        pkg.assets.push((asset_path.clone(), data));
                        self.log_msg(format!("Added asset: {}", asset_path));
                    } else {
                        self.log_msg("No shade package loaded — open or create one first".into());
                    }
                }
            }
            Message::ShadeAddAssetResult(None) => {}
            Message::ShadeRemoveAsset(name) => {
                if let Some(ref mut pkg) = self.shade_package {
                    pkg.assets.retain(|(n, _)| n != &name);
                    self.log_msg(format!("Removed asset: {}", name));
                }
            }
            Message::ShadeAddGlslFile => {
                return Task::perform(
                    async {
                        let h = rfd::AsyncFileDialog::new()
                            .set_title("Add GLSL File")
                            .add_filter("GLSL", &["glsl", "frag"])
                            .pick_file()
                            .await;
                        h.map(|f| f.path().to_path_buf())
                    },
                    Message::ShadeAddGlslFileResult,
                );
            }
            Message::ShadeAddGlslFileResult(Some(path)) => {
                if let Ok(data) = std::fs::read(&path) {
                    let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    let asset_path = format!("assets/{}", filename);
                    if let Some(ref mut pkg) = self.shade_package {
                        pkg.assets.retain(|(n, _)| n != &asset_path);
                        pkg.assets.push((asset_path.clone(), data));
                        self.log_msg(format!("Added GLSL asset: {}", asset_path));
                    } else {
                        self.log_msg("No shade package loaded — open or create one first".into());
                    }
                }
            }
            Message::ShadeAddGlslFileResult(None) => {}
        }
        Task::none()
    }

    /// Sync the shade_config_toml text editor content from shade_config.
    fn sync_shade_toml(&mut self) {
        if let Ok(toml_str) = toml::to_string_pretty(&self.shade_config) {
            self.shade_config_toml = text_editor::Content::with_text(&toml_str);
        }
    }

    // =======================================================================
    // VIEW — top-level layout: sidebar + content area
    // =======================================================================

    fn view(&self) -> Element<'_, Message> {
        let sidebar = self.view_sidebar();
        let content = match self.active_tab {
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

    // -----------------------------------------------------------------------
    // Sidebar
    // -----------------------------------------------------------------------

    fn view_sidebar(&self) -> Element<'_, Message> {
        let brand = container(
            column![
                text("KROMA").size(22),
                text("Wallpaper Engine").size(11),
            ]
            .spacing(2),
        )
        .padding(Padding { top: 20.0, right: 20.0, bottom: 12.0, left: 20.0 });

        let nav = column![
            self.nav_btn("Dashboard", Tab::Dashboard),
            self.nav_btn("Import", Tab::Import),
            self.nav_btn("Editor", Tab::Editor),
            self.nav_btn("Shade", Tab::ShadeEdit),
            self.nav_btn("Settings", Tab::Settings),
        ]
        .spacing(2)
        .padding(Padding::from([4, 8]));

        let dot_color = if self.daemon_connected {
            color!(0x4ade80)
        } else {
            color!(0xf87171)
        };
        let label = if self.daemon_connected {
            "Daemon Online"
        } else {
            "Daemon Offline"
        };

        let status_badge = container(
            row![
                text("\u{25CF}").size(14).color(dot_color),
                text(label).size(12),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([12, 20]));

        container(
            column![
                brand,
                horizontal_rule(1),
                nav,
                Space::with_height(Fill),
                horizontal_rule(1),
                status_badge,
            ]
            .height(Fill),
        )
        .width(200)
        .height(Fill)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                ..Default::default()
            }
        })
        .into()
    }

    fn nav_btn<'a>(&self, label: &'a str, tab: Tab) -> Element<'a, Message> {
        let active = self.active_tab == tab;
        let btn = button(text(label).size(14))
            .width(Fill)
            .padding(Padding::from([10, 16]))
            .on_press(Message::TabSelected(tab));

        if active {
            btn.style(|theme: &Theme, status| {
                let p = theme.extended_palette();
                match status {
                    button::Status::Active => button::Style {
                        background: Some(p.primary.base.color.into()),
                        text_color: p.primary.base.text,
                        border: Border::default().rounded(6),
                        ..Default::default()
                    },
                    _ => button::primary(theme, status),
                }
            })
            .into()
        } else {
            btn.style(|theme: &Theme, status| {
                let p = theme.extended_palette();
                match status {
                    button::Status::Active => button::Style {
                        background: None,
                        text_color: p.background.base.text,
                        border: Border::default().rounded(6),
                        ..Default::default()
                    },
                    button::Status::Hovered => button::Style {
                        background: Some(p.background.weak.color.into()),
                        text_color: p.background.base.text,
                        border: Border::default().rounded(6),
                        ..Default::default()
                    },
                    _ => button::secondary(theme, status),
                }
            })
            .into()
        }
    }

    // -----------------------------------------------------------------------
    // Dashboard tab
    // -----------------------------------------------------------------------

    fn view_dashboard(&self) -> Element<'_, Message> {
        let header = row![
            column![
                text("Dashboard").size(24),
                text("Monitor and control your wallpaper").size(13),
            ]
            .spacing(4),
            horizontal_space(),
            self.btn_secondary("Refresh", Message::RefreshStatus),
        ]
        .align_y(iced::Alignment::Center);

        // Performance card
        let target = if self.target_fps > 0 { self.target_fps as f32 } else { 60.0 };
        let fps_pct = (self.fps / target * 100.0).min(100.0);
        let fps_card = self.card(
            "Performance",
            column![
                row![
                    text(format!("{:.1}", self.fps)).size(36),
                    column![
                        text("FPS").size(14),
                        text(format!("/ {:.0} target", target)).size(10),
                    ]
                    .spacing(2),
                ]
                .spacing(6)
                .align_y(iced::Alignment::End),
                progress_bar(0.0..=target, self.fps).height(6),
                text(format!("{:.0}% of target", fps_pct)).size(10),
            ]
            .spacing(8),
        );

        // Status card
        let status_card = self.card(
            "Status",
            column![
                self.info_row(
                    "Daemon",
                    if self.daemon_connected {
                        "Connected"
                    } else {
                        "Disconnected"
                    },
                ),
                self.info_row(
                    "State",
                    if self.paused { "Paused" } else { "Running" },
                ),
                self.info_row("Info", &self.status_text),
            ]
            .spacing(6),
        );

        let stats = row![fps_card, status_card].spacing(16);

        // Active shade card
        let shade_card = if let Some(ref shade) = self.loaded_shade {
            let short = shade.rsplit('/').next().unwrap_or(shade);
            self.card(
                "Active Wallpaper",
                row![
                    column![text(short).size(16), text(shade).size(11)]
                        .spacing(2)
                        .width(Fill),
                    self.btn_danger("Unload", Message::UnloadShadeClicked),
                ]
                .align_y(iced::Alignment::Center),
            )
        } else {
            self.card(
                "Active Wallpaper",
                column![
                    text("No wallpaper loaded").size(14),
                    text("Load a .shade package to get started").size(12),
                ]
                .spacing(4),
            )
        };

        // Controls card
        let pr_btn = if self.paused {
            self.btn_primary("Resume", Message::ResumeClicked)
        } else {
            self.btn_secondary("Pause", Message::PauseClicked)
        };
        let controls = self.card(
            "Controls",
            row![
                self.btn_primary("Load Shade", Message::LoadShadeClicked),
                pr_btn,
                self.btn_danger("Shutdown Daemon", Message::ShutdownClicked),
            ]
            .spacing(8),
        );

        // Event log card
        let log_content: Element<Message> = if self.log_messages.is_empty() {
            text("No events yet.").size(12).into()
        } else {
            let items: Vec<Element<Message>> = self
                .log_messages
                .iter()
                .rev()
                .take(15)
                .map(|m| text(m).size(11).into())
                .collect();
            column(items).spacing(3).into()
        };
        let log = self.card(
            "Event Log",
            scrollable(log_content).height(Length::Fixed(120.0)),
        );

        scrollable(
            column![header, stats, shade_card, controls, log].spacing(16),
        )
        .into()
    }

    // -----------------------------------------------------------------------
    // Import tab
    // -----------------------------------------------------------------------

    fn view_import(&self) -> Element<'_, Message> {
        let header = column![
            text("Import Shaders").size(24),
            text("Import from local files or download from Shadertoy").size(13),
        ]
        .spacing(4);

        let local_card = self.card(
            "Import Local File",
            column![
                text("Select a Shadertoy-compatible GLSL file to convert into a .shade package.")
                    .size(12),
                Space::with_height(4),
                row![
                    text_input("Path to .glsl file...", &self.import_path)
                        .on_input(Message::ImportPathChanged)
                        .width(Fill)
                        .padding(10),
                    self.btn_secondary("Browse", Message::BrowseImportClicked),
                ]
                .spacing(8),
                row![
                    text_input("Shader name", &self.import_name)
                        .on_input(Message::ImportNameChanged)
                        .padding(10),
                    text_input("Author", &self.import_author)
                        .on_input(Message::ImportAuthorChanged)
                        .padding(10),
                    self.btn_primary("Import", Message::ImportClicked),
                ]
                .spacing(8),
                text(&self.import_result).size(12),
            ]
            .spacing(10),
        );

        let dl_btn = if self.downloading {
            self.btn_secondary("Downloading...", Message::DownloadShadertoyClicked)
        } else {
            self.btn_primary("Download", Message::DownloadShadertoyClicked)
        };

        let download_card = self.card(
            "Download from Shadertoy",
            column![
                text(
                    "Paste a Shadertoy URL or shader ID to download and convert automatically."
                )
                .size(12),
                Space::with_height(4),
                row![
                    text_input(
                        "https://www.shadertoy.com/view/XsXXDn  or  XsXXDn",
                        &self.shadertoy_url,
                    )
                    .on_input(Message::ShadertoyUrlChanged)
                    .width(Fill)
                    .padding(10),
                    dl_btn,
                ]
                .spacing(8),
                text(&self.download_result).size(12),
            ]
            .spacing(10),
        );

        scrollable(
            column![header, local_card, download_card].spacing(16),
        )
        .into()
    }

    // -----------------------------------------------------------------------
    // Editor tab
    // -----------------------------------------------------------------------

    fn view_editor(&self) -> Element<'_, Message> {
        // Mode toggle labels
        let mode_label = if self.editor_text_mode { "GLSL Text" } else { "Node Graph" };
        let live_label = if self.editor_live_preview { "Live: ON" } else { "Live: OFF" };

        // Bottom toolbar
        let toolbar = container(
            row![
                self.btn_secondary(
                    if self.editor_text_mode { "Switch to Nodes" } else { "Switch to GLSL" },
                    Message::EditorToggleTextMode,
                ),
                self.btn_secondary("Open GLSL File", Message::EditorLoadGlsl),
                self.btn_primary("Compile", Message::EditorCompile),
                self.btn_primary("Send to Daemon", Message::EditorLivePreview),
                self.btn_primary("Export .shade", Message::EditorExport),
                if self.editor_live_preview {
                    self.btn_danger(live_label, Message::EditorToggleLive)
                } else {
                    self.btn_secondary(live_label, Message::EditorToggleLive)
                },
                horizontal_space(),
                text(format!("Mode: {}", mode_label)).size(12),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([8, 12]))
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                ..Default::default()
            }
        });

        // Right panel: GLSL preview
        let preview = if self.editor_glsl_preview.is_empty() {
            column![
                text("GLSL Output").size(13),
                text("Click 'Compile' to see output").size(11),
            ]
            .spacing(4)
        } else {
            column![
                text("GLSL Output").size(13),
                scrollable(text(&self.editor_glsl_preview).size(10))
                    .height(Length::Fill),
            ]
            .spacing(4)
        };
        let preview_panel = container(preview)
            .width(220)
            .height(Fill)
            .padding(8)
            .style(|theme: &Theme| {
                let p = theme.extended_palette();
                container::Style {
                    background: Some(p.background.strong.color.into()),
                    ..Default::default()
                }
            });

        // Compile error/warning panel (shown between editor and toolbar)
        let error_panel: Option<Element<'_, Message>> = if !self.compile_errors.is_empty() {
            let mut error_col = column![].spacing(2);
            for err in &self.compile_errors {
                let loc = match (err.line, err.column) {
                    (Some(l), Some(c)) => format!("line {}:{}: ", l, c),
                    (Some(l), None) => format!("line {}: ", l),
                    _ => String::new(),
                };
                error_col = error_col.push(
                    text(format!("  {} {}{}", "\u{2716}", loc, err.message))
                        .size(11)
                        .color(iced::Color::from_rgb(1.0, 0.4, 0.4)),
                );
            }
            for w in &self.compile_warnings {
                error_col = error_col.push(
                    text(format!("  \u{26A0} {}", w))
                        .size(11)
                        .color(iced::Color::from_rgb(1.0, 0.8, 0.3)),
                );
            }
            Some(
                container(
                    column![
                        text("Compilation Errors").size(12).color(
                            iced::Color::from_rgb(1.0, 0.4, 0.4)
                        ),
                        scrollable(error_col).height(Length::Shrink),
                    ]
                    .spacing(4),
                )
                .padding(Padding::from([6, 12]))
                .max_height(120)
                .width(Fill)
                .style(|theme: &Theme| {
                    let _p = theme.extended_palette();
                    container::Style {
                        background: Some(iced::Background::Color(
                            iced::Color::from_rgba(0.3, 0.05, 0.05, 0.8),
                        )),
                        border: iced::Border {
                            color: iced::Color::from_rgb(0.6, 0.15, 0.15),
                            width: 1.0,
                            radius: 0.into(),
                        },
                        ..Default::default()
                    }
                })
                .into(),
            )
        } else if self.compile_success == Some(true) && !self.compile_warnings.is_empty() {
            let mut warn_col = column![].spacing(2);
            for w in &self.compile_warnings {
                warn_col = warn_col.push(
                    text(format!("  \u{26A0} {}", w))
                        .size(11)
                        .color(iced::Color::from_rgb(1.0, 0.8, 0.3)),
                );
            }
            Some(
                container(warn_col)
                    .padding(Padding::from([4, 12]))
                    .max_height(80)
                    .width(Fill)
                    .style(|_theme: &Theme| container::Style {
                        background: Some(iced::Background::Color(
                            iced::Color::from_rgba(0.3, 0.25, 0.0, 0.6),
                        )),
                        ..Default::default()
                    })
                    .into(),
            )
        } else {
            None
        };

        if self.editor_text_mode {
            // ---- TEXT EDITOR MODE ----
            let text_editor = container(
                scrollable(
                    iced::widget::text_editor(&self.editor_glsl_content)
                        .on_action(Message::EditorGlslSourceChanged)
                )
                .width(Fill)
                .height(Fill),
            )
            .width(Fill)
            .height(Fill)
            .padding(4);

            let mut layout = column![
                row![text_editor, preview_panel].width(Fill).height(Fill),
            ];
            if let Some(ep) = error_panel {
                layout = layout.push(ep);
            }
            layout = layout.push(toolbar);
            layout.width(Fill).height(Fill).into()
        } else {
            // ---- NODE GRAPH MODE ----
            let palette = self.view_node_palette();
            let canvas = editor::canvas::graph_canvas(&self.shader_graph, &self.graph_canvas)
                .map(Message::EditorGraph);

            let mut layout = column![
                row![palette, canvas, preview_panel].width(Fill).height(Fill),
            ];
            if let Some(ep) = error_panel {
                layout = layout.push(ep);
            }
            layout = layout.push(toolbar);
            layout.width(Fill).height(Fill).into()
        }
    }

    /// Node palette — categories of nodes user can add to the graph.
    fn view_node_palette(&self) -> Element<'_, Message> {
        let mut palette_col = column![text("Node Palette").size(14)].spacing(6);

        for (category, kinds) in editor::palette() {
            let mut cat_col = column![
                text(category).size(12),
            ]
            .spacing(2);

            for kind in kinds {
                let label = kind.label();
                let btn = button(text(label).size(11))
                    .width(Fill)
                    .padding(Padding::from([4, 8]))
                    .on_press(Message::EditorAddNode(kind))
                    .style(|theme: &Theme, status| {
                        let p = theme.extended_palette();
                        match status {
                            button::Status::Hovered => button::Style {
                                background: Some(p.primary.weak.color.into()),
                                text_color: p.primary.weak.text,
                                border: iced::Border::default().rounded(4),
                                ..Default::default()
                            },
                            _ => button::Style {
                                background: None,
                                text_color: p.background.base.text,
                                border: iced::Border::default().rounded(4),
                                ..Default::default()
                            },
                        }
                    });
                cat_col = cat_col.push(btn);
            }
            palette_col = palette_col.push(cat_col);
            palette_col = palette_col.push(Space::with_height(4));
        }

        container(
            scrollable(palette_col).height(Fill),
        )
        .width(170)
        .height(Fill)
        .padding(8)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                ..Default::default()
            }
        })
        .into()
    }

    // -----------------------------------------------------------------------
    // -----------------------------------------------------------------------
    // Shade Package Editor tab
    // -----------------------------------------------------------------------

    fn view_shade_edit(&self) -> Element<'_, Message> {
        // Header toolbar
        let header = container(
            row![
                text("Shade Package Editor").size(18),
                horizontal_space(),
                self.btn_secondary("New", Message::ShadeNew),
                self.btn_secondary("Open", Message::ShadeOpen),
                self.btn_primary("Save .shade", Message::ShadeSave),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([8, 12]))
        .width(Fill)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                ..Default::default()
            }
        });

        let file_tree = self.view_shade_file_tree();
        let editor_panel = self.view_shade_editor_panel();

        column![
            header,
            row![file_tree, editor_panel].width(Fill).height(Fill),
        ]
        .width(Fill)
        .height(Fill)
        .into()
    }

    /// Left sidebar file tree for the shade package editor.
    fn view_shade_file_tree(&self) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = Vec::new();
        items.push(text(format!("\u{1F4E6} {}", self.shade_config.meta.name)).size(14).into());
        items.push(Space::with_height(4).into());
        items.push(self.shade_tree_btn("\u{1F4C4} config.toml", "config.toml"));
        items.push(self.shade_tree_btn("\u{1F4C4} shader.frag", "shader.frag"));
        items.push(Space::with_height(6).into());
        items.push(text("\u{1F4C1} assets/").size(12).into());
        if let Some(ref pkg) = self.shade_package {
            let mut sorted: Vec<&(String, Vec<u8>)> = pkg.assets.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, data) in sorted {
                let short = name.rsplit('/').next().unwrap_or(name);
                let icon = shade_file_icon(short);
                let sz = if data.len() > 1_048_576 {
                    format!(" ({:.1}MB)", data.len() as f64 / 1_048_576.0)
                } else if data.len() > 1024 {
                    format!(" ({:.1}KB)", data.len() as f64 / 1024.0)
                } else {
                    format!(" ({}B)", data.len())
                };
                let is_sel = self.shade_selected_file.as_deref() == Some(name.as_str());
                let name_c = name.clone();
                let remove_c = name.clone();
                items.push(
                    row![
                        button(text(format!("  {} {}{}", icon, short, sz)).size(11))
                            .width(Fill)
                            .padding(Padding::from([3, 6]))
                            .on_press(Message::ShadeSelectFile(name_c))
                            .style(move |theme: &Theme, status| {
                                let p = theme.extended_palette();
                                if is_sel {
                                    button::Style {
                                        background: Some(p.primary.weak.color.into()),
                                        text_color: p.primary.weak.text,
                                        border: Border::default().rounded(4),
                                        ..Default::default()
                                    }
                                } else {
                                    match status {
                                        button::Status::Hovered => button::Style {
                                            background: Some(p.background.weak.color.into()),
                                            text_color: p.background.base.text,
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
                                }
                            }),
                        button(text("\u{2716}").size(10))
                            .padding(Padding::from([2, 6]))
                            .on_press(Message::ShadeRemoveAsset(remove_c))
                            .style(|_theme: &Theme, _status| button::Style {
                                background: None,
                                text_color: iced::Color::from_rgb(0.8, 0.3, 0.3),
                                border: Border::default().rounded(4),
                                ..Default::default()
                            }),
                    ]
                    .spacing(2)
                    .align_y(iced::Alignment::Center)
                    .into(),
                );
            }
        }
        items.push(Space::with_height(8).into());
        items.push(
            row![
                self.btn_secondary("+ Asset", Message::ShadeAddAsset),
                self.btn_secondary("+ GLSL", Message::ShadeAddGlslFile),
            ]
            .spacing(4)
            .into(),
        );
        container(
            scrollable(column(items).spacing(2)).height(Fill),
        )
        .width(200)
        .height(Fill)
        .padding(8)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                border: Border {
                    color: iced::Color { a: 0.1, ..p.background.base.text },
                    width: 1.0,
                    radius: 0.into(),
                },
                ..Default::default()
            }
        })
        .into()
    }

    /// File tree button for core shade files.
    fn shade_tree_btn<'a>(&self, label: &'a str, file_id: &'a str) -> Element<'a, Message> {
        let is_sel = self.shade_selected_file.as_deref() == Some(file_id);
        let id_owned = file_id.to_string();
        button(text(label).size(12))
            .width(Fill)
            .padding(Padding::from([4, 8]))
            .on_press(Message::ShadeSelectFile(id_owned))
            .style(move |theme: &Theme, status| {
                let p = theme.extended_palette();
                if is_sel {
                    button::Style {
                        background: Some(p.primary.weak.color.into()),
                        text_color: p.primary.weak.text,
                        border: Border::default().rounded(4),
                        ..Default::default()
                    }
                } else {
                    match status {
                        button::Status::Hovered => button::Style {
                            background: Some(p.background.weak.color.into()),
                            text_color: p.background.base.text,
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
                }
            })
            .into()
    }

    /// Right panel: context-sensitive editor for the selected file.
    fn view_shade_editor_panel(&self) -> Element<'_, Message> {
        let selected = self.shade_selected_file.as_deref().unwrap_or("config.toml");
        let mode_bar = self.view_shade_mode_bar(selected);
        let editor_content: Element<'_, Message> = match selected {
            "config.toml" => {
                if self.shade_edit_mode == "code" {
                    container(
                        scrollable(
                            iced::widget::text_editor(&self.shade_config_toml)
                                .on_action(Message::ShadeConfigToml)
                        )
                        .width(Fill)
                        .height(Fill),
                    )
                    .width(Fill)
                    .height(Fill)
                    .padding(4)
                    .into()
                } else {
                    self.view_shade_settings_form()
                }
            }
            "shader.frag" => {
                if self.shade_edit_mode == "preview" {
                    let src = self.shade_shader_content.text();
                    let display = if src.is_empty() { "// No shader source".to_string() } else { src };
                    container(
                        scrollable(text(display).size(12))
                            .width(Fill)
                            .height(Fill),
                    )
                    .width(Fill)
                    .height(Fill)
                    .padding(8)
                    .into()
                } else if self.shade_edit_mode == "nodes" {
                    // Node graph editor for shader.frag
                    let canvas = editor::canvas::graph_canvas(&self.shade_graph, &self.shade_graph_canvas)
                        .map(Message::ShadeGraphMsg);
                    let palette = self.view_shade_node_palette();
                    row![palette, canvas]
                        .width(Fill)
                        .height(Fill)
                        .into()
                } else {
                    container(
                        scrollable(
                            iced::widget::text_editor(&self.shade_shader_content)
                                .on_action(Message::ShadeShaderChanged)
                        )
                        .width(Fill)
                        .height(Fill),
                    )
                    .width(Fill)
                    .height(Fill)
                    .padding(4)
                    .into()
                }
            }
            other => self.view_shade_asset(other),
        };
        container(
            column![mode_bar, editor_content]
                .spacing(0)
                .width(Fill)
                .height(Fill),
        )
        .width(Fill)
        .height(Fill)
        .into()
    }

    /// Mode toggle bar for shade editor.
    fn view_shade_mode_bar(&self, selected: &str) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = Vec::new();
        items.push(text(format!("Editing: {}", selected)).size(13).into());
        items.push(horizontal_space().into());
        match selected {
            "config.toml" => {
                if self.shade_edit_mode != "code" {
                    items.push(self.btn_primary("Settings", Message::ShadeEditMode("settings".into())));
                    items.push(self.btn_secondary("Code", Message::ShadeEditMode("code".into())));
                } else {
                    items.push(self.btn_secondary("Settings", Message::ShadeEditMode("settings".into())));
                    items.push(self.btn_primary("Code", Message::ShadeEditMode("code".into())));
                }
            }
            "shader.frag" => {
                let modes = vec![("Code", "code"), ("Nodes", "nodes"), ("Preview", "preview")];
                for (label, mode) in modes {
                    if self.shade_edit_mode == mode {
                        items.push(self.btn_primary(label, Message::ShadeEditMode(mode.into())));
                    } else {
                        items.push(self.btn_secondary(label, Message::ShadeEditMode(mode.into())));
                    }
                }
                items.push(Space::with_width(8).into());
                if self.shade_edit_mode == "nodes" {
                    items.push(self.btn_secondary("Compile \u{2192} GLSL", Message::ShadeCompileGraph));
                    items.push(self.btn_secondary("\u{2190} Parse GLSL", Message::ShadeParseToNodes));
                }
                items.push(self.btn_primary("Send to Daemon", Message::ShadeSendToDaemon));
            }
            _ => {
                items.push(text("Asset Viewer").size(12).into());
            }
        }
        container(
            row(items).spacing(6).align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([6, 12]))
        .width(Fill)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.weak.color.into()),
                ..Default::default()
            }
        })
        .into()
    }

    /// Mini node palette for the shade editor's node graph mode.
    fn view_shade_node_palette(&self) -> Element<'_, Message> {
        let mut col = column![text("Nodes").size(13)].spacing(4);
        for (category, kinds) in editor::palette() {
            col = col.push(text(category).size(11).color(iced::Color::from_rgb(0.6, 0.6, 0.6)));
            for kind in kinds {
                let btn = button(text(kind.label()).size(10))
                    .width(Fill)
                    .padding(Padding::from([3, 6]))
                    .on_press(Message::ShadeAddNode(kind))
                    .style(|theme: &Theme, status| {
                        let p = theme.extended_palette();
                        match status {
                            button::Status::Hovered => button::Style {
                                background: Some(p.primary.weak.color.into()),
                                text_color: p.primary.weak.text,
                                border: iced::Border::default().rounded(3),
                                ..Default::default()
                            },
                            _ => button::Style {
                                background: None,
                                text_color: p.background.base.text,
                                border: iced::Border::default().rounded(3),
                                ..Default::default()
                            },
                        }
                    });
                col = col.push(btn);
            }
        }
        container(scrollable(col).height(Fill))
            .width(140)
            .height(Fill)
            .padding(6)
            .style(|theme: &Theme| {
                let p = theme.extended_palette();
                container::Style {
                    background: Some(p.background.strong.color.into()),
                    ..Default::default()
                }
            })
            .into()
    }

    /// View for an asset file in the shade editor.
    fn view_shade_asset(&self, asset_name: &str) -> Element<'_, Message> {
        let short = asset_name.rsplit('/').next().unwrap_or(asset_name);
        let ext = short.rsplit('.').next().unwrap_or("").to_lowercase();
        if let Some(ref pkg) = self.shade_package {
            if let Some((_name, data)) = pkg.assets.iter().find(|(n, _)| n == asset_name) {
                let size = data.len();
                let size_str = if size > 1_048_576 {
                    format!("{:.1} MB", size as f64 / 1_048_576.0)
                } else if size > 1024 {
                    format!("{:.1} KB", size as f64 / 1024.0)
                } else {
                    format!("{} bytes", size)
                };
                let content: Element<'_, Message> = match ext.as_str() {
                    "glsl" | "frag" | "vert" => {
                        let source = String::from_utf8_lossy(data);
                        scrollable(text(source.to_string()).size(12))
                            .width(Fill)
                            .height(Fill)
                            .into()
                    }
                    "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" => {
                        column![
                            text(format!("\u{1F5BC} Image Asset: {}", short)).size(16),
                            text(format!("Size: {}", size_str)).size(13),
                            Space::with_height(8),
                            text("Image preview not available in editor").size(11),
                        ]
                        .spacing(6)
                        .into()
                    }
                    "mp4" | "webm" | "avi" | "mkv" => {
                        column![
                            text(format!("\u{1F3AC} Video Asset: {}", short)).size(16),
                            text(format!("Size: {}", size_str)).size(13),
                            Space::with_height(8),
                            text("Video preview not available in editor").size(11),
                        ]
                        .spacing(6)
                        .into()
                    }
                    "ttf" | "otf" | "woff" | "woff2" => {
                        column![
                            text(format!("\u{1F524} Font Asset: {}", short)).size(16),
                            text(format!("Size: {}", size_str)).size(13),
                            Space::with_height(8),
                            text("Font preview not available in editor").size(11),
                            text("Use this font in textures with type = \"font\"").size(11),
                        ]
                        .spacing(6)
                        .into()
                    }
                    "mp3" | "wav" | "ogg" | "flac" => {
                        column![
                            text(format!("\u{1F3B5} Audio Asset: {}", short)).size(16),
                            text(format!("Size: {}", size_str)).size(13),
                        ]
                        .spacing(6)
                        .into()
                    }
                    _ => {
                        column![
                            text(format!("\u{1F4C4} File: {}", short)).size(16),
                            text(format!("Size: {}", size_str)).size(13),
                        ]
                        .spacing(6)
                        .into()
                    }
                };
                return container(content)
                    .width(Fill)
                    .height(Fill)
                    .padding(16)
                    .into();
            }
        }
        container(
            text(format!("Asset not found: {}", asset_name)).size(13),
        )
        .width(Fill)
        .height(Fill)
        .padding(16)
        .into()
    }

    /// Settings form for config.toml (extracted from original shade editor).
    fn view_shade_settings_form(&self) -> Element<'_, Message> {
        // Meta section
        let meta = self.card(
            "Metadata",
            column![
                text_input("Shader name", &self.shade_config.meta.name)
                    .on_input(Message::ShadeMetaName),
                text_input("Author", &self.shade_config.meta.author)
                    .on_input(Message::ShadeMetaAuthor),
                row![
                    text_input("Version", &self.shade_config.meta.version)
                        .on_input(Message::ShadeMetaVersion)
                        .width(100),
                    text_input("Description", &self.shade_config.meta.description)
                        .on_input(Message::ShadeMetaDescription),
                ]
                .spacing(8),
            ]
            .spacing(8),
        );

        // Rendering section
        let render = self.card(
            "Rendering",
            column![
                row![
                    text("Target FPS:").size(13).width(120),
                    text_input("0 = monitor rate", &self.shade_config.rendering.target_fps.to_string())
                        .on_input(Message::ShadeTargetFps)
                        .width(120),
                    text("(0 = match monitor)").size(11),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
                row![
                    iced::widget::checkbox("Pause when offscreen", self.shade_config.rendering.pause_offscreen)
                        .on_toggle(Message::ShadePauseOffscreen),
                    iced::widget::checkbox("Pause on fullscreen app", self.shade_config.rendering.pause_fullscreen)
                        .on_toggle(Message::ShadePauseFullscreen),
                ]
                .spacing(16),
            ]
            .spacing(8),
        );

        // Audio section
        let audio = self.card(
            "Audio",
            column![
                iced::widget::checkbox("Enable audio capture", self.shade_config.audio.enabled)
                    .on_toggle(Message::ShadeAudioEnabled),
                row![
                    text("Source:").size(13).width(80),
                    text_input("desktop / microphone / device name", &self.shade_config.audio.source)
                        .on_input(Message::ShadeAudioSource),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
                text("Use 'desktop' for system audio, 'microphone' for mic, or a device name.").size(11),
            ]
            .spacing(8),
        );

        // Uniforms section
        let mut uniform_items: Vec<Element<'_, Message>> = Vec::new();
        let mut uniform_keys: Vec<String> = self.shade_config.uniforms.keys().cloned().collect();
        uniform_keys.sort();
        for name in uniform_keys {
            if let Some(u) = self.shade_config.uniforms.get(&name) {
                let range_str = match (u.min, u.max) {
                    (Some(lo), Some(hi)) => format!("[{} .. {}]", lo, hi),
                    _ => String::new(),
                };
                let ty = u.ty.clone();
                let label = name.clone();
                uniform_items.push(
                    row![
                        text(label).size(13).width(140),
                        text(ty).size(12).width(60),
                        text(range_str).size(11).width(Fill),
                        button(text("Remove").size(11))
                            .on_press(Message::ShadeRemoveUniform(name))
                            .padding(Padding::from([2, 8])),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center)
                    .into(),
                );
            }
        }
        uniform_items.push(
            row![
                text_input("name", &self.shade_new_uniform_name)
                    .on_input(Message::ShadeNewUniformName)
                    .width(140),
                text_input("type", &self.shade_new_uniform_type)
                    .on_input(Message::ShadeNewUniformType)
                    .width(80),
                self.btn_secondary("Add Uniform", Message::ShadeAddUniform),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into(),
        );
        let uniforms = self.card(
            "Uniforms",
            column(uniform_items).spacing(6),
        );

        // Textures section
        let mut texture_items: Vec<Element<'_, Message>> = Vec::new();
        let mut texture_keys: Vec<String> = self.shade_config.textures.keys().cloned().collect();
        texture_keys.sort();
        for name in texture_keys {
            if let Some(t) = self.shade_config.textures.get(&name) {
                let src = t.source.clone().unwrap_or_else(|| "none".into());
                let ty = t.ty.clone();
                let label = name.clone();
                texture_items.push(
                    row![
                        text(label).size(13).width(120),
                        text(ty).size(12).width(60),
                        text(src).size(11).width(Fill),
                        button(text("Remove").size(11))
                            .on_press(Message::ShadeRemoveTexture(name))
                            .padding(Padding::from([2, 8])),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center)
                    .into(),
                );
            }
        }
        texture_items.push(
            row![
                text_input("channel name", &self.shade_new_texture_name)
                    .on_input(Message::ShadeNewTextureName)
                    .width(120),
                text_input("type (image/video/glsl/font)", &self.shade_new_texture_type)
                    .on_input(Message::ShadeNewTextureType)
                    .width(160),
                self.btn_secondary("Add Texture", Message::ShadeAddTexture),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into(),
        );
        texture_items.push(
            text("Types: image, video, glsl (programmatic texture from shader), font (font file)").size(10).into(),
        );
        if self.shade_config.textures.values().any(|t| t.ty == "glsl") {
            texture_items.push(
                text("\u{2139} GLSL textures generate data from a shader in assets/. Set source to the .glsl filename.")
                    .size(10)
                    .color(color!(0x60a5fa))
                    .into(),
            );
        }
        if self.shade_config.textures.values().any(|t| t.ty == "font") {
            texture_items.push(
                text("\u{2139} Font textures render glyphs from .ttf/.otf files in assets/. Set source to the font file.")
                    .size(10)
                    .color(color!(0xa78bfa))
                    .into(),
            );
        }
        let textures = self.card(
            "Textures",
            column(texture_items).spacing(6),
        );

        // TOML preview
        let toml_preview = self.card(
            "config.toml Preview",
            container(
                scrollable(
                    iced::widget::text_editor(&self.shade_config_toml)
                        .on_action(Message::ShadeConfigToml)
                )
                .height(200),
            )
            .width(Fill),
        );

        scrollable(
            column![
                row![meta, render].spacing(16),
                audio,
                row![uniforms, textures].spacing(16),
                toml_preview,
            ]
            .spacing(12)
            .width(Fill),
        )
        .width(Fill)
        .height(Fill)
        .into()
    }

    // -----------------------------------------------------------------------
    // Settings tab
    // -----------------------------------------------------------------------

    fn view_settings(&self) -> Element<'_, Message> {
        let header = column![
            text("Settings").size(24),
            text("Configure Kroma preferences").size(13),
        ]
        .spacing(4);

        let api_card = self.card(
            "Shadertoy API Key",
            column![
                text("Optional: provide your Shadertoy API key for reliable downloads.").size(12),
                text("Get one at: https://www.shadertoy.com/profile").size(11),
                Space::with_height(4),
                text_input("Paste Shadertoy API key...", &self.api_key)
                    .on_input(Message::ApiKeyChanged)
                    .padding(10),
                text("Key is stored for this session only. Set SHADERTOY_API_KEY for persistence.")
                    .size(11),
            ]
            .spacing(6),
        );

        let dirs_card = self.card(
            "Directories",
            column![
                self.info_row("Shader output", &format!("{}", dirs_output_dir().display())),
                self.info_row("IPC socket", "/tmp/kroma.sock"),
            ]
            .spacing(6),
        );

        let about_card = self.card(
            "About",
            column![
                text(format!(
                    "Kroma Wallpaper Engine v{}",
                    env!("CARGO_PKG_VERSION")
                ))
                .size(14),
                text("High-performance modular wallpaper engine for Linux (Wayland / Hyprland)")
                    .size(12),
                Space::with_height(4),
                self.info_row("Renderer", "wgpu (Vulkan)"),
                self.info_row("Shaders", "GLSL 450 \u{2192} WGSL (naga)"),
                self.info_row("Audio", "cpal + rustfft"),
                self.info_row("IPC", "Unix domain sockets"),
            ]
            .spacing(4),
        );

        scrollable(
            column![header, api_card, dirs_card, about_card].spacing(16),
        )
        .into()
    }

    // =======================================================================
    // Reusable UI components
    // =======================================================================

    /// A rounded card container with a title.
    fn card<'a>(
        &self,
        title: &'a str,
        content: impl Into<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        container(
            column![
                text(title).size(14),
                Space::with_height(6),
                content.into(),
            ],
        )
        .width(Fill)
        .padding(16)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.weak.color.into()),
                border: Border::default()
                    .rounded(10)
                    .width(1)
                    .color(iced::Color {
                        a: 0.1,
                        ..p.background.base.text
                    }),
                ..Default::default()
            }
        })
        .into()
    }

    /// A label: value row used in info panels.
    fn info_row<'a>(&self, label: &str, value: &str) -> Element<'a, Message> {
        row![
            text(format!("{}:", label)).size(13),
            text(value.to_string()).size(13),
        ]
        .spacing(6)
        .into()
    }

    fn btn_primary<'a>(&self, label: &'a str, msg: Message) -> Element<'a, Message> {
        button(text(label).size(13))
            .padding(Padding::from([8, 16]))
            .on_press(msg)
            .style(|theme: &Theme, status| {
                let p = theme.extended_palette();
                match status {
                    button::Status::Active => button::Style {
                        background: Some(p.primary.base.color.into()),
                        text_color: p.primary.base.text,
                        border: Border::default().rounded(6),
                        ..Default::default()
                    },
                    button::Status::Hovered => button::Style {
                        background: Some(p.primary.strong.color.into()),
                        text_color: p.primary.strong.text,
                        border: Border::default().rounded(6),
                        ..Default::default()
                    },
                    _ => button::primary(theme, status),
                }
            })
            .into()
    }

    fn btn_secondary<'a>(&self, label: &'a str, msg: Message) -> Element<'a, Message> {
        button(text(label).size(13))
            .padding(Padding::from([8, 16]))
            .on_press(msg)
            .style(|theme: &Theme, status| {
                let p = theme.extended_palette();
                match status {
                    button::Status::Active => button::Style {
                        background: Some(p.background.strong.color.into()),
                        text_color: p.background.base.text,
                        border: Border::default()
                            .rounded(6)
                            .width(1)
                            .color(iced::Color {
                                a: 0.2,
                                ..p.background.base.text
                            }),
                        ..Default::default()
                    },
                    button::Status::Hovered => button::Style {
                        background: Some(p.background.weak.color.into()),
                        text_color: p.background.base.text,
                        border: Border::default()
                            .rounded(6)
                            .width(1)
                            .color(iced::Color {
                                a: 0.3,
                                ..p.background.base.text
                            }),
                        ..Default::default()
                    },
                    _ => button::secondary(theme, status),
                }
            })
            .into()
    }

    fn btn_danger<'a>(&self, label: &'a str, msg: Message) -> Element<'a, Message> {
        button(text(label).size(13))
            .padding(Padding::from([8, 16]))
            .on_press(msg)
            .style(|_theme: &Theme, status| match status {
                button::Status::Active => button::Style {
                    background: Some(color!(0xdc2626).into()),
                    text_color: iced::Color::WHITE,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
                button::Status::Hovered => button::Style {
                    background: Some(color!(0xef4444).into()),
                    text_color: iced::Color::WHITE,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
                _ => button::Style {
                    background: Some(color!(0xdc2626).into()),
                    text_color: iced::Color::WHITE,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
            })
            .into()
    }

    fn log_msg(&mut self, msg: String) {
        let ts = chrono_now();
        self.log_messages.push(format!("[{}] {}", ts, msg));
        if self.log_messages.len() > 100 {
            self.log_messages.remove(0);
        }
    }

    /// Update live value display strings for Input/System nodes.
    fn update_live_values(&mut self) {
        let elapsed = self.app_start.elapsed().as_secs_f32();
        let frame_count = (elapsed * 60.0) as u32; // approximate

        // Query real system info from the daemon via IPC.
        // Falls back to local /proc reads if daemon is unreachable.
        let (cpu_pct, ram_pct, battery_pct, audio_level, cursor_x, cursor_y) =
            match ipc_client::query_system_info() {
                Some(kroma_shared::ipc::DaemonEvent::SystemInfo {
                    cpu_usage, ram_usage, battery, audio_level, cursor_x, cursor_y,
                }) => (cpu_usage, ram_usage, battery, audio_level, cursor_x, cursor_y),
                _ => {
                    // Daemon not reachable — fallback to local reads
                    let cpu = read_cpu_usage_fast();
                    let (used, total) = read_mem_info();
                    let ram = if total > 0 { (used as f32 / total as f32) * 100.0 } else { 0.0 };
                    (cpu, ram, read_battery_pct(), 0.0, 0.0, 0.0)
                }
            };

        // Helper: generate display value for a node kind
        let cursor_str = format!("{:.0}, {:.0}", cursor_x, cursor_y);
        let audio_str = format!("{:.2}", audio_level);

        fn value_for(
            kind: &editor::NodeKind,
            elapsed: f32,
            frame: u32,
            fps: f32,
            cpu: f32,
            ram: f32,
            battery: Option<f32>,
            cursor: &str,
            audio: &str,
        ) -> Option<String> {
            use editor::NodeKind;
            match kind {
                NodeKind::Time => Some(format!("{:.2}s", elapsed)),
                NodeKind::DeltaTime => Some(format!("{:.4}", if fps > 0.0 { 1.0 / fps } else { 0.016 })),
                NodeKind::Frame => Some(format!("{}", frame)),
                NodeKind::Resolution => Some("1920x1080".into()),
                NodeKind::Mouse => Some(cursor.to_string()),
                NodeKind::UV => Some("0..1".into()),
                NodeKind::CpuUsage => Some(format!("{:.0}%", cpu)),
                NodeKind::RamUsage => Some(format!("{:.0}%", ram)),
                NodeKind::Battery => Some(battery.map(|b| format!("{:.0}%", b)).unwrap_or_else(|| "N/A".into())),
                NodeKind::AudioLevel => Some(audio.to_string()),
                NodeKind::FloatConst => Some("0.0".into()),
                _ => None,
            }
        }

        // Editor graph
        let fps = self.fps;
        let vals: Vec<_> = self.shader_graph.nodes()
            .filter_map(|n| value_for(&n.kind, elapsed, frame_count, fps, cpu_pct, ram_pct, battery_pct, &cursor_str, &audio_str).map(|v| (n.id, v)))
            .collect();
        self.graph_canvas.live_values.clear();
        for (id, val) in vals {
            self.graph_canvas.live_values.insert(id, val);
        }

        // Shade graph
        let vals: Vec<_> = self.shade_graph.nodes()
            .filter_map(|n| value_for(&n.kind, elapsed, frame_count, fps, cpu_pct, ram_pct, battery_pct, &cursor_str, &audio_str).map(|v| (n.id, v)))
            .collect();
        self.shade_graph_canvas.live_values.clear();
        for (id, val) in vals {
            self.shade_graph_canvas.live_values.insert(id, val);
        }
    }

    /// Send GLSL source to the daemon and handle the compile result.
    fn send_to_daemon(&mut self, glsl_source: &str) {
        match ipc_client::send_live_reload(glsl_source) {
            Ok(event) => self.handle_compile_result(event),
            Err(e) => {
                self.compile_success = Some(false);
                self.compile_errors = vec![kroma_shared::ipc::CompileError {
                    message: format!("Connection failed: {}", e),
                    line: None,
                    column: None,
                }];
                self.compile_warnings.clear();
                self.log_msg(format!("Daemon error: {}", e));
            }
        }
    }

    /// Process a `DaemonEvent` compile result and update error state.
    fn handle_compile_result(&mut self, event: kroma_shared::ipc::DaemonEvent) {
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
                    // Collect log messages first to avoid borrow conflict
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn query_daemon_status() -> String {
    ipc_client::query_status().unwrap_or_else(|e| format!("Could not connect: {}", e))
}

fn do_import(path: &str, name: &str, author: &str) -> String {
    let p = PathBuf::from(path);
    if !p.exists() {
        return format!("File not found: {}", p.display());
    }
    match importer::import_shadertoy_file(&p, name, author) {
        Ok(out) => format!("Created: {}", out.display()),
        Err(e) => format!("Import failed: {}", e),
    }
}

async fn do_download(url_or_id: &str) -> String {
    let dir = dirs_output_dir();
    match importer::download_shadertoy(url_or_id, &dir, None).await {
        Ok((path, name)) => format!("Downloaded '{}': {}", name, path.display()),
        Err(e) => format!("Download failed: {}", e),
    }
}

fn dirs_output_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(d).join("kroma/shaders")
    } else if let Some(h) = std::env::var_os("HOME") {
        PathBuf::from(h).join(".local/share/kroma/shaders")
    } else {
        PathBuf::from("./shaders")
    }
}

fn shade_file_icon(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" => "\u{1F5BC}",
        "mp4" | "webm" | "avi" | "mkv" => "\u{1F3AC}",
        "mp3" | "wav" | "ogg" | "flac" => "\u{1F3B5}",
        "ttf" | "otf" | "woff" | "woff2" => "\u{1F524}",
        "glsl" | "frag" | "vert" => "\u{1F4DD}",
        _ => "\u{1F4C4}",
    }
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let s = now.as_secs() % 86400;
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

// ---------------------------------------------------------------------------
// Lightweight Linux system info readers (no sysinfo crate needed)
// ---------------------------------------------------------------------------

/// Read approximate CPU usage from /proc/loadavg (1-minute load average).
/// Returns a percentage estimate: (load1 / num_cpus) * 100, capped at 100.
fn read_cpu_usage_fast() -> f32 {
    let loadavg = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let load1: f32 = loadavg
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    // Get number of online CPUs
    let cpus = std::fs::read_to_string("/proc/cpuinfo")
        .map(|s| s.matches("processor").count() as f32)
        .unwrap_or(1.0)
        .max(1.0);

    ((load1 / cpus) * 100.0).min(100.0)
}

/// Read used and total memory from /proc/meminfo (in MB).
/// Returns (used_mb, total_mb).
fn read_mem_info() -> (u64, u64) {
    let content = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total_kb = 0u64;
    let mut available_kb = 0u64;

    for line in content.lines() {
        if line.starts_with("MemTotal:") {
            total_kb = parse_meminfo_value(line);
        } else if line.starts_with("MemAvailable:") {
            available_kb = parse_meminfo_value(line);
        }
    }

    let used_kb = total_kb.saturating_sub(available_kb);
    (used_kb / 1024, total_kb / 1024)
}

fn parse_meminfo_value(line: &str) -> u64 {
    line.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Read battery percentage from sysfs.
fn read_battery_pct() -> Option<f32> {
    for bat in &["BAT0", "BAT1"] {
        let path = format!("/sys/class/power_supply/{}/capacity", bat);
        if let Ok(contents) = std::fs::read_to_string(&path) {
            if let Ok(pct) = contents.trim().parse::<f32>() {
                return Some(pct.clamp(0.0, 100.0));
            }
        }
    }
    None
}
