/**
 * Documents built to take the process down.
 *
 * A panic in the engine is caught and becomes a rejected promise. A stack
 * overflow is not a panic: it ends the process, every request in it included.
 * Each document below once did; it must now come back as an answer.
 */
import { describe, expect, it } from "vitest";
import { Vellum } from "../../src/Vellum.js";
import { buildPdf, HELVETICA, onePage, stream } from "./rawPdf.js";

const DEEP = 200_000;

describe("vellum > documents nested to overflow the stack", () => {
	const vellum = new Vellum();

	it("refuses a page whose resources nest arrays 200 000 deep", async () => {
		const pdf = onePage(
			stream("", "q Q"),
			`/Foo ${"[".repeat(DEEP)}${"]".repeat(DEEP)}`,
		);
		await expect(vellum.render(pdf)).rejects.toThrow(/page 1 does not exist/);
		await expect(vellum.extractText(pdf)).rejects.toThrow(
			/page 1 does not exist/,
		);
		await expect(vellum.renderAll(pdf)).resolves.toEqual([]);
		expect(await vellum.dimensions(pdf)).toEqual([]);
	});

	it("refuses a page whose resources nest dictionaries 200 000 deep", async () => {
		const pdf = onePage(
			stream("", "q Q"),
			`/Foo ${"<< /A ".repeat(DEEP)}1${" >>".repeat(DEEP)}`,
		);
		await expect(vellum.render(pdf)).rejects.toThrow(/page 1 does not exist/);
		expect(await vellum.dimensions(pdf)).toEqual([]);
	});

	it("renders a page whose content stream nests arrays 200 000 deep, without them", async () => {
		const pdf = onePage(
			stream("", `${"[".repeat(DEEP)}${"]".repeat(DEEP)} pop q Q`),
			HELVETICA,
		);
		const png = await vellum.render(pdf);
		expect(png.subarray(1, 4).toString("latin1")).toBe("PNG");
		await expect(vellum.extractText(pdf)).resolves.toBe("");
	});

	it("renders a page whose colour function nests braces 200 000 deep, without it", async () => {
		const body = `{${"{".repeat(DEEP)}${"}".repeat(DEEP)} pop 0 }`;
		const pdf = onePage(
			stream("", "/CS0 cs 0.5 sc 0 0 100 100 re f"),
			"/ColorSpace << /CS0 [/Separation /Ink /DeviceGray 5 0 R] >>",
			[stream("/FunctionType 4 /Domain [0 1] /Range [0 1]", body)],
		);
		const png = await vellum.render(pdf);
		expect(png.subarray(1, 4).toString("latin1")).toBe("PNG");
	});

	it("is still alive to render an ordinary page afterwards", async () => {
		const pdf = onePage(
			stream("", "BT /F1 12 Tf 72 720 Td (still here) Tj ET"),
			HELVETICA,
		);
		await expect(vellum.extractText(pdf)).resolves.toBe("still here");
	});
});

/** A form that is a chain of `depth` fields, each the only child of the last. */
function fieldChain(depth: number): Buffer {
	const objects: string[] = [
		"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>",
		"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
		"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>",
	];
	for (let level = 0; level < depth; level++) {
		objects.push(`<< /T (a) /Kids [${5 + level} 0 R] >>`);
	}
	objects.push("<< /T (leaf) /FT /Tx /V (x) >>");
	return buildPdf(objects);
}

describe("vellum > a form tree deep enough to overflow the stack", () => {
	const vellum = new Vellum();
	const deep = fieldChain(30_000);

	it("refuses to read it", async () => {
		await expect(vellum.formFields(deep)).rejects.toThrow(
			/deeper than 64 levels/,
		);
	});

	it("refuses to fill, flatten or verify it", async () => {
		await expect(vellum.fillForm(deep, { "a.leaf": "y" })).rejects.toThrow(
			/deeper than 64/,
		);
		await expect(vellum.flattenForm(deep)).rejects.toThrow(/deeper than 64/);
		await expect(vellum.verifySignatures(deep)).rejects.toThrow(
			/deeper than 64/,
		);
	});

	it("still reads a form of ordinary depth", async () => {
		const fields = await vellum.formFields(fieldChain(3));
		expect(fields.map((field) => field.name)).toEqual(["a.a.a.leaf"]);
	});
});
