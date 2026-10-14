import { describe, expect, it } from "vitest";
import { PdfDocument, toBytes, Vellum, VellumError } from "../../src/index.js";

const vellum = new Vellum();

async function bytesOf(doc: PdfDocument): Promise<Buffer> {
	const done = toBytes(doc);
	doc.end();
	return Buffer.from(await done);
}

/** A contact form with every kind of field pdfkit makes. */
function form(): PdfDocument {
	const doc = new PdfDocument({ size: "A5" });
	doc.initForm();
	doc.text("Nom", 40, 40);
	const client = doc.formField("client");
	doc.formText("nom", 40, 60, 200, 22, {
		parent: client,
		value: "Atelier Dubois",
		borderColor: "black",
		required: true,
	});
	doc.formText("remarques", 40, 90, 200, 50, { multiline: true, fontSize: 9 });
	doc.formCheckbox("accord", 40, 150, 14, 14, { value: "Yes" });
	doc.formRadioButton("livraison", 40, 175, 14, 14);
	doc.formCombo("devise", 40, 200, 80, 20, {
		select: ["CHF", "EUR"],
		value: "CHF",
	});
	doc.formList("options", 40, 230, 120, 40, {
		select: ["Pose", "Livraison", "Garantie"],
	});
	doc.formText("montant", 40, 280, 120, 20, {
		align: "right",
		format: {
			type: "number",
			nDec: 2,
			sepComma: true,
			currency: "CHF ",
			currencyPrepend: true,
		},
	});
	doc.formPushButton("envoyer", 40, 310, 80, 24, {
		label: "Envoyer",
		backgroundColor: "#dddddd",
	});
	return doc;
}

describe("PdfDocument — forms", () => {
	it("writes fields that read back, show their value, and fill", async () => {
		const pdf = await bytesOf(form());
		const fields = await vellum.formFields(pdf);
		const byName = new Map(fields.map((field) => [field.name, field]));
		for (const name of [
			"client.nom",
			"remarques",
			"accord",
			"livraison",
			"devise",
			"options",
			"montant",
			"envoyer",
		]) {
			expect(byName.has(name), name).toBe(true);
		}
		expect(byName.get("client.nom")?.value).toBe("Atelier Dubois");
		expect(byName.get("client.nom")?.required).toBe(true);
		expect(byName.get("remarques")?.multiline).toBe(true);
		expect(byName.get("devise")?.options).toEqual(["CHF", "EUR"]);
		const text = await vellum.extractText(pdf);
		expect(text).toContain("Atelier Dubois");
		expect(text).toContain("Envoyer");

		const filled = await vellum.fillForm(pdf, { remarques: "Livrer le matin" });
		expect(await vellum.extractText(filled)).toContain("Livrer le matin");
		expect(pdf.includes(Buffer.from("AFNumber_Keystroke(2,0,"))).toBe(true);
	});

	it("needs initForm first, and refuses a format that would carry script", () => {
		expect(() => new PdfDocument().formText("x", 0, 0, 10, 10)).toThrow(
			expect.objectContaining({ code: "E_VELLUM_NO_FORM" }),
		);
		const doc = new PdfDocument().initForm();
		expect(() =>
			doc.formText("date", 0, 0, 10, 10, {
				format: { type: "date", param: 'dd"); app.alert(1); ("' },
			}),
		).toThrow(VellumError);
		expect(() => doc.formText("a.b", 0, 0, 10, 10)).toThrow(VellumError);
	});
});

describe("PdfDocument — encryption", () => {
	it("encrypts with AES-256, under the permissions given", async () => {
		const doc = new PdfDocument({
			userPassword: "lecture",
			ownerPassword: "proprietaire",
			permissions: { printing: "highResolution", copying: true },
		});
		doc.text("Confidentiel");
		const pdf = await bytesOf(doc);
		expect((await vellum.inspect(pdf)).encrypted).toBe(true);
		expect(pdf.includes(Buffer.from("/AESV3"))).toBe(true);
		expect(pdf.includes(Buffer.from("Confidentiel"))).toBe(false);
	});

	it("refuses encryption in PDF/A, which forbids it", () => {
		const doc = new PdfDocument({ subset: "PDF/A-2b", userPassword: "x" });
		expect(() => doc.end()).toThrow(
			expect.objectContaining({ code: "E_VELLUM_VALIDATION" }),
		);
	});
});
