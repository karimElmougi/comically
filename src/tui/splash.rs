use std::io::Cursor;

use imageproc::image::{self, GrayImage};
use ratatui::layout::Constraint;
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};

use crate::tui::Theme;

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

        let mut image = img.to_luma8();
        if is_dark {
            image::imageops::colorops::invert(&mut image);
        }

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
    fn get_pixel_value(&self, x: u32, y: u32) -> Option<u8> {
        if x >= self.image.width() || y >= self.image.height() {
            return None;
        }

        let brightness = self.get_brightness();
        let luma = self.image.get_pixel(x, y).0[0];
        Some((luma as f32 * brightness) as u8)
    }
}

impl Widget for &SplashScreen {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for y in 0..area.height {
            for x in 0..area.width {
                let term_x = area.left() + x;
                let term_y = area.top() + y;

                let img_x = (x as f32 / area.width as f32 * self.image.width() as f32) as u32;
                let img_y_top =
                    (y as f32 * 2.0 / area.height as f32 * self.image.height() as f32 / 2.0) as u32;
                let img_y_bottom = ((y as f32 * 2.0 + 1.0) / area.height as f32
                    * self.image.height() as f32
                    / 2.0) as u32;

                let top_value = self.get_pixel_value(img_x, img_y_top);
                let bottom_value = self.get_pixel_value(img_x, img_y_bottom);

                match (top_value, bottom_value) {
                    (Some(top), Some(bottom)) => {
                        let cell = &mut buf[(term_x, term_y)];

                        if top > 245 && bottom > 245 {
                            cell.set_char('█').set_fg(grayscale(255));
                        } else if top < 10 && bottom < 10 {
                            cell.set_char('█').set_fg(grayscale(0));
                        } else {
                            let diff = (top as i16 - bottom as i16).abs();

                            if diff > 50 {
                                // Significant difference - use half blocks
                                let top_color = grayscale(top);
                                let bottom_color = grayscale(bottom);

                                if top > bottom {
                                    cell.set_char('▀').set_fg(top_color).set_bg(bottom_color);
                                } else {
                                    cell.set_char('▄').set_fg(bottom_color).set_bg(top_color);
                                }
                            } else {
                                let avg = (top as u16 + bottom as u16) / 2;
                                let gray = avg as u8;

                                // For very light or very dark areas, use absolute black or white
                                if gray < 30 {
                                    cell.set_bg(grayscale(0)).set_char(' ');
                                } else if gray > 225 {
                                    cell.set_bg(grayscale(255)).set_char(' ');
                                } else {
                                    // shading characters only for mid-tones
                                    let ch = match gray {
                                        30..=80 => '░',
                                        81..=130 => '▒',
                                        131..=180 => '▓',
                                        181..=225 => '█',
                                        _ => ' ',
                                    };
                                    let color = grayscale(gray);
                                    cell.set_char(ch).set_fg(color);
                                }
                            }
                        }
                    }
                    (Some(value), None) | (None, Some(value)) => {
                        let color = grayscale(value);
                        buf[(term_x, term_y)].set_bg(color).set_char(' ');
                    }
                    _ => {}
                }
            }
        }
    }
}

#[inline]
fn grayscale(value: u8) -> Color {
    Color::Rgb(value, value, value)
}

const SPLASH_IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/splash.jpg"));

const TITLE: &str = r#"
██████  ██████  ▄████████▄  ██  ██████  ▄████▄  ██      ██      ██  ██
██      ██  ██  ██  ██  ██  ██  ██      ██  ██  ██      ██      ██  ██
██      ██  ██  ██  ██  ██  ██  ██      ██████  ██      ██      ██████
██      ██  ██  ██  ██  ██  ██  ██      ██  ██  ██      ██          ██
██████  ██████  ██  ██  ██  ██  ██████  ██  ██  ██████  ██████  ██████
"#;

fn max_line_width(text: &str) -> u16 {
    text.trim()
        .lines()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as u16
}

pub fn splash_title(frame: &mut Frame, theme: Theme) {
    let area = frame.area();

    let title = TITLE;

    let height = title.trim().lines().count() as u16 + 2;
    let width = max_line_width(title) + 4;

    let centered_area =
        super::utils::center(area, Constraint::Length(width), Constraint::Length(height));

    frame.render_widget(Clear, centered_area);

    let ascii_paragraph = Paragraph::new(Text::from(title.trim()).fg(theme.secondary))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.secondary)
                .border_type(BorderType::QuadrantOutside),
        )
        .alignment(Alignment::Center)
        .bg(if theme.is_dark() {
            Color::Rgb(0, 0, 0)
        } else {
            Color::Rgb(255, 255, 255)
        });

    frame.render_widget(ascii_paragraph, centered_area);
}
