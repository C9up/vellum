// GENERATED FROM THE RUST — do not edit.
//
// Produced by scripts/generate-napi-types.mjs from napi-derive's type-def
// output. Editing this file by hand puts it back where it started: a
// description that can disagree with the code it describes.

export interface DocumentInfo {
	pageCount: number;
	version: string;
	encrypted: boolean;
}

export interface PageSize {
	/** Width in points (72 per inch). A4 is 595.28. */
	width: number;
	/** Height in points. A4 is 841.89. */
	height: number;
}

/** Rasterising options, as a plain JavaScript object. */

export interface RenderOptions {
	/**
	 * Multiplier over the page's natural size, 1 being 72 DPI. Default 1.
	 * Ignored when `width` is given.
	 */
	scale?: number;
	/** Target width in pixels. Wins over `scale`. */
	width?: number;
	/** `"png"` (default) or `"jpeg"`. */
	format?: string;
	/** JPEG quality, 1-100. Only valid alongside `format: "jpeg"`. */
	quality?: number;
	/** `#rgb`, `#rrggbb`, `#rrggbbaa` or `"transparent"`. Default opaque white. */
	background?: string;
}

export interface PageDimensions {
	/** Width in points, before scaling. */
	width: number;
	/** Height in points, before scaling. */
	height: number;
}

/** What the `/Info` dictionary says about a document. */

export interface DocumentMetadata {
	title?: string;
	author?: string;
	subject?: string;
	keywords?: string;
	/** The application that authored the content. */
	creator?: string;
	/** The application that wrote the PDF. */
	producer?: string;
	/**
	 * ISO 8601 when the producer wrote a conforming date, otherwise the raw
	 * string it did write.
	 */
	createdAt?: string;
	modifiedAt?: string;
}

export declare function inspect(bytes: Buffer): DocumentInfo;

export declare function createBlank(pages: Array<PageSize>): Buffer;

export declare function renderPage(
	bytes: Buffer,
	pageIndex: number,
	options?: RenderOptions | undefined | null,
): Promise<Buffer>;

export declare function renderAll(
	bytes: Buffer,
	options?: RenderOptions | undefined | null,
): Promise<Buffer[]>;

export declare function pageDimensions(bytes: Buffer): Array<PageDimensions>;

export declare function metadata(bytes: Buffer): DocumentMetadata;

export declare function extractText(
	bytes: Buffer,
	pageIndex: number,
): Promise<string>;

export declare function extractTextAll(bytes: Buffer): Promise<string[]>;
