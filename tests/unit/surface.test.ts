/**
 * The whole generation API in one document, written as typed code with no
 * `any` and no assertion: the typecheck proves the surface can be used as
 * declared, the run that every call draws.
 */

import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { PdfDocument, toBytes, Vellum } from "../../src/index.js";
import { encodePng } from "./png.js";

const vellum = new Vellum();
const SERIF = fileURLToPath(
	new URL("../../fonts/LiberationSerif-Regular.ttf", import.meta.url),
);
const PNG = encodePng(8, 8, (x, y) =>
	(x + y) % 2 === 0 ? [20, 60, 200] : [255, 255, 255],
);
const SVG = Buffer.from(
	'<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><circle cx="10" cy="10" r="8" fill="green"/></svg>',
);

function everything(doc: PdfDocument): void {
	// Pages, cursor and buffered pages.
	expect(doc.page.contentWidth).toBeGreaterThan(0);
	expect(doc.page.contentHeight).toBeGreaterThan(0);
	expect(doc.page.maxY()).toBeLessThan(doc.page.height);
	expect(doc.sizeToPoint("1in")).toBe(72);
	doc.x = doc.page.margins.left;
	doc.y = doc.page.margins.top;

	// Fonts and text.
	doc.registerFont("Serif", SERIF);
	doc.font("Serif").fontSize(12).lineGap(1);
	expect(doc.currentLineHeight(true)).toBeGreaterThan(12);
	expect(doc.widthOfString("Devis", { characterSpacing: 0.5 })).toBeGreaterThan(
		0,
	);
	doc.text("Surface complète", { align: "center", underline: true });
	doc.font("Helvetica").text("Deuxième ligne", 72, doc.y, {
		width: 300,
		lineBreak: true,
		link: "https://example.com",
	});
	doc.moveDown().moveUp(0.5).moveDown(0.5);
	doc.list(["un", ["deux", "trois"]], { listType: "numbered" });
	expect(doc.heightOfString("Un texte", { width: 100 })).toBeGreaterThan(0);
	expect(doc.boundsOfString("Un texte", { width: 100 }).width).toBeGreaterThan(
		0,
	);

	// Graphics state and paths.
	doc
		.save()
		.translate(10, 0)
		.rotate(5, { origin: [200, 300] })
		.scale(1, { origin: [200, 300] });
	doc.transform(1, 0, 0, 1, 0, 0);
	doc
		.lineWidth(1)
		.lineCap("round")
		.lineJoin("bevel")
		.miterLimit(4)
		.dash(3, { space: 2, phase: 0 });
	doc
		.moveTo(72, 400)
		.lineTo(200, 400)
		.bezierCurveTo(220, 380, 240, 420, 260, 400)
		.quadraticCurveTo(280, 380, 300, 400);
	doc.closePath().stroke("#333333").undash();
	doc.rect(72, 420, 50, 20).fill("#eeeeee", "even-odd");
	doc.roundedRect(130, 420, 50, 20, 4).fillAndStroke("#dddddd", "#111111");
	doc
		.ellipse(220, 430, 20, 10)
		.circle(260, 430, 8)
		.arc(300, 430, 10, 0, Math.PI, false)
		.stroke();
	doc
		.polygon([320, 420], [340, 440], [360, 420])
		.path("M 380 420 L 400 440 Z")
		.clip();
	doc.restore();
	doc
		.fillColor("navy", 0.8)
		.strokeColor([0, 0, 0], 1)
		.opacity(1)
		.fillOpacity(0.9)
		.strokeOpacity(0.9);
	const gradient = doc
		.linearGradient(72, 460, 172, 460)
		.stop(0, "red")
		.stop(1, "blue", 0.5);
	gradient.setTransform(1, 0, 0, 1, 0, 0);
	doc.rect(72, 460, 100, 10).fill(gradient);
	const radial = doc
		.radialGradient(250, 465, 0, 250, 465, 20)
		.stop(0, "white")
		.stop(1, "green");
	doc.circle(250, 465, 10).fill(radial);
	doc
		.addSpotColor("Brand", 0, 50, 100, 0)
		.fillColor("Brand")
		.rect(300, 460, 20, 10)
		.fill();
	doc.addContent("72 480 m 120 480 l").stroke();

	// Links, destinations, structure and the outline.
	doc.addNamedDestination("start", "XYZ", 72, 72, null);
	doc
		.link(72, 490, 60, 12, "https://example.com")
		.goTo(140, 490, 60, 12, "start");
	const section = doc.struct("Sect");
	doc.addStructure(section);
	section.add(doc.struct("P", () => doc.text("Balisé", 72, 510)));
	section.end();
	doc.markContent("Artifact", { type: "Layout" }).endMarkedContent();
	doc.markStructureContent("P");
	doc.endMarkedContent();
	doc.outline.addItem("Surface").addItem("Détail");

	// Images and drawings.
	const image = doc.openImage(PNG);
	expect(image.width).toBe(8);
	doc
		.image(image, 72, 530, { width: 24 })
		.image(PNG, 100, 530, { fit: [24, 24], align: "center", valign: "center" });
	doc.svg(SVG, 130, 530, { width: 24 });

	// Files and annotations.
	const spec = doc.file(Buffer.from("<xml/>"), {
		name: "data.xml",
		type: "application/xml",
		relationship: "Data",
	});
	expect(spec).toBeDefined();
	doc.note(72, 570, 12, 12, "Une note", { color: "yellow" });
	doc
		.highlight(90, 570, 30, 10)
		.underline(130, 570, 30, 10)
		.strike(170, 570, 30, 10);
	doc
		.lineAnnotation(210, 570, 240, 580)
		.rectAnnotation(250, 570, 20, 10)
		.ellipseAnnotation(280, 570, 20, 10);
	doc.textAnnotation(310, 570, 60, 12, "Texte libre");
	doc.fileAnnotation(380, 570, 12, 12, {
		src: Buffer.from("notes"),
		name: "notes.txt",
	});
	doc.annotate(400, 570, 12, 12, { Subtype: "Text", Contents: "Brute" });

	// Forms.
	doc.initForm();
	const group = doc.formField("client");
	doc.formText("nom", 72, 600, 120, 18, { parent: group, value: "Client SA" });
	doc.formPushButton("envoyer", 200, 600, 60, 18, { label: "Envoyer" });
	doc.formCombo("pays", 270, 600, 80, 18, {
		select: ["CH", "FR"],
		value: "CH",
	});
	doc.formList("langue", 360, 600, 80, 30, { select: ["fr", "de"] });
	doc.formCheckbox("accord", 72, 630, 12, 12);
	doc.formRadioButton("choix", 90, 630, 12, 12);
	doc.formAnnotation("autre", "text", 110, 630, 80, 12);

	// Tables, both ways.
	doc.table({
		data: [
			["Poste", "Prix"],
			["Peinture", "85,00"],
		],
		columnStyles: [100, 60],
		position: { x: 72, y: 660 },
	});
	const table = doc.table({ columnStyles: ["*", 60] });
	table.row(["Boiseries", "92,00"]).row(["Nettoyage", "70,00"]);
	table.end();

	// Pages: a second one, then back to the first.
	doc.addPage({ size: "A5", layout: "landscape", margin: 36 });
	doc.continueOnNewPage();
	const { start, count } = doc.bufferedPageRange();
	expect([start, count]).toEqual([0, 3]);
	doc.switchToPage(0);
	doc.text("Revenu à la première page", 300, 40);
	doc.flushPages();
}

describe("the generation API, every call of it", () => {
	it("draws a document that opens and reads back", async () => {
		const doc = new PdfDocument({
			size: "A4",
			bufferPages: true,
			tagged: true,
			lang: "fr-CH",
			info: { Title: "Surface", Author: "vellum" },
			displayTitle: true,
		});
		everything(doc);
		const bytes = toBytes(doc);
		doc.end();
		const pdf = Buffer.from(await bytes);
		expect(await vellum.pageCount(pdf)).toBe(3);
		const text = await vellum.extractText(pdf, { page: 1 });
		expect(text).toContain("Surface complète");
		expect(text).toContain("Revenu à la première page");
		expect(text).toContain("Nettoyage");
	});
});
