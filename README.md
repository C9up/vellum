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

## Status

Rendering to images, metadata, text extraction and document operations are
complete. Authoring content and form filling are the work ahead; `createBlank`
is the part of those paths that exists today.

## Building the native engine

```bash
pnpm build:napi   # cargo build --release + type generation + binary copy
pnpm test         # TypeScript suite
pnpm test:rust    # engine suite
```
