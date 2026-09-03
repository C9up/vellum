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
	extractTextAllNative,
	extractTextNative,
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

/** Options that address a single page. */
export interface PageOptions {
	/**
	 * Which page, counting from 1 — the number printed on the page, not an
	 * array index.
	 */
	page?: number;
}

/** Per-call rendering options. Anything omitted falls back to the config. */
export interface RenderOptions extends VellumConfig, PageOptions {}

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
		return renderPageNative(
			pdf,
			this.#pageIndex(options),
			this.#merge(options),
		);
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

	/**
	 * The text of a single page.
	 *
	 * ```ts
	 * const text = await vellum.extractText(pdf, { page: 1 })
	 * ```
	 *
	 * Glyphs come back in the order the page draws them, with a line break
	 * where the baseline moves. A scanned document with no text layer yields
	 * an empty string rather than an error — it has no text to give.
	 */
	async extractText(pdf: Buffer, options: PageOptions = {}): Promise<string> {
		return extractTextNative(pdf, this.#pageIndex(options));
	}

	/** The text of every page, in document order. */
	async extractTextAll(pdf: Buffer): Promise<string[]> {
		return extractTextAllNative(pdf);
	}

	/** How many pages the document has. */
	async pageCount(pdf: Buffer): Promise<number> {
		return (await this.inspect(pdf)).pageCount;
	}

	/**
	 * Resolve a 1-based page number to the engine's 0-based index.
	 *
	 * Shared by every page-addressing method: converted in exactly one place
	 * so the public numbering and the engine's can never drift apart.
	 */
	#pageIndex(options: PageOptions): number {
		const page = options.page ?? 1;
		if (!Number.isInteger(page) || page < 1) {
			throw new VellumError(
				"INVALID_PAGE",
				`Page numbers start at 1, got ${page}.`,
			);
		}
		return page - 1;
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
