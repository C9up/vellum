import { writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	PdfDocument,
	registerFile,
	toBytes,
	VellumError,
} from "../../src/index.js";

async function bytesOf(doc: PdfDocument): Promise<Buffer> {
	const done = toBytes(doc);
	doc.end();
	return Buffer.from(await done);
}

const XML = Buffer.from(
	'<?xml version="1.0"?><Invoice><ID>2026-041</ID></Invoice>',
);

describe("PdfDocument — embedded files", () => {
	it("embeds files from a path, bytes and a data URL, by name", async () => {
		const path = join(tmpdir(), `vellum-facture-${process.pid}.xml`);
		writeFileSync(path, XML);
		const doc = new PdfDocument({ compress: false });
		const options = { description: "La facture en XML" };
		const fromPath = doc.file(path, options);
		expect(fromPath.name).toBe(`vellum-facture-${process.pid}.xml`);
		expect(options).toEqual({ description: "La facture en XML" });
		doc.file(XML, {
			name: "copie.xml",
			type: "text/xml",
			relationship: "Data",
		});
		doc.file(
			`data:text/plain;base64,${Buffer.from("Bonjour").toString("base64")}`,
			{
				name: "note.txt",
			},
		);
		registerFile("/virtual/annexe.txt", Buffer.from("Annexe"), {
			birthtime: new Date(Date.UTC(2026, 0, 1)),
		});
		doc.file("/virtual/annexe.txt");
		const pdf = await bytesOf(doc);
		expect(pdf.includes(Buffer.from("/EmbeddedFiles"))).toBe(true);
		for (const name of ["copie.xml", "note.txt", "annexe.txt"]) {
			expect(pdf.includes(Buffer.from(name)), name).toBe(true);
		}
		expect(
			pdf.includes(Buffer.from(tmpdir())),
			"no server path in the document",
		).toBe(false);
		registerFile("/virtual/annexe.txt", undefined);
	});

	it("refuses a nameless file, one too large, and two under one name", () => {
		const doc = new PdfDocument({ maxFileBytes: 4 });
		expect(() => doc.file(Buffer.from("x"))).toThrow(/needs a name/);
		expect(() => doc.file(XML, { name: "big.xml" })).toThrow(
			expect.objectContaining({ code: "E_VELLUM_INVALID_OPTIONS" }),
		);
		const twice = new PdfDocument();
		twice.file(Buffer.from("a"), { name: "same.txt" });
		twice.file(Buffer.from("b"), { name: "same.txt" });
		expect(() => twice.end()).toThrow(/two files/);
	});

	it("carries an XML alternative in PDF/A-3, and not in PDF/A-2", async () => {
		const ok = new PdfDocument({ subset: "PDF/A-3b" });
		ok.text("Facture 2026-041");
		ok.file(XML, {
			name: "factur-x.xml",
			type: "text/xml",
			relationship: "Alternative",
			description: "Factur-X",
			modifiedDate: new Date(),
		});
		const pdf = await bytesOf(ok);
		expect(pdf.includes(Buffer.from("/AFRelationship"))).toBe(true);

		const refused = new PdfDocument({ subset: "PDF/A-2b" });
		refused.file(XML, { name: "x.xml", type: "text/xml", description: "x" });
		expect(() => refused.end()).toThrow(VellumError);
	});
});

describe("PdfDocument — annotations and destinations", () => {
	it("writes every annotation pdfkit has", async () => {
		const doc = new PdfDocument({ compress: false });
		doc.note(72, 72, 20, 20, "Voir les conditions", { Name: "Help" });
		doc.highlight(72, 100, 200, 12);
		doc.underline(72, 120, 200, 12, { color: "blue" });
		doc.strike(72, 140, 200, 12);
		doc.lineAnnotation(72, 160, 272, 180);
		doc.rectAnnotation(72, 200, 100, 50);
		doc.ellipseAnnotation(200, 200, 100, 50);
		doc.textAnnotation(72, 260, 200, 30, "Texte libre");
		doc.fileAnnotation(72, 300, 20, 20, {
			src: XML,
			name: "annexe.xml",
			description: "Annexe",
		});
		doc.annotate(72, 340, 100, 20, {
			Subtype: "Square",
			Contents: "Générique",
		});
		const pdf = await bytesOf(doc);
		for (const subtype of [
			"/Text",
			"/Highlight",
			"/Underline",
			"/StrikeOut",
			"/Line",
			"/Square",
			"/Circle",
			"/FreeText",
			"/FileAttachment",
			"/QuadPoints",
		]) {
			expect(
				pdf.includes(Buffer.from(`/Subtype ${subtype}`)) ||
					pdf.includes(Buffer.from(subtype)),
				subtype,
			).toBe(true);
		}
		expect(pdf.includes(Buffer.from("/Help"))).toBe(true);
	});

	it("refuses annotations PDF/A cannot carry, and colours that are not RGB", () => {
		const archived = new PdfDocument({ subset: "PDF/A-2b" });
		archived.note(72, 72, 20, 20, "Note");
		expect(() => archived.end()).toThrow(
			expect.objectContaining({ code: "E_VELLUM_VALIDATION" }),
		);
		expect(() =>
			new PdfDocument().highlight(0, 0, 10, 10, { color: [0, 0, 0, 100] }),
		).toThrow(VellumError);
	});

	it("opens destinations with every view", async () => {
		const doc = new PdfDocument({ compress: false });
		doc.addNamedDestination("haut", "XYZ", 72, 100, 2);
		doc.addNamedDestination("page", "Fit");
		doc.addNamedDestination("bord", "FitB");
		doc.addNamedDestination("largeur", "FitH", 200);
		doc.addNamedDestination("bordh", "FitBH");
		doc.addNamedDestination("hauteur", "FitV", 50);
		doc.addNamedDestination("bordv", "FitBV", 50);
		doc.addNamedDestination("cadre", "FitR", 72, 72, 300, 300);
		doc.goTo(72, 400, 100, 20, "cadre");
		const pdf = await bytesOf(doc);
		for (const view of [
			"/Fit",
			"/FitB",
			"/FitH",
			"/FitBH",
			"/FitV",
			"/FitBV",
			"/FitR",
		]) {
			expect(pdf.includes(Buffer.from(view)), view).toBe(true);
		}
	});
});
