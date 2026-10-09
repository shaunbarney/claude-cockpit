//! Semantic colour tokens. Widgets name *meaning* (`muted`, `ok`, `series[1]`),
//! never a literal colour, so a palette change is one edit here.
//!
//! The palette resolves against three capability tiers — monochrome, 16 ANSI
//! colours, and 24-bit truecolor — following the rule that a TUI should be
//! *usable* at 16 colours and *beautiful* at true color. `NO_COLOR` is honoured
//! unconditionally. Because colour alone must never carry meaning, status is
//! dual-coded: [`Level`] supplies a distinct glyph as well as a colour, so the
//! dashboard still reads correctly in a monochrome terminal or a screenshot.

use ratatui::style::{Color, Modifier, Style};

/// How much colour the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// `NO_COLOR` or an explicit mono request — shape and weight only.
    Mono,
    /// Basic ANSI. Respects the user's own terminal palette.
    Ansi16,
    /// 24-bit RGB.
    True,
}

/// Detect terminal colour capability from the environment.
///
/// `NO_COLOR` wins over everything (any value, per the spec). Otherwise
/// `COLORTERM` advertising truecolor promotes to 24-bit; everything else stays
/// on the safe ANSI tier.
pub fn detect_depth() -> Depth {
    if std::env::var_os("NO_COLOR").is_some() {
        return Depth::Mono;
    }
    match std::env::var("COLORTERM").unwrap_or_default().as_str() {
        "truecolor" | "24bit" => Depth::True,
        _ => Depth::Ansi16,
    }
}

/// A semantic status level. Each carries both a colour and a distinct glyph so
/// the dashboard never depends on colour to be readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Healthy, running, clean.
    Ok,
    /// Needs attention but not broken — dirty tree, degraded health.
    Warn,
    /// Failed, exited, unreachable.
    Err,
    /// Present but doing nothing.
    Idle,
    /// Live right now — the agent that is currently working.
    Active,
    /// Worth knowing, not a problem — e.g. commits ahead of the base branch.
    Info,
}

impl Level {
    /// The glyph for this level. Deliberately distinct shapes, not five dots.
    pub fn glyph(self) -> &'static str {
        match self {
            Level::Ok => "●",
            Level::Warn => "◐",
            Level::Err => "✕",
            Level::Idle => "○",
            Level::Active => "◉",
            Level::Info => "◆",
        }
    }
}

/// The resolved palette.
#[derive(Debug, Clone)]
pub struct Theme {
    pub depth: Depth,
    /// Brand colour: titles, focus, the primary chart series.
    pub accent: Color,
    /// A recessive accent for fills and secondary emphasis.
    pub accent_dim: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    pub info: Color,
    /// Default foreground. `Reset` keeps the user's own terminal colour.
    pub fg: Color,
    /// Secondary text — still readable, clearly subordinate.
    pub muted: Color,
    /// Tertiary text: units, separators, placeholders.
    pub dim: Color,
    pub border: Color,
    pub focus_border: Color,
    /// Background for a freshly-changed row.
    pub flash: Color,
    /// Chart series, in preference order.
    pub series: [Color; 4],
}

impl Default for Theme {
    fn default() -> Self {
        Theme::new(detect_depth(), None)
    }
}

/// Parse `#rrggbb` (or `rrggbb`) into an RGB colour.
pub fn parse_hex(s: &str) -> Option<Color> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Color::Rgb(
        ((v >> 16) & 0xFF) as u8,
        ((v >> 8) & 0xFF) as u8,
        (v & 0xFF) as u8,
    ))
}

impl Theme {
    /// Build the palette for `depth`, optionally overriding the accent (only
    /// meaningful on the truecolor tier — the ANSI tier has no free slots and
    /// mono has no colour at all).
    pub fn new(depth: Depth, accent_override: Option<Color>) -> Self {
        match depth {
            Depth::Mono => Theme {
                depth,
                accent: Color::Reset,
                accent_dim: Color::Reset,
                ok: Color::Reset,
                warn: Color::Reset,
                err: Color::Reset,
                info: Color::Reset,
                fg: Color::Reset,
                muted: Color::Reset,
                dim: Color::Reset,
                border: Color::Reset,
                focus_border: Color::Reset,
                flash: Color::Reset,
                series: [Color::Reset; 4],
            },
            Depth::Ansi16 => Theme {
                depth,
                accent: Color::Magenta,
                accent_dim: Color::Magenta,
                ok: Color::Green,
                warn: Color::Yellow,
                err: Color::Red,
                info: Color::Cyan,
                fg: Color::Reset,
                muted: Color::Gray,
                dim: Color::DarkGray,
                border: Color::DarkGray,
                focus_border: Color::Magenta,
                flash: Color::Magenta,
                series: [Color::Magenta, Color::Cyan, Color::Green, Color::Yellow],
            },
            Depth::True => Theme {
                depth,
                accent: accent_override.unwrap_or(Color::Rgb(0x7C, 0x5C, 0xFF)),
                accent_dim: Color::Rgb(0x4A, 0x36, 0xA8),
                ok: Color::Rgb(0x3F, 0xC9, 0x7E),
                warn: Color::Rgb(0xE8, 0xB3, 0x39),
                err: Color::Rgb(0xE5, 0x5C, 0x5C),
                info: Color::Rgb(0x4C, 0xC2, 0xE0),
                fg: Color::Reset,
                muted: Color::Rgb(0x9A, 0x9A, 0xB0),
                dim: Color::Rgb(0x5E, 0x5E, 0x70),
                border: Color::Rgb(0x3A, 0x3A, 0x48),
                focus_border: accent_override.unwrap_or(Color::Rgb(0x7C, 0x5C, 0xFF)),
                flash: Color::Rgb(0x7C, 0x5C, 0xFF),
                series: [
                    accent_override.unwrap_or(Color::Rgb(0x7C, 0x5C, 0xFF)),
                    Color::Rgb(0x4C, 0xC2, 0xE0),
                    Color::Rgb(0x3F, 0xC9, 0x7E),
                    Color::Rgb(0xE8, 0xB3, 0x39),
                ],
            },
        }
    }

    /// Resolve from user config, falling back to environment detection.
    pub fn from_config(cfg: &crate::config::ThemeConfig) -> Self {
        let depth = match cfg.mode.as_deref().map(str::trim) {
            Some("mono") | Some("none") => Depth::Mono,
            Some("ansi") | Some("16") => Depth::Ansi16,
            Some("true") | Some("truecolor") | Some("24bit") => Depth::True,
            // An explicit accent is pointless below the truecolor tier, but the
            // user asking for one is a strong hint their terminal supports it.
            _ => detect_depth(),
        };
        // NO_COLOR is not overridable by config — it is a user-level opt-out.
        let depth = if std::env::var_os("NO_COLOR").is_some() {
            Depth::Mono
        } else {
            depth
        };
        Theme::new(depth, cfg.accent.as_deref().and_then(parse_hex))
    }

    /// Colour for a status level.
    pub fn level_color(&self, l: Level) -> Color {
        match l {
            Level::Ok => self.ok,
            Level::Warn => self.warn,
            Level::Err => self.err,
            Level::Idle => self.dim,
            Level::Active => self.accent,
            Level::Info => self.info,
        }
    }

    /// Fully-styled status glyph: shape *and* colour.
    pub fn level_span(&self, l: Level) -> ratatui::text::Span<'static> {
        let mut st = Style::new().fg(self.level_color(l));
        // With no colour, weight has to do the work instead.
        if self.depth == Depth::Mono {
            st = match l {
                Level::Active | Level::Err => st.add_modifier(Modifier::BOLD),
                Level::Idle => st.add_modifier(Modifier::DIM),
                _ => st,
            };
        }
        ratatui::text::Span::styled(l.glyph(), st)
    }

    pub fn title(&self) -> Style {
        Style::new().fg(self.accent).add_modifier(Modifier::BOLD)
    }
    pub fn dim_style(&self) -> Style {
        let s = Style::new().fg(self.dim);
        if self.depth == Depth::Mono {
            s.add_modifier(Modifier::DIM)
        } else {
            s
        }
    }
    pub fn muted_style(&self) -> Style {
        Style::new().fg(self.muted)
    }
    /// Border for a widget, brightened and bolded while focused.
    pub fn border_style(&self, focused: bool) -> Style {
        if focused {
            Style::new()
                .fg(self.focus_border)
                .add_modifier(Modifier::BOLD)
        } else {
            self.dim_style()
        }
    }

    /// Row highlight for a row that changed `intensity` ago (1.0 → 0.0).
    ///
    /// Truecolor fades the accent out against the background; the lower tiers
    /// have no blend to give, so they step from bold to nothing.
    pub fn flash_style(&self, intensity: f32) -> Option<Style> {
        if intensity <= 0.0 {
            return None;
        }
        match self.depth {
            Depth::True => {
                let Color::Rgb(r, g, b) = self.flash else {
                    return Some(Style::new().add_modifier(Modifier::BOLD));
                };
                // Blend toward the terminal background, assumed dark. A flash at
                // full intensity sits at ~45% accent so text stays legible.
                let k = intensity * 0.45;
                let mix = |c: u8| (c as f32 * k) as u8;
                Some(Style::new().bg(Color::Rgb(mix(r), mix(g), mix(b))))
            }
            _ if intensity > 0.5 => Some(Style::new().add_modifier(Modifier::BOLD)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_colors_distinct_in_color_modes() {
        let t = Theme::new(Depth::True, None);
        assert_ne!(t.ok, t.err);
        assert_ne!(t.warn, t.err);
        assert_eq!(t.accent, Color::Rgb(0x7C, 0x5C, 0xFF));
    }

    #[test]
    fn mono_is_colorless_but_levels_stay_distinguishable() {
        let t = Theme::new(Depth::Mono, None);
        assert_eq!(t.ok, Color::Reset);
        assert_eq!(t.err, Color::Reset);
        // Colour collapses; glyphs must not.
        let glyphs = [
            Level::Ok,
            Level::Warn,
            Level::Err,
            Level::Idle,
            Level::Active,
        ]
        .map(|l| l.glyph());
        let mut uniq = glyphs.to_vec();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), 5, "every level needs its own shape");
    }

    #[test]
    fn accent_override_applies_on_truecolor() {
        let t = Theme::new(Depth::True, parse_hex("#ff0000"));
        assert_eq!(t.accent, Color::Rgb(255, 0, 0));
        assert_eq!(t.series[0], Color::Rgb(255, 0, 0));
    }

    #[test]
    fn hex_parsing_round_trips_and_rejects_junk() {
        assert_eq!(parse_hex("#7C5CFF"), Some(Color::Rgb(0x7C, 0x5C, 0xFF)));
        assert_eq!(parse_hex("7c5cff"), Some(Color::Rgb(0x7C, 0x5C, 0xFF)));
        assert_eq!(parse_hex("#fff"), None);
        assert_eq!(parse_hex("nope!!"), None);
    }

    #[test]
    fn flash_fades_to_nothing() {
        let t = Theme::new(Depth::True, None);
        assert!(t.flash_style(1.0).is_some());
        assert!(t.flash_style(0.0).is_none());
        // A fresh flash must be visibly stronger than a stale one.
        let hot = t.flash_style(1.0).unwrap();
        let cool = t.flash_style(0.2).unwrap();
        assert_ne!(hot.bg, cool.bg);
    }

    #[test]
    fn mono_flash_uses_weight_not_color() {
        let t = Theme::new(Depth::Mono, None);
        let s = t.flash_style(1.0).unwrap();
        assert!(s.bg.is_none());
        assert!(s.add_modifier.contains(Modifier::BOLD));
        assert!(t.flash_style(0.1).is_none());
    }

    #[test]
    fn focus_border_is_emphasised() {
        let t = Theme::new(Depth::True, None);
        assert!(t.border_style(true).add_modifier.contains(Modifier::BOLD));
        assert_eq!(t.border_style(false).fg, Some(t.dim));
    }
}
