/**
 * Three documents written the way pdfkit documents are written: a quote,
 * an invoice and a contract. Each function takes the document it draws on,
 * so the same code runs under pdfkit and under vellum — porting is the
 * import:
 *
 *   - import PDFDocument from "pdfkit"
 *   + import { PdfDocument as PDFDocument } from "@c9up/vellum"
 *
 * The tests render each one with both and compare them page by page. The
 * invoice carries a Swiss QR-bill, drawn by prism's port of swissqrbill,
 * which takes either document.
 */

import { SwissQRBill } from "@c9up/prism/swissqrbill/pdf";
import type { PdfDocument } from "../src/index.js";

const COMPANY = [
	"Atelier Dubois Sàrl",
	"Rue du Lac 1",
	"1003 Lausanne",
	"CHE-123.456.789 TVA",
];
const CLIENT = ["Client SA", "Chemin des Vignes 2", "1800 Vevey"];

const LINES: Array<[string, string, number, number]> = [
	[
		"Préparation",
		"Protection des sols, ponçage et rebouchage des murs du séjour",
		6,
		85,
	],
	[
		"Peinture",
		"Murs et plafonds, deux couches, teinte au choix du client",
		14,
		85,
	],
	["Boiseries", "Laquage des portes et des plinthes", 5, 92],
	["Fournitures", "Peinture minérale, sous-couche, adhésifs", 1, 640],
	[
		"Nettoyage",
		"Évacuation des déchets et nettoyage de fin de chantier",
		2,
		70,
	],
];

const TERMS =
	"Les présentes conditions générales régissent l'ensemble des prestations fournies par l'entreprise à ses clients, qu'il s'agisse de travaux de rénovation, d'installations neuves ou d'interventions de dépannage. Toute commande implique l'acceptation sans réserve des conditions ci-après. Les devis sont valables trente jours à compter de leur date d'émission ; passé ce délai, les prix peuvent être révisés en fonction de l'évolution du coût des matériaux et de la main-d'œuvre.";

function money(amount: number): string {
	const [whole = "0", cents = "00"] = amount.toFixed(2).split(".");
	return `${whole.replace(/\B(?=(\d{3})+(?!\d))/g, " ")},${cents}`;
}

/** The company on the left, the client on the right: a two-column header. */
function header(doc: PdfDocument, title: string, reference: string): void {
	const top = doc.page.margins.top;
	const left = doc.page.margins.left;
	doc
		.font("Helvetica-Bold")
		.fontSize(12)
		.text(COMPANY[0] ?? "", left, top);
	doc.font("Helvetica").fontSize(9);
	for (const line of COMPANY.slice(1)) doc.text(line);
	doc
		.font("Helvetica")
		.fontSize(10)
		.text(CLIENT.join("\n"), 340, top + 40, { width: 200 });
	doc.moveDown(3);
	doc.font("Helvetica-Bold").fontSize(18).text(title, left);
	doc
		.font("Helvetica")
		.fontSize(9)
		.fillColor("#555555")
		.text(reference)
		.fillColor("black");
	doc.moveDown();
}

function lines(doc: PdfDocument): number {
	let total = 0;
	const rows = LINES.map(([item, description, quantity, price]) => {
		total += quantity * price;
		return [
			{ text: item, font: { src: "Helvetica-Bold" } },
			description,
			{ text: String(quantity), align: { x: "right" } },
			{ text: money(price), align: { x: "right" } },
			{ text: money(quantity * price), align: { x: "right" } },
		] as const;
	});
	doc.fontSize(9).table({
		columnStyles: [80, "*", 40, 60, 70],
		rowStyles: (i) => (i === 0 ? { backgroundColor: "#eeeeee" } : undefined),
		defaultStyle: {
			border: { top: 0, right: 0, bottom: 0.5, left: 0 },
			borderColor: "#999999",
			padding: 4,
		},
		data: [
			[
				{ text: "Poste", font: { src: "Helvetica-Bold" } },
				{ text: "Description", font: { src: "Helvetica-Bold" } },
				{ text: "Qté", align: { x: "right" }, font: { src: "Helvetica-Bold" } },
				{
					text: "Prix",
					align: { x: "right" },
					font: { src: "Helvetica-Bold" },
				},
				{
					text: "Total",
					align: { x: "right" },
					font: { src: "Helvetica-Bold" },
				},
			],
			...rows.map((row) => [...row]),
		],
	});
	return total;
}

/** The totals box; answers the amount due. */
function totals(doc: PdfDocument, total: number): number {
	const vat = Math.round(total * 8.1) / 100;
	const x = 340;
	doc.moveDown();
	doc.roundedRect(x - 8, doc.y - 4, 210, 62, 6).fill("#f4f6f8");
	doc.fillColor("black").font("Helvetica").fontSize(10);
	const row = (label: string, value: string, bold = false) => {
		const y = doc.y;
		doc
			.font(bold ? "Helvetica-Bold" : "Helvetica")
			.text(label, x, y, { width: 110 });
		doc.text(value, x + 100, y, { width: 95, align: "right" });
	};
	row("Total HT", money(total));
	row("TVA 8,1 %", money(vat));
	row("Total TTC (CHF)", money(total + vat), true);
	doc.x = doc.page.margins.left;
	return total + vat;
}

/** "page X / Y" on every page, written once they all exist. */
function footers(doc: PdfDocument): void {
	const { start, count } = doc.bufferedPageRange();
	for (let page = start; page < start + count; page++) {
		doc.switchToPage(page);
		const bottom = doc.page.margins.bottom;
		doc.page.margins.bottom = 0;
		doc
			.font("Helvetica")
			.fontSize(8)
			.fillColor("#777777")
			.text(
				`${COMPANY[0]} — page ${page + 1} / ${count}`,
				doc.page.margins.left,
				doc.page.height - 40,
				{
					width:
						doc.page.width - doc.page.margins.left - doc.page.margins.right,
					align: "center",
				},
			)
			.fillColor("black");
		doc.page.margins.bottom = bottom;
	}
}

export function quote(doc: PdfDocument): void {
	header(doc, "Devis", "Devis n° 2026-041 · Lausanne, le 14 octobre 2026");
	const total = lines(doc);
	totals(doc, total);
	doc.moveDown(2);
	doc.font("Helvetica-Bold").fontSize(10).text("Conditions");
	doc.font("Helvetica").fontSize(9).text(TERMS, { align: "justify" });
	footers(doc);
}

export function invoice(doc: PdfDocument): void {
	header(doc, "Facture", "Facture n° 2026-117 · échéance 30 jours");
	const due = totals(doc, lines(doc));
	doc.moveDown(2);
	doc
		.font("Helvetica")
		.fontSize(10)
		.text("Merci de régler le montant de ", { continued: true });
	doc.font("Helvetica-Bold").text(`${money(due)} CHF`, { continued: true });
	doc
		.font("Helvetica")
		.text(
			" sur le compte CH44 3199 9123 0008 8901 2, en mentionnant la référence de la facture.",
		);
	doc.moveDown();
	doc
		.moveTo(doc.page.margins.left, doc.y)
		.lineTo(doc.page.width - doc.page.margins.right, doc.y)
		.lineWidth(0.5)
		.stroke("#bbbbbb");
	doc.moveDown();
	doc
		.fontSize(9)
		.list([
			"Paiement à 30 jours net",
			"Escompte de 2 % à 10 jours",
			"Retard : intérêt de 5 % l'an",
		]);
	// The payment slip takes the foot of the page, where the others have
	// their footer; on a page too full for it, a page of its own.
	new SwissQRBill(
		{
			amount: due,
			creditor: {
				account: "CH44 3199 9123 0008 8901 2",
				address: "Rue du Lac 1",
				city: "Lausanne",
				country: "CH",
				name: "Atelier Dubois Sàrl",
				zip: 1003,
			},
			currency: "CHF",
			debtor: {
				address: "Chemin des Vignes 2",
				city: "Vevey",
				country: "CH",
				name: "Client SA",
				zip: 1800,
			},
			reference: "210000000003139471430009017",
		},
		{ language: "FR" },
	).attachTo(doc);
}

const ARTICLES = [
	[
		"Objet",
		"Le présent contrat a pour objet la rénovation complète du séjour du maître d'ouvrage, selon le devis n° 2026-041 annexé, qui en fait partie intégrante.",
	],
	[
		"Prix",
		`Le prix forfaitaire est de ${money(5432.5)} CHF hors taxes. Il comprend la main-d'œuvre, les fournitures et l'évacuation des déchets, à l'exclusion de tout travail non décrit au devis.`,
	],
	[
		"Délais",
		"Les travaux débuteront le 2 novembre 2026 et dureront trois semaines. Tout retard imputable au maître d'ouvrage prolonge ce délai d'autant.",
	],
	[
		"Garantie",
		"L'entreprise garantit les travaux pendant deux ans à compter de leur réception, conformément aux normes SIA en vigueur.",
	],
	["Résiliation", `${TERMS} ${TERMS}`],
	["For", "Le for juridique est à Lausanne. Le droit suisse est applicable."],
] as const;

export function contract(doc: PdfDocument): void {
	header(doc, "Contrat d'entreprise", "Contrat n° 2026-041-C");
	ARTICLES.forEach(([title, body], index) => {
		doc
			.font("Helvetica-Bold")
			.fontSize(11)
			.text(`Article ${index + 1} — ${title}`);
		doc.moveDown(0.3);
		doc
			.font("Helvetica")
			.fontSize(10)
			.text(body, { align: "justify", paragraphGap: 4 });
		doc.moveDown();
	});
	doc
		.font("Helvetica")
		.fontSize(10)
		.text(
			"Les parties déclarent avoir pris connaissance des conditions générales :",
		);
	doc.moveDown(0.5);
	doc
		.fontSize(9)
		.list(
			[
				TERMS,
				["Annexe A : devis n° 2026-041", "Annexe B : plan du séjour"],
				"Conditions générales, édition 2026",
			],
			{
				listType: "numbered",
			},
		);
	doc.moveDown(2);
	const y = doc.y;
	const left = doc.page.margins.left;
	doc
		.font("Helvetica")
		.fontSize(10)
		.text("Le maître d'ouvrage", left, y, { width: 200 });
	doc.text("L'entreprise", 340, y, { width: 200 });
	doc
		.moveTo(left, y + 60)
		.lineTo(left + 180, y + 60)
		.moveTo(340, y + 60)
		.lineTo(520, y + 60)
		.lineWidth(0.5)
		.stroke();
	footers(doc);
}
