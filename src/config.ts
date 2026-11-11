import { VellumError } from "./errors.js";
import type { VellumConfig } from "./Vellum.js";

/**
 * The config as it is WRITTEN, before validation.
 *
 * `format` is a plain string here because that is what an environment yields:
 * `env.get('VELLUM_FORMAT', 'png')` is a `string`, and a config that only
 * accepted the union could not read one — the generated `config/vellum.ts` did
 * not typecheck in the project it was written into.
 */
export interface VellumConfigInput extends Omit<VellumConfig, "format"> {
	format?: string;
}

/**
 * Define the rendering defaults, in `config/vellum.ts`.
 *
 * ```ts
 * import { defineConfig } from '@c9up/vellum'
 *
 * export default defineConfig({
 *   format: 'jpeg',
 *   quality: 82,
 *   width: 1200,
 * })
 * ```
 *
 * Named deviation: there is no `default` + service list here, because there is
 * one rendering engine rather than several. Should a second one ever exist,
 * that is when the manager gains `use()` — not before, when it would only be
 * an indirection over a single implementation.
 */
export function defineConfig(config: VellumConfigInput): VellumConfig {
	checkCeilings(config);
	const { format, ...rest } = config;
	if (format === undefined) return rest;
	if (format !== "png" && format !== "jpeg") {
		// Named at boot, with the value, rather than at the first render of a
		// document nobody was looking at.
		throw new Error(
			`[vellum] config/vellum.ts asks for format ${JSON.stringify(format)}; ` +
				'it is "png" or "jpeg".',
		);
	}
	return { ...rest, format };
}

/** The ceilings rendering and reading take, as numbers. */
export interface Ceilings {
	maxPixels?: number;
	maxTotalPixels?: number;
	maxDecodedBytes?: number;
	maxOperations?: number;
}

/**
 * Refuse a ceiling that is not a whole number from 1 to 4 294 967 295.
 *
 * The engine takes them as 32-bit counts, and the conversion JavaScript
 * applies on the way there wraps rather than refuses: `-1` would become four
 * billion and `1.5` one, a typo in `config/vellum.ts` turning a ceiling into
 * none without a word.
 */
export function checkCeilings(ceilings: Ceilings): void {
	for (const name of [
		"maxPixels",
		"maxTotalPixels",
		"maxDecodedBytes",
		"maxOperations",
	] as const) {
		const value = ceilings[name];
		if (value === undefined) continue;
		if (!Number.isInteger(value) || value < 1 || value > 0xffff_ffff) {
			throw new VellumError(
				"INVALID_OPTIONS",
				`${name} must be a whole number from 1 to 4294967295, got ${value}.`,
			);
		}
	}
}

export type { VellumConfig };
