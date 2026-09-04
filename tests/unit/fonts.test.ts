import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { A4, createBlank, Vellum, VellumError } from "../../src/index.js";

/**
 * The font the engine's own tests use. It is a fixture, not something the
 * package ships: nothing at runtime reads it.
 */
const testFont = fileURLToPath(
	new URL(
		"../../crates/vellum-engine/tests/fixtures/VellumTestSans.ttf",
		import.meta.url,
	),
);

describe("fonts declared in the configuration", () => {
	it("embeds a configured font and writes what WinAnsi cannot", async () => {
		const vellum = new Vellum({ fonts: { body: testFont } });
		const beyond = "Uměl Řehoř";

		// The standard fonts have no byte for these letters at all.
		await expect(
			vellum.stampText(createBlank([A4]), beyond, { y: 100 }),
		).rejects.toThrow(VellumError);

		const stamped = await vellum.stampText(createBlank([A4]), beyond, {
			y: 100,
			font: "body",
		});
		await expect(vellum.extractText(stamped, { page: 1 })).resolves.toBe(
			beyond,
		);
	});

	it("leaves the standard fonts reachable by name", async () => {
		// A name that is not configured is not an error — it falls through to
		// the fonts every reader already has.
		const vellum = new Vellum({ fonts: { body: testFont } });
		const stamped = await vellum.stampText(createBlank([A4]), "Amelie", {
			y: 100,
			font: "Times-Roman",
		});

		await expect(vellum.extractText(stamped, { page: 1 })).resolves.toBe(
			"Amelie",
		);
	});

	it("refuses an unknown font rather than quietly using another", async () => {
		const vellum = new Vellum();
		await expect(
			vellum.stampText(createBlank([A4]), "Amelie", { font: "Comic Sans" }),
		).rejects.toThrow(/unknown font/);
	});

	it("says which configured font it could not read", async () => {
		const vellum = new Vellum({ fonts: { body: "/nowhere/Missing.ttf" } });
		await expect(
			vellum.stampText(createBlank([A4]), "Amelie", { font: "body" }),
		).rejects.toThrow(/configured as \/nowhere\/Missing\.ttf/);
	});

	it("reads a configured font once and reuses it", async () => {
		// A watermark loop should not re-read the file on every page.
		const vellum = new Vellum({ fonts: { body: testFont } });
		const first = await vellum.stampText(createBlank([A4]), "Amelie", {
			y: 100,
			font: "body",
		});
		const second = await vellum.stampText(createBlank([A4]), "Amelie", {
			y: 100,
			font: "body",
		});

		expect(second.length).toBe(first.length);
	});
});
