/**
 * Container service accessor — `import vellum from '@c9up/vellum/services/main'`.
 *
 * Populated by `VellumProvider.boot()`. Reading it before the provider has
 * booted throws rather than answering with an unconfigured service, which
 * would render with defaults nobody chose.
 */

import type { Vellum } from "../Vellum.js";

let instance: Vellum | undefined;

/** @internal Called by the provider once the service exists. */
export function setVellum(service: Vellum): void {
	instance = service;
}

/** @internal Test helper — forget the service between cases. */
export function clearVellum(): void {
	instance = undefined;
}

function resolve(): Vellum {
	if (!instance) {
		throw new Error(
			"[vellum] Vellum service accessed before VellumProvider.boot() ran. " +
				"Check that `@c9up/vellum/provider` is listed in your reamrc.ts providers.",
		);
	}
	return instance;
}

const vellum: Vellum = new Proxy(Object.create(null), {
	get(_target, property) {
		// A module loader probes an imported binding for `then` to see whether
		// it is thenable, and for well-known symbols. Answering those by
		// resolving the service would throw during the import itself, before
		// any provider had a chance to boot.
		if (property === "then" || typeof property === "symbol") {
			return undefined;
		}
		const service = resolve();
		const value = Reflect.get(service, property, service);
		return typeof value === "function" ? value.bind(service) : value;
	},
});

export default vellum;
