import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { contract, invoice, quote } from "../../examples/documents.js";
import { PdfDocument, toBytes, Vellum } from "../../src/index.js";
import { decodePng, type Pixels } from "./png.js";

const vellum = new Vellum();

/** The share of pixels that differ visibly between two renders. */
function difference(a: Pixels, b: Pixels): number {
	expect([a.width, a.height]).toEqual([b.width, b.height]);
	let differing = 0;
	for (let y = 0; y < a.height; y++) {
		for (let x = 0; x < a.width; x++) {
			const [r1, g1, b1] = a.at(x, y);
			const [r2, g2, b2] = b.at(x, y);
			if (Math.abs(r1 - r2) + Math.abs(g1 - g2) + Math.abs(b1 - b2) > 96)
				differing++;
		}
	}
	return differing / (a.width * a.height);
}

const DOCUMENTS = { quote, invoice, contract } as const;

/** Each page's text, line by line; a justified line is drawn without spaces. */
async function lines(pdf: Buffer): Promise<string[][]> {
	const pages = await vellum.extractTextAll(pdf);
	return pages.map((text) =>
		text.split("\n").map((line) => line.replace(/\s+/g, "")),
	);
}

describe("documents written for pdfkit, run on vellum", () => {
	for (const [name, draw] of Object.entries(DOCUMENTS)) {
		it(`draws the ${name} as pdfkit does, page for page`, async () => {
			const reference = readFileSync(
				new URL(`../fixtures/pdfkit-documents/${name}.pdf`, import.meta.url),
			);
			const doc = new PdfDocument({ size: "A4", bufferPages: true });
			draw(doc);
			const bytes = toBytes(doc);
			doc.end();
			const ours = Buffer.from(await bytes);
			const pages = await vellum.pageCount(reference);
			expect(await vellum.pageCount(ours)).toBe(pages);
			for (let page = 1; page <= pages; page++) {
				const share = difference(
					decodePng(await vellum.render(ours, { page })),
					decodePng(await vellum.render(reference, { page })),
				);
				// The layout is pdfkit's to the glyph; what differs is the glyphs'
				// drawing: pdfkit leaves Helvetica to the reader, vellum embeds
				// Liberation Sans in its place. Under 1 % of the page.
				expect(share, `${name} page ${page}`).toBeLessThan(0.01);
			}
			// Same words on the same lines of the same pages.
			expect(await lines(ours)).toEqual(await lines(reference));
		});
	}
});
