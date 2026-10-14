//! The standard font names a document can ask for, and what stands in for them.
//!
//! A PDF reader is meant to know Helvetica, Times and Courier without the file
//! carrying them, but krilla writes embedded fonts only. Each name therefore
//! resolves to a free font drawn to the same advance widths — Liberation Sans,
//! Serif and Mono — which is embedded and subset like any other.
//!
//! The vertical metrics are the standard font's own, taken from its AFM, not
//! the stand-in's: line height is ascender minus descender, and Liberation's
//! is a fifth taller than Helvetica's. With its own metrics every line of a
//! document would move down.

/// One of the twelve standard faces that have a stand-in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StandardFace {
    /// The name a document asks for.
    pub name: &'static str,
    /// The stand-in, as shipped in the package's `fonts/` directory.
    pub file: &'static str,
    /// In thousandths of an em, as the AFM gives them.
    pub ascender: f32,
    pub descender: f32,
    /// `[llx, lly, urx, ury]`.
    pub bbox: [f32; 4],
    pub cap_height: f32,
    pub x_height: f32,
}

impl StandardFace {
    /// What the AFM leaves between lines beyond ascender and descender.
    pub fn line_gap(&self) -> f32 {
        self.bbox[3] - self.bbox[1] - (self.ascender - self.descender)
    }
}

const fn face(
    name: &'static str,
    file: &'static str,
    ascender: f32,
    descender: f32,
    bbox: [f32; 4],
    cap_height: f32,
    x_height: f32,
) -> StandardFace {
    StandardFace {
        name,
        file,
        ascender,
        descender,
        bbox,
        cap_height,
        x_height,
    }
}

/// Symbol and ZapfDingbats have no stand-in with the same glyphs at the same
/// code points, so they are not offered: a font that carries the characters
/// wanted is the way to draw them.
pub const STANDARD: [StandardFace; 12] = [
    face(
        "Helvetica",
        "LiberationSans-Regular.ttf",
        718.0,
        -207.0,
        [-166.0, -225.0, 1000.0, 931.0],
        718.0,
        523.0,
    ),
    face(
        "Helvetica-Bold",
        "LiberationSans-Bold.ttf",
        718.0,
        -207.0,
        [-170.0, -228.0, 1003.0, 962.0],
        718.0,
        532.0,
    ),
    face(
        "Helvetica-Oblique",
        "LiberationSans-Italic.ttf",
        718.0,
        -207.0,
        [-170.0, -225.0, 1116.0, 931.0],
        718.0,
        523.0,
    ),
    face(
        "Helvetica-BoldOblique",
        "LiberationSans-BoldItalic.ttf",
        718.0,
        -207.0,
        [-174.0, -228.0, 1114.0, 962.0],
        718.0,
        532.0,
    ),
    face(
        "Times-Roman",
        "LiberationSerif-Regular.ttf",
        683.0,
        -217.0,
        [-168.0, -218.0, 1000.0, 898.0],
        662.0,
        450.0,
    ),
    face(
        "Times-Bold",
        "LiberationSerif-Bold.ttf",
        683.0,
        -217.0,
        [-168.0, -218.0, 1000.0, 935.0],
        676.0,
        461.0,
    ),
    face(
        "Times-Italic",
        "LiberationSerif-Italic.ttf",
        683.0,
        -217.0,
        [-169.0, -217.0, 1010.0, 883.0],
        653.0,
        441.0,
    ),
    face(
        "Times-BoldItalic",
        "LiberationSerif-BoldItalic.ttf",
        683.0,
        -217.0,
        [-200.0, -218.0, 996.0, 921.0],
        669.0,
        462.0,
    ),
    face(
        "Courier",
        "LiberationMono-Regular.ttf",
        629.0,
        -157.0,
        [-23.0, -250.0, 715.0, 805.0],
        562.0,
        426.0,
    ),
    face(
        "Courier-Bold",
        "LiberationMono-Bold.ttf",
        629.0,
        -157.0,
        [-113.0, -250.0, 749.0, 801.0],
        562.0,
        439.0,
    ),
    face(
        "Courier-Oblique",
        "LiberationMono-Italic.ttf",
        629.0,
        -157.0,
        [-27.0, -250.0, 849.0, 805.0],
        562.0,
        426.0,
    ),
    face(
        "Courier-BoldOblique",
        "LiberationMono-BoldItalic.ttf",
        629.0,
        -157.0,
        [-57.0, -250.0, 869.0, 801.0],
        562.0,
        439.0,
    ),
];

/// The face a standard name stands for. Names are matched exactly, as a PDF
/// reader matches them.
pub fn standard(name: &str) -> Option<&'static StandardFace> {
    STANDARD.iter().find(|face| face.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helvetica_keeps_its_own_line_height() {
        let helvetica = standard("Helvetica").expect("Helvetica is standard");
        // 718 + 207 = 925, the line height pdfkit computes for Helvetica.
        assert_eq!(helvetica.ascender - helvetica.descender, 925.0);
        assert_eq!(helvetica.line_gap(), 231.0);
    }

    #[test]
    fn names_are_matched_exactly() {
        assert!(standard("helvetica").is_none());
        assert!(standard("Symbol").is_none(), "no stand-in is offered");
        assert_eq!(STANDARD.len(), 12);
    }
}
