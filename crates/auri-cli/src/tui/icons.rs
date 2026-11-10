//! The glyphs the interface draws with.
//!
//! Two sets: the Unicode one, and a plain ASCII one for terminals (or fonts)
//! that cannot show it. `--ascii` picks the second. Every glyph here is
//! ordinary Unicode — nothing from a private use area, nothing from an icon
//! font.

/// One set of glyphs.
#[derive(Debug, Clone, Copy)]
pub struct Icons {
    /// The note in the header.
    pub note: &'static str,
    /// The marker of the band table.
    pub list: &'static str,
    /// The marker of the response plot.
    pub wave: &'static str,
    /// A ticked confirmation.
    pub checked: &'static str,
    /// An unticked confirmation.
    pub unchecked: &'static str,
    /// A blocking reason.
    pub cross: &'static str,
    /// A warning that does not block.
    pub warn: &'static str,
    /// Something that is in order.
    pub tick: &'static str,
    /// The cursor of the checklist.
    pub arrow: char,
    /// A band node that is not the selected one.
    pub node: char,
    /// The node of the selected band.
    pub node_selected: char,
    /// The filled part of a gain bar.
    pub bar_fill: char,
    /// The empty part of a gain bar.
    pub bar_track: char,
    /// The zero of a gain bar.
    pub bar_zero: char,
    /// A horizontal rule.
    pub rule: char,
    /// A vertical rule.
    pub vertical: char,
    /// The corners of the curve: top-left, top-right, bottom-left, bottom-right.
    pub corners: [char; 4],
    /// The separator dot.
    pub dot: char,
}

impl Icons {
    /// The Unicode set.
    #[must_use]
    pub const fn unicode() -> Self {
        Self {
            note: "♫",
            list: "≡",
            wave: "∿",
            checked: "☑",
            unchecked: "☐",
            cross: "✗",
            warn: "⚠",
            tick: "✓",
            arrow: '▸',
            node: '○',
            node_selected: '●',
            bar_fill: '▄',
            bar_track: '─',
            bar_zero: '│',
            rule: '─',
            vertical: '│',
            corners: ['╭', '╮', '╰', '╯'],
            dot: '·',
        }
    }

    /// The ASCII set: the same drawing, without a single non-ASCII byte.
    #[must_use]
    pub const fn ascii() -> Self {
        Self {
            note: "*",
            list: "=",
            wave: "~",
            checked: "[x]",
            unchecked: "[ ]",
            cross: "x",
            warn: "!",
            tick: "ok",
            arrow: '>',
            node: 'o',
            node_selected: '*',
            bar_fill: '#',
            bar_track: '-',
            bar_zero: '|',
            rule: '-',
            vertical: '|',
            corners: ['+', '+', '+', '+'],
            dot: ':',
        }
    }

    /// The set that matches the mode.
    #[must_use]
    pub const fn for_mode(ascii: bool) -> Self {
        if ascii {
            Self::ascii()
        } else {
            Self::unicode()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every glyph of the Unicode set, one by one.
    fn unicode_glyphs(icons: &Icons) -> Vec<char> {
        let mut glyphs: Vec<char> = Vec::new();
        for text in [
            icons.note,
            icons.list,
            icons.wave,
            icons.checked,
            icons.unchecked,
            icons.cross,
            icons.warn,
            icons.tick,
        ] {
            glyphs.extend(text.chars());
        }
        glyphs.extend([
            icons.arrow,
            icons.node,
            icons.node_selected,
            icons.bar_fill,
            icons.bar_track,
            icons.bar_zero,
            icons.rule,
            icons.vertical,
            icons.dot,
        ]);
        glyphs.extend(icons.corners);
        glyphs
    }

    #[test]
    fn the_unicode_set_only_uses_allowed_glyphs() {
        let allowed = "♫≡∿✓✗⚠●○▸☑☐▁▂▃▄▅▆▇█─│╭╮╰╯·";
        for glyph in unicode_glyphs(&Icons::unicode()) {
            assert!(
                allowed.contains(glyph),
                "{glyph:?} is not one of the allowed glyphs"
            );
        }
    }

    #[test]
    fn the_ascii_set_is_plain_ascii() {
        for glyph in unicode_glyphs(&Icons::ascii()) {
            assert!(glyph.is_ascii(), "{glyph:?} is not ASCII");
        }
    }

    #[test]
    fn the_mode_picks_the_set() {
        assert_eq!(Icons::for_mode(true).note, "*");
        assert_eq!(Icons::for_mode(false).note, "♫");
    }
}
