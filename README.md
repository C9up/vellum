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
const sizes = await vellum.dimensions(pdf)
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

Rasterising an A4 page is around 30ms of pure computation, so it runs on the
libuv thread pool rather than on the thread serving requests. Every method on
the service is therefore asynchronous.

## Status

Rendering to images is complete. Authoring content, editing existing files and
form filling are the work ahead; `createBlank` and `inspect` are the parts of
those paths that exist today.

## Building the native engine

```bash
pnpm build:napi   # cargo build --release + type generation + binary copy
pnpm test         # TypeScript suite
pnpm test:rust    # engine suite
```
