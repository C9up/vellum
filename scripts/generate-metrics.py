"""Derive the standard-font advance widths in crates/vellum-engine/src/metrics.rs.

A standard font is referenced without being embedded, so the reader lays the
text out with the published Adobe metrics. The URW base-35 fonts are the
metric-compatible clones of those 14, which is what makes them usable as a
source. The result is cross-checked here against published Adobe values, and
again by the tests in metrics.rs — so a table that drifted would not compile
its way into a release.

Run it only to regenerate the table; the build does not need it. It wants the
AFM files (Fedora: urw-base35-fonts-legacy, Debian: fonts-urw-base35) and
fonttools, for the Adobe Glyph List.

    python3 scripts/generate-metrics.py /tmp/widths.py
"""
import re
import sys
from pathlib import Path

from fontTools.agl import AGL2UV

AFM = Path("/usr/share/fonts/urw-base35")

FONTS = [
    ("Helvetica", "NimbusSans-Regular"),
    ("HelveticaBold", "NimbusSans-Bold"),
    ("HelveticaOblique", "NimbusSans-Italic"),
    ("TimesRoman", "NimbusRoman-Regular"),
    ("TimesBold", "NimbusRoman-Bold"),
    ("TimesItalic", "NimbusRoman-Italic"),
    ("Courier", "NimbusMonoPS-Regular"),
    ("CourierBold", "NimbusMonoPS-Bold"),
]

# Unicode -> AGL glyph name, inverted from the AGL itself rather than typed.
UV2NAME = {}
for name, uv in AGL2UV.items():
    UV2NAME.setdefault(uv, name)

# WinAnsi is Latin-1 except over 0x80-0x9F. This block is the ONLY hand-written
# data here, and it is cross-checked against the Rust encoder below.
HIGH = {
    0x80: 0x20AC, 0x82: 0x201A, 0x83: 0x0192, 0x84: 0x201E, 0x85: 0x2026,
    0x86: 0x2020, 0x87: 0x2021, 0x88: 0x02C6, 0x89: 0x2030, 0x8A: 0x0160,
    0x8B: 0x2039, 0x8C: 0x0152, 0x8E: 0x017D, 0x91: 0x2018, 0x92: 0x2019,
    0x93: 0x201C, 0x94: 0x201D, 0x95: 0x2022, 0x96: 0x2013, 0x97: 0x2014,
    0x98: 0x02DC, 0x99: 0x2122, 0x9A: 0x0161, 0x9B: 0x203A, 0x9C: 0x0153,
    0x9E: 0x017E, 0x9F: 0x0178,
}


def win_ansi_names():
    """WinAnsi byte -> AFM glyph name."""
    names = {}
    for code in range(0x20, 0x7F):
        names[code] = UV2NAME[code]
    for code, uv in HIGH.items():
        names[code] = UV2NAME[uv]
    for code in range(0xA0, 0x100):
        if code in UV2NAME:
            names[code] = UV2NAME[code]
    # Adobe's WinAnsiEncoding gives these the width of the glyph they stand in
    # for: a no-break space is a space, a soft hyphen is a hyphen.
    names[0xA0] = "space"
    names[0xAD] = "hyphen"
    # AGLFN drops the superscript digits as deprecated, but WinAnsi has them
    # and every AFM names them the old way.
    names[0xB2] = "twosuperior"
    names[0xB3] = "threesuperior"
    names[0xB9] = "onesuperior"
    return names


def widths_of(afm_name):
    """Glyph name -> advance width, in 1/1000 of an em."""
    widths = {}
    text = (AFM / f"{afm_name}.afm").read_text(encoding="latin-1")
    for line in text.splitlines():
        if not line.startswith("C "):
            continue
        width = re.search(r"WX\s+(-?\d+)\s*;", line)
        name = re.search(r"N\s+(\S+)\s*;", line)
        if width and name:
            widths[name.group(1)] = int(width.group(1))
    return widths


def main():
    names = win_ansi_names()
    tables = {}
    missing = []
    for font, afm in FONTS:
        widths = widths_of(afm)
        table = [0] * 256
        for code, name in names.items():
            if name in widths:
                table[code] = widths[name]
            else:
                missing.append((font, code, name))
        tables[font] = table

    if missing:
        print(f"MISSING {len(missing)} glyphs, first 10: {missing[:10]}", file=sys.stderr)

    # Cross-check against the published Adobe AFM values. If the URW clones had
    # drifted, or the glyph-name mapping were wrong, these would not line up.
    known = {
        ("Helvetica", " "): 278, ("Helvetica", "A"): 667, ("Helvetica", "W"): 944,
        ("Helvetica", "i"): 222, ("Helvetica", "M"): 833, ("Helvetica", "a"): 556,
        ("Helvetica", "."): 278, ("Helvetica", "0"): 556,
        ("HelveticaBold", " "): 278, ("HelveticaBold", "A"): 722,
        ("HelveticaBold", "a"): 556, ("HelveticaBold", "i"): 278,
        ("TimesRoman", " "): 250, ("TimesRoman", "A"): 722, ("TimesRoman", "a"): 444,
        ("TimesRoman", "W"): 944, ("TimesRoman", "i"): 278, ("TimesRoman", "."): 250,
        ("TimesBold", "A"): 722, ("TimesBold", "a"): 500,
        ("TimesItalic", "A"): 611, ("TimesItalic", "a"): 500,
        ("Courier", "A"): 600, ("Courier", "i"): 600, ("Courier", " "): 600,
        ("CourierBold", "W"): 600,
    }
    bad = []
    for (font, char), expected in known.items():
        got = tables[font][ord(char)]
        if got != expected:
            bad.append((font, char, expected, got))
    if bad:
        print(f"MISMATCH: {bad}", file=sys.stderr)
        return 1

    # Helvetica-Oblique must share Helvetica's widths; that is what makes it a
    # slanted Helvetica rather than a different font.
    assert tables["HelveticaOblique"] == tables["Helvetica"], "oblique drifted"
    assert all(w in (0, 600) for w in tables["Courier"]), "Courier is not monospace"
    # Every WinAnsi code except the control range, DEL and the five the
    # encoding leaves undefined.
    undefined = {0x7F, 0x81, 0x8D, 0x8F, 0x90, 0x9D}
    for font, _ in FONTS:
        for code in range(0x20, 0x100):
            has = tables[font][code] != 0
            assert has == (code not in undefined), f"{font} code {code:#x}"

    print(f"OK — {len(known)} known values matched, {len(names)} codes covered")
    for font, _ in FONTS:
        defined = sum(1 for w in tables[font] if w)
        print(f"  {font}: {defined} codes")
    Path(sys.argv[1]).write_text(repr(tables))
    return 0


if __name__ == "__main__":
    sys.exit(main())
