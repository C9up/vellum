import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
	PdfDocument,
	type PdfDocumentOptions,
	registerFile,
	toBytes,
	Vellum,
	VellumError,
} from "../../src/index.js";

const vellum = new Vellum();

const TERMS =
	"Les présentes conditions générales régissent l'ensemble des prestations fournies par l'entreprise à ses clients, qu'il s'agisse de travaux de rénovation, d'installations neuves ou d'interventions de dépannage. Toute commande implique l'acceptation sans réserve des conditions ci-après.";

async function bytesOf(doc: PdfDocument): Promise<Buffer> {
	const done = toBytes(doc);
	doc.end();
	return Buffer.from(await done);
}

describe("PdfDocument — pages and cursor", () => {
	it("starts with one letter page, its cursor at the margins", () => {
		const doc = new PdfDocument();
		expect(doc.page.width).toBe(612);
		expect(doc.page.height).toBe(792);
		expect(doc.page.margins).toEqual({
			top: 72,
			right: 72,
			bottom: 72,
			left: 72,
		});
		expect([doc.x, doc.y]).toEqual([72, 72]);
		expect(doc.bufferedPageRange()).toEqual({ start: 0, count: 1 });
	});

	it("takes sizes by name, layout and margins in every form", () => {
		const doc = new PdfDocument({
			size: "a4",
			layout: "landscape",
			margins: { top: "1cm", right: 10, bottom: "0.5in", left: "2em" },
		});
		expect(doc.page.width).toBeCloseTo(841.89, 2);
		expect(doc.page.height).toBeCloseTo(595.28, 2);
		expect(doc.page.margins.top).toBeCloseTo(28.35, 2);
		expect(doc.page.margins.bottom).toBe(36);
		expect(doc.page.margins.left).toBe(24);
		expect(doc.page.contentWidth).toBeCloseTo(841.89 - 10 - 24, 2);
		expect(doc.page.maxY()).toBeCloseTo(595.28 - 36, 2);

		// The document's landscape layout carries over to the new page.
		doc.addPage({ size: [300, 400], margin: [10, 20] });
		expect([doc.page.width, doc.page.height]).toEqual([400, 300]);
		expect(doc.page.margins).toEqual({
			top: 10,
			right: 20,
			bottom: 10,
			left: 20,
		});
	});

	it("merges a page's options over the document's", () => {
		const doc = new PdfDocument({ size: "A5", margin: 20 });
		doc.addPage({ layout: "landscape" });
		expect(doc.page.width).toBeCloseTo(595.28, 2);
		expect(doc.page.margins.left).toBe(20);
	});

	it("refuses a length it cannot read, where pdfkit would read 0", () => {
		expect(() => new PdfDocument({ margin: "wide" })).toThrow(VellumError);
		expect(() => new PdfDocument({ size: "A99" })).toThrow(VellumError);
	});

	it("writes page X of Y on buffered pages after the fact", async () => {
		const doc = new PdfDocument({ size: "A4", bufferPages: true });
		doc.text(TERMS.repeat(20));
		const { start, count } = doc.bufferedPageRange();
		expect(count).toBeGreaterThan(1);
		// As fluveo does it with pdfkit: drop the bottom margin for the
		// footer, or the footer itself would run onto a new page.
		for (let page = start; page < start + count; page++) {
			doc.switchToPage(page);
			const bottom = doc.page.margins.bottom;
			doc.page.margins.bottom = 0;
			doc.text(`${page + 1}/${count}`, 0, doc.page.height - 50, {
				align: "center",
			});
			doc.page.margins.bottom = bottom;
		}
		const pdf = await bytesOf(doc);
		const texts = await vellum.extractTextAll(pdf);
		expect(texts).toHaveLength(count);
		texts.forEach((text, page) => {
			expect(text).toContain(`${page + 1}/${count}`);
		});
	});

	it("closes pages to switchToPage unless they are buffered", () => {
		const doc = new PdfDocument();
		doc.addPage();
		expect(doc.bufferedPageRange()).toEqual({ start: 1, count: 1 });
		expect(() => doc.switchToPage(0)).toThrow(/out of bounds/);
	});

	it("emits pageAdded when text runs onto a new page, and lets it draw", async () => {
		const doc = new PdfDocument({ size: "A4", bufferPages: true });
		let added = 0;
		doc.on("pageAdded", () => {
			added++;
			const { x, y } = doc;
			doc.fontSize(8).text(`En-tête ${added}`, 72, 30, { lineBreak: false });
			doc.fontSize(12);
			doc.x = x;
			doc.y = y;
		});
		doc.text(TERMS.repeat(15));
		expect(added).toBe(doc.bufferedPageRange().count - 1);
		const texts = await vellum.extractTextAll(await bytesOf(doc));
		expect(texts[1]).toContain("En-tête 1");
		// The body carried on below the header, not inside it.
		expect(texts[1]).toContain("conditions");
	});
});

describe("PdfDocument — fonts and text", () => {
	it("writes text that reads back, accents included", async () => {
		const doc = new PdfDocument({ size: "A4" });
		doc.font("Helvetica-Bold").fontSize(18).text("Devis n° 2026-041");
		doc.moveDown().font("Times-Roman").fontSize(11).text(TERMS);
		const text = await vellum.extractText(await bytesOf(doc));
		expect(text).toContain("Devis n° 2026-041");
		expect(text.replace(/\s+/g, " ")).toContain("rénovation, d'installations");
	});

	it("measures the way it lays out", () => {
		const doc = new PdfDocument({ size: "A4" });
		const height = doc.heightOfString(TERMS, { width: 300 });
		const top = doc.y;
		doc.text(TERMS, { width: 300 });
		expect(doc.y - top).toBeCloseTo(height, 3);
		// Helvetica's own advance widths: H 722, e 556, l 222, o 556.
		expect(doc.fontSize(10).widthOfString("Hello")).toBeCloseTo(22.78, 1);
		// pdfkit's line height for Helvetica: (718 + 207) / 1000 * size.
		expect(doc.currentLineHeight()).toBeCloseTo(9.25, 3);
	});

	it("loads a font from bytes, a path, a registered name or a registered file", async () => {
		const sans = new URL(
			"../../fonts/LiberationSans-Regular.ttf",
			import.meta.url,
		);
		const { readFileSync } = await import("node:fs");
		const bytes = readFileSync(sans);
		registerFile("/virtual/Body.ttf", bytes);

		const doc = new PdfDocument();
		doc.registerFont("Body", "/virtual/Body.ttf");
		doc.font(bytes).text("from bytes");
		doc.font(fileURLToPath(sans)).text("from a path");
		doc.font("Body").text("from a registered name");
		const text = await vellum.extractText(await bytesOf(doc));
		for (const line of [
			"from bytes",
			"from a path",
			"from a registered name",
		]) {
			expect(text).toContain(line);
		}
		registerFile("/virtual/Body.ttf", undefined);
	});

	it("refuses a file that is not a font", () => {
		const doc = new PdfDocument();
		expect(() => doc.font(Buffer.from("not a font"))).toThrow(
			expect.objectContaining({ code: "E_VELLUM_INVALID_FONT" }),
		);
		expect(() => doc.font("/nowhere/at/all.ttf")).toThrow(
			expect.objectContaining({ code: "E_VELLUM_FILE_NOT_FOUND" }),
		);
	});

	it("puts a bold word inside a sentence", async () => {
		const doc = new PdfDocument();
		doc.text("Montant : ", { continued: true });
		doc.font("Helvetica-Bold").text("1 200,00 CHF", { continued: true });
		doc.font("Helvetica").text(" à trente jours.");
		const text = await vellum.extractText(await bytesOf(doc));
		expect(text.replace(/\s+/g, " ")).toContain(
			"Montant : 1 200,00 CHF à trente jours.",
		);
	});
});

describe("PdfDocument — output", () => {
	it("streams the bytes of a valid document", async () => {
		const doc = new PdfDocument({
			info: { Title: "Facture 2026-041", Author: "Atelier" },
			lang: "fr-CH",
		});
		doc.info.Subject = "Set after construction, written at the end";
		doc.text("Bonjour");
		const pdf = await bytesOf(doc);
		expect(pdf.subarray(0, 5).toString()).toBe("%PDF-");
		const metadata = await vellum.metadata(pdf);
		expect(metadata.title).toBe("Facture 2026-041");
		expect(metadata.subject).toBe("Set after construction, written at the end");
	});

	it("refuses anything after the end", async () => {
		const doc = new PdfDocument();
		await bytesOf(doc);
		expect(() => doc.text("late")).toThrow(
			expect.objectContaining({ code: "E_VELLUM_DOCUMENT_ENDED" }),
		);
		expect(() => doc.end()).toThrow(VellumError);
	});
});

describe("PdfDocument — boundsOfString", () => {
	// pdfkit 0.20.2's answers for the same text, font and options.
	const sans = fileURLToPath(
		new URL("../../fonts/LiberationSans-Regular.ttf", import.meta.url),
	);
	const FR =
		"Les présentes conditions générales régissent l'ensemble des prestations fournies par l'entreprise à ses clients, qu'il s'agisse de travaux de rénovation, d'installations neuves ou d'interventions de dépannage. Toute commande implique l'acceptation sans réserve des conditions ci-après, à l'exclusion de tout autre document. Les devis sont valables trente jours à compter de leur date d'émission ; passé ce délai, les prix peuvent être révisés en fonction de l'évolution du coût des matériaux et de la main-d'œuvre.";

	function document() {
		return new PdfDocument({ size: "A4" }).font(sans).fontSize(11);
	}

	it("measures a wrapped paragraph as pdfkit does", () => {
		const bounds = document().boundsOfString(FR, 72, 72, { width: 300 });
		expect(bounds.x).toBe(72);
		expect(bounds.y).toBe(72);
		expect(bounds.width).toBeCloseTo(299.8896, 2);
		expect(bounds.height).toBeCloseTo(113.8403, 2);
	});

	it("turns the box with the rotation", () => {
		const bounds = document().boundsOfString(FR, 72, 72, {
			width: 300,
			rotation: 90,
		});
		expect(bounds.y).toBeCloseTo(-227.8896, 2);
		expect(bounds.width).toBeCloseTo(113.8403, 2);
		expect(bounds.height).toBeCloseTo(299.8896, 2);
	});

	it("stops at a height, and measures lines given as they are", () => {
		const doc = document();
		expect(
			doc.boundsOfString(FR, 72, 72, { width: 300, height: 50 }).height,
		).toBeCloseTo(37.9468, 2);
		const lines = doc.boundsOfString("Première ligne\nSeconde", 72, 72, {
			lineBreak: false,
		});
		expect(lines.width).toBeCloseTo(70.9199, 2);
		expect(lines.height).toBeCloseTo(25.2979, 2);
	});

	it("measures a justified paragraph by the width it fills", () => {
		// pdfkit answers 310.6 here, wider than the 300 the lines fill.
		const bounds = document().boundsOfString(FR, 72, 72, {
			width: 300,
			align: "justify",
		});
		expect(bounds.width).toBeCloseTo(300, 1);
	});

	it("adds no page while measuring", () => {
		const doc = document();
		doc.boundsOfString(FR.repeat(20), 72, 700, { width: 300 });
		expect(doc.bufferedPageRange().count).toBe(1);
	});
});

describe("PdfDocument — rendering", () => {
	it("renders the pages it writes", async () => {
		const doc = new PdfDocument({ size: "A4" });
		doc.fontSize(24).text("Rendu");
		const png = await vellum.render(await bytesOf(doc), { page: 1 });
		expect(png.subarray(1, 4).toString()).toBe("PNG");
	});
});

describe("PdfDocument — lengths, sides and sources", () => {
	it("converts every unit pdfkit knows", () => {
		const doc = new PdfDocument({ size: "A4" }).fontSize(10);
		const page = { width: 200, height: 100 };
		const cases: Array<[string, number]> = [
			["12", 12],
			["12pt", 12],
			["1in", 72],
			["96px", 72],
			["2.54cm", 72],
			["25.4mm", 72],
			["1pc", 12],
			["2em", 20],
			["2rem", 24],
			["50vw", 100],
			["50vh", 50],
			["50vmin", 50],
			["50vmax", 100],
			["50%", 5],
			["-1in", -72],
		];
		for (const [size, points] of cases) {
			expect(doc.sizeToPoint(size, 0, page)).toBeCloseTo(points, 6);
		}
		expect(doc.sizeToPoint("1ex", 0, page)).toBeCloseTo(
			doc.currentLineHeight(),
			6,
		);
		expect(doc.sizeToPoint("1ch", 0, page)).toBeCloseTo(
			doc.widthOfString("0"),
			6,
		);
		expect(doc.sizeToPoint("50%", 0, page, 300)).toBe(150);
		expect(doc.sizeToPoint(true)).toBe(1);
		expect(doc.sizeToPoint(undefined, "1in")).toBe(72);
		expect(() => doc.sizeToPoint(Number.POSITIVE_INFINITY)).toThrow(
			VellumError,
		);
		expect(() => doc.sizeToPoint("12 parsecs")).toThrow(VellumError);
	});

	it("reads margins in every side form", () => {
		const page = (margin: PdfDocumentOptions["margin"]) =>
			new PdfDocument({ margin }).page.margins;
		expect(page({ vertical: 10, horizontal: 20 })).toEqual({
			top: 10,
			right: 20,
			bottom: 10,
			left: 20,
		});
		expect(page({ top: 1, right: 2, bottom: 3, left: 4 })).toEqual({
			top: 1,
			right: 2,
			bottom: 3,
			left: 4,
		});
		expect(page([1, 2, 3, 4])).toEqual({
			top: 1,
			right: 2,
			bottom: 3,
			left: 4,
		});
		expect(page(undefined)).toEqual({
			top: 72,
			right: 72,
			bottom: 72,
			left: 72,
		});
		const doc = new PdfDocument({ margin: 50 });
		expect(doc.page.contentHeight).toBe(792 - 100);
	});

	it("reads a font from a URL or an ArrayBuffer", async () => {
		const { readFileSync } = await import("node:fs");
		const url = new URL(
			"../../fonts/LiberationSerif-Regular.ttf",
			import.meta.url,
		);
		const bytes = readFileSync(url);
		const doc = new PdfDocument();
		doc.font(url).text("from a URL");
		doc.font(
			bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
		);
		doc.text("from an ArrayBuffer");
		const text = await vellum.extractText(await bytesOf(doc));
		expect(text).toContain("from a URL");
		expect(text).toContain("from an ArrayBuffer");
	});
});

describe("PdfDocument — more of pdfkit's surface", () => {
	it("takes a page's font and size when it is added", () => {
		const doc = new PdfDocument();
		doc.addPage({ font: "Times-Roman", fontSize: 14 });
		// Times: (683 + 217) / 1000 * 14
		expect(doc.currentLineHeight()).toBeCloseTo(12.6, 3);
		doc.font("Courier", 20);
		expect(doc.currentLineHeight()).toBeCloseTo((629 + 157) / 50, 3);
	});

	it("moves by lines, with the document's line gap", () => {
		const doc = new PdfDocument().fontSize(10).lineGap(5);
		const line = doc.currentLineHeight(true);
		const start = doc.y;
		doc.moveDown(2);
		expect(doc.y - start).toBeCloseTo(2 * line + 5, 3);
		doc.moveUp();
		expect(doc.y - start).toBeCloseTo(line, 3);
	});

	it("has no current page once the pages are flushed", () => {
		const doc = new PdfDocument();
		doc.flushPages();
		expect(() => doc.page).toThrow(
			expect.objectContaining({ code: "E_VELLUM_NO_PAGE" }),
		);
		doc.addPage();
		expect(doc.page.width).toBe(612);
	});

	it("writes information keys of the caller's own", async () => {
		const doc = new PdfDocument({
			info: { Dossier: "Ç-42", Échéance: new Date(Date.UTC(2026, 10, 1)) },
			compress: false,
		});
		const pdf = await bytesOf(doc);
		expect(pdf.includes(Buffer.from("/Dossier"))).toBe(true);
	});

	it("turns bounds by any angle", () => {
		const doc = new PdfDocument().fontSize(10);
		const flat = doc.boundsOfString("Bonjour", 100, 100);
		const half = doc.boundsOfString("Bonjour", 100, 100, { rotation: 180 });
		expect(half).toEqual({
			x: 100 - flat.width,
			y: 100 - flat.height,
			width: flat.width,
			height: flat.height,
		});
		const quarter = doc.boundsOfString("Bonjour", 100, 100, { rotation: -90 });
		expect(quarter.width).toBeCloseTo(flat.height, 6);
		const tilted = doc.boundsOfString("Bonjour", 100, 100, { rotation: 45 });
		const diagonal = (flat.width + flat.height) * Math.SQRT1_2;
		expect(tilted.width).toBeCloseTo(diagonal, 3);
		expect(tilted.height).toBeCloseTo(diagonal, 3);
	});

	it("can be read before it ends", async () => {
		const doc = new PdfDocument();
		const chunks: Buffer[] = [];
		doc.on("data", (chunk: Buffer) => chunks.push(chunk));
		await new Promise((resolve) => setImmediate(resolve));
		const ended = new Promise((resolve) => doc.on("end", resolve));
		doc.text("Bonjour").end();
		await ended;
		expect(Buffer.concat(chunks).subarray(0, 5).toString()).toBe("%PDF-");
	});

	it("rejects toBytes when the stream fails", async () => {
		const doc = new PdfDocument();
		const bytes = toBytes(doc);
		doc.destroy(new Error("disk full"));
		await expect(bytes).rejects.toThrow("disk full");
	});
});

describe("PdfDocument — lists", () => {
	it("numbers nested items from 1 at each level, and reads back in order", async () => {
		const doc = new PdfDocument();
		doc.list(["Un", ["Un point un", "Un point deux"], "Deux"], {
			listType: "numbered",
		});
		doc.moveDown().list(["Alpha", "Bravo"], { listType: "lettered" });
		doc.moveDown().list(["Puce", ["Sous-puce"]]);
		// A label and its text are two runs: no space between them reads back.
		const text = (await vellum.extractText(await bytesOf(doc))).replace(
			/\s+/g,
			"",
		);
		expect(text).toContain("1.Un1.Unpointun2.Unpointdeux2.Deux");
		expect(text).toContain("A.AlphaB.Bravo");
		expect(text).toContain("PuceSous-puce");
	});

	it("runs a long list onto the next page", async () => {
		const doc = new PdfDocument({ size: "A5", bufferPages: true });
		const items = Array.from(
			{ length: 60 },
			(_, index) => `Élément ${index + 1}`,
		);
		doc.list(items, 50, 50, { listType: "numbered" });
		expect(doc.bufferedPageRange().count).toBeGreaterThan(1);
		const pages = await vellum.extractTextAll(await bytesOf(doc));
		expect(pages.join("").replace(/\s+/g, "")).toContain("60.Élément60");
	});
});
