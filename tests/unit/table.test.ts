import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { PdfDocument, toBytes, Vellum } from "../../src/index.js";
import { decodePng, type Pixels } from "./png.js";

const vellum = new Vellum();
const SANS = fileURLToPath(
	new URL("../../fonts/LiberationSans-Regular.ttf", import.meta.url),
);

async function bytesOf(doc: PdfDocument): Promise<Buffer> {
	const done = toBytes(doc);
	doc.end();
	return Buffer.from(await done);
}

async function page(pdf: Buffer, number: number): Promise<Pixels> {
	return decodePng(await vellum.render(pdf, { page: number }));
}

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

/** The document tests/fixtures/pdfkit-tables.pdf was drawn from, with pdfkit 0.20.2. */
function tables(doc: PdfDocument): void {
	doc.fontSize(10);
	doc.table({
		columnStyles: [120, "*", 80],
		rowStyles: (i) => (i === 0 ? { backgroundColor: "#dddddd" } : undefined),
		data: [
			[
				"Désignation",
				"Description",
				{ text: "Montant", align: { x: "right" } },
			],
			[
				"Peinture",
				"Murs et plafonds du séjour, deux couches, préparation comprise",
				{ text: "1 200,00", align: { x: "right" } },
			],
			[
				"Électricité",
				"Remplacement du tableau",
				{ text: "850,00", align: { x: "right" } },
			],
			[
				{ text: "Total", colSpan: 2, align: { x: "right" } },
				{ text: "2 050,00", align: { x: "right" }, textColor: "red" },
			],
		],
	});
	doc.moveDown();
	doc.table({
		defaultStyle: { border: 0.5, padding: 4 },
		columnStyles: ["40%", "*"],
		data: [
			[
				{
					text: "Fusion",
					rowSpan: 2,
					backgroundColor: "#eeeeff",
					align: "center",
				},
				"Ligne un",
			],
			["Ligne deux"],
			["Bas", { text: "Centré", align: { x: "center", y: "center" } }],
		],
	});
	doc.addPage();
	const table = doc.table({ columnStyles: [60, "*"], rowStyles: () => 30 });
	for (let i = 0; i < 30; i++)
		table.row([`${i + 1}`, `Ligne ${i + 1} du long tableau`]);
	table.end();
}

describe("PdfDocument — tables", () => {
	// Rendered side by side, the two documents come out identical.
	it("draws the same tables as pdfkit, page for page", async () => {
		const reference = readFileSync(
			new URL("../fixtures/pdfkit-tables.pdf", import.meta.url),
		);
		const doc = new PdfDocument({ size: "A4", font: SANS, bufferPages: true });
		tables(doc);
		const ours = await bytesOf(doc);
		expect(await vellum.pageCount(ours)).toBe(
			await vellum.pageCount(reference),
		);
		for (let number = 1; number <= 3; number++) {
			const share = difference(
				await page(ours, number),
				await page(reference, number),
			);
			expect(
				share,
				`page ${number} differs on ${(share * 100).toFixed(2)}% of its pixels`,
			).toBeLessThan(0.002);
		}
	});

	it("repeats the header rows at the top of every page", async () => {
		const doc = new PdfDocument({ size: "A5", bufferPages: true });
		const table = doc.table({ headerRows: 1, columnStyles: [40, "*"] });
		table.row(["N°", "Désignation"]);
		for (let i = 1; i <= 80; i++) table.row([`${i}`, `Article ${i}`]);
		table.end();
		const pages = await vellum.extractTextAll(await bytesOf(doc));
		expect(pages.length).toBeGreaterThan(2);
		for (const text of pages)
			expect(text.trimStart().startsWith("N°")).toBe(true);
		const all = pages.join("\n");
		expect(all).toContain("Article 80");
		// Every article once: the header is the only thing repeated.
		expect(all.match(/Article 37\b/g)).toHaveLength(1);
	});

	it("answers the document with data, the table without, and refuses rows after the end", () => {
		const doc = new PdfDocument();
		expect(doc.table({ data: [["a", "b"]] })).toBe(doc);
		const table = doc.table();
		table.row([{ text: "Fusionnée", rowSpan: 3 }, "un"]);
		// end() draws the rows the span still holds back.
		expect(table.end()).toBe(doc);
		expect(() => table.row(["trop tard"])).toThrow(/ended/);
	});

	it("keeps each side's border colour, and tells of a row too tall", async () => {
		const warnings: string[] = [];
		const listener = (warning: Error & { code?: string }) => {
			if (warning.code === "VELLUM_TABLE_ROW_CLAMPED")
				warnings.push(warning.message);
		};
		process.on("warning", listener);
		const doc = new PdfDocument({ size: [200, 200], margin: 10 });
		doc.table({
			data: [
				[
					{
						text: "Côtés",
						border: 6,
						borderColor: {
							top: "red",
							right: "blue",
							bottom: "red",
							left: "blue",
						},
					},
				],
			],
		});
		doc.table({ rowStyles: () => 500, data: [["Trop haut"]] });
		const bytes = await bytesOf(doc);
		await new Promise((resolve) => setImmediate(resolve));
		process.off("warning", listener);
		expect(warnings).toHaveLength(1);
		const image = decodePng(await vellum.render(bytes, { page: 1 }));
		const top = image.at(60, 10);
		const left = image.at(10, 14);
		expect(top[0]).toBeGreaterThan(200);
		expect(left[2]).toBeGreaterThan(200);
	});

	it("takes every form of style pdfkit takes", async () => {
		const doc = new PdfDocument({ bufferPages: true });
		const table = doc.table({
			debug: true,
			maxWidth: 400,
			defaultStyle: {
				padding: [2, 6],
				border: { vertical: 1, horizontal: 0.5 },
			},
			columnStyles: (i) =>
				i === 0
					? { width: "*", minWidth: 50, maxWidth: 120 }
					: i === 1
						? "*"
						: { width: 90 },
			rowStyles: [{ maxHeight: 40 }, 25],
		});
		table.row([
			{ text: "Gras", font: { src: "Helvetica-Bold", size: 12 } },
			{ text: "Tourné", textOptions: { rotation: 90 } },
			{ text: "Penché", textOptions: { rotation: 45 } },
		]);
		table.row([
			{ text: "Famille", font: { family: "Helvetica" } },
			{ text: "Contour", textStroke: 0.5, textStrokeColor: "blue" },
			{
				text: "Retourné",
				textOptions: { rotation: 135 },
				padding: { top: 1, right: 2, bottom: 3, left: 4 },
			},
		]);
		table.row(["dernier", "rang", "ici"], true);
		const crowded = doc.table({
			maxWidth: 100,
			columnStyles: { width: "*", minWidth: 80 },
		});
		crowded.row(["a", "b"]).end();
		doc.table({ rowStyles: 20, data: [["Hauteur fixe"]] });
		const text = (await vellum.extractText(await bytesOf(doc))).replace(
			/\s+/g,
			"",
		);
		for (const word of [
			"Gras",
			"Famille",
			"Contour",
			"dernier",
			"Hauteurfixe",
		]) {
			expect(text).toContain(word);
		}
	});
});
