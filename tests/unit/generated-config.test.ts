/**
 * The config `ream configure @c9up/vellum` writes must be usable.
 *
 * It read its format from the environment — `env.get('VELLUM_FORMAT', 'png')`,
 * which is a `string` — into a field typed as the `"png" | "jpeg"` union. So
 * the very first thing an application did after installing vellum left it with
 * a `config/vellum.ts` that did not typecheck.
 *
 * A generated file is only correct if it compiles where it lands, so it is
 * compiled here rather than read.
 */
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { defineConfig } from "../../src/config.js";
import { configure } from "../../src/configure.js";

const packageRoot = path.resolve(
	path.dirname(fileURLToPath(import.meta.url)),
	"../..",
);
let scratch: string | undefined;

afterEach(() => {
	if (scratch) rmSync(scratch, { recursive: true, force: true });
	scratch = undefined;
});

/** What `configure` writes, without touching a disk. */
async function generated(): Promise<string> {
	let written = "";
	await configure({
		addEnvVars: vi.fn(async () => {}),
		addProvider: vi.fn(async () => {}),
		writeFile: vi.fn(async (_path: string, contents: string) => {
			written = contents;
		}),
	} as never);
	return written;
}

/** Compile `source` against this package, the way an application would. */
function typecheck(source: string): string {
	// Inside the package: resolution has to find `@c9up/vellum` as a project's
	// would, which a directory outside the tree cannot.
	scratch = mkdtempSync(path.join(packageRoot, ".generated-config-"));
	writeFileSync(path.join(scratch, "config.ts"), source);
	try {
		execFileSync(
			process.execPath,
			[
				path.join(packageRoot, "node_modules/typescript/bin/tsc"),
				"--noEmit",
				"--ignoreConfig",
				"--strict",
				"--module",
				"nodenext",
				"--moduleResolution",
				"nodenext",
				"--target",
				"es2022",
				"--skipLibCheck",
				"--experimentalDecorators",
				"--emitDecoratorMetadata",
				"--types",
				"node",
				path.join(scratch, "config.ts"),
			],
			{ cwd: packageRoot, encoding: "utf8", stdio: "pipe" },
		);
		return "";
	} catch (error) {
		const shown = error as { stdout?: string; stderr?: string };
		return `${shown.stdout ?? ""}${shown.stderr ?? ""}`;
	}
}

describe("vellum > the config `ream configure` generates", () => {
	it("compiles where it lands", async () => {
		// `#start/env` belongs to the application, so it stands in for one that
		// returns what a real one returns: a string.
		const source = (await generated()).replace(
			"import env from '#start/env'",
			"const env = { get: (_k: string, d: string): string => d }",
		);
		expect(typecheck(source)).toBe("");
	});

	it("closes every block it opens", async () => {
		const source = await generated();
		const opens = (source.match(/\[/g) ?? []).length;
		const closes = (source.match(/\]/g) ?? []).length;
		expect(closes).toBe(opens);
		expect((source.match(/\{/g) ?? []).length).toBe(
			(source.match(/\}/g) ?? []).length,
		);
	});
});

describe("vellum > defineConfig", () => {
	it("takes the string an environment yields", () => {
		expect(defineConfig({ format: "jpeg" }).format).toBe("jpeg");
	});

	it("names a format it cannot use, at boot, with the value", () => {
		// Rather than at the first render of a document nobody was watching.
		expect(() => defineConfig({ format: "tiff" })).toThrow(
			/asks for format "tiff"/,
		);
	});

	it("leaves the format out when none was given", () => {
		expect("format" in defineConfig({ scale: 2 })).toBe(false);
	});
});
