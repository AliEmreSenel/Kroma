# Kroma GUI v2 — Design Document

**Version:** 1.0
**Date:** 2025-07-24
**Status:** Approved — Implementation Starting

---

## 1. Vision

A professional, modular, fully dockable creative IDE for authoring and managing real-time wallpaper shaders. The GUI should feel like a lightweight Blender/Godot — giving users complete freedom to arrange their workspace while authoring shaders visually or textually with instant daemon-rendered feedback.

## 2. Design Decisions

All decisions were collaboratively made and are final:

| Decision | Choice | Rationale |
|---|---|---|
| **Live Preview** | Daemon IPC — low-res JPEG frames over Unix socket | Uses the real render pipeline; user sees actual output |
| **Sub-graphs** | Tabbed navigation with breadcrumbs + tree sidebar | Clean, scalable UE5 Blueprint-style navigation |
| **Layout System** | Full dock system (drag-split-tab) | Maximum user freedom; floating windows deferred |
| **Refactor Strategy** | Incremental — extract modules from monolith | Preserves working code, avoids big-bang rewrite risk |
| **Undo/Redo** | Full command-based from day one | All mutations go through Command trait; enables reliable undo |
| **Theme** | User-selectable (Tokyo Night, Catppuccin, Nord, Dracula, One Dark) | Personalization without hardcoded styles |
| **Video Preview** | Full inline playback via FFmpeg in GUI | Complete asset preview without external tools |
| **Floating Windows** | Deferred — split/tab docking first | Split+tab covers 90% of value; float is hard in iced |

## 3. Architecture

### 3.1 Target Module Structure

```
kroma-gui/src/
├── main.rs                  # Entry point — CLI parsing, app boot
├── app.rs                   # KromaApp — thin orchestrator (update/view dispatch)
├── message.rs               # Top-level Message enum with namespaced sub-messages
│
├── dock/                    # === Docking System ===
│   ├── mod.rs               # DockState, DockArea, public API
│   ├── tree.rs              # Binary split tree data structure
│   ├── container.rs         # Dockable panel container widget
│   ├── tab_bar.rs           # Tab bar widget for panel groups
│   ├── drop_zone.rs         # Drop zone overlay for drag-and-drop
│   └── persistence.rs       # Layout save/load (JSON config)
│
├── panels/                  # === Self-contained panels ===
│   ├── mod.rs               # Panel trait definition + registry
│   ├── dashboard.rs         # Daemon status, FPS, quick actions
│   ├── node_editor.rs       # Graph canvas + sub-graph breadcrumbs
│   ├── code_editor.rs       # Syntax-highlighted GLSL text editor
│   ├── asset_browser.rs     # File tree with drag-to-import
│   ├── asset_preview.rs     # Image/video/font/shader preview
│   ├── properties.rs        # Selected node/asset inspector
│   ├── library.rs           # Grid view of .shade packages
│   ├── live_preview.rs      # Daemon-rendered frame stream display
│   ├── import.rs            # Shadertoy URL/file import
│   ├── error_log.rs         # Compile errors, daemon messages
│   └── settings.rs          # Theme, directories, API keys
│
├── editor/                  # === Node Editor Internals ===
│   ├── mod.rs               # Editor types, NodeLayout trait
│   ├── canvas.rs            # Canvas rendering (iced canvas::Program)
│   ├── sub_graph.rs         # Sub-graph tab management + breadcrumbs
│   ├── palette.rs           # Searchable node palette popup
│   ├── selection.rs         # Multi-select, box select, clipboard
│   └── wires.rs             # Wire rendering, hit-testing, deletion
│
├── commands/                # === Command Pattern (Undo/Redo) ===
│   ├── mod.rs               # Command trait, CommandHistory
│   ├── graph.rs             # AddNode, RemoveNode, Connect, MoveNode, SetValue
│   ├── editor.rs            # Code editor actions
│   └── shade.rs             # Asset add/remove, config changes
│
├── ipc/                     # === Async Daemon Communication ===
│   ├── mod.rs               # Async IPC client, reconnect logic
│   ├── preview.rs           # Frame streaming receiver
│   └── protocol.rs          # Command/response types
│
├── theme/                   # === Theme System ===
│   ├── mod.rs               # KromaTheme trait, ThemeRegistry
│   ├── tokens.rs            # Color/spacing/font token definitions
│   ├── tokyo_night.rs       # Tokyo Night implementation
│   ├── catppuccin.rs        # Catppuccin Mocha implementation
│   ├── nord.rs              # Nord implementation
│   ├── dracula.rs           # Dracula implementation
│   └── one_dark.rs          # One Dark implementation
│
├── importer.rs              # Shadertoy import (existing, cleaned up)
└── utils.rs                 # Shared helpers
```

### 3.2 Panel Trait

```rust
pub trait Panel {
    /// Unique identifier for this panel type
    fn id(&self) -> PanelId;

    /// Display title for tab bar
    fn title(&self) -> &str;

    /// Icon identifier for tab bar
    fn icon(&self) -> Icon;

    /// Render the panel content
    fn view(&self, ctx: &AppContext) -> Element<Message>;

    /// Handle a message, return a Command if needed
    fn update(&mut self, msg: PanelMessage, ctx: &mut AppContext) -> Command<Message>;

    /// Optional subscription (e.g., for live preview polling)
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    /// Whether this panel can have multiple instances
    fn allow_multiple(&self) -> bool {
        false
    }
}
```

### 3.3 Dock Tree Structure

The docking system uses a binary split tree:

```
DockNode:
  ├── Split { axis: Horizontal|Vertical, ratio: f32, left: Box<DockNode>, right: Box<DockNode> }
  └── Leaf { tabs: Vec<PanelInstance>, active: usize }
```

- Each `Leaf` holds 1+ panels as tabs
- Dragging a panel's title bar shows drop zone overlays (left/right/top/bottom/center)
- Dropping on center = add as tab; dropping on edge = create new split
- Divider bars are draggable to resize ratios
- Layout serialized as JSON for persistence

### 3.4 Command System

```rust
pub trait UndoableCommand: std::fmt::Debug {
    fn execute(&mut self, state: &mut AppState) -> anyhow::Result<()>;
    fn undo(&mut self, state: &mut AppState) -> anyhow::Result<()>;
    fn description(&self) -> &str;
    /// Whether this command can merge with the previous one (e.g., consecutive typing)
    fn merge_with(&self, _other: &dyn UndoableCommand) -> bool { false }
}

pub struct CommandHistory {
    undo_stack: Vec<Box<dyn UndoableCommand>>,
    redo_stack: Vec<Box<dyn UndoableCommand>>,
    max_history: usize, // default 500
}
```

### 3.5 Async IPC

```
┌─────────────┐     iced Subscription      ┌──────────────────┐
│  GUI Thread  │ ◄─────────────────────────► │  IPC Background  │
│  (messages)  │    channel: Message         │  Task (tokio)    │
└─────────────┘                              └────────┬─────────┘
                                                      │ Unix Socket
                                                      ▼
                                             ┌──────────────────┐
                                             │  kroma-daemon    │
                                             └──────────────────┘
```

- GUI sends commands via `mpsc::Sender`
- Background task reads/writes the Unix socket asynchronously
- Responses arrive as iced Messages via Subscription
- Auto-reconnect with exponential backoff (1s → 2s → 4s → 8s max)
- Preview frames: daemon renders at 1/4 resolution, JPEG-encodes, sends as binary blob

### 3.6 Sub-graph Navigation

```
┌─────────────────────────────────────────────────────────┐
│  Root Graph  ▸  ForLoop_1  ▸  calcLighting              │  ← Breadcrumb bar
├─────────────────────────────────────────────────────────┤
│ ┌──────────┐                                            │
│ │ Graph    │   ┌───────────────────────────────────┐    │
│ │ Tree     │   │                                   │    │
│ │          │   │   [Canvas: calcLighting sub-graph] │    │
│ │ ▼ Root   │   │                                   │    │
│ │   ├ Mix  │   │   (Dot) ──► (Normalize) ──► (Out) │    │
│ │   ├ Add  │   │                                   │    │
│ │   ▼ For..│   └───────────────────────────────────┘    │
│ │     ├ Mul│                                            │
│ │     ▼ ca.│ ◄── you are here                           │
│ │       ├..│                                            │
│ └──────────┘                                            │
└─────────────────────────────────────────────────────────┘
```

- Left sidebar: hierarchical tree of all (sub-)graphs
- Top bar: breadcrumb trail showing navigation path
- Double-click a function/loop node = navigate into it
- Click breadcrumb segment = navigate back to that level
- Each sub-graph level has its own `ShaderGraph` instance

## 4. Panel Specifications

### 4.1 Dashboard Panel
- Daemon status indicator (Online/Offline/Reconnecting) with colored dot
- Active shader name and path
- FPS counter (from daemon status)
- Quick action buttons: Pause, Resume, Reload, Shutdown
- Event log (scrollable, timestamped)
- System resource mini-graphs (CPU, RAM) — data from daemon

### 4.2 Node Editor Panel
- Full graph canvas with pan, zoom, grid background
- Right-click → searchable palette popup at cursor
- Shift+click or drag-box for multi-select
- Ctrl+C/V for copy/paste of selected nodes
- Ctrl+G to group selected nodes with a comment frame
- Ctrl+Z/Ctrl+Shift+Z for undo/redo
- Delete/Backspace to remove selected nodes/wires
- Live value display on node outputs
- Scroll-to-adjust on numeric input ports
- Bézier wire rendering with type-colored connections
- Wire hit-testing for click-to-select and delete
- Minimap overlay (toggle with M key)
- Auto-compile on graph change (debounced 500ms)

### 4.3 Code Editor Panel
- Syntax-highlighted GLSL editing (iced text_editor)
- Line numbers
- Error markers (red squiggles from compile errors)
- Auto-compile on change (debounced 500ms)
- Bidirectional sync with node graph (code ↔ graph)

### 4.4 Asset Browser Panel
- Tree view of .shade package contents
- Icons by file type (shader, image, video, font, config)
- File size display
- Drag-and-drop from OS file manager → auto-import
- Right-click context menu: Rename, Delete, Open in Preview
- Click to select → shows in Asset Preview panel
- Auto-refresh via filesystem watcher

### 4.5 Asset Preview Panel
- **Images**: Full-resolution display with zoom/pan, metadata overlay (dimensions, format, size)
- **Videos**: Inline playback with play/pause, scrub bar, frame counter (FFmpeg decoding)
- **Fonts**: Sample text rendered at sizes 12/18/24/36/48/72pt, editable sample string
- **Shaders**: Syntax-highlighted read-only view
- **Config**: TOML viewer/editor

### 4.6 Properties Panel
- When a graph node is selected: shows all input ports as editable fields (sliders, color pickers, dropdowns)
- When an asset is selected: shows metadata and binding configuration
- When nothing selected: shows shade package metadata (name, author, description, version)

### 4.7 Library Panel
- Grid view of all .shade packages in the shader directory
- Thumbnail previews (from preview.jpg)
- Search/filter by name
- One-click "Apply to Desktop" button
- "New", "Import", "Open" actions
- Sortable by name, date, size

### 4.8 Live Preview Panel
- Displays frames streamed from daemon via IPC
- ~15fps at 1/4 resolution (configurable)
- Shows "Daemon Offline" placeholder when disconnected
- Shows compile errors overlay when shader fails
- Click to cycle through monitors (if multi-monitor)

### 4.9 Error Log Panel
- Scrollable timestamped log
- Severity levels: Error (red), Warning (yellow), Info (default)
- Click on compile errors → jumps to line in Code Editor
- Clear button
- Filter by severity

### 4.10 Settings Panel
- **Theme**: Dropdown selector with live preview
- **Directories**: Shader directory path, temp directory
- **API Keys**: Shadertoy API key
- **Daemon**: Socket path, reconnect interval
- **Editor**: Font size, tab width, auto-compile toggle
- **Keybindings**: Customizable shortcuts (future)

## 5. Theme System

### Color Tokens
```rust
pub struct ThemeTokens {
    // Backgrounds
    pub bg_primary: Color,        // Main background
    pub bg_secondary: Color,      // Panel backgrounds
    pub bg_tertiary: Color,       // Input fields, elevated surfaces
    pub bg_accent: Color,         // Highlighted/selected items

    // Text
    pub text_primary: Color,      // Main text
    pub text_secondary: Color,    // Dimmed/label text
    pub text_accent: Color,       // Links, highlights

    // Borders
    pub border_default: Color,    // Panel/input borders
    pub border_focused: Color,    // Focused element border

    // Semantic
    pub success: Color,           // Green — connected, online
    pub warning: Color,           // Yellow — warnings
    pub error: Color,             // Red — errors, danger
    pub info: Color,              // Blue — informational

    // Node Editor
    pub wire_float: Color,        // Float type wires
    pub wire_vec2: Color,         // Vec2 type wires
    pub wire_vec3: Color,         // Vec3 type wires
    pub wire_vec4: Color,         // Vec4 type wires
    pub wire_color: Color,        // Color type wires
    pub node_bg: Color,           // Node background
    pub node_header: Color,       // Node title bar
    pub node_selected: Color,     // Selection highlight

    // Interactive
    pub button_primary: Color,
    pub button_hover: Color,
    pub button_pressed: Color,
    pub tab_active: Color,
    pub tab_inactive: Color,
    pub drop_zone: Color,         // Dock drop zone highlight

    // Spacing
    pub spacing_xs: f32,          // 2px
    pub spacing_sm: f32,          // 4px
    pub spacing_md: f32,          // 8px
    pub spacing_lg: f32,          // 16px
    pub spacing_xl: f32,          // 24px
    pub border_radius: f32,       // 4px
    pub font_size_sm: f32,        // 12px
    pub font_size_md: f32,        // 14px
    pub font_size_lg: f32,        // 18px
}
```

### Built-in Themes
1. **Tokyo Night** (default) — Current theme, dark blue-purple palette
2. **Catppuccin Mocha** — Warm dark theme with pastel accents
3. **Nord** — Cool, muted arctic palette
4. **Dracula** — High-contrast dark with vivid accents
5. **One Dark** — Atom-inspired balanced dark theme

## 6. Error Tolerance Principles

1. **No `unwrap()` in GUI code** — All fallible operations return `Result`, errors shown in Error Log
2. **Daemon offline = editing mode** — All editing features work without daemon; preview disabled with banner
3. **Malformed .shade files** — Load what's possible, show warnings for missing/corrupt parts
4. **Compile errors** — Display inline in editor + error log, never crash, always recoverable
5. **IPC timeout** — Non-blocking with async, auto-retry with backoff
6. **Asset load failure** — Show placeholder with error message, don't crash panel
7. **Config corruption** — Fall back to defaults, warn user
8. **Panic handler** — Catch panics at panel level, show error, keep other panels running

## 7. Implementation Roadmap

### Phase 1: Foundation — Docking System & Module Extraction
**Goal**: Transform monolith into modular architecture with working dock system

1. Create module structure (`app.rs`, `message.rs`, `dock/`, `panels/`)
2. Define `Panel` trait and `PanelId` enum
3. Extract `KromaApp` fields into per-panel state structs
4. Build `DockTree` — binary split tree data structure
5. Build `DockArea` widget — renders split tree with resizable dividers
6. Build `TabBar` widget — tab headers with close buttons
7. Implement panel drag-and-drop between tab groups
8. Implement drop zone overlays for split creation
9. Port existing Dashboard view → `DashboardPanel`
10. Port existing Editor view → `NodeEditorPanel`
11. Port existing ShadeEdit view → split into `CodeEditorPanel` + `AssetBrowserPanel` + `PropertiesPanel`
12. Port existing Import view → `ImportPanel`
13. Port existing Settings view → `SettingsPanel`
14. Layout persistence (save/load from JSON config)
15. Default layout preset

**Milestone**: All existing functionality works in the new docked layout.

### Phase 2: Command System & Undo/Redo
**Goal**: All mutations go through command pattern

1. Define `UndoableCommand` trait
2. Build `CommandHistory` with undo/redo stacks
3. Wrap graph mutations: `AddNodeCmd`, `RemoveNodeCmd`, `ConnectCmd`, `DisconnectCmd`, `MoveNodeCmd`, `SetValueCmd`
4. Wrap code editor actions
5. Wire Ctrl+Z / Ctrl+Shift+Z
6. Add undo/redo buttons to toolbar
7. Extend to shade package operations (asset add/remove)

**Milestone**: Full undo/redo across node editor and code editor.

### Phase 3: Node Editor Enhancements
**Goal**: Professional node editing experience

1. Searchable palette popup at right-click position
2. Multi-select: Shift+click and box-select
3. Copy/paste: Ctrl+C/V with clipboard serialization
4. Wire hit-testing and click-to-select
5. Wire deletion via Backspace/Delete
6. Drag-and-drop nodes from palette to canvas
7. Node grouping with comment frames (Ctrl+G)
8. Sub-graph tabs with breadcrumb bar
9. Sub-graph tree sidebar
10. ForLoop sub-graph: iteration variable as input port, body as sub-graph
11. Conditional sub-graph: true/false branches
12. CustomFunc: named function with typed input/output ports
13. Minimap overlay
14. Type coercion visual indicators on ports

**Milestone**: Feature-complete node editor with sub-graphs.

### Phase 4: Async IPC & Live Preview
**Goal**: Non-blocking daemon ccargo test --workspace 2>&1 | tail -20  && echo continue building. Currently I cannot select multiple nodes and move them all. I cant drag from the nodes list to wherever I weant and the minimap indicator for showing where I am goes off the minimap and floats aroundommunication with live frame streaming

1. Add `tokio` as async runtime for IPC background task
2. Rewrite IPC client as async with mpsc channels
3. Implement iced `Subscription` for IPC message reception
4. Add `PreviewFrame` IPC command to daemon (render at 1/4 res, JPEG-encode, return)
5. Build `LivePreviewPanel` to display streamed frames
6. Auto-recompile on shader change (debounced 500ms)
7. Auto-reconnect with exponential backoff
8. Compile error forwarding: daemon → GUI → inline markers
9. Graceful degradation: offline banner, disable preview-dependent features

**Milestone**: Live preview works with non-blocking IPC.

### Phase 5: Asset Management & Previews
**Goal**: Rich asset browsing and preview experience

1. Drag-and-drop file import from OS file manager
2. Filesystem watcher (notify crate) for auto-reload
3. Image preview: full-resolution display with zoom/pan and metadata
4. Video preview: inline FFmpeg playback with scrub bar
5. Font preview: sample text at multiple sizes
6. Shader/TOML: syntax-highlighted viewer
7. Auto-update config.toml texture definitions on asset import
8. Asset delete with confirmation

**Milestone**: All asset types previewable with hot-reload.

### Phase 6: Theme System
**Goal**: User-selectable themes with full token coverage

1. Define `ThemeTokens` struct and `KromaTheme` trait
2. Convert all panels to use theme tokens (remove hardcoded colors)
3. Implement Tokyo Night theme
4. Implement Catppuccin Mocha theme
5. Implement Nord theme
6. Implement Dracula theme
7. Implement One Dark theme
8. Theme selection in Settings panel with live switching
9. Persist selected theme to config

**Milestone**: 5 themes, all panels themed consistently.

### Phase 7: Polish & Error Tolerance
**Goal**: Production-quality robustness

1. Audit and replace all `unwrap()` with proper error handling
2. Toast/notification system replacing `log_msg()`
3. Panel-level panic catching
4. Keyboard shortcuts system with customization
5. Responsive layout (minimum window size, overflow handling)
6. Performance profiling and optimization
7. Accessibility improvements (focus order, contrast)
8. Help tooltips on all major UI elements

**Milestone**: Release-quality GUI.

### Phase 8: Floating Windows (Future)
**Goal**: Full multi-window docking

1. Research iced multi-window support
2. Panel detach from dock → floating window
3. Floating window → reattach to dock
4. Cross-window drag-and-drop
5. Multi-monitor layout persistence

**Milestone**: Complete IDE-level docking.

## 8. Dependencies (New)

| Crate | Purpose |
|---|---|
| `tokio` | Async runtime for IPC background task |
| `notify` | Filesystem watcher for hot-reload |
| `image` | Image decoding for asset preview |
| `ffmpeg-next` | Video decoding for inline preview |
| `serde_json` | Dock layout persistence |

## 9. Keybindings (Default)

| Key | Action |
|---|---|
| Ctrl+Z | Undo |
| Ctrl+Shift+Z | Redo |
| Ctrl+C | Copy selected nodes |
| Ctrl+V | Paste nodes |
| Ctrl+G | Group selected nodes |
| Ctrl+S | Save shade package |
| Delete / Backspace | Delete selection |
| Shift+Click | Add to selection |
| Right-click (canvas) | Open node palette |
| M | Toggle minimap |
| F | Fit graph to view |
| Space+Drag | Pan canvas |
| Scroll | Zoom canvas |
| Double-click (func node) | Enter sub-graph |
| Escape | Back to parent graph |
