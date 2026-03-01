//! Simple Designer panel — no-code wallpaper creation with premade components.
//!
//! Users pick a background style and drag-and-drop widgets (clock, CPU meter,
//! battery, audio visualizer, weather) onto a canvas layout. The designer
//! generates a complete .shade project with GLSL and config.

use iced::widget::space::horizontal;
use iced::widget::{Space, button, column, container, row, scrollable, slider, text, text_input};
use iced::{Border, Element, Fill, Padding, Theme};
// Audio is now a texture type — no separate AudioConfig needed.

use crate::Message;
use crate::icons;
use crate::panels::{AppContext, Panel};

// ---------------------------------------------------------------------------
// Component types
// ---------------------------------------------------------------------------

/// A premade component that can be placed on the wallpaper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesignerComponent {
    /// Digital clock display (hours:minutes:seconds).
    Clock,
    /// Date display (day/month/year).
    Date,
    /// CPU usage bar/percentage.
    CpuMeter,
    /// RAM usage bar/percentage.
    RamMeter,
    /// Battery level indicator.
    Battery,
    /// Audio spectrum visualizer bars.
    AudioSpectrum,
    /// Audio waveform display.
    AudioWaveform,
    /// Gradient color background.
    GradientBg,
    /// Animated noise background.
    NoiseBg,
    /// Solid color background.
    SolidBg,
    /// Particle system.
    Particles,
    /// Static image overlay.
    ImageOverlay,
    /// Video overlay.
    VideoOverlay,
}

impl DesignerComponent {
    /// All available components.
    pub fn all() -> &'static [Self] {
        &[
            Self::Clock,
            Self::Date,
            Self::CpuMeter,
            Self::RamMeter,
            Self::Battery,
            Self::AudioSpectrum,
            Self::AudioWaveform,
            Self::GradientBg,
            Self::NoiseBg,
            Self::SolidBg,
            Self::Particles,
            Self::ImageOverlay,
            Self::VideoOverlay,
        ]
    }

    /// Human-readable name.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Clock => "Clock",
            Self::Date => "Date",
            Self::CpuMeter => "CPU Meter",
            Self::RamMeter => "RAM Meter",
            Self::Battery => "Battery",
            Self::AudioSpectrum => "Audio Spectrum",
            Self::AudioWaveform => "Audio Waveform",
            Self::GradientBg => "Gradient Background",
            Self::NoiseBg => "Animated Noise",
            Self::SolidBg => "Solid Color",
            Self::Particles => "Particles",
            Self::ImageOverlay => "Image",
            Self::VideoOverlay => "Video",
        }
    }

    /// Icon/emoji for the component.
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Clock => crate::icons::ACCESS_TIME,
            Self::Date => crate::icons::EVENT,
            Self::CpuMeter => crate::icons::COMPUTER,
            Self::RamMeter => crate::icons::MEMORY,
            Self::Battery => crate::icons::BATTERY,
            Self::AudioSpectrum => crate::icons::EQUALIZER,
            Self::AudioWaveform => crate::icons::MUSIC,
            Self::GradientBg => crate::icons::PALETTE,
            Self::NoiseBg => crate::icons::TEXTURE,
            Self::SolidBg => crate::icons::BRUSH,
            Self::Particles => crate::icons::STAR,
            Self::ImageOverlay => crate::icons::IMAGE,
            Self::VideoOverlay => crate::icons::VIDEO,
        }
    }

    /// Category for grouping.
    pub fn category(&self) -> &'static str {
        match self {
            Self::Clock | Self::Date => "Time & Date",
            Self::CpuMeter | Self::RamMeter | Self::Battery => "System Info",
            Self::AudioSpectrum | Self::AudioWaveform => "Audio",
            Self::GradientBg | Self::NoiseBg | Self::SolidBg | Self::Particles => "Backgrounds",
            Self::ImageOverlay | Self::VideoOverlay => "Assets",
        }
    }

    /// Default position and size for a new component (x, y, w, h) in 0-1 normalized coords.
    pub fn default_rect(&self) -> (f32, f32, f32, f32) {
        match self {
            // Backgrounds fill the entire canvas
            Self::GradientBg | Self::NoiseBg | Self::SolidBg => (0.0, 0.0, 1.0, 1.0),
            // Clock in upper right
            Self::Clock => (0.6, 0.05, 0.35, 0.1),
            // Date below clock
            Self::Date => (0.6, 0.16, 0.35, 0.06),
            // System bars on the left
            Self::CpuMeter => (0.02, 0.85, 0.25, 0.04),
            Self::RamMeter => (0.02, 0.90, 0.25, 0.04),
            Self::Battery => (0.88, 0.02, 0.1, 0.05),
            // Audio fills bottom
            Self::AudioSpectrum => (0.0, 0.6, 1.0, 0.4),
            Self::AudioWaveform => (0.0, 0.4, 1.0, 0.2),
            Self::Particles => (0.0, 0.0, 1.0, 1.0),
            // Assets centered
            Self::ImageOverlay => (0.2, 0.2, 0.6, 0.6),
            Self::VideoOverlay => (0.1, 0.1, 0.8, 0.8),
        }
    }

    /// Generate the GLSL code snippet for this component.
    pub fn glsl_snippet(&self) -> &'static str {
        match self {
            Self::Clock => {
                r#"
    // Clock display
    float hours = mod(u_time / 3600.0, 24.0);
    float minutes = mod(u_time / 60.0, 60.0);
    float seconds = mod(u_time, 60.0);
    // Render clock digits at center
    vec2 clock_uv = (uv - vec2(0.5, 0.8)) * 10.0;
    float digit = step(abs(clock_uv.y), 0.4) * step(abs(clock_uv.x), 2.5);
    color = mix(color, vec3(1.0), digit * 0.3);
"#
            }
            Self::Date => {
                r#"
    // Date display (uses time-based approximation)
    float day_of_year = floor(u_time / 86400.0);
    vec2 date_uv = (uv - vec2(0.5, 0.7)) * 12.0;
    float date_area = step(abs(date_uv.y), 0.3) * step(abs(date_uv.x), 2.0);
    color = mix(color, vec3(0.8, 0.8, 1.0), date_area * 0.2);
"#
            }
            Self::CpuMeter => {
                r#"
    // CPU meter bar
    float cpu = u_cpu;
    vec2 cpu_uv = (uv - vec2(0.1, 0.9)) * vec2(5.0, 20.0);
    float cpu_bar = step(0.0, cpu_uv.x) * step(cpu_uv.x, cpu * 1.0) *
                    step(0.0, cpu_uv.y) * step(cpu_uv.y, 1.0);
    vec3 cpu_color = mix(vec3(0.0, 1.0, 0.0), vec3(1.0, 0.0, 0.0), cpu);
    color = mix(color, cpu_color, cpu_bar * 0.8);
"#
            }
            Self::RamMeter => {
                r#"
    // RAM meter bar
    float ram = u_ram;
    vec2 ram_uv = (uv - vec2(0.1, 0.85)) * vec2(5.0, 20.0);
    float ram_bar = step(0.0, ram_uv.x) * step(ram_uv.x, ram * 1.0) *
                    step(0.0, ram_uv.y) * step(ram_uv.y, 1.0);
    vec3 ram_color = mix(vec3(0.2, 0.6, 1.0), vec3(1.0, 0.3, 0.3), ram);
    color = mix(color, ram_color, ram_bar * 0.8);
"#
            }
            Self::Battery => {
                r#"
    // Battery indicator
    float batt = u_battery;
    vec2 batt_uv = (uv - vec2(0.9, 0.95)) * vec2(10.0, 30.0);
    float batt_outline = step(0.0, batt_uv.x) * step(batt_uv.x, 1.0) *
                         step(0.0, batt_uv.y) * step(batt_uv.y, 1.0);
    float batt_fill = step(0.05, batt_uv.x) * step(batt_uv.x, 0.95) *
                      step(0.05, batt_uv.y) * step(batt_uv.y, batt * 0.9 + 0.05);
    vec3 batt_color = mix(vec3(1.0, 0.2, 0.2), vec3(0.2, 1.0, 0.2), batt);
    color = mix(color, vec3(0.5), batt_outline * 0.3);
    color = mix(color, batt_color, batt_fill * 0.7);
"#
            }
            Self::AudioSpectrum => {
                r#"
    // Audio spectrum visualizer
    float band = floor(uv.x * 32.0) / 32.0;
    float spectrum_height = texture(iChannel0, vec2(band, 0.0)).r;
    float bar = step(1.0 - uv.y, spectrum_height * 0.5) * step(0.02, fract(uv.x * 32.0));
    vec3 spec_color = mix(vec3(0.1, 0.5, 1.0), vec3(1.0, 0.2, 0.8), uv.x);
    color = mix(color, spec_color, bar * 0.9);
"#
            }
            Self::AudioWaveform => {
                r#"
    // Audio waveform
    float wave_sample = texture(iChannel0, vec2(uv.x, 0.5)).r;
    float wave_line = 1.0 - smoothstep(0.0, 0.01, abs(uv.y - 0.5 - wave_sample * 0.3));
    color = mix(color, vec3(0.3, 0.8, 1.0), wave_line * 0.8);
"#
            }
            Self::GradientBg => {
                r#"
    // Animated gradient background
    float angle = u_time * 0.1;
    vec2 grad_dir = vec2(cos(angle), sin(angle));
    float grad_t = dot(uv, grad_dir) * 0.5 + 0.5;
    color = mix(vec3(0.05, 0.02, 0.15), vec3(0.1, 0.05, 0.3), grad_t);
    color = mix(color, vec3(0.2, 0.1, 0.4), sin(grad_t * 3.14159) * 0.5);
"#
            }
            Self::NoiseBg => {
                r#"
    // Animated noise background
    vec2 noise_uv = uv * 3.0 + u_time * 0.05;
    float n = fract(sin(dot(noise_uv, vec2(12.9898, 78.233))) * 43758.5453);
    float n2 = fract(sin(dot(noise_uv * 2.0 + 0.5, vec2(12.9898, 78.233))) * 43758.5453);
    float noise = mix(n, n2, 0.5) * 0.15;
    color = vec3(0.05 + noise, 0.02 + noise * 0.8, 0.12 + noise * 1.2);
"#
            }
            Self::SolidBg => {
                r#"
    // Solid color background
    color = vec3(0.08, 0.04, 0.16);
"#
            }
            Self::Particles => {
                r#"
    // Simple particle system
    float particles = 0.0;
    for (int i = 0; i < 20; i++) {
        float fi = float(i);
        vec2 p = vec2(
            fract(sin(fi * 123.456) * 789.012),
            fract(cos(fi * 456.789) * 321.654 + u_time * (0.02 + fi * 0.005))
        );
        float d = length(uv - p);
        particles += smoothstep(0.015, 0.0, d) * (0.5 + 0.5 * sin(fi));
    }
    color += vec3(0.3, 0.5, 1.0) * particles;
"#
            }
            Self::ImageOverlay => {
                r#"
    // Image overlay
    vec4 img = texture(iChannel1, uv);
    color = mix(color, img.rgb, img.a);
"#
            }
            Self::VideoOverlay => {
                r#"
    // Video overlay
    vec4 vid = texture(iChannel2, uv);
    color = mix(color, vid.rgb, vid.a);
"#
            }
        }
    }

    /// Whether this component requires audio data.
    pub fn needs_audio(&self) -> bool {
        matches!(self, Self::AudioSpectrum | Self::AudioWaveform)
    }

    /// Whether this component needs system uniforms.
    pub fn needs_system_uniforms(&self) -> bool {
        matches!(self, Self::CpuMeter | Self::RamMeter | Self::Battery)
    }

    /// Whether this component needs image/video texture channels.
    pub fn needs_asset_texture(&self) -> bool {
        matches!(self, Self::ImageOverlay | Self::VideoOverlay)
    }
}

// ---------------------------------------------------------------------------
// Placed component instance
// ---------------------------------------------------------------------------

/// A component placed in the designer with position and size.
#[derive(Debug, Clone)]
pub struct PlacedComponent {
    /// The component type.
    pub component: DesignerComponent,
    /// Whether this component is enabled.
    pub enabled: bool,
    /// Position on canvas (0.0-1.0 normalized coordinates).
    pub x: f32,
    pub y: f32,
    /// Size (0.0-1.0 normalized).
    pub width: f32,
    pub height: f32,
    /// Rotation in degrees.
    #[allow(dead_code)]
    pub rotation: f32,
    /// Opacity (0.0-1.0).
    pub opacity: f32,
    /// Optional custom GLSL snippet override.
    pub custom_shader: Option<String>,
    /// Optional image/video asset path (for Image/Video components).
    pub asset_path: Option<String>,
}

impl PlacedComponent {
    /// Create a new placed component with default position/size for its type.
    pub fn new(component: DesignerComponent) -> Self {
        let (x, y, w, h) = component.default_rect();
        Self {
            component,
            enabled: true,
            x,
            y,
            width: w,
            height: h,
            rotation: 0.0,
            opacity: 1.0,
            custom_shader: None,
            asset_path: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Designer state
// ---------------------------------------------------------------------------

/// State for the simple designer.
pub struct DesignerPanel {
    /// Components added to the wallpaper.
    pub components: Vec<PlacedComponent>,
    /// Background color (hex string).
    #[allow(dead_code)]
    pub bg_color: String,
    /// Index of currently selected component.
    pub selected: Option<usize>,
    /// Whether we're currently dragging a component.
    #[allow(dead_code)]
    pub dragging: Option<usize>,
    /// Drag offset from component origin.
    #[allow(dead_code)]
    pub drag_offset: [f32; 2],
    /// Whether we're resizing (dragging corner).
    #[allow(dead_code)]
    pub resizing: Option<usize>,
}

impl DesignerPanel {
    pub fn new() -> Self {
        Self {
            components: Vec::new(),
            bg_color: "#0D0520".into(),
            selected: None,
            dragging: None,
            drag_offset: [0.0, 0.0],
            resizing: None,
        }
    }

    /// Generate the full GLSL shader from the current design.
    pub fn generate_glsl(&self) -> String {
        let needs_audio = self
            .components
            .iter()
            .any(|c| c.enabled && c.component.needs_audio());
        let needs_system = self
            .components
            .iter()
            .any(|c| c.enabled && c.component.needs_system_uniforms());
        let needs_image = self
            .components
            .iter()
            .any(|c| c.enabled && matches!(c.component, DesignerComponent::ImageOverlay));
        let needs_video = self
            .components
            .iter()
            .any(|c| c.enabled && matches!(c.component, DesignerComponent::VideoOverlay));

        let mut uniforms = String::new();
        uniforms.push_str("uniform float u_time;\n");
        uniforms.push_str("uniform vec2 u_resolution;\n");
        uniforms.push_str("uniform vec2 u_mouse;\n");

        if needs_system {
            uniforms.push_str("uniform float u_cpu;\n");
            uniforms.push_str("uniform float u_ram;\n");
            uniforms.push_str("uniform float u_battery;\n");
        }
        if needs_audio {
            uniforms.push_str("uniform sampler2D iChannel0;\n");
        }
        if needs_image {
            uniforms.push_str("uniform sampler2D iChannel1;\n");
        }
        if needs_video {
            uniforms.push_str("uniform sampler2D iChannel2;\n");
        }

        let mut body = String::new();
        body.push_str("    vec2 uv = gl_FragCoord.xy / u_resolution;\n");
        body.push_str("    vec3 color = vec3(0.0);\n\n");

        // Background components first
        for placed in &self.components {
            if !placed.enabled {
                continue;
            }
            if placed.component.category() == "Backgrounds" {
                body.push_str(&format!(
                    "    // Component: {} at ({:.2}, {:.2}) size ({:.2}, {:.2}) opacity {:.2}\n",
                    placed.component.label(),
                    placed.x,
                    placed.y,
                    placed.width,
                    placed.height,
                    placed.opacity,
                ));
                if let Some(ref custom) = placed.custom_shader {
                    body.push_str(custom);
                } else {
                    body.push_str(placed.component.glsl_snippet());
                }
            }
        }

        // Then overlays
        for placed in &self.components {
            if !placed.enabled {
                continue;
            }
            if placed.component.category() != "Backgrounds" {
                body.push_str(&format!(
                    "    // Component: {} at ({:.2}, {:.2}) size ({:.2}, {:.2}) opacity {:.2}\n",
                    placed.component.label(),
                    placed.x,
                    placed.y,
                    placed.width,
                    placed.height,
                    placed.opacity,
                ));
                if let Some(ref custom) = placed.custom_shader {
                    body.push_str(custom);
                } else {
                    body.push_str(placed.component.glsl_snippet());
                }
            }
        }

        body.push_str("\n    fragColor = vec4(color, 1.0);\n");

        format!(
            "#version 450\n\
            precision highp float;\n\n\
            {uniforms}\n\
            out vec4 fragColor;\n\n\
            void main() {{\n\
            {body}\
            }}\n"
        )
    }

    /// Generate a ShadeConfig for the current design.
    pub fn generate_config(&self) -> kroma_shared::types::ShadeConfig {
        let _needs_audio = self
            .components
            .iter()
            .any(|c| c.enabled && c.component.needs_audio());

        kroma_shared::types::ShadeConfig {
            meta: kroma_shared::types::ShadeMeta {
                name: "Designer Wallpaper".into(),
                author: "Kroma Designer".into(),
                version: "1.0".into(),
                description: "Created with the Kroma Simple Designer".into(),
                tags: vec!["designer".into(), "auto-generated".into()],
            },
            rendering: kroma_shared::types::RenderingConfig {
                target_fps: 30,
                pause_offscreen: true,
                pause_fullscreen: true,
            },
            uniforms: Default::default(),
            textures: Default::default(),
            buffers: Default::default(),
        }
    }
}

impl Panel for DesignerPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let t = ctx.tokens;
        let bg_sec = t.bg_secondary;
        let bg_tert = t.bg_tertiary;
        let txt = t.text_primary;
        let txt_sec = t.text_secondary;
        let accent = t.text_accent;
        let success = t.success;
        let radius = t.border_radius;
        let hover = t.button_hover;

        // Left: component palette
        let mut palette_items: Vec<Element<'_, Message>> = Vec::new();
        let mut current_category = "";

        for comp in DesignerComponent::all() {
            if comp.category() != current_category {
                current_category = comp.category();
                palette_items.push(text(current_category).size(11).color(accent).into());
            }

            let already_added = self
                .components
                .iter()
                .any(|c| c.component == *comp && c.enabled);

            let comp_val = *comp;
            let bg_c = if already_added { success } else { bg_tert };
            palette_items.push(
                button(
                    row![
                        text(comp.icon()).font(icons::ICON_FONT).size(11).color(txt),
                        text(comp.label()).size(11).color(txt),
                    ]
                    .spacing(4)
                    .align_y(iced::alignment::Vertical::Center),
                )
                .on_press(Message::DesignerToggleComponent(comp_val))
                .width(Fill)
                .padding(Padding::from([4, 8]))
                .style(move |_: &Theme, status| {
                    let b = match status {
                        button::Status::Hovered => hover,
                        _ => bg_c,
                    };
                    button::Style {
                        background: Some(iced::Background::Color(b)),
                        text_color: txt,
                        border: Border::default().rounded(radius * 0.5),
                        ..Default::default()
                    }
                })
                .into(),
            );
        }

        // Palette instructions
        palette_items.push(Space::new().width(0).height(8).into());
        palette_items.push(text("Click to add/remove.").size(9).color(txt_sec).into());
        palette_items.push(text("Select below to edit.").size(9).color(txt_sec).into());

        let palette = scrollable(
            column(palette_items)
                .spacing(3)
                .padding(Padding::from([8, 8])),
        )
        .width(200)
        .height(Fill);

        // Center: preview of current design
        let active_components: Vec<_> = self.components.iter().filter(|c| c.enabled).collect();

        let preview_content = if active_components.is_empty() {
            column![
                text("Simple Wallpaper Designer").size(18).color(txt),
                Space::new().width(0).height(8),
                text("Click components on the left to add them to your wallpaper.")
                    .size(12)
                    .color(txt_sec),
                text("Backgrounds are rendered first, then overlays on top.")
                    .size(11)
                    .color(txt_sec),
            ]
            .spacing(4)
        } else {
            let mut comp_list: Vec<Element<'_, Message>> = Vec::new();
            comp_list.push(text("Active Components:").size(13).color(txt).into());

            for (idx, placed) in self.components.iter().enumerate() {
                if !placed.enabled {
                    continue;
                }
                let is_selected = self.selected == Some(idx);
                let audio_suffix = if placed.component.needs_audio() {
                    " (audio)"
                } else {
                    ""
                };
                let pos_info = format!(
                    " [{:.0}%,{:.0}% {:.0}%x{:.0}%]",
                    placed.x * 100.0,
                    placed.y * 100.0,
                    placed.width * 100.0,
                    placed.height * 100.0,
                );
                let remove_idx = idx;
                let select_idx = idx;
                let sel_bg = if is_selected {
                    accent
                } else {
                    iced::Color::TRANSPARENT
                };
                comp_list.push(
                    button(
                        row![
                            text(placed.component.icon())
                                .font(icons::ICON_FONT)
                                .size(11)
                                .color(txt),
                            text(format!(
                                "{}{}{}",
                                placed.component.label(),
                                audio_suffix,
                                pos_info
                            ))
                            .size(10)
                            .color(txt),
                            horizontal(),
                            button(text("X").size(10).color(t.error))
                                .on_press(Message::DesignerRemoveComponent(remove_idx))
                                .padding(Padding::from([2, 6]))
                                .style(move |_: &Theme, _| button::Style {
                                    background: Some(iced::Background::Color(
                                        iced::Color::TRANSPARENT,
                                    )),
                                    ..Default::default()
                                }),
                        ]
                        .spacing(4)
                        .align_y(iced::alignment::Vertical::Center),
                    )
                    .on_press(Message::DesignerSelectComponent(select_idx))
                    .width(Fill)
                    .padding(Padding::from([2, 4]))
                    .style(move |_: &Theme, status| {
                        let b = match status {
                            button::Status::Hovered => hover,
                            _ => sel_bg,
                        };
                        button::Style {
                            background: Some(iced::Background::Color(b)),
                            text_color: txt,
                            border: Border::default().rounded(3.0),
                            ..Default::default()
                        }
                    })
                    .into(),
                );
            }

            // GLSL preview
            let glsl = self.generate_glsl();
            comp_list.push(Space::new().width(0).height(8).into());
            comp_list.push(text("Generated GLSL:").size(11).color(accent).into());
            comp_list.push(
                container(
                    scrollable(text(glsl).size(10).color(txt_sec))
                        .width(Fill)
                        .height(200),
                )
                .style(move |_: &Theme| container::Style {
                    background: Some(iced::Background::Color(bg_tert)),
                    border: Border::default().rounded(4),
                    ..Default::default()
                })
                .padding(8)
                .width(Fill)
                .into(),
            );

            column(comp_list).spacing(4)
        };

        // Generate button
        let gen_btn = button(text("Generate Shade Project").size(13).color(txt))
            .on_press(Message::DesignerGenerate)
            .padding(Padding::from([8, 20]))
            .style(move |_: &Theme, status| {
                let b = match status {
                    button::Status::Hovered => hover,
                    _ => t.button_primary,
                };
                button::Style {
                    background: Some(iced::Background::Color(b)),
                    text_color: txt,
                    border: Border::default().rounded(radius),
                    ..Default::default()
                }
            });

        let center = container(
            column![
                scrollable(preview_content.padding(16).width(Fill))
                    .width(Fill)
                    .height(Fill),
                container(gen_btn).padding(8).center_x(Fill),
            ]
            .width(Fill)
            .height(Fill),
        )
        .style(move |_: &Theme| container::Style {
            background: Some(iced::Background::Color(bg_sec)),
            border: Border::default().rounded(radius),
            ..Default::default()
        })
        .width(Fill)
        .height(Fill);

        // Right: properties panel for selected component
        let properties: Element<'_, Message> = if let Some(sel) = self.selected {
            if let Some(comp) = self.components.get(sel) {
                let idx = sel;
                let mut props: Vec<Element<'_, Message>> = Vec::new();

                props.push(
                    text(format!("Properties: {}", comp.component.label()))
                        .size(12)
                        .color(accent)
                        .into(),
                );
                props.push(Space::new().width(0).height(4).into());

                // Position row
                props.push(text("Position").size(10).color(txt_sec).into());
                props.push(
                    row![
                        text("X:").size(10).color(txt_sec),
                        text_input("", &format!("{:.2}", comp.x))
                            .size(10)
                            .width(50)
                            .on_input(move |v| Message::DesignerSetComponentX(idx, v)),
                        text("Y:").size(10).color(txt_sec),
                        text_input("", &format!("{:.2}", comp.y))
                            .size(10)
                            .width(50)
                            .on_input(move |v| Message::DesignerSetComponentY(idx, v)),
                    ]
                    .spacing(4)
                    .align_y(iced::alignment::Vertical::Center)
                    .into(),
                );

                // Size row
                props.push(text("Size").size(10).color(txt_sec).into());
                props.push(
                    row![
                        text("W:").size(10).color(txt_sec),
                        text_input("", &format!("{:.2}", comp.width))
                            .size(10)
                            .width(50)
                            .on_input(move |v| Message::DesignerSetComponentW(idx, v)),
                        text("H:").size(10).color(txt_sec),
                        text_input("", &format!("{:.2}", comp.height))
                            .size(10)
                            .width(50)
                            .on_input(move |v| Message::DesignerSetComponentH(idx, v)),
                    ]
                    .spacing(4)
                    .align_y(iced::alignment::Vertical::Center)
                    .into(),
                );

                // Opacity slider
                props.push(Space::new().width(0).height(4).into());
                props.push(text("Opacity").size(10).color(txt_sec).into());
                props.push(
                    row![
                        slider(0.0..=1.0, comp.opacity, move |v| {
                            Message::DesignerSetOpacity(idx, v)
                        })
                        .step(0.01)
                        .width(120),
                        text(format!("{:.0}%", comp.opacity * 100.0))
                            .size(10)
                            .color(txt_sec),
                    ]
                    .spacing(4)
                    .align_y(iced::alignment::Vertical::Center)
                    .into(),
                );

                // Asset path (for Image/Video)
                if comp.component.needs_asset_texture() {
                    props.push(Space::new().width(0).height(4).into());
                    props.push(text("Asset Path").size(10).color(txt_sec).into());
                    props.push(
                        text_input("path/to/asset", comp.asset_path.as_deref().unwrap_or(""))
                            .size(10)
                            .width(Fill)
                            .on_input(move |v| Message::DesignerSetAssetPath(idx, v))
                            .into(),
                    );
                }

                // Custom shader override
                props.push(Space::new().width(0).height(6).into());
                props.push(text("Custom GLSL Override").size(10).color(txt_sec).into());
                props.push(
                    text_input(
                        "Leave empty for default",
                        comp.custom_shader.as_deref().unwrap_or(""),
                    )
                    .size(10)
                    .width(Fill)
                    .on_input(move |v| Message::DesignerSetCustomShader(idx, v))
                    .into(),
                );

                container(
                    scrollable(column(props).spacing(3).padding(Padding::from([8, 8])))
                        .height(Fill),
                )
                .style(move |_: &Theme| container::Style {
                    background: Some(iced::Background::Color(bg_tert)),
                    border: Border::default().rounded(radius),
                    ..Default::default()
                })
                .width(220)
                .height(Fill)
                .into()
            } else {
                container(text("Select a component").size(11).color(txt_sec))
                    .width(220)
                    .height(Fill)
                    .into()
            }
        } else {
            container(
                column![
                    text("Properties").size(12).color(txt_sec),
                    Space::new().width(0).height(8),
                    text("Select a component to\nedit its properties.")
                        .size(10)
                        .color(txt_sec),
                ]
                .spacing(4)
                .padding(8),
            )
            .style(move |_: &Theme| container::Style {
                background: Some(iced::Background::Color(bg_tert)),
                border: Border::default().rounded(radius),
                ..Default::default()
            })
            .width(220)
            .height(Fill)
            .into()
        };

        row![palette, center, properties]
            .spacing(4)
            .width(Fill)
            .height(Fill)
            .into()
    }
}
