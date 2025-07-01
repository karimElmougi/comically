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
            border: palette::tailwind::STONE.c300,
            content: palette::tailwind::STONE.c100,
            background: palette::tailwind::STONE.c950,
            focused: palette::tailwind::AMBER.c400,
            primary: palette::tailwind::CYAN.c400,
            secondary: palette::tailwind::FUCHSIA.c500,
            key_hint: palette::tailwind::YELLOW.c400,
            error: palette::tailwind::RED.c500,
            success: palette::tailwind::EMERALD.c500,
            warning: palette::tailwind::AMBER.c500,
            scrollbar: Color::White,
            scrollbar_thumb: palette::tailwind::CYAN.c400,
            stage_colors: StageColors {
                extract: palette::tailwind::STONE.c100,
                process: palette::tailwind::STONE.c300,
                mobi: palette::tailwind::STONE.c400,
                epub: palette::tailwind::STONE.c500,
            },
        }
    }

    pub fn light() -> Self {
        Self {
            mode: ThemeMode::Light,
            border: palette::tailwind::STONE.c400,
            content: palette::tailwind::STONE.c800,
            background: palette::tailwind::STONE.c50,
            focused: palette::tailwind::AMBER.c600,
            primary: palette::tailwind::CYAN.c600,
            secondary: palette::tailwind::FUCHSIA.c600,
            key_hint: palette::tailwind::AMBER.c700,
            error: palette::tailwind::RED.c600,
            success: palette::tailwind::EMERALD.c600,
            warning: palette::tailwind::AMBER.c600,
            scrollbar: palette::tailwind::STONE.c300,
            scrollbar_thumb: palette::tailwind::CYAN.c600,
            stage_colors: StageColors {
                extract: palette::tailwind::STONE.c300,
                process: palette::tailwind::STONE.c400,
                mobi: palette::tailwind::STONE.c500,
                epub: palette::tailwind::STONE.c600,
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
