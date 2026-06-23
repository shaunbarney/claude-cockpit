//! Colour theme. Single source of truth — widgets never hardcode colours.
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone)]
pub struct Theme {
    pub accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    pub dim: Color,
    // reserved: explicit default foreground for future widgets that need it over Color::Reset.
    #[allow(dead_code)]
    pub fg: Color,
    pub focus_border: Color,
}
impl Default for Theme {
    fn default() -> Self {
        Theme {
            accent: Color::Rgb(0x7C, 0x5C, 0xFF),
            ok: Color::Green,
            warn: Color::Yellow,
            err: Color::Red,
            dim: Color::DarkGray,
            fg: Color::Reset,
            focus_border: Color::Rgb(0x7C, 0x5C, 0xFF),
        }
    }
}
impl Theme {
    pub fn title(&self) -> Style {
        Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
    }
    pub fn dim_style(&self) -> Style {
        Style::new().fg(self.dim)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_colors_distinct() {
        let t = Theme::default();
        assert_ne!(t.ok, t.err);
        assert_ne!(t.warn, t.err);
        assert_eq!(t.accent, Color::Rgb(0x7C, 0x5C, 0xFF));
    }
}
