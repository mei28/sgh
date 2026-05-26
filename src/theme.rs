use ratatui::style::{Color, Modifier, Style};

/// Modern dark palette. RGB tuples are stored explicitly so the theme remains
/// readable when comparing against the design notes in `.tmp/design-plan.md`.
pub struct Theme {
    pub primary: Color,
    pub accent: Color,
    pub success: Color,
    pub muted: Color,
    pub border: Color,
    pub border_focused: Color,
    pub selection_bg: Color,
    pub selection_marker: Color,
    pub match_highlight: Color,
    pub text: Color,
    pub text_dim: Color,
}

impl Theme {
    pub const fn dark() -> Self {
        Self {
            primary: Color::Rgb(0x7D, 0xD3, 0xFC),       // sky-300
            accent: Color::Rgb(0xC0, 0x84, 0xFC),        // purple-400
            success: Color::Rgb(0x86, 0xEF, 0xAC),       // green-300
            muted: Color::Rgb(0x64, 0x74, 0x8B),         // slate-500
            border: Color::Rgb(0x33, 0x41, 0x55),        // slate-700
            border_focused: Color::Rgb(0x7D, 0xD3, 0xFC), // sky-300
            selection_bg: Color::Rgb(0x1E, 0x29, 0x3B),  // slate-800
            selection_marker: Color::Rgb(0xC0, 0x84, 0xFC),
            match_highlight: Color::Rgb(0xFB, 0xBF, 0x24), // amber-400
            text: Color::Rgb(0xE2, 0xE8, 0xF0),          // slate-200
            text_dim: Color::Rgb(0x94, 0xA3, 0xB8),      // slate-400
        }
    }

    pub fn header_style(&self) -> Style {
        Style::default()
            .fg(self.text_dim)
            .add_modifier(Modifier::BOLD)
    }

    pub fn selection_style(&self) -> Style {
        Style::default()
            .bg(self.selection_bg)
            .add_modifier(Modifier::BOLD)
    }

    pub fn border_style(&self) -> Style {
        Style::default().fg(self.border)
    }

    pub fn border_focused_style(&self) -> Style {
        Style::default().fg(self.border_focused)
    }

    pub fn match_style(&self) -> Style {
        Style::default()
            .fg(self.match_highlight)
            .add_modifier(Modifier::BOLD)
    }
}
