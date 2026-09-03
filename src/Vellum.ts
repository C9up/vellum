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
	mergeNative,
	metadataNative,
	pageDimensionsNative,
	renderAllNative,
	renderPageNative,
	rotateNative,
	selectPagesNative,
	splitNative,
	stampNative,
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

/** Where and how an image is laid onto a page. */
export interface StampOptions extends PageOptions {
	/**
	 * Points from the left edge. Default 0.
	 *
	 * Coordinates count from the TOP-LEFT corner, the way a screen layout is
	 * written.
	 */
	x?: number;
	/** Points from the top edge. Default 0. */
	y?: number;
	/** Drawn width in points. With `height` absent, the ratio is kept. */
	width?: number;
	/** Drawn height in points. With `width` absent, the ratio is kept. */
	height?: number;
	/** 0 is invisible, 1 is opaque. A watermark usually wants about 0.15. */
	opacity?: number;
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

	/**
	 * Join documents end to end, in the order given.
	 *
	 * ```ts
	 * const dossier = await vellum.merge([contract, annexe, signature])
	 * ```
	 *
	 * Attributes a page inherits from its parent — size, resources — are
	 * materialised onto it first, so a page keeps its own size instead of
	 * falling back to Letter.
	 */
	async merge(pdfs: ReadonlyArray<Buffer>): Promise<Buffer> {
		if (pdfs.length === 0) {
			throw new VellumError(
				"EMPTY_MERGE",
				"Merging needs at least one document.",
			);
		}
		return mergeNative(pdfs);
	}

	/**
	 * Keep only the pages listed, counting from 1, in document order.
	 *
	 * ```ts
	 * const extract = await vellum.selectPages(pdf, [1, 3, 4])
	 * ```
	 */
	async selectPages(
		pdf: Buffer,
		pages: ReadonlyArray<number>,
	): Promise<Buffer> {
		if (pages.length === 0) {
			throw new VellumError(
				"EMPTY_SELECTION",
				"Selecting needs at least one page.",
			);
		}
		return selectPagesNative(pdf, this.#pageIndexes(pages));
	}

	/** One single-page document per page, in document order. */
	async split(pdf: Buffer): Promise<Buffer[]> {
		return splitNative(pdf);
	}

	/**
	 * Rotate pages clockwise by `degrees`, a multiple of 90.
	 *
	 * ```ts
	 * const upright = await vellum.rotate(scan, 90, { pages: [1] })
	 * ```
	 *
	 * The rotation is added to what a page already carries, because a scan can
	 * arrive already turned.
	 */
	async rotate(
		pdf: Buffer,
		degrees: number,
		options: { pages?: ReadonlyArray<number> } = {},
	): Promise<Buffer> {
		if (!Number.isInteger(degrees) || degrees % 90 !== 0) {
			throw new VellumError(
				"INVALID_ROTATION",
				`Rotation must be a whole multiple of 90, got ${degrees}.`,
			);
		}
		return rotateNative(
			pdf,
			degrees,
			options.pages ? this.#pageIndexes(options.pages) : undefined,
		);
	}

	/**
	 * Draw an image onto the document — a signature, a photo, a watermark.
	 *
	 * ```ts
	 * const signed = await vellum.stamp(workOrder, signature, {
	 *   page: 1, x: 380, y: 690, width: 140,
	 * })
	 * ```
	 *
	 * PNG and JPEG are accepted, chosen by signature rather than by file name.
	 * Omitting `page` stamps every page, which is what a watermark wants.
	 */
	async stamp(
		pdf: Buffer,
		image: Buffer,
		options: StampOptions = {},
	): Promise<Buffer> {
		return stampNative(pdf, image, {
			// Absent means every page here, so the 1-based conversion only
			// applies when a page was actually named.
			page: options.page === undefined ? undefined : this.#pageIndex(options),
			x: options.x,
			y: options.y,
			width: options.width,
			height: options.height,
			opacity: options.opacity,
		});
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
	 * Resolve several 1-based page numbers to the engine's 0-based indexes.
	 *
	 * Routed through {@link Vellum.#pageIndex} so one method decides what a
	 * page number means for the whole service.
	 */
	#pageIndexes(pages: ReadonlyArray<number>): number[] {
		return pages.map((page) => this.#pageIndex({ page }));
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
