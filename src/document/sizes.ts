/**
 * Page sizes, side definitions and lengths, in the forms pdfkit takes them.
 */

import { VellumError } from "../errors.js";

/** Named page sizes in points, portrait. */
export const PAGE_SIZES = {
	"4A0": [4767.87, 6740.79],
	"2A0": [3370.39, 4767.87],
	A0: [2383.94, 3370.39],
	A1: [1683.78, 2383.94],
	A2: [1190.55, 1683.78],
	A3: [841.89, 1190.55],
	A4: [595.28, 841.89],
	A5: [419.53, 595.28],
	A6: [297.64, 419.53],
	A7: [209.76, 297.64],
	A8: [147.4, 209.76],
	A9: [104.88, 147.4],
	A10: [73.7, 104.88],
	B0: [2834.65, 4008.19],
	B1: [2004.09, 2834.65],
	B2: [1417.32, 2004.09],
	B3: [1000.63, 1417.32],
	B4: [708.66, 1000.63],
	B5: [498.9, 708.66],
	B6: [354.33, 498.9],
	B7: [249.45, 354.33],
	B8: [175.75, 249.45],
	B9: [124.72, 175.75],
	B10: [87.87, 124.72],
	C0: [2599.37, 3676.54],
	C1: [1836.85, 2599.37],
	C2: [1298.27, 1836.85],
	C3: [918.43, 1298.27],
	C4: [649.13, 918.43],
	C5: [459.21, 649.13],
	C6: [323.15, 459.21],
	C7: [229.61, 323.15],
	C8: [161.57, 229.61],
	C9: [113.39, 161.57],
	C10: [79.37, 113.39],
	RA0: [2437.8, 3458.27],
	RA1: [1729.13, 2437.8],
	RA2: [1218.9, 1729.13],
	RA3: [864.57, 1218.9],
	RA4: [609.45, 864.57],
	SRA0: [2551.18, 3628.35],
	SRA1: [1814.17, 2551.18],
	SRA2: [1275.59, 1814.17],
	SRA3: [907.09, 1275.59],
	SRA4: [637.8, 907.09],
	EXECUTIVE: [521.86, 756.0],
	FOLIO: [612.0, 936.0],
	LEGAL: [612.0, 1008.0],
	LETTER: [612.0, 792.0],
	TABLOID: [792.0, 1224.0],
} as const satisfies Record<string, readonly [number, number]>;

export type PageSizeName = keyof typeof PAGE_SIZES;

/** A length: points, or a number with a unit (`"2cm"`, `"1.5em"`, `"50%"`). */
export type Size = number | string;

/**
 * One value for every side, `[vertical, horizontal]`, `[top, right, bottom,
 * left]`, or the sides by name.
 */
export type SideDefinition<T> =
	| T
	| readonly [T, T]
	| readonly [T, T, T, T]
	| { readonly vertical: T; readonly horizontal: T }
	| {
			readonly top: T;
			readonly right: T;
			readonly bottom: T;
			readonly left: T;
	  };

export interface Sides<T> {
	top: T;
	right: T;
	bottom: T;
	left: T;
}

/** One side's value: a length, or `true`/`false` for 1 and 0. */
export type Side = Size | boolean;

function isSide(value: unknown): value is Side {
	return (
		typeof value === "number" ||
		typeof value === "string" ||
		typeof value === "boolean"
	);
}

function isTuple(
	sides: SideDefinition<Side>,
): sides is readonly [Side, Side] | readonly [Side, Side, Side, Side] {
	return Array.isArray(sides);
}

/** pdfkit's `normalizeSides`. */
export function normalizeSides(
	sides: SideDefinition<Side> | undefined,
	fallback: SideDefinition<Side>,
	transform: (value: Side) => number,
): Sides<number> {
	const given =
		sides === undefined ||
		(typeof sides === "object" &&
			!isTuple(sides) &&
			Object.keys(sides).length === 0)
			? fallback
			: sides;
	let expanded: Sides<Side>;
	if (isSide(given)) {
		expanded = { top: given, right: given, bottom: given, left: given };
	} else if (isTuple(given)) {
		expanded =
			given.length === 2
				? { top: given[0], right: given[1], bottom: given[0], left: given[1] }
				: { top: given[0], right: given[1], bottom: given[2], left: given[3] };
	} else if ("vertical" in given) {
		expanded = {
			top: given.vertical,
			right: given.horizontal,
			bottom: given.vertical,
			left: given.horizontal,
		};
	} else {
		expanded = {
			top: given.top,
			right: given.right,
			bottom: given.bottom,
			left: given.left,
		};
	}
	return {
		top: transform(expanded.top),
		right: transform(expanded.right),
		bottom: transform(expanded.bottom),
		left: transform(expanded.left),
	};
}

/** What a unit is relative to, besides absolute lengths. */
export interface SizeContext {
	fontSize: number;
	/** The document's root font size: its initial size, fixed. */
	remSize: number;
	lineHeight: () => number;
	zeroWidth: () => number;
	page: { width: number; height: number };
	/** What `%` is a percentage of. Default: the font size. */
	percentageOf?: number;
}

const LENGTH =
	/^([+-]?(?:\d+(?:\.\d*)?|\.\d+))(em|in|px|cm|mm|pc|ex|ch|rem|vw|vh|vmin|vmax|%|pt)?$/;

/**
 * pdfkit's `sizeToPoint`, with one difference: a string that is not a
 * length is refused rather than read as 0, and a sign is honoured.
 */
export function sizeToPoint(
	size: Size | boolean | undefined,
	fallback: Size | boolean,
	context: SizeContext,
): number {
	if (size === undefined) {
		return sizeToPoint(fallback, 0, context);
	}
	if (typeof size === "number") {
		if (!Number.isFinite(size)) {
			throw new VellumError(
				"INVALID_OPTIONS",
				`${size} is not a finite length`,
			);
		}
		return size;
	}
	if (typeof size === "boolean") return Number(size);
	const match = LENGTH.exec(size.trim());
	const value = match?.[1];
	if (match === null || value === undefined) {
		throw new VellumError(
			"INVALID_OPTIONS",
			`Unsupported size ${JSON.stringify(size)}`,
		);
	}
	const amount = Number(value);
	switch (match[2]) {
		case "em":
			return amount * context.fontSize;
		case "in":
			return amount * 72;
		case "px":
			return (amount * 72) / 96;
		case "cm":
			return (amount * 72) / 2.54;
		case "mm":
			return (amount * 72) / 25.4;
		case "pc":
			return amount * 12;
		case "ex":
			return amount * context.lineHeight();
		case "ch":
			return amount * context.zeroWidth();
		case "rem":
			return amount * context.remSize;
		case "vw":
			return (amount * context.page.width) / 100;
		case "vh":
			return (amount * context.page.height) / 100;
		case "vmin":
			return (amount * Math.min(context.page.width, context.page.height)) / 100;
		case "vmax":
			return (amount * Math.max(context.page.width, context.page.height)) / 100;
		case "%":
			return (amount * (context.percentageOf || context.fontSize)) / 100;
		default:
			return amount;
	}
}

function isSizeName(name: string): name is PageSizeName {
	return Object.hasOwn(PAGE_SIZES, name);
}

/** A page's width and height in points, its layout applied. */
export function pageDimensions(
	size: string | readonly [number, number] | undefined,
	layout: "portrait" | "landscape" | undefined,
): [number, number] {
	let dimensions: readonly [number, number];
	if (typeof size === "string" || size === undefined) {
		const name = (size ?? "letter").toUpperCase();
		if (!isSizeName(name)) {
			throw new VellumError(
				"INVALID_OPTIONS",
				`Unknown page size ${JSON.stringify(size)}`,
			);
		}
		dimensions = PAGE_SIZES[name];
	} else {
		dimensions = size;
	}
	const [width, height] = dimensions;
	return (layout ?? "portrait") === "portrait"
		? [width, height]
		: [height, width];
}
