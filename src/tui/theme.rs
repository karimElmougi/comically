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
    pub secondary: Color,
    pub key_hint: Color,
    pub error: Color,
    pub success: Color,
    pub warning: Color,
    pub scrollbar: Color,
    pub scrollbar_thumb: Color,
    pub stage_colors: StageColors,
    pub gauge_label: Color, // Color for text on progress bars
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
            border: palette::tailwind::SLATE.c600,
            content: palette::tailwind::SLATE.c200, // Lighter for better contrast
            background: Color::Rgb(15, 15, 23),     // Very dark blue-black
            focused: palette::tailwind::AMBER.c400,
            primary: palette::tailwind::CYAN.c400,
            secondary: palette::tailwind::FUCHSIA.c400,
            key_hint: palette::tailwind::YELLOW.c300,
            error: palette::tailwind::RED.c400,
            success: palette::tailwind::EMERALD.c400,
            warning: palette::tailwind::ORANGE.c400,
            scrollbar: palette::tailwind::SLATE.c700,
            scrollbar_thumb: palette::tailwind::CYAN.c500,
            gauge_label: palette::tailwind::SLATE.c200, // Same as content for dark mode
            stage_colors: StageColors {
                extract: palette::tailwind::BLUE.c700,   // Darker, more subtle
                process: palette::tailwind::PURPLE.c700, // Darker, more subtle
                mobi: palette::tailwind::PINK.c700,      // Darker, more subtle
                epub: palette::tailwind::EMERALD.c700,   // Darker, more subtle
            },
        }
    }

    pub fn light() -> Self {
        // Paper-like theme inspired by Solarized Light and old books
        Self {
            mode: ThemeMode::Light,
            border: Color::Rgb(147, 161, 161), // Solarized base1
            content: Color::Rgb(88, 110, 117), // Solarized base01 - better contrast
            background: Color::Rgb(253, 246, 227), // Warm paper color (Solarized base3)
            focused: Color::Rgb(181, 137, 0),  // Solarized yellow
            primary: Color::Rgb(38, 139, 210), // Solarized blue
            secondary: Color::Rgb(108, 113, 196), // Solarized violet
            key_hint: Color::Rgb(133, 153, 0), // Solarized green
            error: Color::Rgb(220, 50, 47),    // Solarized red
            success: Color::Rgb(133, 153, 0),  // Solarized green
            warning: Color::Rgb(203, 75, 22),  // Solarized orange
            scrollbar: Color::Rgb(238, 232, 213), // Solarized base2
            scrollbar_thumb: Color::Rgb(147, 161, 161), // Solarized base1
            gauge_label: Color::Rgb(7, 54, 66), // Solarized base02 - dark enough for both backgrounds
            stage_colors: StageColors {
                extract: Color::Rgb(191, 211, 230), // Soft pastel blue
                process: Color::Rgb(211, 191, 230), // Soft pastel lavender
                mobi: Color::Rgb(230, 191, 211),    // Soft pastel pink
                epub: Color::Rgb(191, 230, 211),    // Soft pastel mint
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
