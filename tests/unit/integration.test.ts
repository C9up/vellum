import type { ContainerBindings } from "@c9up/ream/types";
import { afterEach, describe, expect, it } from "vitest";
import { configure } from "../../src/configure.js";
import { defineConfig } from "../../src/index.js";
import vellumService, { clearVellum } from "../../src/services/main.js";
import { Vellum } from "../../src/Vellum.js";
import VellumProvider from "../../src/VellumProvider.js";

/**
 * A container standing in for the host's.
 *
 * The overloaded generic signature over a non-generic implementation is what
 * makes this assignable to the provider's interface without a cast.
 */
class FakeContainer {
	readonly #factories = new Map<unknown, () => unknown>();
	readonly #resolved = new Map<unknown, unknown>();

	singleton(token: unknown, factory: () => unknown): void {
		this.#factories.set(token, factory);
	}

	resolve<T = unknown>(token: unknown): Promise<T>;
	async resolve(token: unknown): Promise<unknown> {
		if (this.#resolved.has(token)) return this.#resolved.get(token);

		const factory = this.#factories.get(token);
		if (!factory) throw new Error(`nothing bound for ${String(token)}`);

		const value = await factory();
		this.#resolved.set(token, value);
		return value;
	}
}

class FakeConfigStore {
	readonly #values: Record<string, unknown>;

	constructor(values: Record<string, unknown> = {}) {
		this.#values = values;
	}

	get<T = unknown>(key: string): T | undefined;
	get(key: string): unknown {
		return this.#values[key];
	}
}

describe("VellumProvider", () => {
	afterEach(() => {
		clearVellum();
	});

	it("binds the service and its string alias", async () => {
		const container = new FakeContainer();
		const config = new FakeConfigStore({ vellum: { format: "jpeg" } });
		const provider = new VellumProvider({ container, config });

		provider.register();

		const byClass = await container.resolve<Vellum>(Vellum);
		expect(byClass).toBeInstanceOf(Vellum);
		expect(byClass.config.format).toBe("jpeg");

		// The alias exists so a consumer that cannot import the class still
		// resolves the same instance.
		const byToken = await container.resolve<Vellum>("vellum");
		expect(byToken).toBe(byClass);
	});

	it("still builds a usable service with no config file", async () => {
		const container = new FakeContainer();
		const provider = new VellumProvider({
			container,
			config: new FakeConfigStore(),
		});

		provider.register();

		const service = await container.resolve<Vellum>(Vellum);
		expect(service.config).toEqual({});
	});

	it("populates the service accessor on boot", async () => {
		const container = new FakeContainer();
		const provider = new VellumProvider({
			container,
			config: new FakeConfigStore({ vellum: { width: 800 } }),
		});

		provider.register();
		await provider.boot();

		expect(vellumService.config).toEqual({ width: 800 });
	});
});

describe("services/main", () => {
	afterEach(() => {
		clearVellum();
	});

	it("throws with actionable guidance when read before boot", () => {
		expect(() => vellumService.config).toThrow(/VellumProvider.boot/);
		expect(() => vellumService.config).toThrow(/reamrc.ts/);
	});

	it("answers `then` and symbols with undefined so importing it never throws", () => {
		// A module loader probes an imported binding for `then` to decide
		// whether it is thenable. Resolving the service there would throw
		// during the import itself, before any provider could boot.
		// Reflect.get triggers the Proxy's trap the way a loader does, without
		// needing a cast to reach a property the service type does not declare.
		expect(Reflect.get(vellumService, "then")).toBeUndefined();
		expect(Reflect.get(vellumService, Symbol.toStringTag)).toBeUndefined();
	});
});

describe("configure", () => {
	it("registers the provider, the env var and the config file", async () => {
		const providers: string[] = [];
		const envVars: Record<string, string> = {};
		const files: Record<string, string> = {};

		await configure({
			addProvider: async (path) => {
				providers.push(path);
			},
			addEnvVars: async (vars) => {
				Object.assign(envVars, vars);
			},
			writeFile: async (path, content) => {
				files[path] = content;
			},
		});

		expect(providers).toEqual(["@c9up/vellum/provider"]);
		expect(envVars).toEqual({ VELLUM_FORMAT: "png" });

		const config = files["config/vellum.ts"];
		expect(config).toBeDefined();
		// The config reads the env var the hook declares — a config asking for
		// something nothing ever set is a half-installation.
		expect(config).toContain("VELLUM_FORMAT");
		expect(config).toContain("defineConfig");
	});
});

describe("augmentations", () => {
	it("teaches ream's container what the 'vellum' token resolves to", () => {
		// A `declare module` aimed at a path that does not resolve is silently
		// inert — it compiles and contributes nothing. Annotating with the
		// augmented member is what proves it landed: if the augmentation
		// stopped reaching ream's interface, this stops type-checking.
		const service: ContainerBindings["vellum"] = new Vellum({ format: "png" });

		expect(service).toBeInstanceOf(Vellum);
	});
});

describe("defineConfig", () => {
	it("returns the configuration for the provider to read", () => {
		const config = defineConfig({ format: "jpeg", quality: 82 });

		expect(config).toEqual({ format: "jpeg", quality: 82 });
	});
});
