import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	PdfDocument,
	type PdfDocumentOptions,
	toBytes,
	Vellum,
} from "../../src/index.js";

const vellum = new Vellum();

/**
 * When set, each profile's document is written there, for the CI job that
 * runs veraPDF over them.
 */
const OUT = process.env.VELLUM_CONFORMANCE_DIR;

const PROFILES = [
	"PDF/A-1b",
	"PDF/A-2b",
	"PDF/A-3b",
	"PDF/A-2a",
	"PDF/UA",
] as const;

/** An invoice-like page using every part that is tagged. */
function invoice(subset: (typeof PROFILES)[number]): PdfDocument {
	const doc = new PdfDocument({
		subset,
		tagged: true,
		lang: "fr-CH",
		displayTitle: true,
		info: { Title: "Facture 2026-041", Author: "Atelier" },
	});
	const section = doc.struct("Sect");
	doc.addStructure(section);
	doc.outline.addItem("Facture 2026-041").addItem("Conditions");
	doc
		.fontSize(18)
		.text("Facture 2026-041", { structParent: section, structType: "H1" });
	doc
		.fontSize(11)
		.text(
			"Les présentes conditions générales régissent les prestations.\nSeconde ligne.",
			{
				structParent: section,
				link: "https://c9up.dev",
			},
		);
	const list = doc.struct("L");
	section.add(list);
	doc.list(["Un", "Deux"], { structParent: list });
	doc.table({
		structParent: section,
		data: [
			[
				{ text: "Article", type: "TH", scope: "Column" },
				{ text: "Prix", type: "TH", scope: "Column" },
			],
			["Peinture", "1 200,00"],
		],
	});
	section.add(
		doc.struct("Figure", { alt: "Un carré bleu" }, () => {
			doc.rect(300, 600, 50, 50).fill("#336699");
		}),
	);
	// A footer below the margin, written the way pdfkit documents do it.
	doc.markContent("Artifact", { type: "Pagination" });
	const bottom = doc.page.margins.bottom;
	doc.page.margins.bottom = 0;
	doc.fontSize(8).text("Page 1/1", 72, 760);
	doc.page.margins.bottom = bottom;
	doc.endMarkedContent();
	return doc;
}

async function end(doc: PdfDocument): Promise<Buffer> {
	const bytes = toBytes(doc);
	doc.end();
	return Buffer.from(await bytes);
}

describe("PdfDocument — PDF/A and PDF/UA", () => {
	for (const subset of PROFILES) {
		it(`writes a ${subset} document that meets its standard`, async () => {
			const pdf = await end(invoice(subset));
			expect(await vellum.pageCount(pdf)).toBe(1);
			const text = await vellum.extractText(pdf);
			expect(text).toContain("Facture 2026-041");
			if (subset !== "PDF/UA") {
				const marker = subset.slice(-2, -1);
				expect(
					pdf.includes(Buffer.from(`pdfaid:part>${marker}<`)) ||
						pdf.includes(Buffer.from(`pdfaid:part="${marker}"`)),
				).toBe(true);
			}
			if (OUT !== undefined) {
				mkdirSync(OUT, { recursive: true });
				writeFileSync(join(OUT, `${subset.replace("/", "_")}.pdf`), pdf);
			}
		});
	}

	it("refuses what a standard forbids, saying why", () => {
		const untitled = new PdfDocument({ subset: "PDF/UA", lang: "fr" });
		untitled.text("Sans titre ni sommaire");
		expect(() => untitled.end()).toThrow(
			expect.objectContaining({ code: "E_VELLUM_VALIDATION" }),
		);

		const custom = new PdfDocument({
			subset: "PDF/A-2b",
			info: { Dossier: "42" },
		});
		expect(() => custom.end()).toThrow(/information keys of your own/);

		const transparent = new PdfDocument({ subset: "PDF/A-1b" });
		transparent.rect(0, 0, 10, 10).fillOpacity(0.5).fill("red");
		expect(() => transparent.end()).toThrow(
			expect.objectContaining({ code: "E_VELLUM_VALIDATION" }),
		);
	});

	it("writes the structure a tagged document is read with", async () => {
		const options: PdfDocumentOptions = {
			tagged: true,
			compress: false,
			lang: "fr",
		};
		const doc = new PdfDocument(options);
		const article = doc.struct("Art", { lang: "fr" });
		doc.addStructure(article);
		doc.text("Titre", { structParent: article, structType: "H2" });
		const quote = doc.struct("BlockQuote");
		article.add(quote);
		quote.add(doc.markStructureContent("P"));
		doc.text("Citée");
		doc.endMarkedContent();
		const link = doc.struct("Link", [
			() => doc.link(72, 200, 50, 10, "https://c9up.dev"),
		]);
		article.add(link);
		const pdf = await end(doc);
		for (const tag of [
			"/StructTreeRoot",
			"/H2",
			"/BlockQuote",
			"/Link",
			"/OBJR",
			"/MarkInfo",
		]) {
			expect(pdf.includes(Buffer.from(tag)), tag).toBe(true);
		}
		expect(() => doc.struct("P").end().add(doc.struct("Span"))).toThrow(
			/already-ended/,
		);
	});
});
