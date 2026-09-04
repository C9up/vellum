/**
 * @c9up/vellum — PDF toolkit.
 *
 * Rendering pages to images, reading text and metadata, reshaping documents,
 * stamping them, filling and flattening their forms, and signing them. The
 * engine is Rust behind NAPI, because PDF has no adequate JavaScript
 * implementation — a capability the platform lacks, not an optimisation.
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
	FormField,
	PageDimensions,
	PageSize,
	RevocationAnswer,
	SignatureReport,
} from "./native.js";
export { isNativeAvailable, VellumNativeRequiredError } from "./native.js";
export type { Pkcs8SignerOptions, TimestampOptions } from "./signers.js";
export { pkcs8Signer, timestamped } from "./signers.js";
export type {
	CheckedSignature,
	ImageFormat,
	PageOptions,
	RenderOptions,
	Signer,
	SignOptions,
	StampOptions,
	StandardFont,
	TextStampOptions,
	VellumConfig,
	VerifyOptions,
} from "./Vellum.js";
export { Vellum } from "./Vellum.js";

import type { DocumentInfo, PageSize } from "./native.js";
import { createBlankNative, inspectNative } from "./native.js";

/**
 * Report the shape of a PDF: how many pages it has, which version of the
 * format it claims, and whether it is encrypted.
 *
 * The `/Info` strings are read by {@link Vellum.metadata} instead, which has
 * the decoder they need: they are stored in UTF-16BE, UTF-8 or
 * PDFDocEncoding, and guessing between them mangles every accent.
 */
export function inspect(pdf: Buffer): DocumentInfo {
	return inspectNative(pdf);
}

/**
 * Author a document of blank pages, sized in points (72 per inch).
 *
 * Draw onto them with {@link Vellum.stamp} and {@link Vellum.stampText}.
 */
export function createBlank(pages: ReadonlyArray<PageSize>): Buffer {
	return createBlankNative(pages);
}

/** A4 in points, the size every document here starts from. */
export const A4: PageSize = { width: 595.28, height: 841.89 };
