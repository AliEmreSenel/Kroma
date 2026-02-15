//! Theme system — user-selectable themes with full color tokens.
//!
//! Each theme provides a complete set of color/spacing tokens that all
//! panels use for consistent styling.

pub mod tokens;

pub use tokens::ThemeTokens;

// ---------------------------------------------------------------------------
// Built-in theme names
// ---------------------------------------------------------------------------

/// Available built-in themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KromaThemeId {
    TokyoNight,
    CatppuccinMocha,
    Nord,
    Dracula,
    OneDark,
}

impl KromaThemeId {
    /// All available themes.
    pub fn all() -> &'static [KromaThemeId] {
        &[
            Self::TokyoNight,
            Self::CatppuccinMocha,
            Self::Nord,
            Self::Dracula,
            Self::OneDark,
        ]
    }

    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::TokyoNight => "Tokyo Night",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::Nord => "Nord",
            Self::Dracula => "Dracula",
            Self::OneDark => "One Dark",
        }
    }

    /// Map to an iced built-in theme (closest match for base styling).
    pub fn iced_theme(&self) -> iced::Theme {
        match self {
            Self::TokyoNight => iced::Theme::TokyoNight,
            Self::CatppuccinMocha => iced::Theme::CatppuccinMocha,
            Self::Nord => iced::Theme::Nord,
            Self::Dracula => iced::Theme::Dracula,
            Self::OneDark => iced::Theme::Ferra, // closest dark theme
        }
    }

    /// Get the full set of Kroma color/spacing tokens for this theme.
    pub fn tokens(&self) -> ThemeTokens {
        match self {
            Self::TokyoNight => tokens::tokyo_night(),
            Self::CatppuccinMocha => tokens::catppuccin_mocha(),
            Self::Nord => tokens::nord(),
            Self::Dracula => tokens::dracula(),
            Self::OneDark => tokens::one_dark(),
        }
    }
}

impl std::fmt::Display for KromaThemeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}
