//! Panel trait and registry — every dockable view implements [`Panel`].
//!
//! Panels are self-contained UI modules with their own state, view, and
//! update logic. The docking system composes them into the workspace layout.

pub mod asset_browser;
pub mod asset_preview;
pub mod code_editor;
pub mod dashboard;
pub mod designer;
pub mod error_log;
pub mod import;
pub mod library;
pub mod live_preview;
pub mod node_editor;
pub mod properties;
pub mod settings;

use iced::Element;
use serde::{Deserialize, Serialize};

use crate::Message;

// ---------------------------------------------------------------------------
// Panel ID
// ---------------------------------------------------------------------------

/// Identifies a panel type. Used as keys in the dock tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PanelId {
    Dashboard,
    Designer,
    NodeEditor,
    CodeEditor,
    AssetBrowser,
    AssetPreview,
    Properties,
    Library,
    LivePreview,
    Import,
    ErrorLog,
    Settings,
}

impl PanelId {
    /// Human-readable title for the tab bar.
    pub fn title(&self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Designer => "Designer",
            Self::NodeEditor => "Node Editor",
            Self::CodeEditor => "Code Editor",
            Self::AssetBrowser => "Assets",
            Self::AssetPreview => "Preview",
            Self::Properties => "Properties",
            Self::Library => "Library",
            Self::LivePreview => "Live Preview",
            Self::Import => "Import",
            Self::ErrorLog => "Errors",
            Self::Settings => "Settings",
        }
    }
}

impl std::fmt::Display for PanelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.title())
    }
}

// ---------------------------------------------------------------------------
// Panel trait
// ---------------------------------------------------------------------------

/// Shared context passed to all panels for reading global app state.
#[derive(Clone)]
pub struct AppContext<'a> {
    // --- Theme ---
    /// Color and spacing tokens for the active theme.
    pub tokens: &'a crate::theme::ThemeTokens,

    // --- Core ---
    /// Whether the daemon is connected.
    pub daemon_connected: bool,
    /// Currently loaded shade path.
    pub loaded_shade: Option<&'a str>,
    /// Current FPS from daemon.
    pub fps: f32,
    /// Whether rendering is paused.
    pub paused: bool,
    /// Status text from daemon.
    pub status_text: &'a str,
    /// Log messages.
    pub log_messages: &'a std::collections::VecDeque<String>,
    /// Last compile errors.
    pub compile_errors: &'a [kroma_shared::ipc::CompileError],
    /// Last compile warnings.
    pub compile_warnings: &'a [String],
    /// Whether last compile succeeded.
    pub compile_success: Option<bool>,

    // --- Import ---
    pub import_path: &'a str,
    pub import_name: &'a str,
    pub import_author: &'a str,
    pub import_result: &'a str,
    pub shadertoy_url: &'a str,
    pub download_result: &'a str,
    pub downloading: bool,

    // --- Shade project ---
    pub shade_config: &'a kroma_shared::types::ShadeConfig,
    pub shade_package: Option<&'a kroma_shared::shade::LiveShadePackage>,
    pub shade_selected_file: Option<&'a str>,
    pub shade_config_toml: &'a iced::widget::text_editor::Content,
    pub shade_shader_content: &'a iced::widget::text_editor::Content,
    pub shade_loaded_path: Option<&'a std::path::Path>,
    pub shade_new_uniform_name: &'a str,
    pub shade_new_uniform_type: &'a str,
    pub shade_new_texture_name: &'a str,
    pub shade_new_texture_type: &'a str,
    /// Current shade edit mode ("settings", "toml", "code", "nodes", "preview").
    pub shade_edit_mode: &'a str,
    /// Shade graph for node editing.
    pub shade_graph: &'a crate::editor::ShaderGraph,
    /// Shade graph canvas state.
    pub shade_graph_canvas: &'a crate::editor::canvas::GraphCanvas,
    /// Sub-graph data for shade graph.
    #[allow(dead_code)]
    pub shade_subgraphs: &'a std::collections::HashMap<
        kroma_graph::types::NodeId,
        (
            crate::editor::ShaderGraph,
            crate::editor::canvas::GraphCanvas,
        ),
    >,
    /// Navigation path for shade sub-graphs.
    #[allow(dead_code)]
    pub shade_nav_path: &'a [kroma_graph::types::NodeId],
    /// Human-readable labels for shade nav path (computed from root graph).
    pub shade_nav_labels: Vec<String>,

    // --- Editor ---
    pub editor_glsl_content: &'a iced::widget::text_editor::Content,
    pub editor_glsl_preview: &'a str,
    pub editor_text_mode: bool,
    pub editor_live_preview: bool,
    pub shader_graph: &'a crate::editor::ShaderGraph,
    pub graph_canvas: &'a crate::editor::canvas::GraphCanvas,
    /// Editor palette filter text.
    pub editor_palette_filter: &'a str,
    /// Current sub-graph navigation path for editor (stack of NodeIds).
    pub editor_nav_path: &'a [kroma_graph::types::NodeId],
    /// Sub-graph data for editor.
    pub editor_subgraphs: &'a std::collections::HashMap<
        kroma_graph::types::NodeId,
        (
            crate::editor::ShaderGraph,
            crate::editor::canvas::GraphCanvas,
        ),
    >,

    // --- Live Preview ---
    /// Latest decoded preview frame from daemon.
    pub preview_frame: Option<&'a crate::PreviewFrameData>,
    /// Whether preview streaming is active.
    pub preview_streaming: bool,

    // --- Settings ---
    /// Shadertoy API key (owned by KromaApp, exposed here for SettingsPanel).
    pub api_key: &'a str,
    /// Whether a file is being hovered over the window (drag-drop indicator).
    pub drop_hover_active: bool,
    /// Available audio sources from PulseAudio/PipeWire.
    pub available_audio_sources: &'a [String],
    /// Active video player state for asset preview playback.
    pub video_player: Option<&'a crate::video::VideoPlayerState>,
}

/// Trait that every dockable panel implements.
///
/// Panels are self-contained: they own their state, render their view,
/// and handle their messages. The orchestrator (`KromaApp`) routes messages
/// to the correct panel based on `PanelId`.
pub trait Panel {
    /// Render the panel content.
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message>;
}
