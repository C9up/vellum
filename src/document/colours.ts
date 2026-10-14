/**
 * Colours in the forms pdfkit takes: `#rgb`, `#rrggbb`, a CSS colour name,
 * `[r, g, b]` from 0 to 255, `[c, m, y, k]` from 0 to 100, or the name of a
 * spot colour the document defined.
 */

import { VellumError } from "../errors.js";
import type { PaintInput } from "../native/generated.js";
import { NAMED_COLOURS } from "./namedColours.js";

export type ColourValue =
	| string
	| readonly [number, number, number]
	| readonly [number, number, number, number];

/** A plain colour, as a gradient stop takes it. */
export interface PlainColour {
	kind: "rgb" | "cmyk";
	values: number[];
}

function invalid(colour: unknown): VellumError {
	return new VellumError(
		"INVALID_COLOR",
		`${JSON.stringify(colour)} is not a colour: use #rgb, #rrggbb, a CSS name, [r, g, b] (0-255), [c, m, y, k] (0-100) or a spot colour's name`,
	);
}

/**
 * pdfkit silently ignores a colour it cannot read and keeps drawing in the
 * previous one; a typo here is an error instead.
 */
export function plainColour(colour: ColourValue): PlainColour {
	if (typeof colour === "string") {
		const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(colour)?.[1];
		if (hex !== undefined) {
			const full =
				hex.length === 3
					? hex
							.split("")
							.map((digit) => digit + digit)
							.join("")
					: hex;
			const value = Number.parseInt(full, 16);
			return {
				kind: "rgb",
				values: [value >> 16, (value >> 8) & 0xff, value & 0xff],
			};
		}
		const named = NAMED_COLOURS[colour.toLowerCase()];
		if (named !== undefined) return { kind: "rgb", values: [...named] };
		throw invalid(colour);
	}
	const inRange = (max: number) =>
		colour.every((part) => Number.isFinite(part) && part >= 0 && part <= max);
	if (colour.length === 3 && inRange(255)) {
		return { kind: "rgb", values: [...colour] };
	}
	if (colour.length === 4 && inRange(100)) {
		return { kind: "cmyk", values: colour.map((part) => (part * 255) / 100) };
	}
	throw invalid(colour);
}

/** A colour, or a spot colour by name: named colours win, as in pdfkit. */
export function paintOf(
	colour: ColourValue,
	spots: ReadonlyMap<string, number>,
): PaintInput {
	if (typeof colour === "string" && !colour.startsWith("#")) {
		const named = NAMED_COLOURS[colour.toLowerCase()];
		const spot = spots.get(colour);
		if (named === undefined && spot !== undefined) {
			return { kind: "spot", handle: spot };
		}
	}
	return plainColour(colour);
}
