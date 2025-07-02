use ratatui::style::{palette, Color};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThemeMode {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub mode: ThemeMode,
    pub border: Color,
    pub content: Color,
    pub background: Color,
    pub focused: Color,

    pub primary: Color,
    pub primary_bg: Color,
    pub primary_pressed: Color,

    pub secondary: Color,
    pub secondary_bg: Color,
    pub secondary_pressed: Color,

    pub key_hint: Color,
    pub error_fg: Color,
    pub error_bg: Color,
    pub scrollbar_thumb: Color,
    pub stage_colors: StageColors,
    // for text on progress bars
    pub gauge_label: Color,
}

#[derive(Debug, Clone, Copy)]
pub struct StageColors {
    pub extract: Color,
    pub process: Color,
    pub mobi: Color,
    pub epub: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            border: palette::tailwind::SLATE.c400,
            content: palette::tailwind::SLATE.c200,
            background: palette::tailwind::SLATE.c950,
            focused: palette::tailwind::AMBER.c400,
            primary: palette::tailwind::CYAN.c200,
            primary_bg: palette::tailwind::CYAN.c600,
            primary_pressed: palette::tailwind::CYAN.c500,
            secondary: palette::tailwind::FUCHSIA.c300,
            secondary_bg: palette::tailwind::FUCHSIA.c600,
            secondary_pressed: palette::tailwind::FUCHSIA.c500,
            key_hint: palette::tailwind::YELLOW.c300,
            error_fg: palette::tailwind::RED.c400,
            error_bg: palette::tailwind::RED.c800,
            scrollbar_thumb: palette::tailwind::CYAN.c500,
            gauge_label: palette::tailwind::SLATE.c200,
            stage_colors: StageColors {
                extract: palette::tailwind::BLUE.c700,
                process: palette::tailwind::PURPLE.c700,
                mobi: palette::tailwind::PINK.c700,
                epub: palette::tailwind::EMERALD.c700,
            },
        }
    }

    pub fn light() -> Self {
        // paper-like theme inspired by solarized light and old books
        Self {
            mode: ThemeMode::Light,
            border: Color::Rgb(101, 123, 131), // Solarized base00 - darker for better contrast
            content: Color::Rgb(88, 110, 117), // Solarized base01 - better contrast
            background: Color::Rgb(253, 246, 227), // Warm paper color (Solarized base3)
            focused: palette::tailwind::AMBER.c500,
            primary: palette::tailwind::SKY.c500,
            primary_bg: palette::tailwind::SKY.c200,
            primary_pressed: palette::tailwind::SKY.c100,
            secondary: palette::tailwind::VIOLET.c500,
            secondary_bg: palette::tailwind::VIOLET.c200,
            secondary_pressed: palette::tailwind::VIOLET.c100,
            key_hint: palette::tailwind::GREEN.c400,
            error_fg: palette::tailwind::RED.c800,
            error_bg: palette::tailwind::RED.c100,
            scrollbar_thumb: palette::tailwind::SLATE.c400,
            gauge_label: palette::tailwind::SLATE.c700,
            stage_colors: StageColors {
                extract: palette::tailwind::BLUE.c300,
                process: palette::tailwind::PURPLE.c300,
                mobi: palette::tailwind::PINK.c300,
                epub: palette::tailwind::EMERALD.c300,
            },
        }
    }

    pub fn toggle(&mut self) {
        *self = match self.mode {
            ThemeMode::Dark => Self::light(),
            ThemeMode::Light => Self::dark(),
        };
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// Detect terminal background and return appropriate theme using termbg
    /// Falls back to dark theme if detection fails or times out
    pub fn detect() -> Self {
        use std::time::Duration;

        // Try to detect terminal background with a 100ms timeout
        match termbg::theme(Duration::from_millis(100)) {
            Ok(termbg::Theme::Light) => Self::light(),
            Ok(termbg::Theme::Dark) => Self::dark(),
            Err(_) => {
                // If detection fails, default to dark theme
                Self::dark()
            }
        }
    }
}
