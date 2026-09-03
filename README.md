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

## Status

Rendering to images, metadata, text extraction, document operations and
stamping — image and text — are complete. Filling interactive forms (AcroForm)
and embedding custom fonts are the work ahead.

## Building the native engine

```bash
pnpm build:napi   # cargo build --release + type generation + binary copy
pnpm test         # TypeScript suite
pnpm test:rust    # engine suite
```
