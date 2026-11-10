//! The palette: one accent, one green, one red, and grey for structure.
//!
//! The interface is deliberately colour-poor. Exactly one accent marks the
//! response curve, the band under the cursor and that band's node; green means
//! a gate that said yes — and a journal entry that reached the cable; red
//! means a gate that said no and nothing else. Everything that is only
//! structure — rules, labels, inactive rows — is grey.

use ratatui::style::{Color, Modifier, Style};

/// The single accent colour of the interface.
///
/// It marks exactly three things: the response curve, the band the cursor is
/// on, and that band's node on the curve. Nothing else may use it.
pub const ACCENT: Color = Color::Indexed(208);

/// The colours every widget draws from.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// The one accent.
    pub accent: Color,
    /// A gate that said yes. Green means nothing else.
    pub pass: Color,
    /// A gate that said no. Red means nothing else.
    pub refused: Color,
    /// Rules, labels and anything that is only structure.
    pub structure: Color,
    /// Ordinary text.
    pub text: Color,
    /// A boost.
    pub boost: Color,
    /// A cut.
    pub cut: Color,
    /// A warning that does not block a write.
    pub warn: Color,
    /// A journal entry that reached the cable.
    pub log_ok: Color,
    /// A journal entry that failed midway: what the cable holds is unknown.
    pub log_interrupted: Color,
    /// A journal entry that never reached the flash.
    pub log_simulated: Color,
}

impl Theme {
    /// The default palette.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            accent: ACCENT,
            pass: Color::Green,
            refused: Color::Red,
            structure: Color::DarkGray,
            text: Color::White,
            boost: Color::LightRed,
            cut: Color::LightBlue,
            warn: Color::LightYellow,
            log_ok: Color::Green,
            log_interrupted: Color::LightYellow,
            log_simulated: Color::DarkGray,
        }
    }

    /// The style of the accent.
    #[must_use]
    pub const fn accent_style(&self) -> Style {
        Style::new().fg(self.accent)
    }

    /// The style of ordinary text.
    #[must_use]
    pub const fn text_style(&self) -> Style {
        Style::new().fg(self.text)
    }

    /// The style of structure: rules, labels, inactive rows.
    #[must_use]
    pub const fn structure_style(&self) -> Style {
        Style::new().fg(self.structure)
    }

    /// The style of a section label.
    #[must_use]
    pub const fn label_style(&self) -> Style {
        Style::new().fg(self.text).add_modifier(Modifier::BOLD)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_accent_is_one_colour_and_nothing_else_claims_it() {
        let theme = Theme::new();
        assert_eq!(theme.accent, Color::Indexed(208));
        assert_ne!(theme.accent, theme.pass);
        assert_ne!(theme.accent, theme.refused);
        assert_ne!(theme.accent, theme.structure);
    }

    #[test]
    fn green_and_red_mean_the_verdict_and_nothing_else() {
        let theme = Theme::new();
        assert_eq!(theme.pass, Color::Green);
        assert_eq!(theme.refused, Color::Red);
        // No other colour in the palette is green or red.
        assert_ne!(theme.boost, Color::Green);
        assert_ne!(theme.cut, Color::Green);
        assert_ne!(theme.warn, Color::Green);
        assert_ne!(theme.boost, Color::Red);
        assert_ne!(theme.cut, Color::Red);
        assert_ne!(theme.warn, Color::Red);
    }

    #[test]
    fn the_journal_has_its_own_status_colours() {
        let theme = Theme::new();
        assert_eq!(theme.log_ok, Color::Green, "a write that reached the cable");
        assert_eq!(
            theme.log_interrupted,
            Color::LightYellow,
            "a write that failed midway"
        );
        assert_eq!(
            theme.log_simulated,
            Color::DarkGray,
            "a write that never reached the flash"
        );
    }
}
