/**
 * Wires `config/vellum.ts` into the container.
 *
 * Vellum does not import `@c9up/ream`: the slice of the host it needs is
 * duck-typed below, which is what keeps the package publishable on its own and
 * usable from a host that is not Ream.
 */

import { setVellum } from "./services/main.js";
import "./augmentations.js";
import type { VellumConfig } from "./Vellum.js";
import { Vellum } from "./Vellum.js";

interface VellumContainer {
	singleton(token: unknown, factory: () => unknown): void;
	resolve<T = unknown>(token: unknown): Promise<T>;
}

interface VellumConfigStore {
	get<T = unknown>(key: string): T | undefined;
}

export interface VellumAppContext {
	container: VellumContainer;
	config: VellumConfigStore;
}

export default class VellumProvider {
	constructor(protected app: VellumAppContext) {}

	register(): void {
		this.app.container.singleton(Vellum, () => {
			// An absent config is not an error: every rendering option has a
			// defensible default, so a provider registered without
			// `config/vellum.ts` still renders PNG at natural size.
			const config = this.app.config.get<VellumConfig>("vellum") ?? {};
			return new Vellum(config);
		});
		// String alias, so a consumer that cannot import Vellum still resolves
		// it — the same convention the other providers follow.
		this.app.container.singleton("vellum", () =>
			this.app.container.resolve<Vellum>(Vellum),
		);
	}

	async boot(): Promise<void> {
		setVellum(await this.app.container.resolve<Vellum>(Vellum));
	}

	async shutdown(): Promise<void> {}
}
