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
