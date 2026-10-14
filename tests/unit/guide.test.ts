/**
 * The examples of the document generation guide, run. Each test body up to
 * its checks is the guide's example, as printed there.
 */

import { createWriteStream, readFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { finished } from "node:stream/promises";
import { describe, expect, it } from "vitest";
import { PdfDocument, toBytes, Vellum } from "../../src/index.js";
import { encodePng } from "./png.js";

const vellum = new Vellum();
const fontPath = new URL(
	"../../fonts/LiberationSerif-Regular.ttf",
	import.meta.url,
).pathname;
const logo = encodePng(40, 20, (x) =>
	x < 20 ? [0, 64, 133] : [204, 229, 255],
);
const photo = encodePng(60, 40, (x, y) => [x * 4, y * 6, 120]);

async function read(bytes: Promise<Uint8Array>): Promise<Buffer> {
	return Buffer.from(await bytes);
}

describe("guide: generating documents", () => {
	it("introduction", async () => {
		const doc = new PdfDocument({ size: "A4" });
		const bytes = toBytes(doc);
		doc.font("Helvetica-Bold").fontSize(18).text("Devis 2026-041");
		doc.font("Helvetica").fontSize(10).text("Lausanne, le 14 octobre 2026");
		doc.end();

		const pdf = await bytes; // Uint8Array

		expect(await vellum.extractText(Buffer.from(pdf))).toContain(
			"Lausanne, le 14 octobre 2026",
		);
	});

	it("the README's example", async () => {
		const conditions =
			"Les devis sont valables trente jours. Passé ce délai, les prix peuvent être révisés.";

		const doc = new PdfDocument({ size: "A4", bufferPages: true });
		const bytes = toBytes(doc);

		doc.font("Helvetica-Bold").fontSize(18).text("Devis 2026-041");
		doc.font("Helvetica").fontSize(10).text(conditions, { align: "justify" });
		doc.table({
			columnStyles: ["*", 80],
			headerRows: 1,
			data: [
				["Poste", "Montant"],
				["Peinture", "1 190,00"],
			],
		});
		doc.end();

		const pdf = await bytes;

		const text = await vellum.extractText(Buffer.from(pdf));
		expect(text).toContain("Devis 2026-041");
		expect(text).toContain("1 190,00");
	});

	it("the document and its pages", async () => {
		const doc = new PdfDocument({
			size: "A4",
			margins: { top: 50, bottom: 50, left: 72, right: 72 },
			bufferPages: true,
			info: { Title: "Devis 2026-041", Author: "Atelier Dubois Sàrl" },
		});
		const bytes = toBytes(doc);

		doc.text("Première page");
		doc.addPage({ size: "A5", layout: "landscape" });
		doc.text("Deuxième page, en A5 à l'italienne");

		// A footer on every page, once they all exist.
		const { start, count } = doc.bufferedPageRange();
		for (let page = start; page < start + count; page++) {
			doc.switchToPage(page);
			doc.page.margins.bottom = 0;
			doc
				.fontSize(8)
				.text(`page ${page + 1} / ${count}`, 0, doc.page.height - 30, {
					width: doc.page.width,
					align: "center",
				});
		}
		doc.end();
		const pdf = await bytes;

		const buffer = Buffer.from(pdf);
		expect(await vellum.pageCount(buffer)).toBe(2);
		expect(await vellum.extractText(buffer, { page: 2 })).toContain(
			"page 2 / 2",
		);
		expect((await vellum.metadata(buffer)).title).toBe("Devis 2026-041");
	});

	it("text", async () => {
		const doc = new PdfDocument();
		const bytes = toBytes(doc);

		doc.font("Helvetica-Bold").fontSize(16).text("Conditions générales");
		doc.moveDown(0.5);
		doc
			.font("Helvetica")
			.fontSize(10)
			.text(
				"Les devis sont valables trente jours. Passé ce délai, les prix peuvent être révisés.",
				{
					align: "justify",
					columns: 2,
					columnGap: 18,
					paragraphGap: 6,
				},
			);
		doc
			.text("Total : ", { continued: true })
			.font("Helvetica-Bold")
			.text("3 178,14 CHF");
		doc
			.font("Helvetica")
			.list(["Paiement à 30 jours", ["Escompte de 2 %", "Retard : 5 % l'an"]], {
				listType: "numbered",
			});

		// Measured without drawing.
		const width = doc.widthOfString("3 178,14 CHF");
		const height = doc.heightOfString("Un paragraphe à mesurer", {
			width: 200,
		});
		doc.end();

		const text = await vellum.extractText(await read(bytes));
		expect(text).toContain("Conditions générales");
		expect(text).toContain("3 178,14 CHF");
		expect(text).toContain("Escompte de 2 %");
		expect(width).toBeGreaterThan(50);
		expect(height).toBeGreaterThan(10);
	});

	it("fonts", async () => {
		const doc = new PdfDocument();
		const bytes = toBytes(doc);

		doc.registerFont("Corps", fontPath);
		doc
			.font("Corps")
			.fontSize(11)
			.text("Une police embarquée, réduite aux glyphes utilisés.");
		doc.text("Office, ligatures comprises", { features: ["liga"] });
		doc.font("Times-Italic").text("Une des douze polices standard.");
		doc.end();

		const text = await vellum.extractText(await read(bytes));
		expect(text).toContain("Une police embarquée");
		expect(text).toContain("Office");
		expect(text).toContain("douze polices standard");
	});

	it("images", async () => {
		const doc = new PdfDocument();
		const bytes = toBytes(doc);

		doc.image(logo, 72, 40, { width: 120 });
		doc.image(photo, 72, 120, {
			fit: [250, 150],
			align: "center",
			valign: "center",
		});
		doc.image(photo, 340, 120, { cover: [180, 150] });
		const image = doc.openImage(photo);
		doc.image(image, 72, 300, { scale: 2 });
		doc.end();

		const pdf = await read(bytes);
		expect(image.width).toBe(60);
		expect(await vellum.pageCount(pdf)).toBe(1);
	});

	it("vector graphics", async () => {
		const doc = new PdfDocument();
		const bytes = toBytes(doc);

		doc.roundedRect(72, 72, 220, 80, 8).fill("#f4f6f8");
		doc
			.moveTo(72, 170)
			.lineTo(523, 170)
			.lineWidth(0.5)
			.dash(3, { space: 2 })
			.stroke("#999999")
			.undash();
		doc.circle(100, 220, 20).fillAndStroke("#cce5ff", "#004085");
		const gradient = doc
			.linearGradient(72, 260, 292, 260)
			.stop(0, "#004085")
			.stop(1, "#cce5ff");
		doc.rect(72, 250, 220, 20).fill(gradient);
		doc.path("M 320 200 L 360 240 L 400 200 Z").fill("darkorange");
		doc
			.save()
			.rotate(-10, { origin: [450, 300] })
			.fontSize(24)
			.fillColor("red", 0.4)
			.text("PAYÉ", 400, 290)
			.restore();
		doc.end();

		const text = await vellum.extractText(await read(bytes));
		expect(text).toContain("PAYÉ");
	});

	it("tables", async () => {
		const doc = new PdfDocument();
		const bytes = toBytes(doc);

		doc.fontSize(9).table({
			columnStyles: [80, "*", 70],
			rowStyles: (row) =>
				row === 0 ? { backgroundColor: "#eeeeee" } : undefined,
			headerRows: 1,
			data: [
				["Poste", "Description", { text: "Montant", align: { x: "right" } }],
				[
					"Peinture",
					"Murs et plafonds, deux couches",
					{ text: "1 190,00", align: { x: "right" } },
				],
				[
					"Boiseries",
					"Laquage des portes",
					{ text: "460,00", align: { x: "right" } },
				],
			],
		});

		// Row by row, for rows that come from a query.
		const table = doc.table({ columnStyles: ["*", 70], headerRows: 1 });
		table.row(["Poste", "Montant"]);
		for (const [label, amount] of [
			["Fournitures", "640,00"],
			["Nettoyage", "140,00"],
		]) {
			table.row([label ?? "", amount ?? ""]);
		}
		table.end();
		doc.end();

		const text = await vellum.extractText(await read(bytes));
		expect(text).toContain("Murs et plafonds");
		expect(text).toContain("Nettoyage");
	});

	it("navigation", async () => {
		const doc = new PdfDocument();
		const bytes = toBytes(doc);

		doc.text("Voir les conditions en ligne", {
			link: "https://example.ch/conditions",
			underline: true,
		});
		doc.text("Aller à l'annexe", { goTo: "annexe" });
		doc.outline.addItem("Devis");
		doc.addPage();
		doc.addNamedDestination("annexe");
		doc.text("Annexe : plan du séjour");
		doc.outline.addItem("Annexe", { pageNumber: 1 });
		doc.end();

		const pdf = await read(bytes);
		expect(pdf.includes(Buffer.from("https://example.ch/conditions"))).toBe(
			true,
		);
		expect(await vellum.pageCount(pdf)).toBe(2);
	});

	it("tagged, PDF/A and PDF/UA documents", async () => {
		const doc = new PdfDocument({
			subset: "PDF/UA",
			tagged: true,
			lang: "fr-CH",
			displayTitle: true,
			info: { Title: "Facture 2026-117" },
		});
		const bytes = toBytes(doc);

		const body = doc.struct("Document");
		doc.addStructure(body);
		doc.outline.addItem("Facture 2026-117");
		doc
			.fontSize(18)
			.text("Facture 2026-117", { structParent: body, structType: "H1" });
		body.add(
			doc.struct("P", () => doc.fontSize(10).text("Merci de votre confiance.")),
		);
		body.add(
			doc.struct("Figure", { alt: "Logo de l'atelier" }, () =>
				doc.image(logo, { width: 80 }),
			),
		);
		body.end();
		doc.end();

		const text = await vellum.extractText(await read(bytes));
		expect(text).toContain("Facture 2026-117");
		expect(text).toContain("Merci de votre confiance.");
	});

	it("attached files", async () => {
		const xml = Buffer.from('<?xml version="1.0"?><Invoice/>');
		const doc = new PdfDocument({ subset: "PDF/A-3b" });
		const bytes = toBytes(doc);

		doc.text("Facture 2026-117");
		doc.file(xml, {
			name: "factur-x.xml",
			type: "text/xml",
			relationship: "Alternative",
			description: "Factur-X",
			modifiedDate: new Date(),
		});
		doc.end();

		const pdf = await read(bytes);
		expect(pdf.includes(Buffer.from("factur-x.xml"))).toBe(true);
	});

	it("annotations", async () => {
		const doc = new PdfDocument();
		const bytes = toBytes(doc);

		doc.text("Montant à confirmer", 72, 72);
		doc.highlight(72, 72, 110, 12, { color: "yellow" });
		doc.note(190, 70, 14, 14, "Vérifier avec le client", { color: "orange" });
		doc.end();

		const pdf = await read(bytes);
		expect(pdf.includes(Buffer.from("/Highlight"))).toBe(true);
		expect(await vellum.extractText(pdf)).toContain("Montant à confirmer");
	});

	it("forms and encryption", async () => {
		const doc = new PdfDocument({
			ownerPassword: "propriétaire",
			permissions: { printing: "highResolution", fillingForms: true },
		});
		const bytes = toBytes(doc);

		doc.initForm();
		doc.text("Nom", 72, 100);
		doc.formText("nom", 140, 96, 200, 18);
		doc.text("Montant", 72, 130);
		doc.formText("montant", 140, 126, 100, 18, {
			format: { type: "number", nDec: 2, sepComma: false },
		});
		doc.formCombo("pays", 140, 156, 120, 18, {
			select: ["CH", "FR", "DE"],
			value: "CH",
		});
		doc.formCheckbox("accord", 140, 186, 12, 12);
		doc.end();

		const pdf = await read(bytes);
		expect((await vellum.inspect(pdf)).encrypted).toBe(true);
	});

	it("SVG", async () => {
		const doc = new PdfDocument();
		const bytes = toBytes(doc);

		doc.svg(
			'<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><circle cx="20" cy="20" r="18" fill="#004085"/></svg>',
			72,
			72,
			{ width: 120 },
		);
		doc.end();

		expect(await vellum.pageCount(await read(bytes))).toBe(1);
	});

	it("writing it out", async () => {
		const directory = await mkdtemp(join(tmpdir(), "vellum-guide-"));
		const path = join(directory, "devis.pdf");

		const doc = new PdfDocument();
		const file = createWriteStream(path);
		doc.pipe(file);
		doc.text("Devis 2026-041");
		doc.end();
		await finished(file);

		expect(await vellum.extractText(readFileSync(path))).toContain(
			"Devis 2026-041",
		);
		await rm(directory, { recursive: true });
	});
});
