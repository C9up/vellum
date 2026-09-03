/**
 * Teach ream's `ContainerBindings` what `container.make('vellum')` returns.
 *
 * ream declares that interface open on purpose: it registers its own entries
 * and expects each package to contribute its own — the comment on the
 * interface names `auth` (warden), `logger` (spectrum) and `db` (atlas) as
 * exactly this. Without the augmentation, resolving by the string token falls
 * back to `unknown` and every call site has to assert a type it cannot prove.
 *
 * AdonisJS does the same from its own packages' providers.
 *
 * Loaded from the package barrel and from the provider, so registering vellum
 * is enough — an application writes no `declare module` of its own.
 */

// Referenced so the augmentation below resolves the module it augments.
import type {} from "@c9up/ream/types";
import type { Vellum } from "./Vellum.js";

declare module "@c9up/ream/types" {
	interface ContainerBindings {
		/** The PDF service, bound by `VellumProvider`. */
		vellum: Vellum;
	}
}
