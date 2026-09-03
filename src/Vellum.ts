/**
 * Vellum — the PDF service.
 *
 * Resolved from the container as `vellum`, or imported directly from
 * `@c9up/vellum/services/main`. Every method is asynchronous: rasterising is
 * real computation, and it runs on the libuv thread pool rather than on the
 * thread serving requests.
 */

import { VellumError } from "./errors.js";
import type {
	DocumentInfo,
	DocumentMetadata,
	PageDimensions,
} from "./native.js";
import {
	inspectNative,
	metadataNative,
	pageDimensionsNative,
	renderAllNative,
	renderPageNative,
} from "./native.js";

/** Image encodings a page can be rasterised to. */
export type ImageFormat = "png" | "jpeg";

/** Rendering defaults, set in `config/vellum.ts`. */
export interface VellumConfig {
	/** Default encoding. `"png"` unless set. */
	format?: ImageFormat;
	/** Default multiplier over the page's natural size, 1 being 72 DPI. */
	scale?: number;
	/** Default target width in pixels. Takes precedence over `scale`. */
	width?: number;
	/** Default JPEG quality, 1-100. */
	quality?: number;
	/** Default background: `#rgb`, `#rrggbb`, `#rrggbbaa` or `"transparent"`. */
	background?: string;
}

/** Per-call rendering options. Anything omitted falls back to the config. */
export interface RenderOptions extends VellumConfig {
	/**
	 * Which page to render, counting from 1 — the number printed on the page,
	 * not an array index.
	 */
	page?: number;
}

export class Vellum {
	readonly #config: VellumConfig;

	constructor(config: VellumConfig = {}) {
		this.#config = config;
	}

	/** The configured defaults, as the service resolved them. */
	get config(): Readonly<VellumConfig> {
		return this.#config;
	}

	/**
	 * Rasterise a single page to an image.
	 *
	 * ```ts
	 * const preview = await vellum.render(pdf, { page: 1, width: 1200 })
	 * ```
	 */
	async render(pdf: Buffer, options: RenderOptions = {}): Promise<Buffer> {
		const page = options.page ?? 1;
		if (!Number.isInteger(page) || page < 1) {
			throw new VellumError(
				"INVALID_PAGE",
				`Page numbers start at 1, got ${page}.`,
			);
		}

		// The engine addresses pages from zero; the public surface counts from
		// one. Converted in exactly one place so the two never drift.
		return renderPageNative(pdf, page - 1, this.#merge(options));
	}

	/**
	 * Rasterise every page, in document order.
	 *
	 * ```ts
	 * const pages = await vellum.renderAll(pdf, { format: 'jpeg' })
	 * ```
	 */
	async renderAll(pdf: Buffer, options: RenderOptions = {}): Promise<Buffer[]> {
		return renderAllNative(pdf, this.#merge(options));
	}

	/** The natural size of every page, in points, before any scaling. */
	async dimensions(pdf: Buffer): Promise<PageDimensions[]> {
		return pageDimensionsNative(pdf);
	}

	/** How many pages the document has, which format version, and whether it is encrypted. */
	async inspect(pdf: Buffer): Promise<DocumentInfo> {
		return inspectNative(pdf);
	}

	/**
	 * What the document says about itself: title, author, subject, keywords,
	 * the applications involved, and the dates.
	 *
	 * Every field is optional, because a PDF is valid with no `/Info` at all
	 * and producers fill in whichever ones they like. Dates come back as ISO
	 * 8601 when the producer wrote a conforming one, otherwise as the raw
	 * string it did write.
	 */
	async metadata(pdf: Buffer): Promise<DocumentMetadata> {
		return metadataNative(pdf);
	}

	/** How many pages the document has. */
	async pageCount(pdf: Buffer): Promise<number> {
		return (await this.inspect(pdf)).pageCount;
	}

	/**
	 * Fold the per-call options onto the configured defaults.
	 *
	 * `page` is dropped: it addresses the document, not the raster, and the
	 * engine takes it as a separate argument.
	 */
	#merge(options: RenderOptions): {
		scale?: number;
		width?: number;
		format?: string;
		quality?: number;
		background?: string;
	} {
		const merged = { ...this.#config, ...options };
		return {
			scale: merged.scale,
			width: merged.width,
			format: merged.format,
			quality: merged.quality,
			background: merged.background,
		};
	}
}
