//! Color and spacing tokens for the theme system.
//!
//! Every panel uses these tokens instead of hardcoded colors, enabling
//! consistent theme switching.

use iced::Color;

/// Complete set of visual tokens for one theme.
#[derive(Debug, Clone)]
pub struct ThemeTokens {
    // Backgrounds
    pub bg_primary: Color,
    pub bg_secondary: Color,
    pub bg_tertiary: Color,
    #[allow(dead_code)]
    pub bg_accent: Color,

    // Text
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_accent: Color,

    // Borders
    pub border_default: Color,
    pub border_focused: Color,

    // Semantic
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,

    // Node editor
    pub wire_float: Color,
    pub wire_vec2: Color,
    pub wire_vec3: Color,
    pub wire_vec4: Color,
    pub wire_color: Color,
    pub node_bg: Color,
    pub node_header: Color,
    pub node_selected: Color,

    // Interactive
    pub button_primary: Color,
    pub button_hover: Color,
    pub button_pressed: Color,
    pub tab_active: Color,
    pub tab_inactive: Color,
    pub drop_zone: Color,

    // Spacing
    pub spacing_xs: f32,
    pub spacing_sm: f32,
    pub spacing_md: f32,
    #[allow(dead_code)]
    pub spacing_lg: f32,
    #[allow(dead_code)]
    pub spacing_xl: f32,
    pub border_radius: f32,
    pub font_size_sm: f32,
    pub font_size_md: f32,
    pub font_size_lg: f32,
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a)
}

// ---------------------------------------------------------------------------
// Theme implementations
// ---------------------------------------------------------------------------

pub fn tokyo_night() -> ThemeTokens {
    ThemeTokens {
        bg_primary: rgb(0x1a, 0x1b, 0x26),
        bg_secondary: rgb(0x24, 0x28, 0x3b),
        bg_tertiary: rgb(0x2f, 0x33, 0x4d),
        bg_accent: rgb(0x3d, 0x59, 0xa1),
        text_primary: rgb(0xc0, 0xca, 0xf5),
        text_secondary: rgb(0x56, 0x5f, 0x89),
        text_accent: rgb(0x7a, 0xa2, 0xf7),
        border_default: rgb(0x3b, 0x40, 0x61),
        border_focused: rgb(0x7a, 0xa2, 0xf7),
        success: rgb(0x9e, 0xce, 0x6a),
        warning: rgb(0xe0, 0xaf, 0x68),
        error: rgb(0xf7, 0x76, 0x8e),
        info: rgb(0x7d, 0xcf, 0xff),
        wire_float: rgb(0x9e, 0xce, 0x6a),
        wire_vec2: rgb(0x7a, 0xa2, 0xf7),
        wire_vec3: rgb(0xbb, 0x9a, 0xf7),
        wire_vec4: rgb(0xff, 0x9e, 0x64),
        wire_color: rgb(0xf7, 0x76, 0x8e),
        node_bg: rgb(0x24, 0x28, 0x3b),
        node_header: rgb(0x2f, 0x33, 0x4d),
        node_selected: rgba(0x7a, 0xa2, 0xf7, 0.3),
        button_primary: rgb(0x7a, 0xa2, 0xf7),
        button_hover: rgb(0x89, 0xb4, 0xfa),
        button_pressed: rgb(0x5a, 0x82, 0xd7),
        tab_active: rgb(0x3d, 0x59, 0xa1),
        tab_inactive: rgb(0x24, 0x28, 0x3b),
        drop_zone: rgba(0x7a, 0xa2, 0xf7, 0.2),
        spacing_xs: 2.0,
        spacing_sm: 4.0,
        spacing_md: 8.0,
        spacing_lg: 16.0,
        spacing_xl: 24.0,
        border_radius: 4.0,
        font_size_sm: 12.0,
        font_size_md: 14.0,
        font_size_lg: 18.0,
    }
}

pub fn catppuccin_mocha() -> ThemeTokens {
    ThemeTokens {
        bg_primary: rgb(0x1e, 0x1e, 0x2e),
        bg_secondary: rgb(0x31, 0x32, 0x44),
        bg_tertiary: rgb(0x45, 0x47, 0x5a),
        bg_accent: rgb(0x58, 0x5b, 0x70),
        text_primary: rgb(0xcd, 0xd6, 0xf4),
        text_secondary: rgb(0xa6, 0xad, 0xc8),
        text_accent: rgb(0x89, 0xb4, 0xfa),
        border_default: rgb(0x45, 0x47, 0x5a),
        border_focused: rgb(0x89, 0xb4, 0xfa),
        success: rgb(0xa6, 0xe3, 0xa1),
        warning: rgb(0xf9, 0xe2, 0xaf),
        error: rgb(0xf3, 0x8b, 0xa8),
        info: rgb(0x89, 0xdc, 0xeb),
        wire_float: rgb(0xa6, 0xe3, 0xa1),
        wire_vec2: rgb(0x89, 0xb4, 0xfa),
        wire_vec3: rgb(0xcb, 0xa6, 0xf7),
        wire_vec4: rgb(0xfa, 0xb3, 0x87),
        wire_color: rgb(0xf3, 0x8b, 0xa8),
        node_bg: rgb(0x31, 0x32, 0x44),
        node_header: rgb(0x45, 0x47, 0x5a),
        node_selected: rgba(0x89, 0xb4, 0xfa, 0.3),
        button_primary: rgb(0x89, 0xb4, 0xfa),
        button_hover: rgb(0xa0, 0xc4, 0xfb),
        button_pressed: rgb(0x72, 0x9d, 0xe3),
        tab_active: rgb(0x58, 0x5b, 0x70),
        tab_inactive: rgb(0x31, 0x32, 0x44),
        drop_zone: rgba(0x89, 0xb4, 0xfa, 0.2),
        spacing_xs: 2.0,
        spacing_sm: 4.0,
        spacing_md: 8.0,
        spacing_lg: 16.0,
        spacing_xl: 24.0,
        border_radius: 4.0,
        font_size_sm: 12.0,
        font_size_md: 14.0,
        font_size_lg: 18.0,
    }
}

pub fn nord() -> ThemeTokens {
    ThemeTokens {
        bg_primary: rgb(0x2e, 0x34, 0x40),
        bg_secondary: rgb(0x3b, 0x42, 0x52),
        bg_tertiary: rgb(0x43, 0x4c, 0x5e),
        bg_accent: rgb(0x4c, 0x56, 0x6a),
        text_primary: rgb(0xec, 0xef, 0xf4),
        text_secondary: rgb(0xd8, 0xde, 0xe9),
        text_accent: rgb(0x88, 0xc0, 0xd0),
        border_default: rgb(0x4c, 0x56, 0x6a),
        border_focused: rgb(0x88, 0xc0, 0xd0),
        success: rgb(0xa3, 0xbe, 0x8c),
        warning: rgb(0xeb, 0xcb, 0x8b),
        error: rgb(0xbf, 0x61, 0x6a),
        info: rgb(0x81, 0xa1, 0xc1),
        wire_float: rgb(0xa3, 0xbe, 0x8c),
        wire_vec2: rgb(0x88, 0xc0, 0xd0),
        wire_vec3: rgb(0xb4, 0x8e, 0xad),
        wire_vec4: rgb(0xd0, 0x87, 0x70),
        wire_color: rgb(0xbf, 0x61, 0x6a),
        node_bg: rgb(0x3b, 0x42, 0x52),
        node_header: rgb(0x43, 0x4c, 0x5e),
        node_selected: rgba(0x88, 0xc0, 0xd0, 0.3),
        button_primary: rgb(0x88, 0xc0, 0xd0),
        button_hover: rgb(0x8f, 0xbc, 0xbb),
        button_pressed: rgb(0x5e, 0x81, 0xac),
        tab_active: rgb(0x4c, 0x56, 0x6a),
        tab_inactive: rgb(0x3b, 0x42, 0x52),
        drop_zone: rgba(0x88, 0xc0, 0xd0, 0.2),
        spacing_xs: 2.0,
        spacing_sm: 4.0,
        spacing_md: 8.0,
        spacing_lg: 16.0,
        spacing_xl: 24.0,
        border_radius: 4.0,
        font_size_sm: 12.0,
        font_size_md: 14.0,
        font_size_lg: 18.0,
    }
}

pub fn dracula() -> ThemeTokens {
    ThemeTokens {
        bg_primary: rgb(0x28, 0x2a, 0x36),
        bg_secondary: rgb(0x44, 0x47, 0x5a),
        bg_tertiary: rgb(0x6c, 0x71, 0x86),
        bg_accent: rgb(0xbd, 0x93, 0xf9),
        text_primary: rgb(0xf8, 0xf8, 0xf2),
        text_secondary: rgb(0x62, 0x72, 0xa4),
        text_accent: rgb(0xbd, 0x93, 0xf9),
        border_default: rgb(0x44, 0x47, 0x5a),
        border_focused: rgb(0xbd, 0x93, 0xf9),
        success: rgb(0x50, 0xfa, 0x7b),
        warning: rgb(0xf1, 0xfa, 0x8c),
        error: rgb(0xff, 0x55, 0x55),
        info: rgb(0x8b, 0xe9, 0xfd),
        wire_float: rgb(0x50, 0xfa, 0x7b),
        wire_vec2: rgb(0x8b, 0xe9, 0xfd),
        wire_vec3: rgb(0xbd, 0x93, 0xf9),
        wire_vec4: rgb(0xff, 0xb8, 0x6c),
        wire_color: rgb(0xff, 0x79, 0xc6),
        node_bg: rgb(0x44, 0x47, 0x5a),
        node_header: rgb(0x6c, 0x71, 0x86),
        node_selected: rgba(0xbd, 0x93, 0xf9, 0.3),
        button_primary: rgb(0xbd, 0x93, 0xf9),
        button_hover: rgb(0xd6, 0xac, 0xff),
        button_pressed: rgb(0x99, 0x70, 0xd6),
        tab_active: rgb(0x44, 0x47, 0x5a),
        tab_inactive: rgb(0x28, 0x2a, 0x36),
        drop_zone: rgba(0xbd, 0x93, 0xf9, 0.2),
        spacing_xs: 2.0,
        spacing_sm: 4.0,
        spacing_md: 8.0,
        spacing_lg: 16.0,
        spacing_xl: 24.0,
        border_radius: 4.0,
        font_size_sm: 12.0,
        font_size_md: 14.0,
        font_size_lg: 18.0,
    }
}

pub fn one_dark() -> ThemeTokens {
    ThemeTokens {
        bg_primary: rgb(0x28, 0x2c, 0x34),
        bg_secondary: rgb(0x31, 0x35, 0x3f),
        bg_tertiary: rgb(0x3e, 0x44, 0x51),
        bg_accent: rgb(0x61, 0xaf, 0xef),
        text_primary: rgb(0xab, 0xb2, 0xbf),
        text_secondary: rgb(0x5c, 0x63, 0x70),
        text_accent: rgb(0x61, 0xaf, 0xef),
        border_default: rgb(0x3e, 0x44, 0x51),
        border_focused: rgb(0x61, 0xaf, 0xef),
        success: rgb(0x98, 0xc3, 0x79),
        warning: rgb(0xe5, 0xc0, 0x7b),
        error: rgb(0xe0, 0x6c, 0x75),
        info: rgb(0x56, 0xb6, 0xc2),
        wire_float: rgb(0x98, 0xc3, 0x79),
        wire_vec2: rgb(0x61, 0xaf, 0xef),
        wire_vec3: rgb(0xc6, 0x78, 0xdd),
        wire_vec4: rgb(0xd1, 0x9a, 0x66),
        wire_color: rgb(0xe0, 0x6c, 0x75),
        node_bg: rgb(0x31, 0x35, 0x3f),
        node_header: rgb(0x3e, 0x44, 0x51),
        node_selected: rgba(0x61, 0xaf, 0xef, 0.3),
        button_primary: rgb(0x61, 0xaf, 0xef),
        button_hover: rgb(0x79, 0xc0, 0xf0),
        button_pressed: rgb(0x4a, 0x8e, 0xc9),
        tab_active: rgb(0x3e, 0x44, 0x51),
        tab_inactive: rgb(0x31, 0x35, 0x3f),
        drop_zone: rgba(0x61, 0xaf, 0xef, 0.2),
        spacing_xs: 2.0,
        spacing_sm: 4.0,
        spacing_md: 8.0,
        spacing_lg: 16.0,
        spacing_xl: 24.0,
        border_radius: 4.0,
        font_size_sm: 12.0,
        font_size_md: 14.0,
        font_size_lg: 18.0,
    }
}
