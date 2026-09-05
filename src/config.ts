import type { ImageFormat, VellumConfig } from "./Vellum.js";

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
	return { ...rest, format: format as ImageFormat };
}

export type { VellumConfig };
