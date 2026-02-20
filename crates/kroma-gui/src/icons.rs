//! Material Icons integration for the Kroma GUI.
//!
//! Loads the system-installed Material Icons font and provides constants
//! for commonly used icons as Unicode codepoint strings, plus a helper
//! widget function for rendering icon text with the correct font.

use iced::{Font, Task};

/// The Material Icons font descriptor.
/// Loaded at startup from the system font directory.
pub const ICON_FONT: Font = Font::with_name("Material Icons");

/// System path to the Material Icons TTF file.
const FONT_PATH: &str = "/home/aliemre/.local/share/fonts/material-design-icons.ttf";

/// Fallback: try a few common system paths.
const FONT_PATHS: &[&str] = &[
    "/home/aliemre/.local/share/fonts/material-design-icons.ttf",
    "/usr/share/fonts/material-design-icons/MaterialIcons-Regular.ttf",
    "/usr/share/fonts/TTF/MaterialIcons-Regular.ttf",
    "/usr/share/fonts/truetype/material-design-icons/MaterialIcons-Regular.ttf",
];

/// Load the Material Icons font from the system.
/// Returns a Task that completes when the font is loaded.
pub fn load_icon_font() -> Task<Result<(), iced::font::Error>> {
    // Try each path until we find the font
    for path in FONT_PATHS {
        if let Ok(data) = std::fs::read(path) {
            return iced::font::load(std::borrow::Cow::Owned(data));
        }
    }
    // Primary path
    if let Ok(data) = std::fs::read(FONT_PATH) {
        return iced::font::load(std::borrow::Cow::Owned(data));
    }

    log::warn!("Material Icons font not found on system; icons will not render");
    Task::none()
}

/// Create a text widget using the Material Icons font.
pub fn icon<'a>(codepoint: &'a str) -> iced::widget::Text<'a> {
    iced::widget::text(codepoint).font(ICON_FONT)
}

/// Create an icon + label row — icon on the left, text label on the right.
pub fn icon_label<'a, M: 'a + Clone>(
    codepoint: &'a str,
    label: &'a str,
    size: f32,
) -> iced::widget::Row<'a, M> {
    iced::widget::row![
        iced::widget::text(codepoint).font(ICON_FONT).size(size),
        iced::widget::text(label).size(size),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center)
}

// ─── Material Icons codepoints ───────────────────────────────────────────────
// These are the standard Google Material Icons PUA codepoints.
// Reference: https://github.com/google/material-design-icons/blob/master/font/MaterialIcons-Regular.codepoints

// File types
pub const FOLDER: &str = "\u{E2C7}";           // folder
pub const FILE: &str = "\u{E24D}";             // insert_drive_file
pub const DESCRIPTION: &str = "\u{E873}";       // description
pub const IMAGE: &str = "\u{E3F4}";            // image
pub const VIDEO: &str = "\u{E04B}";            // videocam
pub const AUDIO: &str = "\u{E3A1}";            // audiotrack
pub const MUSIC: &str = "\u{E405}";            // music_note
pub const FONT: &str = "\u{E167}";             // font_download
pub const CODE: &str = "\u{E86F}";             // code
pub const SOURCE: &str = "\u{E1DB}";           // source (or use E86F code)

// Playback
pub const PLAY: &str = "\u{E037}";             // play_arrow
pub const PAUSE: &str = "\u{E034}";            // pause
pub const STOP: &str = "\u{E047}";             // stop
pub const LOOP: &str = "\u{E028}";             // loop

// Actions
pub const SETTINGS: &str = "\u{E8B8}";         // settings
pub const EDIT: &str = "\u{E3C9}";             // edit
pub const SAVE: &str = "\u{E161}";             // save
pub const ADD: &str = "\u{E145}";              // add
pub const REMOVE: &str = "\u{E15B}";           // remove
pub const CLOSE: &str = "\u{E5CD}";            // close
pub const SEARCH: &str = "\u{E8B6}";           // search
pub const REFRESH: &str = "\u{E5D5}";          // refresh
pub const BUILD: &str = "\u{E869}";            // build
pub const TUNE: &str = "\u{E263}";             // tune
pub const LINK: &str = "\u{E157}";             // link
pub const UNDO: &str = "\u{E166}";             // undo
pub const REDO: &str = "\u{E15A}";             // redo

// Status
pub const ERROR: &str = "\u{E000}";            // error
pub const WARNING: &str = "\u{E002}";          // warning
pub const INFO: &str = "\u{E88E}";             // info
pub const CHECK: &str = "\u{E5CA}";            // check / done
pub const CHECK_CIRCLE: &str = "\u{E86C}";     // check_circle
pub const VISIBILITY: &str = "\u{E8F4}";       // visibility

// Navigation
pub const ARROW_FORWARD: &str = "\u{E5C8}";    // arrow_forward
pub const ARROW_BACK: &str = "\u{E5C4}";       // arrow_back
pub const CHEVRON_RIGHT: &str = "\u{E5CC}";    // chevron_right
pub const CHEVRON_LEFT: &str = "\u{E5CB}";     // chevron_left
pub const EXPAND_MORE: &str = "\u{E5CF}";      // expand_more
pub const EXPAND_LESS: &str = "\u{E5CE}";      // expand_less
pub const MENU: &str = "\u{E5D2}";             // menu
pub const NAVIGATE_NEXT: &str = "\u{E409}";    // navigate_next

// Dashboard / System
pub const DASHBOARD: &str = "\u{E871}";         // dashboard
pub const COMPUTER: &str = "\u{E30A}";          // computer
pub const MEMORY: &str = "\u{E322}";            // memory
pub const BATTERY: &str = "\u{E1A4}";           // battery_full
pub const ACCESS_TIME: &str = "\u{E192}";       // access_time (clock)
pub const EVENT: &str = "\u{E878}";             // event (calendar)

// Creative
pub const PALETTE: &str = "\u{E40A}";           // palette
pub const BRUSH: &str = "\u{E3AE}";             // brush
pub const TEXTURE: &str = "\u{E421}";           // texture
pub const GRID: &str = "\u{E3EC}";              // grid_on
pub const STAR: &str = "\u{E838}";              // star
pub const EQUALIZER: &str = "\u{E01D}";         // equalizer (audio spectrum)
pub const RECORD: &str = "\u{E061}";            // fiber_manual_record (dot)

// Layout
pub const VIEW_MODULE: &str = "\u{E8F3}";       // view_module
pub const SWAP_HORIZ: &str = "\u{E8D4}";        // swap_horiz
pub const FILE_DOWNLOAD: &str = "\u{E2C4}";     // file_download
pub const FILE_UPLOAD: &str = "\u{E2C6}";       // file_upload

// Graph/Logic
pub const CONDITIONAL: &str = "\u{E8B8}";       // (re-use settings wheel for IF)
pub const GRAPH_NODE: &str = "\u{E061}";         // fiber_manual_record (dot for node)

// Package
pub const PACKAGE: &str = "\u{E2C4}";           // file_download (closest to package)
