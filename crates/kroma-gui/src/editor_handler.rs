//! Editor message handler (extracted from update() in main.rs).

use crate::Message;
use crate::commands;
use crate::editor;
use crate::importer;
use crate::utils;

use iced::Task;
use iced::widget::text_editor;

use super::KromaApp;

impl KromaApp {
    /// Handle all `Message::Editor*` variants, returning a `Task<Message>`.
    pub(crate) fn handle_editor_msg(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::EditorGraph(graph_msg) => {
                self.handle_graph_msg(commands::GraphTarget::Editor, graph_msg);
            }
            Message::EditorAddNode(kind) => {
                self.graph_canvas.pending_node = Some(kind);
                self.editor_palette_open = false;
            }
            Message::EditorCompile => {
                self.editor_dirty = false;
                self.editor_graph_dirty = false;
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
                        if self.editor_text_mode {
                            self.shader_graph = editor::parse_glsl_to_graph(&src);
                        } else {
                            self.editor_glsl_content = text_editor::Content::with_text(&src);
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
                        let dir = utils::dirs_output_dir();
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
                if self.editor_text_mode {
                    if self.editor_glsl_content.text().trim().is_empty() {
                        if let Ok(glsl) = self.shader_graph.compile_glsl() {
                            self.editor_glsl_content = text_editor::Content::with_text(&glsl);
                        }
                    }
                } else {
                    let glsl = self.editor_glsl_content.text();
                    if !glsl.trim().is_empty() {
                        self.shader_graph = editor::parse_glsl_to_graph(&glsl);
                        self.graph_canvas = editor::canvas::GraphCanvas::default();
                        let n = self.shader_graph.nodes().count();
                        let c = self.shader_graph.connections().len();
                        self.log_msg(format!("Parsed GLSL → {} nodes, {} connections", n, c));
                    }
                }
            }
            Message::EditorGlslSourceChanged(action) => {
                self.editor_glsl_content.perform(action);
                self.editor_dirty = true;
                self.editor_last_edit = std::time::Instant::now();
            }
            Message::EditorLivePreview => {
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
            Message::EditorLoadGlslResult(Some(path)) => match std::fs::read_to_string(&path) {
                Ok(source) => {
                    self.editor_glsl_content = text_editor::Content::with_text(&source);
                    self.editor_text_mode = true;
                    self.log_msg(format!("Loaded: {}", path.display()));
                }
                Err(e) => self.log_msg(format!("Failed to read: {}", e)),
            },
            Message::EditorLoadGlslResult(None) => {}
            _ => {} // Not an editor message — ignore
        }
        Task::none()
    }
}
