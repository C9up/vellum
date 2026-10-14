# Test fixtures

## `VellumTestSans.ttf`

A small subset of DejaVu Sans, kept to the glyphs the font-embedding tests
need. It exists because those tests cannot be written without a real font: a
character map, glyph advances and the metrics a font descriptor declares all
have to come from somewhere, and a font assembled by hand in the test would
prove only that our assembly matches our reader.

DejaVu Sans is distributed under the Bitstream Vera licence, reproduced in
`VellumTestSans.LICENSE.txt`. It permits modification and redistribution and
requires that a modified font not carry the original names, so this one was
subsetted and renamed to "Vellum Test Sans".

It is a test fixture. It is not shipped with the package and nothing at
runtime reads it.

## `NotoSansArabic-Subset.ttf`, `Montserrat-Bold-Subset.otf`, `Pair.ttc`

Fonts for the layout tests, which would otherwise depend on what a machine
has installed — and a test that quietly skips on a CI runner without the
font proves nothing.

- `NotoSansArabic-Subset.ttf`: Noto Sans Arabic at weight 400, cut to the
  Arabic text the right-to-left tests write.
- `Montserrat-Bold-Subset.otf`: Montserrat Bold, cut to printable ASCII and
  a few French accents. It keeps its CFF outlines, the case a TrueType font
  does not cover.
- `Pair.ttc`: a collection of the Arabic subset and Vellum Test Sans, for
  choosing a face by its PostScript name.

Both source fonts are under the SIL Open Font License 1.1 with no Reserved
Font Name, which allows a subset to be redistributed under the same licence
without renaming it; the licence is in `OFL-fixtures.txt`. They were cut
with fontTools (`varLib.instancer`, `subset`, `ttCollection`).

Like Vellum Test Sans, they are test fixtures only: not shipped, never read
at runtime.

## `pdfkit-text.tsv`

Where pdfkit 0.20.2 puts each line of text in a set of layouts — wrapping,
alignment, page and column breaks, continued text, ellipsis, soft hyphens,
words wider than the line — with Liberation Sans. The layout tests replay
each one and must land on the same lines at the same positions.
