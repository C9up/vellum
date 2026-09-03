/**
 * @c9up/vellum — PDF toolkit.
 *
 * Converting a PDF to an image is what the package does today; authoring,
 * editing and form filling are the work ahead. The rendering engine is Rust
 * behind NAPI, because PDF has no adequate JavaScript implementation.
 *
 * ```ts
 * import vellum from '@c9up/vellum/services/main'
 *
 * const preview = await vellum.render(pdf, { page: 1, width: 1200 })
 * const pages = await vellum.renderAll(pdf, { format: 'jpeg' })
 * ```
 */

import "./augmentations.js";

export { defineConfig } from "./config.js";
export { VellumError } from "./errors.js";
export type {
	DocumentInfo,
	DocumentMetadata,
	PageDimensions,
	PageSize,
} from "./native.js";
export { isNativeAvailable, VellumNativeRequiredError } from "./native.js";
export type { ImageFormat, RenderOptions, VellumConfig } from "./Vellum.js";
export { Vellum } from "./Vellum.js";

import type { DocumentInfo, PageSize } from "./native.js";
import { createBlankNative, inspectNative } from "./native.js";

/**
 * Report the shape of a PDF: how many pages it has, which version of the
 * format it claims, and whether it is encrypted.
 *
 * Metadata strings are not reported yet. The `/Info` dictionary stores them in
 * either UTF-16BE or PDFDocEncoding, and a half-correct decoder would quietly
 * mangle every accented character — so that lands with a real decoder.
 */
export function inspect(pdf: Buffer): DocumentInfo {
	return inspectNative(pdf);
}

/**
 * Author a document of blank pages, sized in points (72 per inch).
 *
 * This is the authoring path with nothing drawn on it yet; drawing arrives
 * with the generation work.
 */
export function createBlank(pages: ReadonlyArray<PageSize>): Buffer {
	return createBlankNative(pages);
}

/** A4 in points, the size every document here starts from. */
export const A4: PageSize = { width: 595.28, height: 841.89 };
