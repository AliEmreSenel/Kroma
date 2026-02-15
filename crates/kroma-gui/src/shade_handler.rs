//! Shade project message handler (extracted from update() in main.rs).

use crate::commands;
use crate::editor;
use crate::utils;
use crate::Message;

use iced::widget::text_editor;
use iced::Task;

use super::KromaApp;

impl KromaApp {
    /// Handle all `Message::Shade*` variants, returning a `Task<Message>`.
    pub(crate) fn handle_shade_msg(&mut self, message: Message) -> Task<Message> {
        match message {
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
                    buffers: Default::default(),
                };
                let default_frag = "// Kroma shader\nvoid mainImage(out vec4 fragColor, in vec2 fragCoord) {\n    vec2 uv = fragCoord / u_resolution;\n    fragColor = vec4(uv, 0.5 + 0.5 * sin(u_time), 1.0);\n}\n";
                self.shade_config = config.clone();
                self.shade_package = Some(kroma_shared::shade::LiveShadePackage::from_package(
                    kroma_shared::shade::ShadePackage {
                        config,
                        shader_source: Some(default_frag.into()),
                        preview: None,
                        assets: Vec::new(),
                    },
                ));
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
                    match kroma_shared::shade::LiveShadePackage::load(&path) {
                        Ok(pkg) => {
                            self.shade_config = pkg.config.clone();
                            self.shade_shader_content = text_editor::Content::with_text(
                                pkg.shader_source.as_deref().unwrap_or(""),
                            );
                            self.shade_package = Some(pkg);
                            self.shade_loaded_path = Some(path.clone());
                            self.shade_selected_file = Some("shader.frag".into());
                            self.shade_edit_mode = "code".into();
                            self.sync_shade_toml();
                            self.log_msg(format!("Opened: {}", path.display()));
                        }
                        Err(e) => self.log_msg(format!("Failed to open shade: {}", e)),
                    }
                } else {
                    if let Ok(toml_str) = std::fs::read_to_string(&path) {
                        match toml::from_str::<kroma_shared::types::ShadeConfig>(&toml_str) {
                            Ok(config) => {
                                self.shade_config = config.clone();
                                self.shade_package =
                                    Some(kroma_shared::shade::LiveShadePackage::from_package(
                                        kroma_shared::shade::ShadePackage {
                                            config,
                                            shader_source: None,
                                            preview: None,
                                            assets: Vec::new(),
                                        },
                                    ));
                                self.shade_loaded_path = Some(path.clone());
                                self.sync_shade_toml();
                                self.log_msg(format!("Opened: {}", path.display()));
                                self.shade_selected_file = Some("config.toml".into());
                                self.shade_edit_mode = "settings".into();
                            }
                            Err(e) => self.log_msg(format!("Invalid config: {}", e)),
                        }
                    }
                }
            }
            Message::ShadeOpenResult(None) => {}
            Message::ShadeSave => {
                let shader_src = self.shade_shader_content.text();
                // Update the live package in-place, then save
                if let Some(ref mut pkg) = self.shade_package {
                    pkg.config = self.shade_config.clone();
                    pkg.shader_source = Some(shader_src);
                } else {
                    // No package loaded — create one
                    let mut pkg =
                        kroma_shared::shade::LiveShadePackage::new_empty(self.shade_config.clone());
                    pkg.shader_source = Some(shader_src);
                    self.shade_package = Some(pkg);
                }
                let shade_path = if let Some(ref existing) = self.shade_loaded_path {
                    existing.clone()
                } else {
                    let dir = utils::dirs_output_dir();
                    std::fs::create_dir_all(&dir).ok();
                    let name = self.shade_config.meta.name.replace(' ', "_").to_lowercase();
                    dir.join(format!("{}.shade", name))
                };
                if let Some(ref pkg) = self.shade_package {
                    match pkg.save(&shade_path) {
                        Ok(()) => {
                            self.shade_loaded_path = Some(shade_path.clone());
                            self.log_msg(format!("Saved: {}", shade_path.display()));
                        }
                        Err(e) => self.log_msg(format!("Save failed: {}", e)),
                    }
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
            Message::ShadeAddBuffer => {
                let letters = ["A", "B", "C", "D"];
                for letter in letters {
                    if !self.shade_config.buffers.contains_key(letter) {
                        self.shade_config.buffers.insert(
                            letter.to_string(),
                            kroma_shared::types::BufferDef {
                                shader: format!("buffer_{}.frag", letter.to_lowercase()),
                                inputs: Vec::new(),
                                feedback: false,
                            },
                        );
                        break;
                    }
                }
                self.sync_shade_toml();
            }
            Message::ShadeRemoveBuffer(name) => {
                self.shade_config.buffers.remove(&name);
                self.sync_shade_toml();
            }
            Message::ShadeBufferShaderChanged(name, shader) => {
                if let Some(buf) = self.shade_config.buffers.get_mut(&name) {
                    buf.shader = shader;
                }
                self.sync_shade_toml();
            }
            Message::ShadeBufferFeedbackToggled(name) => {
                if let Some(buf) = self.shade_config.buffers.get_mut(&name) {
                    buf.feedback = !buf.feedback;
                }
                self.sync_shade_toml();
            }
            Message::ShadeSelectFile(file) => {
                self.shade_selected_file = Some(file);
            }
            Message::ShadeEditMode(mode) => {
                if self.shade_edit_mode == "nodes" && mode == "code" {
                    if let Ok(glsl) = self.shade_graph.compile_glsl() {
                        self.shade_shader_content = text_editor::Content::with_text(&glsl);
                        if let Some(ref mut pkg) = self.shade_package {
                            pkg.shader_source = Some(glsl);
                        }
                    }
                }
                if self.shade_edit_mode == "code" && mode == "nodes" {
                    let glsl = self.shade_shader_content.text();
                    if !glsl.trim().is_empty() {
                        eprintln!(
                            "[kroma-gui] Parsing GLSL to nodes (code→nodes switch), {} chars",
                            glsl.len()
                        );
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
                if let Some(ref mut pkg) = self.shade_package {
                    pkg.shader_source = Some(self.shade_shader_content.text());
                }
            }
            Message::ShadeGraphMsg(msg) => {
                self.handle_graph_msg(commands::GraphTarget::Shade, msg);
            }
            Message::ShadeAddNode(kind) => {
                self.shade_graph_canvas.pending_node = Some(kind);
            }
            Message::ShadeCompileGraph => match self.shade_graph.compile_glsl() {
                Ok(glsl) => {
                    self.shade_shader_content = text_editor::Content::with_text(&glsl);
                    if let Some(ref mut pkg) = self.shade_package {
                        pkg.shader_source = Some(glsl);
                    }
                    self.log_msg("Node graph compiled to GLSL".into());
                }
                Err(e) => self.log_msg(format!("Graph compile error: {}", e)),
            },
            Message::ShadeParseToNodes => {
                let glsl = self.shade_shader_content.text();
                if glsl.trim().is_empty() {
                    self.log_msg("No GLSL source to parse into nodes".into());
                } else {
                    eprintln!(
                        "[kroma-gui] ShadeParseToNodes: parsing {} chars",
                        glsl.len()
                    );
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
                    let filename = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let asset_path = format!("assets/{}", filename);
                    if let Some(ref mut pkg) = self.shade_package {
                        pkg.add_asset(asset_path.clone(), data);
                        self.log_msg(format!("Added asset: {}", asset_path));
                    } else {
                        self.log_msg("No shade package loaded — open or create one first".into());
                    }
                }
            }
            Message::ShadeAddAssetResult(None) => {}
            Message::ShadeRemoveAsset(name) => {
                if let Some(ref mut pkg) = self.shade_package {
                    pkg.remove_asset(&name);
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
                    let filename = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let asset_path = format!("assets/{}", filename);
                    if let Some(ref mut pkg) = self.shade_package {
                        pkg.add_asset(asset_path.clone(), data);
                        self.log_msg(format!("Added GLSL asset: {}", asset_path));
                    } else {
                        self.log_msg("No shade package loaded — open or create one first".into());
                    }
                }
            }
            Message::ShadeAddGlslFileResult(None) => {}
            Message::ShadeBindAssetToTexture(asset_name, channel) => {
                let channel_name = format!("iChannel{}", channel);
                // Determine texture type from extension
                let ext = asset_name
                    .rsplit('.')
                    .next()
                    .unwrap_or("")
                    .to_lowercase();
                let tex_type = match ext.as_str() {
                    "mp4" | "webm" | "avi" | "mkv" => "video",
                    "glsl" | "frag" => "glsl",
                    "ttf" | "otf" | "woff" | "woff2" => "font",
                    _ => "image",
                };
                let is_video = tex_type == "video";
                self.shade_config.textures.insert(
                    channel_name.clone(),
                    kroma_shared::types::TextureDef {
                        ty: tex_type.into(),
                        source: Some(asset_name.clone()),
                        looping: is_video,
                        filter: Default::default(),
                        wrap: Default::default(),
                        binding: Some(channel),
                    },
                );
                self.sync_shade_toml();
                self.log_msg(format!(
                    "Bound '{}' to {} (binding {})",
                    asset_name, channel_name, channel
                ));
            }
            _ => {} // Not a shade message — ignore
        }
        Task::none()
    }
}
