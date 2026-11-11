import { describe, expect, it } from "vitest";
import { A4, createBlank, PdfDocument, Vellum } from "../../src/index.js";

/**
 * Names an object answers without having been given them. A colour, a font
 * or a signer name is often a request's parameter.
 */
const INHERITED = ["constructor", "__proto__", "hasOwnProperty", "toString"];

describe("a name that only the prototype answers", () => {
	it("is not a colour", () => {
		const doc = new PdfDocument();
		for (const name of INHERITED) {
			expect(() => doc.fillColor(name), name).toThrow(
				expect.objectContaining({ code: "E_VELLUM_INVALID_COLOR" }),
			);
		}
	});

	it("is not a configured signer, and nothing is prepared for it", async () => {
		const vellum = new Vellum({ signers: {} });
		for (const name of INHERITED) {
			await expect(
				vellum.sign(createBlank([A4]), { signer: name }),
				name,
			).rejects.toMatchObject({ code: "E_VELLUM_UNKNOWN_SIGNER" });
		}
	});

	it("is not a configured font", async () => {
		const vellum = new Vellum({ fonts: { body: "/nonexistent.ttf" } });
		for (const name of INHERITED) {
			await expect(
				vellum.stampText(createBlank([A4]), "x", { font: name }),
				name,
			).rejects.toThrow(/standard fonts/);
		}
	});

	it("is not an annotation subtype", () => {
		const doc = new PdfDocument();
		expect(() =>
			Reflect.apply(doc.annotate, doc, [
				10,
				10,
				50,
				50,
				{ Subtype: "constructor" },
			]),
		).toThrow(expect.objectContaining({ code: "E_VELLUM_INVALID_OPTIONS" }));
	});
});
