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

/** Where and how an image is laid onto a page. */

export interface StampOptions {
	/** Which page, counting from zero. Absent stamps every page. */
	page?: number;
	/** Points from the left edge. Default 0. */
	x?: number;
	/** Points from the TOP edge. Default 0. */
	y?: number;
	/** Drawn width in points. With `height` absent, the ratio is kept. */
	width?: number;
	/** Drawn height in points. With `width` absent, the ratio is kept. */
	height?: number;
	/** 0 is invisible, 1 is opaque. Default 1. */
	opacity?: number;
}

/** Where and how a line of text is written onto a page. */

export interface TextStampOptions {
	/** Which page, counting from zero. Absent writes on every page. */
	page?: number;
	/** Points from the left edge. Default 0. */
	x?: number;
	/** Points from the TOP edge, to the text's baseline. Default 0. */
	y?: number;
	/** Type size in points. Default 12. */
	size?: number;
	/** One of the 14 standard fonts, e.g. `"Helvetica"`, `"Times-Roman"`. */
	font?: string;
	/** `#rgb` or `#rrggbb`. Default black. */
	color?: string;
	/** 0 is invisible, 1 is opaque. Default 1. */
	opacity?: number;
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

export declare function merge(documents: Array<Buffer>): Promise<Buffer>;

export declare function selectPages(
	bytes: Buffer,
	pages: Array<number>,
): Promise<Buffer>;

export declare function split(bytes: Buffer): Promise<Buffer[]>;

export declare function rotate(
	bytes: Buffer,
	degrees: number,
	pages?: Array<number> | undefined | null,
): Promise<Buffer>;

export declare function stamp(
	pdf: Buffer,
	image: Buffer,
	options?: StampOptions | undefined | null,
): Promise<Buffer>;

export declare function stampText(
	pdf: Buffer,
	text: string,
	options?: TextStampOptions | undefined | null,
): Promise<Buffer>;
