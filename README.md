# @c9up/vellum

PDF toolkit — convert PDF pages to images, author and inspect documents.

Agnostic package: the service itself depends on no other part of the
ecosystem. The provider and the `configure` hook integrate with a Ream host
through an optional peer dependency.

## Install

```bash
ream configure @c9up/vellum
```

That registers the provider and writes `config/vellum.ts`.

## Usage

```ts
import vellum from '@c9up/vellum/services/main'

// A preview of the first page, 1200px wide
const preview = await vellum.render(pdf, { page: 1, width: 1200 })

// Every page as JPEG
const pages = await vellum.renderAll(pdf, { format: 'jpeg', quality: 82 })

// What is in the document
const { pageCount, version, encrypted } = await vellum.inspect(pdf)
const { title, author, createdAt } = await vellum.metadata(pdf)
const sizes = await vellum.dimensions(pdf)

// Its form
const fields = await vellum.formFields(mandate)
const filled = await vellum.fillForm(mandate, {
  'assure.nom': 'Amélie Durand',
  accepted: 'Yes',
})
const closed = await vellum.flattenForm(filled)

// Its text
const text = await vellum.extractText(pdf, { page: 1 })
const perPage = await vellum.extractTextAll(pdf)

// Reshaping it
const dossier = await vellum.merge([contract, annexe])
const extract = await vellum.selectPages(pdf, [1, 3, 4])
const parts = await vellum.split(pdf)
const upright = await vellum.rotate(scan, 90, { pages: [1] })

// Stamping it — a signature, a photo, a watermark
const signed = await vellum.stamp(workOrder, signature, {
  page: 1, x: 380, y: 690, width: 140,
})
const draft = await vellum.stamp(pdf, watermark, { opacity: 0.15 })

// Writing text onto it
const marked = await vellum.stampText(invoice, 'PAYÉ', {
  x: 400, y: 80, size: 24, color: '#c00', opacity: 0.6,
})
```

Pages are numbered from **1** — the number printed on the page, not an array
index.

### Rendering options

| Option | Meaning |
| --- | --- |
| `page` | Which page to render, from 1. Only on `render`. |
| `scale` | Multiplier over natural size; 1 is 72 DPI. |
| `width` | Target width in pixels. Takes precedence over `scale`. |
| `format` | `"png"` (default) or `"jpeg"`. |
| `quality` | JPEG quality 1-100. Refused without `format: 'jpeg'`. |
| `background` | `#rgb`, `#rrggbb`, `#rrggbbaa` or `"transparent"`. Default opaque white. |

Every option can be defaulted in `config/vellum.ts` and overridden per call.

## Configuration

```ts
// config/vellum.ts
import { defineConfig } from '@c9up/vellum'

export default defineConfig({
  format: 'jpeg',
  quality: 82,
  width: 1200,
})
```

## Engine

The work happens in Rust, behind NAPI, because PDF has no adequate JavaScript
implementation — this is a capability the platform lacks, not an optimisation
of one it has.

| Crate | Role |
| --- | --- |
| `hayro` | Rasterising a page. Pure Rust, so the binary stays self-contained |
| `lopdf` | Documents that already exist: object tree, pages, metadata, encryption |
| `krilla` | Documents we author — and, through its `pdf` feature, re-embedding pages of an existing file |
| `image` | JPEG encoding |

Text extraction rides on hayro's interpreter — it already resolves fonts,
encodings and `/ToUnicode` maps. `pdf-extract` would have been the obvious
choice but it pins `lopdf ^0.42` against our 0.44, which would put two copies
of the parser in the binary.

Rasterising an A4 page is around 30ms of pure computation, so it runs on the
libuv thread pool rather than on the thread serving requests. Every method on
the service is therefore asynchronous.

## Reading text

Glyphs come back in the order the page draws them, with a line break where the
baseline moves. That order is the reading order in practice; no reordering by
coordinates is attempted, because doing it well needs column detection and
doing it badly makes multi-column pages worse. No spaces are invented either —
a PDF encodes its own, and guessing them from gaps duplicates them.

A scanned document with no text layer yields an empty string rather than an
error: it has no text to give.

## Reshaping documents

`merge`, `selectPages`, `split` and `rotate` all move pages between page
trees, which is where PDF hides a trap: `Resources`, `MediaBox`, `CropBox` and
`Rotate` may live on a parent node and be *inherited* by the page. Re-parent
such a page naively and it loses its size — readers then fall back to Letter,
quietly resizing an A4 document. Every operation materialises the inherited
attributes onto the page first.

Rotation adds to whatever a page already carries, because a scan can arrive
already turned.

## Stamping

`stamp` draws an image onto a document — the signature a technician traces on a
tablet, a photo attached to a report, a watermark on a draft. PNG and JPEG are
accepted, chosen by file signature rather than by name. Coordinates count from
the top-left corner, the way a screen layout is written. Naming no page stamps
every page, which is what a watermark wants.

It works by re-embedding each existing page as a Form XObject and drawing over
it, through krilla's `pdf` feature. (Krilla's README says embedding existing
pages is out of scope; its published manifest says otherwise.)

`stampText` writes a line of text. It uses the 14 standard fonts — `Helvetica`,
`Helvetica-Bold`, `Helvetica-Oblique`, `Times-Roman`, `Times-Bold`,
`Times-Italic`, `Courier`, `Courier-Bold` — which a PDF may reference *without
embedding*: nothing is added to the file and no font has to be supplied. The
trade-off is the WinAnsi character set. Western European text is covered,
accents and typographic punctuation included; anything outside it is refused
rather than mangled, because silently dropping a character from a contract is
worse than failing. For `stampText`, `y` is the text's baseline.

Text written onto a page is escaped, so a document title cannot inject content
stream operators.

## Stamping and the document underneath

`stamp` and `stampText` write into the document that already exists: the
picture becomes an image XObject named in the page's resources, the text a
content stream appended to the page. Neither re-authors the file.

That is the whole point. Re-authoring — drawing each page onto a fresh one —
loses everything the page structure carries: the interactive form, the
annotations, the links. A signature is stamped onto exactly the kind of
document that has all three.

A JPEG goes in untouched, as `DCTDecode`, so a photograph stays the size it
arrived at. A PNG becomes raw samples, and its alpha channel becomes a soft
mask — which is what makes a signature drawn on a tablet transparent
everywhere but the stroke. A CMYK JPEG is refused rather than silently
inverted.

## Fonts

`stampText` uses the 14 standard fonts by default. A PDF may reference those
without embedding them, so nothing is added to the file and no font has to be
supplied — at the cost of the WinAnsi character set, outside which text is
refused rather than mangled.

A font declared in `config/vellum.ts` is embedded instead:

```ts
// config/vellum.ts
export default defineConfig({
  fonts: { body: app.makePath('resources/fonts/Inter-Regular.ttf') },
})

await vellum.stampText(pdf, 'Uměl Řehoř', { font: 'body' })
```

It is **subsetted to the characters actually written**, because embedding a
family whole would put megabytes into every stamped document, and a
`/ToUnicode` table is written alongside it — without one the text is drawn
correctly and cannot be selected, copied or searched, a loss that only shows up
when someone tries to read the document back.

A configured name is looked up before the standard fonts, so calling one
`Helvetica` shadows the standard one; a name that is not configured falls
through, which is what keeps `font: 'Times-Roman'` working with no
configuration at all. A character the supplied font has no glyph for is
refused by name rather than dropped.

## Interactive forms

`formFields` lists a document's AcroForm fields in declaration order. `name` is
the fully qualified name — every ancestor's partial name joined with dots —
which is the name a field is filled in by.

Two details of PDF 32000-1 §12.7.3 that a caller would otherwise trip on, and
which this resolves for them: a field's type, flags and value are *inherited*
down `/Parent`, so a field commonly declares none of them itself; and a
checkbox or radio's "on" state is chosen by the DOCUMENT (`/Yes`, `/On`, `/1`,
…), not fixed by the spec. Those accepted states are reported in `options`,
because writing anything else leaves the control untouched.

For a choice field, `options` reports the *exported* values rather than the
labels — the export value is what gets written back.

`fillForm` writes values by their qualified name and **regenerates each filled
field's appearance stream**. That second half is the one that matters: most
readers paint a field from its appearance, not from its value, so a document
filled without it opens looking empty while holding every answer. A checkbox or
radio already ships one appearance per state, so there only the widget's `/AS`
is repointed.

Refusals are loud rather than silent, because a filled document quietly missing
an answer is worse than a failure: an unknown field name, a read-only field, a
value over the declared maximum length, a choice the form does not offer, and a
checkbox state the document does not accept are all errors.

Text is laid out with the **published widths of the standard fonts**, because
that is what the reader lays it out with. A field's `/Q` is honoured, a
multiline field wraps at the width of its box, and a `/DA` asking for size 0
gets a size chosen to fit. A word too long for the line is broken across lines
rather than left to run past the edge, where the appearance's bounding box
would clip it away.

The widths are generated from the URW base-35 metrics by
`scripts/generate-metrics.py` and cross-checked against published Adobe values
in the tests — a table that had drifted would not compile its way into a
release.

`flattenForm` closes the document: every widget's appearance becomes ordinary
page content, the widget annotations go, and the form itself is dropped. What
comes back looks the same and can no longer be edited back.

The placement follows §12.5.5 — the appearance's `/BBox` transformed by its
`/Matrix`, and the resulting box mapped onto the annotation's `/Rect`. Painting
at the rectangle's corner instead would misplace every appearance whose form
matrix is not the identity, which is most of the ones a real form ships. The
page's own content is wrapped in `q`/`Q` first: a `cm` outside any pair is
legal and never restored, so appended content would otherwise inherit a
transform it never asked for. `stampText` does the same, for the same reason.

Annotations that are not form widgets — links, notes — are left where they are;
flattening removes the form, not the document's other furniture. A hidden
widget is dropped without being painted, since making visible what a document
hid is not preservation. And a field holding a value that ships no appearance
to paint is an error rather than a silent erasure: the answer would vanish from
a document that still looks complete.

## Signing

A PDF signature covers a byte range **of the document it lives in**, which
makes the usual order impossible: the value cannot be computed and then
assembled, because assembling it would change what it covers. The document is
written with a hole where the value goes, the `/ByteRange` records everything
but the hole, and the value is dropped into the reserved space without moving
another byte.

That is also what makes a key you hold and a key held by a certified provider
the same interface: **a signer never sees the document, only the digest of
it**.

```ts
// config/vellum.ts
export default defineConfig({
  signers: {
    internal: myLocalSigner,
    qualified: myProviderSigner,
  },
})

const signed = await vellum.sign(mandate, {
  signer: 'qualified',
  reason: 'Mandat de prévoyance',
  name: 'Amélie Durand',
})
```

A `Signer` is anything with `sign(digest: Buffer): Promise<Buffer>`, returning
the CMS `SignedData`. Signing over the network belongs there rather than in the
engine, which does no I/O.

The signature is appended as an **incremental revision**: the original bytes
are preserved exactly. Rewriting the file would invalidate any signature
already on it and destroy the history a signature exists to establish.

A visible signature — a drawn one, an image — is a separate matter: `stamp` it
on first, then sign.

**What is not here yet**: no signer ships with the package. A local key needs a
CMS builder, which is a dependency the application chooses; a certified
provider generally returns a complete CMS and needs none. Timestamping (PAdES
B-T) is an HTTP call and belongs in a signer too — worth having for a document
kept for years, since a signature cannot be validated once its certificate has
expired without one.

## Status

Rendering to images, metadata, text extraction, document operations, stamping
— image and text — supplied fonts, and interactive forms, read, filled, laid
out and flattened, are complete. Signing has its document half: what remains is
a signer to plug into it.

## Building the native engine

```bash
pnpm build:napi   # cargo build --release + type generation + binary copy
pnpm test         # TypeScript suite
pnpm test:rust    # engine suite
```
