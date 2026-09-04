//! Advance widths for the standard fonts.
//!
//! A standard font is referenced without being embedded, so the reader lays
//! the text out with the published metrics. Anything we compute — where a line
//! breaks, where a right-aligned string starts — has to be computed with those
//! same numbers, or the text lands somewhere the reader did not put it.
//!
//! The tables are generated from the URW base-35 AFM files, the
//! metric-compatible clones the free software world lays out standard-font
//! text with, and cross-checked against published Adobe values in the tests
//! below. They are indexed by WinAnsi byte, which is what the encoder produces,
//! and hold thousandths of an em: a width of 667 at 12pt is 8.004pt.

use crate::stamp_text::StandardFont;

/// `Helvetica`, and the fonts that share its widths.
#[rustfmt::skip]
const HELVETICA: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, 0,
    556, 0, 222, 556, 333, 1000, 556, 556, 333, 1000, 667, 333, 1000, 0, 611, 0,
    0, 222, 222, 333, 333, 350, 556, 1000, 333, 1000, 500, 333, 944, 0, 500, 667,
    278, 333, 556, 556, 556, 556, 260, 556, 333, 737, 370, 556, 584, 333, 737, 333,
    400, 584, 333, 333, 333, 556, 537, 278, 333, 333, 365, 556, 834, 834, 834, 611,
    667, 667, 667, 667, 667, 667, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278,
    722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611,
    556, 556, 556, 556, 556, 556, 889, 500, 556, 556, 556, 556, 278, 278, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 584, 611, 556, 556, 556, 556, 500, 556, 500,
];

/// `HelveticaBold`, and the fonts that share its widths.
#[rustfmt::skip]
const HELVETICA_BOLD: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611,
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556,
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611,
    611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584, 0,
    556, 0, 278, 556, 500, 1000, 556, 556, 333, 1000, 667, 333, 1000, 0, 611, 0,
    0, 278, 278, 500, 500, 350, 556, 1000, 333, 1000, 556, 333, 944, 0, 500, 667,
    278, 333, 556, 556, 556, 556, 280, 556, 333, 737, 370, 556, 584, 333, 737, 333,
    400, 584, 333, 333, 333, 611, 556, 278, 333, 333, 365, 556, 834, 834, 834, 611,
    722, 722, 722, 722, 722, 722, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278,
    722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611,
    556, 556, 556, 556, 556, 556, 889, 556, 556, 556, 556, 556, 278, 278, 278, 278,
    611, 611, 611, 611, 611, 611, 611, 584, 611, 611, 611, 611, 611, 556, 611, 556,
];

/// `TimesRoman`, and the fonts that share its widths.
#[rustfmt::skip]
const TIMES_ROMAN: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    250, 333, 408, 500, 500, 833, 778, 180, 333, 333, 500, 564, 250, 333, 250, 278,
    500, 500, 500, 500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564, 444,
    921, 722, 667, 667, 722, 611, 556, 722, 722, 333, 389, 722, 611, 889, 722, 722,
    556, 722, 667, 556, 611, 722, 722, 944, 722, 722, 611, 333, 278, 333, 469, 500,
    333, 444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500, 278, 778, 500, 500,
    500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444, 480, 200, 480, 541, 0,
    500, 0, 333, 500, 444, 1000, 500, 500, 333, 1000, 556, 333, 889, 0, 611, 0,
    0, 333, 333, 444, 444, 350, 500, 1000, 333, 980, 389, 333, 722, 0, 444, 722,
    250, 333, 500, 500, 500, 500, 200, 500, 333, 760, 276, 500, 564, 333, 760, 333,
    400, 564, 300, 300, 333, 500, 453, 250, 333, 300, 310, 500, 750, 750, 750, 444,
    722, 722, 722, 722, 722, 722, 889, 667, 611, 611, 611, 611, 333, 333, 333, 333,
    722, 722, 722, 722, 722, 722, 722, 564, 722, 722, 722, 722, 722, 722, 556, 500,
    444, 444, 444, 444, 444, 444, 667, 444, 444, 444, 444, 444, 278, 278, 278, 278,
    500, 500, 500, 500, 500, 500, 500, 564, 500, 500, 500, 500, 500, 500, 500, 500,
];

/// `TimesBold`, and the fonts that share its widths.
#[rustfmt::skip]
const TIMES_BOLD: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    250, 333, 555, 500, 500, 1000, 833, 278, 333, 333, 500, 570, 250, 333, 250, 278,
    500, 500, 500, 500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500,
    930, 722, 667, 722, 722, 667, 611, 778, 778, 389, 500, 778, 667, 944, 722, 778,
    611, 778, 722, 556, 667, 722, 722, 1000, 722, 722, 667, 333, 278, 333, 581, 500,
    333, 500, 556, 444, 556, 444, 333, 500, 556, 278, 333, 556, 278, 833, 556, 500,
    556, 556, 444, 389, 333, 556, 500, 722, 500, 500, 444, 394, 220, 394, 520, 0,
    500, 0, 333, 500, 500, 1000, 500, 500, 333, 1000, 556, 333, 1000, 0, 667, 0,
    0, 333, 333, 500, 500, 350, 500, 1000, 333, 1000, 389, 333, 722, 0, 444, 722,
    250, 333, 500, 500, 500, 500, 220, 500, 333, 747, 300, 500, 570, 333, 747, 333,
    400, 570, 300, 300, 333, 556, 540, 250, 333, 300, 330, 500, 750, 750, 750, 500,
    722, 722, 722, 722, 722, 722, 1000, 722, 667, 667, 667, 667, 389, 389, 389, 389,
    722, 722, 778, 778, 778, 778, 778, 570, 778, 722, 722, 722, 722, 722, 611, 556,
    500, 500, 500, 500, 500, 500, 722, 444, 444, 444, 444, 444, 278, 278, 278, 278,
    500, 556, 500, 500, 500, 500, 500, 570, 500, 556, 556, 556, 556, 500, 556, 500,
];

/// `TimesItalic`, and the fonts that share its widths.
#[rustfmt::skip]
const TIMES_ITALIC: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    250, 333, 420, 500, 500, 833, 778, 214, 333, 333, 500, 675, 250, 333, 250, 278,
    500, 500, 500, 500, 500, 500, 500, 500, 500, 500, 333, 333, 675, 675, 675, 500,
    920, 611, 611, 667, 722, 611, 611, 722, 722, 333, 444, 667, 556, 833, 667, 722,
    611, 722, 611, 500, 556, 722, 611, 833, 611, 556, 556, 389, 278, 389, 422, 500,
    333, 500, 500, 444, 500, 444, 278, 500, 500, 278, 278, 444, 278, 722, 500, 500,
    500, 500, 389, 389, 278, 500, 444, 667, 444, 444, 389, 400, 275, 400, 541, 0,
    500, 0, 333, 500, 556, 889, 500, 500, 333, 1000, 500, 333, 944, 0, 556, 0,
    0, 333, 333, 556, 556, 350, 500, 889, 333, 980, 389, 333, 667, 0, 389, 556,
    250, 389, 500, 500, 500, 500, 275, 500, 333, 760, 276, 500, 675, 333, 760, 333,
    400, 675, 300, 300, 333, 500, 523, 250, 333, 300, 310, 500, 750, 750, 750, 500,
    611, 611, 611, 611, 611, 611, 889, 667, 611, 611, 611, 611, 333, 333, 333, 333,
    722, 667, 722, 722, 722, 722, 722, 675, 722, 722, 722, 722, 722, 556, 611, 500,
    500, 500, 500, 500, 500, 500, 667, 444, 444, 444, 444, 444, 278, 278, 278, 278,
    500, 500, 500, 500, 500, 500, 500, 675, 500, 500, 500, 500, 500, 444, 500, 444,
];

/// `Courier`, and the fonts that share its widths.
#[rustfmt::skip]
const COURIER: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600,
    600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600,
    600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600,
    600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600,
    600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600,
    600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 0,
    600, 0, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 0, 600, 0,
    0, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 0, 600, 600,
    600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600,
    600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600,
    600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600,
    600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600,
    600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600,
    600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600, 600,
];

/// The widths a font lays text out with.
fn table(font: StandardFont) -> &'static [u16; 256] {
    match font {
        // An oblique is a slanted upright, not a different font.
        StandardFont::Helvetica | StandardFont::HelveticaOblique => &HELVETICA,
        StandardFont::HelveticaBold => &HELVETICA_BOLD,
        StandardFont::TimesRoman => &TIMES_ROMAN,
        StandardFont::TimesBold => &TIMES_BOLD,
        StandardFont::TimesItalic => &TIMES_ITALIC,
        // Courier is monospace, so bold measures the same as regular.
        StandardFont::Courier | StandardFont::CourierBold => &COURIER,
    }
}

/// How wide a WinAnsi string is, in points.
pub(crate) fn width_of(font: StandardFont, text: &[u8], size: f32) -> f32 {
    let table = table(font);
    let thousandths: u32 = text
        .iter()
        .map(|byte| u32::from(table[usize::from(*byte)]))
        .sum();
    thousandths as f32 * size / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width(font: StandardFont, character: char) -> u16 {
        table(font)[character as usize]
    }

    /// The published Adobe metrics. These are what a reader lays out with, so
    /// the tables are only useful insofar as they agree with them.
    #[test]
    fn the_tables_hold_the_published_widths() {
        use StandardFont::*;

        assert_eq!(width(Helvetica, ' '), 278);
        assert_eq!(width(Helvetica, 'A'), 667);
        assert_eq!(width(Helvetica, 'M'), 833);
        assert_eq!(width(Helvetica, 'W'), 944);
        assert_eq!(width(Helvetica, 'a'), 556);
        assert_eq!(width(Helvetica, 'i'), 222);
        assert_eq!(width(Helvetica, '.'), 278);
        assert_eq!(width(Helvetica, '0'), 556);

        assert_eq!(width(HelveticaBold, 'A'), 722);
        assert_eq!(width(HelveticaBold, 'i'), 278);

        assert_eq!(width(TimesRoman, ' '), 250);
        assert_eq!(width(TimesRoman, 'A'), 722);
        assert_eq!(width(TimesRoman, 'a'), 444);
        assert_eq!(width(TimesRoman, 'i'), 278);
        assert_eq!(width(TimesBold, 'a'), 500);
        assert_eq!(width(TimesItalic, 'A'), 611);
    }

    #[test]
    fn courier_measures_the_same_whatever_the_letter() {
        // Everything WinAnsi defines is 600 wide; what it leaves undefined
        // has no width because no encoder can produce it.
        for (code, width) in table(StandardFont::Courier).iter().enumerate().skip(0x20) {
            assert!(matches!(width, 0 | 600), "code {code:#x}");
        }
        assert_eq!(width(StandardFont::Courier, 'i'), 600);
        assert_eq!(width(StandardFont::CourierBold, 'W'), 600);
    }

    /// An oblique is the upright slanted, so a document laid out with one and
    /// rendered with the other must not reflow.
    #[test]
    fn an_oblique_measures_like_its_upright() {
        assert_eq!(
            table(StandardFont::HelveticaOblique),
            table(StandardFont::Helvetica)
        );
    }

    /// Every code the encoder can produce has to have a width, or a line of
    /// perfectly ordinary French would measure short.
    #[test]
    fn every_encodable_byte_has_a_width() {
        let undefined = [0x7F, 0x81, 0x8D, 0x8F, 0x90, 0x9D];
        for font in [
            StandardFont::Helvetica,
            StandardFont::HelveticaBold,
            StandardFont::HelveticaOblique,
            StandardFont::TimesRoman,
            StandardFont::TimesBold,
            StandardFont::TimesItalic,
            StandardFont::Courier,
            StandardFont::CourierBold,
        ] {
            for code in 0x20..=0xFF_usize {
                let expected = !undefined.contains(&code);
                assert_eq!(table(font)[code] != 0, expected, "{font:?} at {code:#x}");
            }
        }
    }

    #[test]
    fn accented_french_measures_like_its_base_letter() {
        // The reader draws é in the same advance as e; a wrapper that treated
        // it as unknown would measure a French line short.
        let encoded = crate::stamp_text::to_win_ansi("é").expect("é is WinAnsi");
        let accented = table(StandardFont::Helvetica)[usize::from(encoded[0])];
        assert_eq!(accented, width(StandardFont::Helvetica, 'e'));
    }

    #[test]
    fn a_string_is_the_sum_of_its_letters_at_the_size_asked_for() {
        let width = width_of(StandardFont::Helvetica, b"AA", 12.0);
        // 667 thousandths of an em, twice, at 12pt.
        assert!((width - 16.008).abs() < 0.001, "got {width}");
    }
}
