use std::io::Cursor;
use std::time::Duration;

use imageproc::image::{self, GrayImage};
use ratatui::layout::{Constraint, Flex, Layout};
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Widget};

use crate::tui::Theme;

const SPLASH_IMAGE: &[u8] = include_bytes!("../assets/goku-splash-downscaled.png");

pub struct SplashScreen {
    image: GrayImage,
    current_step: u32,
    total_steps: u32,
}

impl SplashScreen {
    pub fn new(total_steps: u32, is_dark: bool) -> anyhow::Result<Self> {
        let cursor = Cursor::new(SPLASH_IMAGE);
        let img = image::ImageReader::new(cursor)
            .with_guessed_format()?
            .decode()?;

        let image = img.to_luma8();

        Ok(Self {
            image,
            current_step: 0,
            total_steps,
        })
    }

    pub fn is_complete(&self) -> bool {
        self.current_step >= self.total_steps
    }

    pub fn advance(&mut self) {
        if self.current_step < self.total_steps {
            self.current_step += 1;
        }
    }

    /// Calculate brightness based on current animation step
    #[inline(always)]
    fn get_brightness(&self) -> f32 {
        let progress = self.current_step as f32 / self.total_steps as f32;
        if progress < 0.5 {
            progress * 2.0 * 0.8 + 0.2
        } else {
            1.0
        }
    }

    #[inline(always)]
    fn get_pixel(&self, x: u32, y: u32) -> Option<Color> {
        if x >= self.image.width() || y >= self.image.height() {
            return None;
        }

        let brightness = self.get_brightness();
        let luma = self.image.get_pixel(x, y).0[0];
        let value = (luma as f32 * brightness) as u8;

        Some(Color::Rgb(value, value, value))
    }
}

impl Widget for &SplashScreen {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for y in 0..area.height {
            for x in 0..area.width {
                let term_x = area.left() + x;
                let term_y = area.top() + y;

                let img_x = (x as f32 / area.width as f32 * self.image.width() as f32) as u32;
                let img_y = (y as f32 / area.height as f32 * self.image.height() as f32) as u32;

                if let Some(color) = self.get_pixel(img_x, img_y) {
                    buf[(term_x, term_y)].set_bg(color).set_char(' ');
                }
            }
        }
    }
}

pub fn show_splash_screen(
    terminal: &mut Terminal<impl Backend>,
    is_dark: bool,
) -> anyhow::Result<()> {
    let mut splash = SplashScreen::new(10, is_dark)?;

    while !splash.is_complete() {
        terminal.draw(|frame| {
            let area = frame.area();
            frame.render_widget(&splash, area);
        })?;

        splash.advance();
        std::thread::sleep(Duration::from_millis(100));
    }

    terminal.draw(|frame| {
        frame.render_widget(&splash, frame.area());
        render_ascii(frame, is_dark);
    })?;

    std::thread::sleep(Duration::from_millis(1000));

    Ok(())
}

const ASCII_ART: &str = r#"
██████  ██████  ███    ███  ██  ██████   █████   ██       ██       ██    ██
██      ██  ██  ████  ████  ██  ██      ██   ██  ██       ██        ██  ██
██      ██  ██  ██ ████ ██  ██  ██      ███████  ██       ██         ████
██      ██  ██  ██  ██  ██  ██  ██      ██   ██  ██       ██          ██
██████  ██████  ██      ██  ██  ██████  ██   ██  ███████  ███████     ██
"#;

fn center(area: Rect, horizontal: Constraint, vertical: Constraint) -> Rect {
    let [area] = Layout::horizontal([horizontal])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([vertical]).flex(Flex::Center).areas(area);
    area
}

fn render_ascii(frame: &mut Frame, is_dark: bool) {
    let area = frame.area();

    let height = ASCII_ART.trim().lines().count() as u16;
    let width = ASCII_ART
        .trim()
        .lines()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as u16;

    let centered_area = center(area, Constraint::Length(width), Constraint::Length(height));

    // Create the paragraph
    let ascii_paragraph = Paragraph::new(ASCII_ART.trim()).style(
        Style::default()
            .fg(if is_dark {
                Theme::dark().primary
            } else {
                Theme::light().primary
            })
            .add_modifier(Modifier::BOLD),
    );

    frame.render_widget(ascii_paragraph, centered_area);
}
