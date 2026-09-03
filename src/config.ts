import type { VellumConfig } from "./Vellum.js";

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
export function defineConfig(config: VellumConfig): VellumConfig {
	return config;
}

export type { VellumConfig };
